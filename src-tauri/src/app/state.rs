use crate::app::activity::Activity;
use crate::app::settings::Settings;
use crate::core::types::RawImage;
use crate::infra::capture::Screen;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::menu::MenuItem;
use tauri::Wry;

#[derive(Default)]
pub struct AppState {
    /// Señal de cancelación de la captura con scroll en curso.
    pub scroll_stop: Arc<AtomicBool>,
    /// Última captura mostrada/editable en la ventana principal.
    pub last_capture: Mutex<Option<RawImage>>,
    /// Captura de la pantalla que cubre el overlay de selección, junto con su
    /// geometría (para mapear coordenadas).
    pub overlay_capture: Mutex<Option<(RawImage, Screen)>>,
    /// Preferencias del usuario (carpeta de autoguardado, etc.), cargadas de disco.
    pub settings: Mutex<Settings>,
    /// Sesión en curso (o nada).
    pub activity: Mutex<Activity>,
    /// Qué hacer con lo que se elija en el selector: "capture" o "session".
    pub picker_purpose: Mutex<String>,
    /// Idioma elegido en el selector para la sesión que va a empezar.
    pub session_language: Mutex<Option<String>>,
    /// Opciones de la bandeja cuyo texto cambia al empezar/terminar una sesión.
    pub tray_session_item: Mutex<Option<MenuItem<Wry>>>,
    pub tray_capture_item: Mutex<Option<MenuItem<Wry>>>,
    pub tray_record_item: Mutex<Option<MenuItem<Wry>>>,
}
