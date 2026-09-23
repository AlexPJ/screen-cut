//! Captura de pantalla en macOS y Linux vía xcap, sobre un monitor concreto.
//! En macOS usa CoreGraphics; en Linux, X11 (XCB) o el portal de Wayland.

use super::Screen;
use crate::core::types::RawImage;
use crate::infra::input;
use xcap::Monitor;

/// Monitor bajo el cursor; si no se puede averiguar, el principal.
fn monitor_under_cursor() -> Result<Monitor, String> {
    if let Some((x, y)) = input::cursor_position() {
        if let Ok(m) = Monitor::from_point(x, y) {
            return Ok(m);
        }
    }
    let monitors = Monitor::all().map_err(|e| format!("No se pudieron listar los monitores: {e}"))?;
    let primary = monitors.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
    monitors
        .into_iter()
        .nth(primary)
        .ok_or_else(|| "No se encontró ningún monitor".into())
}

fn monitor_by_id(id: u32) -> Result<Monitor, String> {
    Monitor::all()
        .map_err(|e| format!("No se pudieron listar los monitores: {e}"))?
        .into_iter()
        .find(|m| m.id().ok() == Some(id))
        .ok_or_else(|| "El monitor de la captura ya no está disponible".into())
}

fn grab(monitor: &Monitor) -> Result<RawImage, String> {
    ensure_permission()?;
    let img = monitor
        .capture_image()
        .map_err(|e| format!("No se pudo capturar la pantalla: {e}"))?;
    let (w, h) = img.dimensions();
    let mut bgra = img.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2); // RGBA -> BGRA
        px[3] = 255;
    }
    Ok(RawImage::new(w, h, bgra))
}

/// macOS solo deja ver el fondo de escritorio (sin ventanas) a las apps sin
/// permiso de Grabación de pantalla, así que lo pedimos antes de capturar.
#[cfg(target_os = "macos")]
fn ensure_permission() -> Result<(), String> {
    use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};
    if CGPreflightScreenCaptureAccess() {
        return Ok(());
    }
    // La primera vez muestra el aviso del sistema; después solo devuelve false.
    CGRequestScreenCaptureAccess();
    Err("ScreenCut necesita permiso de Grabación de pantalla. Actívalo en Ajustes del Sistema → \
         Privacidad y seguridad → Grabación de pantalla y del audio del sistema, y vuelve a abrir la app."
        .into())
}

#[cfg(not(target_os = "macos"))]
fn ensure_permission() -> Result<(), String> {
    Ok(())
}

/// Captura el monitor bajo el cursor.
pub fn capture_screen() -> Result<(RawImage, Screen), String> {
    let monitor = monitor_under_cursor()?;
    let img = grab(&monitor)?;
    let err = |e: xcap::XCapError| e.to_string();
    let width = monitor.width().map_err(err)? as i32;
    let screen = Screen {
        id: monitor.id().map_err(err)?,
        x: monitor.x().map_err(err)?,
        y: monitor.y().map_err(err)?,
        width,
        height: monitor.height().map_err(err)? as i32,
        // En Retina la imagen tiene más píxeles que puntos tiene el monitor.
        scale: img.width as f64 / width.max(1) as f64,
    };
    Ok((img, screen))
}

/// Captura un rectángulo de `screen`, en píxeles relativos a su origen.
pub fn capture_rect(screen: &Screen, x: u32, y: u32, width: u32, height: u32) -> Result<RawImage, String> {
    if width == 0 || height == 0 {
        return Err("Región de captura vacía".into());
    }
    let img = grab(&monitor_by_id(screen.id)?)?;
    Ok(img.crop(x, y, width, height))
}
