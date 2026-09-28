//! Sesiones de ScreenCut: un objetivo fijo (región, ventana o pantalla) que el
//! atajo global captura al instante, sin overlay, durante toda la sesión. El
//! audio se transcribe en local, durante la sesión o al terminarla, y se
//! genera un visor (`index.html`) con las imágenes y la transcripción.

pub mod export;
pub mod model;
pub mod store;
pub mod transcribe;
pub mod transcript;

use crate::app::activity::Activity;
use crate::app::helpers::{hide_main, local_timestamp, open_floating_window, show_main};
use crate::app::state::AppState;
use crate::app::target::CaptureTarget;
use crate::infra::audio::{Source, TrackRecorder};
use crate::infra::{clipboard, png_io};
use model::{AudioTrack, Session, SessionImage, SessionSummary, Status, TranscriptInfo, TranscriptStatus, FORMAT_VERSION};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

/// Sesión en curso.
pub struct ActiveSession {
    pub dir: PathBuf,
    pub session: Session,
    pub started: Instant,
    /// Números de captura ya asignados (las capturas pueden solaparse).
    next_n: u32,
    /// Pistas de audio grabándose: (hablante, archivo relativo, grabador).
    recorders: Vec<(&'static str, String, TrackRecorder)>,
    /// Transcripción en directo: se pone a `false` al dejar de grabar.
    live_transcript: Option<Arc<AtomicBool>>,
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
        _ => SessionStatus { active: false, id: None, started_at_ms: None, images: 0, target: None },
    }
}

fn notify_state(app: &AppHandle) {
    let status = status(app);
    crate::app::activity::update_tray(app);
    let _ = app.emit("session-state", status);
}

/// Empieza una sesión sobre `target`: crea su carpeta, oculta la ventana
/// principal y muestra el control flotante.
pub fn start(app: &AppHandle, target: CaptureTarget) -> Result<(), String> {
    let state: State<AppState> = app.state();
    let (max_image_secs, want_mic, want_system, live) = {
        let settings = state.settings.lock().unwrap();
        (settings.max_image_secs, settings.session_mic, settings.session_system_audio, settings.transcribe_live)
    };
    let language = session_language(app);
    let mut warnings = Vec::new();
    {
        let mut activity = state.activity.lock().unwrap();
        match &*activity {
            Activity::Idle => {}
            Activity::Session(_) => return Err("Ya hay una sesión en curso".into()),
            Activity::Recording(_) => return Err("Hay una grabación de vídeo en curso; detenla antes".into()),
        }
        let id = local_timestamp();
        let dir = store::session_dir(&sessions_root(app), &id)?;
        for sub in ["images", "thumbs"] {
            std::fs::create_dir_all(dir.join(sub)).map_err(|e| format!("No se pudo crear la carpeta de la sesión: {e}"))?;
        }
        let started = Instant::now();
        let recorders = start_audio(&dir, started, want_mic, want_system, &mut warnings);
        let mut session = Session {
            version: FORMAT_VERSION,
            id,
            started_at_ms: now_ms(),
            duration_ms: 0,
            status: Status::Recording,
            target: target.clone(),
            max_image_secs,
            images: Vec::new(),
            segments: Vec::new(),
            audio: Vec::new(),
            transcript: None,
        };
        let mut live_transcript = None;
        if !recorders.is_empty() {
            let model = crate::app::models::for_transcription(app);
            let status = match &model {
                None => TranscriptStatus::NoModel,
                Some(_) if !live => TranscriptStatus::Pending,
                Some(_) => TranscriptStatus::Running,
            };
            let model_id = model.as_ref().map(|(spec, ..)| spec.id.to_string());
            session.transcript = Some(TranscriptInfo { status, language: language.clone(), model: model_id, error: None });
            if let (Some((spec, model, vad)), true) = (model, live) {
                let flag = Arc::new(AtomicBool::new(true));
                let job = transcribe::Job {
                    id: session.id.clone(),
                    dir: dir.clone(),
                    language,
                    model_id: spec.id,
                    model,
                    vad,
                    recording: flag.clone(),
                };
                match transcribe::spawn(app, job) {
                    Ok(()) => live_transcript = Some(flag),
                    Err(e) => warnings.push(e),
                }
            }
        }
        store::write(&dir, &session)?;
        *activity = Activity::Session(Box::new(ActiveSession {
            dir,
            session,
            started,
            next_n: 1,
            recorders,
            live_transcript,
        }));
    }
    hide_main(app);
    open_control(app, &target)?;
    notify_state(app);
    if !warnings.is_empty() {
        // El control flotante tarda un momento en escuchar eventos.
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(800));
            let _ = app.emit("session-error", warnings.join(" · "));
        });
    }
    Ok(())
}

