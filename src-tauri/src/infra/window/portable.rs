//! Ventanas en macOS y Linux (X11) vía xcap.

use super::{pickable, tidy, WindowInfo};
use crate::core::types::RawImage;
use crate::infra::capture::portable::{ensure_permission, from_rgba};
use xcap::Window;

/// Ventanas visibles, de la de delante a la de atrás, sin las de ScreenCut.
pub fn list() -> Result<Vec<WindowInfo>, String> {
    // Sin el permiso, macOS oculta los títulos de las ventanas ajenas y xcap
    // las descarta todas: mejor explicar por qué la lista sale vacía.
    ensure_permission()?;
    let own = std::process::id();
    let windows = Window::all().map_err(|e| format!("No se pudieron listar las ventanas: {e}"))?;
    #[cfg(target_os = "macos")]
    let app_windows = crate::infra::macos::app_window_ids();
    Ok(windows
        .iter()
        .filter_map(|w| {
            if w.pid().ok()? == own || w.is_minimized().unwrap_or(false) {
                return None;
            }
            #[cfg(target_os = "macos")]
            if !app_windows.contains(&w.id().ok()?) {
                return None;
            }
            let (width, height) = (w.width().ok()?, w.height().ok()?);
            let title = tidy(w.title().unwrap_or_default());
            let app = tidy(w.app_name().unwrap_or_default());
            pickable(&title, &app, width, height).then_some(WindowInfo {
                id: w.id().ok()? as u64,
                title,
                app,
                width,
                height,
            })
        })
        .collect())
}

/// Captura el contenido de una ventana, aunque esté tapada por otras.
pub fn capture(id: u64) -> Result<RawImage, String> {
    ensure_permission()?;
    let window = Window::all()
        .map_err(|e| format!("No se pudieron listar las ventanas: {e}"))?
        .into_iter()
        .find(|w| w.id().ok().map(u64::from) == Some(id))
        .ok_or("La ventana ya no está abierta")?;
    if window.is_minimized().unwrap_or(false) {
        return Err("La ventana está minimizada".into());
    }
    let img = window
        .capture_image()
        .map_err(|e| format!("No se pudo capturar la ventana: {e}"))?;
    Ok(from_rgba(img))
}
