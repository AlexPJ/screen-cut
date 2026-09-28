//! Sesiones de ScreenCut: un objetivo fijo (región, ventana o pantalla) que el
//! atajo global captura al instante, sin overlay, durante toda la sesión. Al
//! terminar se genera un visor (`index.html`) con las imágenes en el tiempo.

pub mod export;
pub mod model;
pub mod store;

use crate::app::activity::Activity;
use crate::app::helpers::{hide_main, local_timestamp, open_floating_window, show_main};
use crate::app::state::AppState;
use crate::app::target::CaptureTarget;
use crate::infra::{capture, clipboard, png_io};
use model::{Session, SessionImage, SessionSummary, Status, FORMAT_VERSION};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// Sesión en curso.
pub struct ActiveSession {
    pub dir: PathBuf,
    pub session: Session,
    pub started: Instant,
    /// Números de captura ya asignados (las capturas pueden solaparse).
    next_n: u32,
}

#[derive(Serialize, Clone)]
pub struct SessionStatus {
    active: bool,
    id: Option<String>,
    started_at_ms: Option<u64>,
    images: usize,
    target: Option<String>,
}

#[derive(Serialize, Clone)]
struct CaptureEvent {
    n: u32,
    t_ms: u64,
    count: usize,
}

fn sessions_root(app: &AppHandle) -> PathBuf {
    let state: State<AppState> = app.state();
    let root = state.settings.lock().unwrap().sessions_dir();
    root
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// "01-02-13" (h-m-s) para los nombres de archivo.
fn hms(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}-{:02}-{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn status(app: &AppHandle) -> SessionStatus {
    let state: State<AppState> = app.state();
    let activity = state.activity.lock().unwrap();
    match &*activity {
        Activity::Session(s) => SessionStatus {
            active: true,
            id: Some(s.session.id.clone()),
            started_at_ms: Some(s.session.started_at_ms),
            images: s.session.images.len(),
            target: Some(s.session.target.label()),
        },
        Activity::Idle => SessionStatus { active: false, id: None, started_at_ms: None, images: 0, target: None },
    }
}

fn notify_state(app: &AppHandle) {
    let status = status(app);
    crate::app::activity::update_tray(app, status.active);
    let _ = app.emit("session-state", status);
}

/// Empieza una sesión sobre `target`: crea su carpeta, oculta la ventana
/// principal y muestra el control flotante.
pub fn start(app: &AppHandle, target: CaptureTarget) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let max_image_secs = state.settings.lock().unwrap().max_image_secs;
    {
        let mut activity = state.activity.lock().unwrap();
        if !matches!(*activity, Activity::Idle) {
            return Err("Ya hay una sesión en curso".into());
        }
        let id = local_timestamp();
        let dir = store::session_dir(&sessions_root(app), &id)?;
        for sub in ["images", "thumbs"] {
            std::fs::create_dir_all(dir.join(sub)).map_err(|e| format!("No se pudo crear la carpeta de la sesión: {e}"))?;
        }
        let session = Session {
            version: FORMAT_VERSION,
            id,
            started_at_ms: now_ms(),
            duration_ms: 0,
            status: Status::Recording,
            target: target.clone(),
            max_image_secs,
            images: Vec::new(),
            segments: Vec::new(),
        };
        store::write(&dir, &session)?;
        *activity = Activity::Session(Box::new(ActiveSession { dir, session, started: Instant::now(), next_n: 1 }));
    }
    hide_main(app);
    open_control(app, &target)?;
    notify_state(app);
    Ok(())
}

/// Control flotante arriba en el centro de la pantalla del objetivo.
fn open_control(app: &AppHandle, target: &CaptureTarget) -> Result<(), String> {
    let screen = match target {
        CaptureTarget::Screen { screen } | CaptureTarget::Region { screen, .. } => Some(*screen),
        CaptureTarget::Window { .. } => capture::list_screens().ok().and_then(|l| l.first().map(|s| s.screen)),
    };
    // Tamaño lógico; en Windows y Linux las coordenadas son píxeles físicos.
    let scale = if cfg!(target_os = "macos") {
        1.0
    } else {
        app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0)
    };
    let (w, h) = ((440.0 * scale) as i32, (56.0 * scale) as i32);
    let (x, y) = match screen {
        Some(s) => (s.x + (s.width - w) / 2, s.y + (if cfg!(target_os = "macos") { 44 } else { 12 })),
        None => (100, 100),
    };
    open_floating_window(app, "sessionctl", "sessionctl.html", "Sesión de ScreenCut", (x, y, w, h))?;
    Ok(())
}

/// Captura el objetivo fijo ahora mismo (atajo global, bandeja o botón).
pub fn capture_now(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(e) = capture_now_inner(&app) {
            let _ = app.emit("session-error", e);
        }
    });
}