/// Idioma elegido en el selector para esta sesión, o el de los ajustes.
fn session_language(app: &AppHandle) -> String {
    let state: State<AppState> = app.state();
    let chosen = state.session_language.lock().unwrap().take();
    chosen.unwrap_or_else(|| {
        let settings = state.settings.lock().unwrap();
        if settings.remember_language { settings.transcription_language.clone() } else { "auto".into() }
    })
}

/// Arranca la grabación del micrófono y del audio del sistema. Si una fuente
/// falla (sin permiso, sin dispositivo, macOS anterior a 14.6 para el audio
/// del sistema), la sesión sigue sin ella y se avisa.
fn start_audio(
    dir: &std::path::Path,
    started: Instant,
    mic: bool,
    system: bool,
    warnings: &mut Vec<String>,
) -> Vec<(&'static str, String, TrackRecorder)> {
    let mut recorders = Vec::new();
    if (mic || system) && std::fs::create_dir_all(dir.join("audio")).is_err() {
        warnings.push("No se pudo crear la carpeta del audio".into());
        return recorders;
    }
    let sources = [(mic, "me", "audio/mic.wav", Source::Microphone), (system, "others", "audio/system.wav", Source::System)];
    for (wanted, speaker, file, source) in sources {
        if !wanted {
            continue;
        }
        match TrackRecorder::start(source, dir.join(file), started) {
            Ok(rec) => recorders.push((speaker, file.to_string(), rec)),
            Err(e) => warnings.push(e),
        }
    }
    recorders
}

/// Control flotante arriba en el centro de la pantalla del objetivo.
fn open_control(app: &AppHandle, target: &CaptureTarget) -> Result<(), String> {
    let rect = target.control_rect(app, 440.0, 56.0);
    open_floating_window(app, "sessionctl", "sessionctl.html", "Sesión de ScreenCut", rect)?;
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
            other => {
                *activity = other;
                return Err("No hay ninguna sesión en curso".into());
            }
        }
    };
    let ActiveSession { dir, mut session, started, recorders, live_transcript, .. } = *active;
    session.duration_ms = started.elapsed().as_millis() as u64;
    session.status = Status::Complete;
    for (speaker, file, recorder) in recorders {
        match recorder.stop() {
            Ok(duration_ms) => session.audio.push(AudioTrack { speaker: speaker.into(), file, duration_ms }),
            Err(e) => {
                let _ = app.emit("capture-error", e);
            }
        }
    }
    // La transcripción en directo puede haber añadido fragmentos desde que la
    // sesión salió de memoria: los de disco son los buenos.
    let (session, _) = store::update(&dir, |disk| {
        session.segments = std::mem::take(&mut disk.segments);
        session.transcript = disk.transcript.take();
        *disk = session;
    })?;
    let export = export::write_outputs(&dir, &session);
    match live_transcript {
        // Ya con los WAV cerrados, termina de leer lo que falte.
        Some(flag) => flag.store(false, Ordering::Release),
        None => start_transcription(app, &dir, &session, None),
    }

    if let Some(w) = app.get_webview_window("sessionctl") {
        let _ = w.close();
    }
    notify_state(app);
    show_main(app);
    export?;
    open_viewer(app, &session.id)
}

