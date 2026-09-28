//! Qué está haciendo la app ahora mismo: nada, una sesión o una grabación de
//! vídeo (una cosa cada vez). El atajo global y la bandeja se comportan
//! distinto según el caso.

use crate::app::recording::{self, ActiveRecording};
use crate::app::session::{self, ActiveSession};
use crate::app::state::AppState;
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub enum Activity {
    #[default]
    Idle,
    Session(Box<ActiveSession>),
    Recording(Box<ActiveRecording>),
}

/// Por qué no se puede empezar otra sesión o grabación, si es el caso.
pub fn busy_reason(app: &AppHandle) -> Option<&'static str> {
    let state: State<AppState> = app.state();
    let activity = state.activity.lock().unwrap();
    match &*activity {
        Activity::Idle => None,
        Activity::Session(_) => Some("Ya hay una sesión en curso"),
        Activity::Recording(_) => Some("Hay una grabación de vídeo en curso; detenla antes"),
    }
}

/// Atajo global (y la opción "Capturar" de la bandeja): con una sesión en curso
/// captura su objetivo fijo al instante; si no, abre el overlay de región.
pub fn on_hotkey(app: &AppHandle) {
    let state: State<AppState> = app.state();
    let in_session = matches!(*state.activity.lock().unwrap(), Activity::Session(_));
    if in_session {
        session::capture_now(app.clone());
    } else {
        let _ = crate::app::commands::open_region_overlay(app.clone());
    }
}

/// Pone los textos del menú de la bandeja según lo que esté en curso.
pub fn update_tray(app: &AppHandle) {
    let (in_session, recording) = {
        let state: State<AppState> = app.state();
        let activity = state.activity.lock().unwrap();
        (matches!(*activity, Activity::Session(_)), matches!(*activity, Activity::Recording(_)))
    };
    let state: State<AppState> = app.state();
    let session_item = state.tray_session_item.lock().unwrap();
    if let Some(item) = session_item.as_ref() {
        let _ = item.set_text(if in_session { "Terminar sesión" } else { "Iniciar sesión…" });
        let _ = item.set_enabled(!recording);
    }
    let capture_item = state.tray_capture_item.lock().unwrap();
    if let Some(item) = capture_item.as_ref() {
        let _ = item.set_text(if in_session { "Capturar (sesión)" } else { "Capturar región" });
    }
    let record_item = state.tray_record_item.lock().unwrap();
    if let Some(item) = record_item.as_ref() {
        let _ = item.set_text(if recording { "Detener grabación" } else { "Grabar vídeo…" });
        let _ = item.set_enabled(!in_session && recording::SUPPORTED);
    }
}
