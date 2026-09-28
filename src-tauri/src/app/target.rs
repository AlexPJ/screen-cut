//! Objetivo de captura (pantalla, región o ventana) y el selector que lo elige.
//! Hoy sirve para "Capturar ventana"; las sesiones y la grabación de vídeo
//! guardarán un `CaptureTarget` fijo y lo capturarán una y otra vez.

use crate::app::commands::store_and_notify;
use crate::app::helpers::{hide_main, show_main};
use crate::core::types::RawImage;
use crate::infra::capture::{self, Screen, ScreenInfo};
use crate::infra::png_io;
use crate::infra::window::{self, WindowInfo};
use serde::{Deserialize, Serialize};
use std::thread::sleep;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CaptureTarget {
    /// Un monitor entero.
    Screen { screen: Screen },
    /// Un rectángulo de un monitor, en píxeles de su captura (como devuelve el overlay).
    Region { screen: Screen, x: u32, y: u32, width: u32, height: u32 },
    /// Una ventana concreta; se sigue capturando aunque se mueva o la tapen.
    Window { id: u64, title: String, app: String },
}

impl CaptureTarget {
    pub fn capture(&self) -> Result<RawImage, String> {
        match self {
            CaptureTarget::Screen { screen } => capture::capture_monitor(screen),
            CaptureTarget::Region { screen, x, y, width, height } => {
                capture::capture_rect(screen, *x, *y, *width, *height)
            }
            CaptureTarget::Window { id, .. } => window::capture(*id),
        }
    }

    /// Para capturar una pantalla hay que quitar antes las ventanas de ScreenCut;
    /// una ventana o región ajena se captura sin tocar nada.
    fn needs_clear_screen(&self) -> bool {
        matches!(self, CaptureTarget::Screen { .. })
    }
}

#[derive(Serialize)]
pub struct CaptureSources {
    screens: Vec<ScreenInfo>,
    windows: Vec<WindowInfo>,
    /// Por qué no hay ventanas (p. ej. en Wayland no se pueden listar).
    windows_error: Option<String>,
}

#[tauri::command]
pub async fn list_capture_sources() -> Result<CaptureSources, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let screens = capture::list_screens()?;
        let (windows, windows_error) = match window::list() {
            Ok(list) => (list, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        Ok(CaptureSources { screens, windows, windows_error })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Miniatura PNG (base64) de una fuente, para el selector.
#[tauri::command]
pub async fn source_thumbnail(target: CaptureTarget) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let img = target.capture()?.thumbnail(480, 300);
        png_io::encode_png_base64(&img)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Abre el selector de pantalla/ventana. El propósito (qué hacer con lo que se
/// elija) lo guarda la ventana principal en localStorage, como con el overlay.
#[tauri::command]
pub fn open_target_picker(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("picker") {
        let _ = w.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "picker", WebviewUrl::App("picker.html".into()))
        .title("Elige qué capturar")
        .inner_size(780.0, 560.0)
        .min_inner_size(520.0, 380.0)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn choose_target(app: AppHandle, purpose: String, target: CaptureTarget) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("picker") {
        let _ = w.close();
    }
    match purpose.as_str() {
        "capture" => {
            std::thread::spawn(move || {
                if let Err(e) = capture_once(&app, &target) {
                    let _ = app.emit("capture-error", e);
                }
            });
            Ok(())
        }
        other => Err(format!("Propósito desconocido: {other}")),
    }
}

/// Captura el objetivo una vez y lo muestra en el editor, como una captura normal.
fn capture_once(app: &AppHandle, target: &CaptureTarget) -> Result<(), String> {
    if target.needs_clear_screen() && hide_main(app) {
        sleep(Duration::from_millis(400));
    }
    let result = target.capture();
    show_main(app);
    store_and_notify(app, result?)?;
    Ok(())
}