/// Transcribe la sesión entera (al terminar, si no se hizo en directo, o
/// cuando el usuario pide volver a transcribir). `language` = `None` mantiene
/// el que ya tenía.
fn start_transcription(app: &AppHandle, dir: &std::path::Path, session: &Session, language: Option<String>) {
    if !transcribe::has_audio(dir) {
        return;
    }
    let language = language
        .or_else(|| session.transcript.as_ref().map(|t| t.language.clone()))
        .unwrap_or_else(|| "auto".into());
    let Some((spec, model, vad)) = crate::app::models::for_transcription(app) else {
        let _ = store::update(dir, |s| {
            s.transcript = Some(TranscriptInfo { status: TranscriptStatus::NoModel, language, model: None, error: None });
        });
        return;
    };
    let job = transcribe::Job {
        id: session.id.clone(),
        dir: dir.to_path_buf(),
        language,
        model_id: spec.id,
        model,
        vad,
        recording: Arc::new(AtomicBool::new(false)),
    };
    let running = transcribe::info(&job, TranscriptStatus::Running);
    let result = store::update(dir, |s| {
        s.segments.clear();
        s.transcript = Some(running);
    })
    .and_then(|_| transcribe::spawn(app, job));
    if let Err(e) = result {
        let _ = app.emit("capture-error", e);
    }
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
    if status(&app).id.as_deref() == Some(id.as_str()) {
        return Err("La sesión sigue en curso; edítala cuando termine".into());
    }
    let (session, _) = store::update(&dir, |session| {
        for edit in edits {
            if let Some(img) = session.images.iter_mut().find(|i| i.n == edit.n) {
                img.start_ms = edit.start_ms;
                img.end_ms = edit.end_ms;
            }
        }
        session.max_image_secs = max_image_secs;
    })?;
    export::write_outputs(&dir, &session)
}

/// Vuelve a transcribir el audio guardado de una sesión (p. ej. con otro
/// idioma, otro modelo o tras una interrupción).
#[tauri::command]
pub fn transcribe_session(app: AppHandle, id: String, language: Option<String>) -> Result<(), String> {
    let dir = store::session_dir(&sessions_root(&app), &id)?;
    if status(&app).id.as_deref() == Some(id.as_str()) {
        return Err("La sesión sigue en curso".into());
    }
    if transcribe::is_running(&id) {
        return Err("Esa sesión ya se está transcribiendo".into());
    }
    if !transcribe::has_audio(&dir) {
        return Err("Esta sesión no conserva el audio".into());
    }
    if crate::app::models::for_transcription(&app).is_none() {
        return Err("Descarga antes un modelo de transcripción en Ajustes → Transcripción".into());
    }
    let session = store::read(&dir)?;
    start_transcription(&app, &dir, &session, language);
    Ok(())
}

#[derive(Serialize)]
pub struct SessionSetup {
    /// Idioma que se propone: el recordado o "auto".
    language: String,
    remember: bool,
    /// Modelo que se usará; `None` si no hay ninguno descargado.
    model: Option<&'static str>,
    /// Modelo elegido en los ajustes, para ofrecer descargarlo.
    wanted: crate::infra::stt::catalog::ModelSpec,
    records_audio: bool,
}

/// Lo que el selector necesita para preguntar el idioma al empezar la sesión.
#[tauri::command]
pub fn session_setup(app: AppHandle) -> SessionSetup {
    use crate::infra::stt::catalog;
    let state: State<AppState> = app.state();
    let (language, remember, wanted, records_audio) = {
        let s = state.settings.lock().unwrap();
        let language = if s.remember_language { s.transcription_language.clone() } else { "auto".into() };
        (language, s.remember_language, s.whisper_model.clone(), s.session_mic || s.session_system_audio)
    };
    SessionSetup {
        language,
        remember,
        model: crate::app::models::for_transcription(&app).map(|(spec, ..)| spec.label),
        wanted: *catalog::find(&wanted).unwrap_or(catalog::recommended()),
        records_audio,
    }
}

/// Idioma para la próxima sesión; con `remember` se guarda para las siguientes.
#[tauri::command]
pub fn set_session_language(app: AppHandle, language: String, remember: bool) -> Result<(), String> {
    let valid = language == "auto" || crate::infra::stt::languages().iter().any(|l| l.code == language);
    if !valid {
        return Err("Idioma no válido".into());
    }
    let state: State<AppState> = app.state();
    *state.session_language.lock().unwrap() = Some(language.clone());
    let mut settings = state.settings.lock().unwrap();
    if settings.remember_language != remember || (remember && settings.transcription_language != language) {
        settings.remember_language = remember;
        if remember {
            settings.transcription_language = language;
        }
        settings.save(&app)?;
    }
    Ok(())
}

#[tauri::command]
pub fn reveal_session(app: AppHandle, id: String) -> Result<(), String> {
    let dir = store::session_dir(&sessions_root(&app), &id)?;
    tauri_plugin_opener::reveal_item_in_dir(dir.join("index.html")).map_err(|e| e.to_string())
}
