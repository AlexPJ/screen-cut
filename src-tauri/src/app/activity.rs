//! Qué está haciendo la app ahora mismo. El atajo global y la bandeja se
//! comportan distinto según haya o no una sesión en curso.

use crate::app::session::{self, ActiveSession};
use crate::app::state::AppState;
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub enum Activity {
    #[default]
    Idle,
    Session(Box<ActiveSession>),
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

/// Cambia el texto de la opción de sesión del menú de la bandeja.
pub fn update_tray(app: &AppHandle, session_active: bool) {
    let state: State<AppState> = app.state();
    let session_item = state.tray_session_item.lock().unwrap();
    if let Some(item) = session_item.as_ref() {
        let _ = item.set_text(if session_active { "Terminar sesión" } else { "Iniciar sesión…" });
    }
    let capture_item = state.tray_capture_item.lock().unwrap();
    if let Some(item) = capture_item.as_ref() {
        let _ = item.set_text(if session_active { "Capturar (sesión)" } else { "Capturar región" });
    }
}
