//! Objetivo de captura (pantalla, región o ventana) y el selector que lo elige.
//! Sirve para "Capturar ventana" y para fijar el objetivo de una sesión (y,
//! más adelante, de una grabación de vídeo).

use crate::app::commands::store_and_notify;
use crate::app::helpers::{hide_main, show_main};
use crate::app::state::AppState;
use crate::core::types::RawImage;
use crate::infra::capture::{self, Screen, ScreenInfo};
use crate::infra::png_io;
use crate::infra::window::{self, WindowInfo};
use serde::{Deserialize, Serialize};
use std::thread::sleep;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

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

/// Abre el selector de pantalla/ventana. `purpose` dice qué hacer con lo que
/// se elija: "capture" (capturarlo ahora) o "session" (fijarlo para una sesión).
pub fn open_picker(app: &AppHandle, purpose: &str) -> Result<(), String> {
    let state: State<AppState> = app.state();
    *state.picker_purpose.lock().unwrap() = purpose.to_string();
    if let Some(w) = app.get_webview_window("picker") {
        // Ya abierto (quizá con otro propósito): se recarga para mostrar el nuevo.
        let _ = w.eval("location.reload()");
        let _ = w.set_focus();
        return Ok(());
    }
    let title = if purpose == "session" { "Elige qué fijar para la sesión" } else { "Elige qué capturar" };
    WebviewWindowBuilder::new(app, "picker", WebviewUrl::App("picker.html".into()))
        .title(title)
        .inner_size(780.0, 560.0)
        .min_inner_size(520.0, 380.0)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_target_picker(app: AppHandle, purpose: String) -> Result<(), String> {
    // Crear ventanas desde el hilo del event loop bloquearía la app.
    std::thread::spawn(move || {
        if let Err(e) = open_picker(&app, &purpose) {
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn get_picker_purpose(state: State<AppState>) -> String {
    state.picker_purpose.lock().unwrap().clone()
}

fn close_picker(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("picker") {
        let _ = w.close();
    }
}

#[tauri::command]
pub fn choose_target(app: AppHandle, target: CaptureTarget) -> Result<(), String> {
    let purpose = app.state::<AppState>().picker_purpose.lock().unwrap().clone();
    std::thread::spawn(move || {
        close_picker(&app);
        let result = match purpose.as_str() {
            "session" => crate::app::session::start(&app, target),
            _ => capture_once(&app, &target),
        };
        if let Err(e) = result {
            show_main(&app);
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

/// "Región…" en el selector: cierra el selector y abre el overlay, que con el
/// modo "target-session" fija la región para la sesión en vez de recortarla.
#[tauri::command]
pub fn choose_region_target(app: AppHandle) -> Result<(), String> {
    close_picker(&app);
    crate::app::commands::open_region_overlay(app)
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
