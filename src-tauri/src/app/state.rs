use crate::app::settings::Settings;
use crate::core::types::RawImage;
use crate::infra::capture::Screen;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

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
}