fn capture_now_inner(app: &AppHandle) -> Result<(), String> {
    let state: State<AppState> = app.state();
    // Se reserva número y momento antes de capturar, que puede tardar.
    let (target, dir, n, t_ms) = {
        let mut activity = state.activity.lock().unwrap();
        let Activity::Session(s) = &mut *activity else { return Err("No hay ninguna sesión en curso".into()) };
        let n = s.next_n;
        s.next_n += 1;
        (s.session.target.clone(), s.dir.clone(), n, s.started.elapsed().as_millis() as u64)
    };

    let img = target.capture()?;
    let file = format!("images/{n:03}_{}.png", hms(t_ms));
    let thumb = format!("thumbs/{n:03}.png");
    std::fs::write(dir.join(&file), png_io::encode_png(&img)?).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(&thumb), png_io::encode_png(&img.thumbnail(320, 200))?).map_err(|e| e.to_string())?;
    let _ = clipboard::copy_image(&img);

    let count = {
        let mut activity = state.activity.lock().unwrap();
        let Activity::Session(s) = &mut *activity else { return Ok(()) }; // terminó mientras tanto
        s.session.images.push(SessionImage {
            n,
            file,
            thumb,
            t_ms,
            start_ms: None,
            end_ms: None,
            width: img.width,
            height: img.height,
        });
        s.session.images.sort_by_key(|i| i.n);
        s.session.duration_ms = s.started.elapsed().as_millis() as u64;
        store::write(&s.dir, &s.session)?;
        s.session.images.len()
    };
    let _ = app.emit("session-capture", CaptureEvent { n, t_ms, count });
    Ok(())
}

/// Termina la sesión en curso, genera el visor y lo abre.
pub fn end(app: &AppHandle) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let active = {
        let mut activity = state.activity.lock().unwrap();
        match std::mem::replace(&mut *activity, Activity::Idle) {
            Activity::Session(s) => s,
            Activity::Idle => return Err("No hay ninguna sesión en curso".into()),
        }
    };
    let ActiveSession { dir, mut session, started, .. } = *active;
    session.duration_ms = started.elapsed().as_millis() as u64;
    session.status = Status::Complete;
    store::write(&dir, &session)?;
    let export = export::write_index(&dir, &session);

    if let Some(w) = app.get_webview_window("sessionctl") {
        let _ = w.close();
    }
    notify_state(app);
    show_main(app);
    export?;
    open_viewer(app, &session.id)
}

/// Abre (o reutiliza) la ventana del visor con la sesión `id`.
pub fn open_viewer(app: &AppHandle, id: &str) -> Result<(), String> {
    store::session_dir(&sessions_root(app), id)?; // valida el id antes de meterlo en JS
    if let Some(w) = app.get_webview_window("viewer") {
        w.eval(format!("window.openSession({id:?})")).map_err(|e| e.to_string())?;
        let _ = w.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "viewer", WebviewUrl::App("viewer.html".into()))
        .title("Sesión de ScreenCut")
        .inner_size(1200.0, 780.0)
        .min_inner_size(760.0, 480.0)
        .center()
        .initialization_script(format!("window.__SESSION_ID__ = {id:?};"))
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// "Iniciar sesión…" / "Terminar sesión" en la bandeja.
pub fn toggle_from_tray(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let result = if status(&app).active {
            end(&app)
        } else {
            crate::app::target::open_picker(&app, "session")
        };
        if let Err(e) = result {
            let _ = app.emit("capture-error", e);
        }
    });
}

/// Al arrancar la app: las sesiones que quedaron "en curso" se marcan como interrumpidas.
pub fn recover(app: &AppHandle) {
    store::mark_interrupted(&sessions_root(app));
}

// ---------------- Comandos ----------------

#[tauri::command]
pub fn session_status(app: AppHandle) -> SessionStatus {
    status(&app)
}

#[tauri::command]
pub fn capture_session_now(app: AppHandle) {
    capture_now(app);
}

#[tauri::command]
pub fn end_session(app: AppHandle) -> Result<(), String> {
    // Cerrar y crear ventanas desde el hilo del event loop bloquearía la app.
    std::thread::spawn(move || {
        if let Err(e) = end(&app) {
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn list_sessions(app: AppHandle) -> Vec<SessionSummary> {
    store::list(&sessions_root(&app))
}

#[tauri::command]
pub fn open_session_viewer(app: AppHandle, id: String) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = open_viewer(&app, &id) {
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn load_session(app: AppHandle, id: String) -> Result<Session, String> {
    store::read(&store::session_dir(&sessions_root(&app), &id)?)
}

/// Un archivo de la sesión (imagen o miniatura) en base64, para el visor.
#[tauri::command]
pub async fn session_file(app: AppHandle, id: String, path: String) -> Result<String, String> {
    let dir = store::session_dir(&sessions_root(&app), &id)?;
    let safe = (path.starts_with("images/") || path.starts_with("thumbs/"))
        && !path.contains("..")
        && !path.contains('\\');
    if !safe {
        return Err("Ruta no válida".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        use base64::Engine;
        let bytes = std::fs::read(dir.join(&path)).map_err(|e| format!("No se pudo leer {path}: {e}"))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
pub struct ImageEdit {
    n: u32,
    start_ms: Option<u64>,
    end_ms: Option<u64>,
}

/// Guarda los cambios del visor (rangos de cada imagen y duración máxima) y
/// regenera el `index.html` estático.
#[tauri::command]
pub fn save_session_edits(
    app: AppHandle,
    id: String,
    edits: Vec<ImageEdit>,
    max_image_secs: Option<u32>,
) -> Result<(), String> {
    let dir = store::session_dir(&sessions_root(&app), &id)?;
    let mut session = store::read(&dir)?;
    for edit in edits {
        if let Some(img) = session.images.iter_mut().find(|i| i.n == edit.n) {
            img.start_ms = edit.start_ms;
            img.end_ms = edit.end_ms;
        }
    }
    session.max_image_secs = max_image_secs;
    store::write(&dir, &session)?;
    export::write_index(&dir, &session)
}

#[tauri::command]
pub fn reveal_session(app: AppHandle, id: String) -> Result<(), String> {
    let dir = store::session_dir(&sessions_root(&app), &id)?;
    tauri_plugin_opener::reveal_item_in_dir(dir.join("index.html")).map_err(|e| e.to_string())
}
