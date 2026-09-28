//! Modelos de transcripción en disco (`<datos de la app>/models`): listar,
//! descargar (con progreso, reanudación y comprobación SHA-256), importar un
//! archivo ya descargado y borrar.

use crate::app::state::AppState;
use crate::infra::stt::{self, catalog, catalog::ModelSpec};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/// Descargas en curso, con su marca de cancelación.
static DOWNLOADS: Mutex<Option<HashMap<&'static str, Arc<AtomicBool>>>> = Mutex::new(None);

fn downloads<R>(f: impl FnOnce(&mut HashMap<&'static str, Arc<AtomicBool>>) -> R) -> R {
    f(DOWNLOADS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

pub fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| format!("No se pudo crear la carpeta de modelos: {e}"))?;
    Ok(dir)
}

/// Ruta del modelo si está instalado entero (el tamaño se comprobó al descargarlo).
pub fn installed(app: &AppHandle, spec: &ModelSpec) -> Option<PathBuf> {
    installed_in(&models_dir(app).ok()?, spec)
}

fn installed_in(dir: &Path, spec: &ModelSpec) -> Option<PathBuf> {
    let path = dir.join(spec.file);
    (std::fs::metadata(&path).ok()?.len() == spec.size).then_some(path)
}

/// Modelo y detector de voz para transcribir: el elegido en los ajustes o, si
/// no está descargado, cualquier otro que sí lo esté.
pub fn for_transcription(app: &AppHandle) -> Option<(&'static ModelSpec, PathBuf, Option<PathBuf>)> {
    let state: State<AppState> = app.state();
    let chosen = state.settings.lock().unwrap().whisper_model.clone();
    let candidates = catalog::find(&chosen).into_iter().chain(catalog::MODELS.iter());
    let (spec, path) = candidates
        .filter(|m| m.id != catalog::VAD.id)
        .find_map(|m| installed(app, m).map(|p| (m, p)))?;
    Some((spec, path, installed(app, &catalog::VAD)))
}

#[derive(Serialize)]
pub struct ModelEntry {
    #[serde(flatten)]
    spec: ModelSpec,
    installed: bool,
    downloading: bool,
    selected: bool,
    /// El que mejor va en este equipo (con o sin GPU).
    recommended: bool,
}

#[tauri::command]
pub fn list_models(app: AppHandle) -> Vec<ModelEntry> {
    let state: State<AppState> = app.state();
    let chosen = state.settings.lock().unwrap().whisper_model.clone();
    let busy = downloads(|d| d.keys().copied().collect::<Vec<_>>());
    catalog::MODELS
        .iter()
        .map(|m| ModelEntry {
            spec: *m,
            installed: installed(&app, m).is_some(),
            downloading: busy.contains(&m.id),
            selected: m.id == chosen,
            recommended: m.id == catalog::recommended().id,
        })
        .collect()
}

#[tauri::command]
pub fn set_whisper_model(app: AppHandle, id: String) -> Result<(), String> {
    catalog::MODELS.iter().find(|m| m.id == id).ok_or("Modelo desconocido")?;
    let state: State<AppState> = app.state();
    let mut settings = state.settings.lock().unwrap();
    settings.whisper_model = id;
    settings.save(&app)
}

#[derive(Serialize, Clone)]
struct Progress {
    id: &'static str,
    /// "progress", "done", "error" o "cancelled".
    state: &'static str,
    done: u64,
    total: u64,
    error: Option<String>,
}

/// Descarga un modelo (y el detector de voz, si falta). Emite
/// `model-download` con el progreso.
#[tauri::command]
pub async fn download_model(app: AppHandle, id: String) -> Result<(), String> {
    let spec = catalog::MODELS.iter().find(|m| m.id == id).ok_or("Modelo desconocido")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let fresh = downloads(|d| {
        if d.contains_key(spec.id) {
            return false;
        }
        d.insert(spec.id, cancel.clone());
        true
    });
    if !fresh {
        return Err("Ese modelo ya se está descargando".into());
    }
    let emit = |state, done, error: Option<String>| {
        let _ = app.emit("model-download", Progress { id: spec.id, state, done, total: spec.size, error });
    };

    let result = async {
        let dir = models_dir(&app)?;
        fetch(&dir, &catalog::VAD, &cancel, |_| {}).await?;
        let mut last = Instant::now();
        fetch(&dir, spec, &cancel, |done| {
            if last.elapsed() >= Duration::from_millis(250) {
                last = Instant::now();
                emit("progress", done, None);
            }
        })
        .await
    }
    .await;
    downloads(|d| d.remove(spec.id));
    match result {
        Ok(_) => {
            emit("done", spec.size, None);
            Ok(())
        }
        Err(_) if cancel.load(Ordering::Relaxed) => {
            emit("cancelled", 0, None);
            Ok(())
        }
        Err(e) => {
            emit("error", 0, Some(e.clone()));
            Err(e)
        }
    }
}

#[tauri::command]
pub fn cancel_model_download(id: String) {
    downloads(|d| {
        if let Some(flag) = d.get(id.as_str()) {
            flag.store(true, Ordering::Relaxed);
        }
    });
}

/// Descarga `spec` a `<archivo>.part` y lo renombra al comprobar el hash. Si
/// ya hay un `.part` de antes, continúa donde se quedó.
async fn fetch(dir: &Path, spec: &ModelSpec, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<PathBuf, String> {
    let target = dir.join(spec.file);
    if installed_in(dir, spec).is_some() {
        return Ok(target);
    }
    let part = dir.join(format!("{}.part", spec.file));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > spec.size {
        have = 0;
    }
    // Lo ya descargado también cuenta para el hash.
    let mut hasher = Sha256::new();
    if have > 0 {
        let (h, n) = tauri::async_runtime::spawn_blocking({
            let part = part.clone();
            move || hash_file(&part, Sha256::new())
        })
        .await
        .map_err(|e| e.to_string())??;
        hasher = h;
        have = n;
    }

    // reqwest usa rustls sin proveedor de criptografía por defecto; el updater
    // instala ring de la misma forma.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("ScreenCut/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.get(spec.download_url());
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request.send().await.map_err(|e| format!("No se pudo descargar el modelo: {e}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::OK && have > 0 {
        // El servidor no admite reanudar: se empieza de cero.
        have = 0;
        hasher = Sha256::new();
    } else if !status.is_success() {
        return Err(format!("No se pudo descargar el modelo (HTTP {})", status.as_u16()));
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(&part)
        .map_err(|e| format!("No se pudo guardar el modelo: {e}"))?;
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("Se cortó la descarga: {e}"))? {
        if cancel.load(Ordering::Relaxed) {
            return Err("Descarga cancelada".into());
        }
        file.write_all(&chunk).map_err(|e| format!("No se pudo guardar el modelo: {e}"))?;
        hasher.update(&chunk);
        have += chunk.len() as u64;
        progress(have);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    if have != spec.size || format!("{:x}", hasher.finalize()) != spec.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("El modelo descargado está dañado; vuelve a intentarlo".into());
    }
    std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

fn hash_file(path: &Path, mut hasher: Sha256) -> Result<(Sha256, u64), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok((hasher, total));
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
}

/// Instala un modelo descargado a mano (p. ej. en redes que bloquean Hugging
/// Face). Tiene que ser uno de los del catálogo, idéntico byte a byte.
/// Devuelve el id del modelo reconocido.
#[tauri::command]
pub async fn import_model(app: AppHandle, path: PathBuf) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let size = std::fs::metadata(&path).map_err(|e| format!("No se pudo leer el archivo: {e}"))?.len();
        let unknown = "No es ninguno de los modelos que admite ScreenCut (ggml de whisper.cpp del catálogo)";
        let candidates: Vec<&ModelSpec> =
            catalog::MODELS.iter().chain(std::iter::once(&catalog::VAD)).filter(|m| m.size == size).collect();
        if candidates.is_empty() {
            return Err(unknown.to_string());
        }
        let (hasher, _) = hash_file(&path, Sha256::new())?;
        let hash = format!("{:x}", hasher.finalize());
        let spec = candidates.into_iter().find(|m| m.sha256 == hash).ok_or(unknown)?;
        let dir = models_dir(&app)?;
        let part = dir.join(format!("{}.part", spec.file));
        std::fs::copy(&path, &part).map_err(|e| format!("No se pudo copiar el modelo: {e}"))?;
        std::fs::rename(&part, dir.join(spec.file)).map_err(|e| e.to_string())?;
        Ok(spec.id.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_model(app: AppHandle, id: String) -> Result<(), String> {
    let spec = catalog::MODELS.iter().find(|m| m.id == id).ok_or("Modelo desconocido")?;
    if downloads(|d| d.contains_key(spec.id)) {
        return Err("Ese modelo se está descargando".into());
    }
    stt::unload();
    let dir = models_dir(&app)?;
    let _ = std::fs::remove_file(dir.join(format!("{}.part", spec.file)));
    match std::fs::remove_file(dir.join(spec.file)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("No se pudo borrar el modelo: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Descarga real (unos 60 MB): `cargo test probe_download -- --ignored`.
    /// Corta la descarga a mitad y comprueba que se reanuda y valida el hash.
    #[test]
    #[ignore]
    fn probe_download() {
        let dir = std::env::var("SCREENCUT_MODELS").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("screencut-models"));
        std::fs::create_dir_all(&dir).unwrap();
        let spec = catalog::find("base-q5_1").unwrap();
        let _ = std::fs::remove_file(dir.join(spec.file));
        let cancel = AtomicBool::new(false);
        let err = tauri::async_runtime::block_on(fetch(&dir, spec, &cancel, |n| {
            if n > 10_000_000 {
                cancel.store(true, Ordering::Relaxed);
            }
        }));
        assert!(err.is_err());
        let partial = std::fs::metadata(dir.join(format!("{}.part", spec.file))).unwrap().len();
        println!("cortada en {partial} bytes");
        cancel.store(false, Ordering::Relaxed);
        let path = tauri::async_runtime::block_on(fetch(&dir, spec, &cancel, |_| {})).unwrap();
        assert_eq!(std::fs::metadata(path).unwrap().len(), spec.size);
        tauri::async_runtime::block_on(fetch(&dir, &catalog::VAD, &cancel, |_| {})).unwrap();
    }
}
