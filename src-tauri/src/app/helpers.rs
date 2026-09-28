//! Utilidades compartidas por los comandos: colocar y mostrar ventanas, marcas
//! de tiempo locales y autoguardado de capturas.

use crate::app::state::AppState;
use crate::core::types::RawImage;
use crate::infra::png_io;
use std::path::PathBuf;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, State,
    WebviewUrl, WebviewWindowBuilder,
};

/// Marca de tiempo local "AAAA-MM-DD_HH-mm-ss-mmm" sin depender de crates de
/// fecha/hora: usa la hora local del sistema vía WinAPI.
#[cfg(windows)]
pub fn local_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// Igual que la versión de Windows, con `localtime_r` de libc.
#[cfg(not(windows))]
pub fn local_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
}

/// Autoguarda la captura en la carpeta configurada por el usuario.
/// Devuelve la ruta final si tiene éxito.
pub fn auto_save(app: &AppHandle, img: &RawImage) -> Result<PathBuf, String> {
    let dir = {
        let state: State<AppState> = app.state();
        let guard = state.settings.lock().unwrap();
        guard.screenshots_dir.clone()
    };
    crate::app::settings::ensure_dir(&dir)?;
    let path = dir.join(format!("Captura_{}.png", local_timestamp()));
    let bytes = png_io::encode_png(img)?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Coloca una ventana en coordenadas del SO (ver `capture::Screen`): puntos en
/// macOS, píxeles físicos en Windows y Linux.
pub fn place_window(win: &tauri::WebviewWindow, x: i32, y: i32, width: i32, height: i32) -> Result<(), String> {
    let result = if cfg!(target_os = "macos") {
        // Primero el tamaño: `setContentSize:` de Cocoa mantiene fija la esquina
        // inferior izquierda, así que redimensionar después de colocar haría
        // crecer la ventana hacia arriba y la sacaría de la pantalla.
        win.set_size(LogicalSize::new(width as f64, height as f64)).and_then(|_| {
            win.set_position(LogicalPosition::new(x as f64, y as f64))
        })
    } else {
        win.set_position(PhysicalPosition::new(x, y))
            .and_then(|_| win.set_size(PhysicalSize::new(width as u32, height as u32)))
    };
    result.map_err(|e| e.to_string())
}

/// Quita la ventana principal de en medio antes de capturar. Devuelve si estaba.
/// En Windows se minimiza (sigue en la barra de tareas); en macOS y Linux se
/// oculta, porque minimizar anima la ventana hacia el Dock y tarda más.
pub fn hide_main(app: &AppHandle) -> bool {
    let Some(w) = app.get_webview_window("main") else { return false };
    if cfg!(windows) {
        let _ = w.minimize();
    } else {
        let _ = w.hide();
    }
    true
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
/// Crea una ventanita flotante sin bordes (controles de scroll, sesión o
/// grabación) en `x, y` con tamaño `width × height`, en unidades del SO.
/// Queda siempre encima, fuera de la barra de tareas y sin robar el foco, y
/// se excluye de las capturas de pantalla (para no salir en las de la sesión).
pub fn open_floating_window(
    app: &AppHandle,
    label: &str,
    html: &str,
    title: &str,
    (x, y, width, height): (i32, i32, i32, i32),
) -> Result<tauri::WebviewWindow, String> {
    let win = WebviewWindowBuilder::new(app, label, WebviewUrl::App(html.into()))
        .title(title)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .content_protected(true)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    place_window(&win, x, y, width, height)?;
    win.show().map_err(|e| e.to_string())?;
    Ok(win)
}
