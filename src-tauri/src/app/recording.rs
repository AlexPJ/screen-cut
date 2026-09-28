//! Grabación de vídeo de una pantalla, región o ventana a MP4. Se elige el
//! objetivo con el mismo selector que las sesiones y, mientras se graba, un
//! control flotante (que no sale en el vídeo) muestra el tiempo y "Detener".

use crate::app::activity::Activity;
use crate::app::helpers::{hide_main, local_timestamp, open_floating_window, show_main};
use crate::app::state::AppState;
use crate::app::target::CaptureTarget;
use crate::infra::record::{self, Recording};
use serde::Serialize;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// ¿Hay grabación de vídeo en este sistema?
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", windows, target_os = "linux"));

pub struct ActiveRecording {
    recording: Recording,
    started: Instant,
    started_at_ms: u64,
    path: PathBuf,
    target: CaptureTarget,
}

#[derive(Serialize, Clone)]
pub struct RecordingStatus {
    supported: bool,
    active: bool,
    started_at_ms: Option<u64>,
    target: Option<String>,
}

#[derive(Serialize, Clone)]
struct Saved {
    path: String,
    duration_ms: u64,
}

pub fn recordings_dir(app: &AppHandle) -> PathBuf {
    let state: State<AppState> = app.state();
    let dir = state.settings.lock().unwrap().screenshots_dir.join("Grabaciones");
    dir
}

pub fn status(app: &AppHandle) -> RecordingStatus {
    let state: State<AppState> = app.state();
    let activity = state.activity.lock().unwrap();
    match &*activity {
        Activity::Recording(r) => RecordingStatus {
            supported: SUPPORTED,
            active: true,
            started_at_ms: Some(r.started_at_ms),
            target: Some(r.target.label()),
        },
        _ => RecordingStatus { supported: SUPPORTED, active: false, started_at_ms: None, target: None },
    }
}

fn notify_state(app: &AppHandle) {
    crate::app::activity::update_tray(app);
    let _ = app.emit("recording-state", status(app));
}

/// Qué pedirle al grabador para cada tipo de objetivo.
fn source(target: &CaptureTarget) -> record::Source {
    match target {
        CaptureTarget::Screen { screen } => record::Source::Display { screen: *screen, rect: None },
        CaptureTarget::Region { screen, x, y, width, height } => record::Source::Display {
            screen: *screen,
            rect: Some(record::Rect { x: *x as f64, y: *y as f64, width: *width as f64, height: *height as f64 }),
        },
        CaptureTarget::Window { id, .. } => record::Source::Window { id: *id },
    }
}

/// Empieza a grabar `target`.
pub fn start(app: &AppHandle, target: CaptureTarget) -> Result<(), String> {
    let state: State<AppState> = app.state();
    if let Some(busy) = crate::app::activity::busy_reason(app) {
        return Err(busy.into());
    }
    let (system_audio, microphone) = {
        let settings = state.settings.lock().unwrap();
        (settings.rec_system_audio, settings.rec_mic)
    };
    let dir = recordings_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| format!("No se pudo crear la carpeta de grabaciones: {e}"))?;
    let path = dir.join(format!("ScreenCut_{}.mp4", local_timestamp()));
    let opts = record::Options { source: source(&target), path: path.clone(), system_audio, microphone };

    // Si el sistema corta la captura (se cierra la ventana, se retira el
    // permiso…), se guarda lo grabado hasta ese momento.
    let on_error = {
        let app = app.clone();
        move |message: String| {
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = app.emit("capture-error", message);
                let _ = stop(&app);
            });
        }
    };
    hide_main(app);
    let (recording, warnings) = match Recording::start(opts, on_error) {
        Ok(started) => started,
        Err(e) => {
            show_main(app);
            return Err(e);
        }
    };
    // En Linux puede acabar en WebM si faltan los codificadores de MP4.
    #[cfg(target_os = "linux")]
    let path = recording.path().to_path_buf();
    {
        let mut activity = state.activity.lock().unwrap();
        if !matches!(*activity, Activity::Idle) {
            drop(activity);
            let _ = recording.stop();
            return Err("Ya hay otra grabación o sesión en curso".into());
        }
        let started_at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        *activity = Activity::Recording(Box::new(ActiveRecording {
            recording,
            started: Instant::now(),
            started_at_ms,
            path,
            target: target.clone(),
        }));
    }
    let rect = target.control_rect(app, 320.0, 56.0);
    open_floating_window(app, "recctl", "recctl.html", "Grabación de ScreenCut", rect)?;
    notify_state(app);
    if !warnings.is_empty() {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let _ = app.emit("recording-error", warnings.join(" · "));
        });
    }
    Ok(())
}

/// Detiene la grabación en curso y guarda el vídeo.
pub fn stop(app: &AppHandle) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let active = {
        let mut activity = state.activity.lock().unwrap();
        match std::mem::replace(&mut *activity, Activity::Idle) {
            Activity::Recording(r) => r,
            other => {
                *activity = other;
                return Err("No hay ninguna grabación en curso".into());
            }
        }
    };
    let ActiveRecording { recording, started, path, .. } = *active;
    let duration_ms = started.elapsed().as_millis() as u64;
    if let Some(w) = app.get_webview_window("recctl") {
        let _ = w.close();
    }
    let result = recording.stop();
    notify_state(app);
    show_main(app);
    result?;
    let _ = app.emit("recording-saved", Saved { path: path.to_string_lossy().into(), duration_ms });
    Ok(())
}

/// "Grabar vídeo…" / "Detener grabación" en la bandeja.
pub fn toggle_from_tray(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let result = if status(&app).active { stop(&app) } else { crate::app::target::open_picker(&app, "record") };
        if let Err(e) = result {
            let _ = app.emit("capture-error", e);
        }
    });
}

// ---------------- Comandos ----------------

#[tauri::command]
pub fn recording_status(app: AppHandle) -> RecordingStatus {
    status(&app)
}

#[tauri::command]
pub fn stop_recording(app: AppHandle) -> Result<(), String> {
    // Cerrar ventanas desde el hilo del event loop bloquearía la app.
    std::thread::spawn(move || {
        if let Err(e) = stop(&app) {
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

/// Muestra una grabación en el Finder/Explorador. Solo dentro de la carpeta
/// de grabaciones.
#[tauri::command]
pub fn reveal_recording(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let dir = recordings_dir(&app);
    if path.parent() != Some(dir.as_path()) || path.extension().is_none_or(|e| e != "mp4" && e != "webm") {
        return Err("Ruta no válida".into());
    }
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string())
}
