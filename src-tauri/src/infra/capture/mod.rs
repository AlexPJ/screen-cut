//! Captura de pantalla. En Windows se usa GDI sobre todo el escritorio virtual;
//! en macOS y Linux, xcap sobre el monitor que hay bajo el cursor.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{capture_rect, capture_screen};

#[cfg(not(windows))]
mod portable;
#[cfg(not(windows))]
pub use self::portable::{capture_rect, capture_screen};

/// Zona de pantalla que cubre el overlay de selección.
///
/// `x/y/width/height` van en el sistema de coordenadas de ventanas del SO:
/// píxeles físicos en Windows y Linux, puntos en macOS. `scale` pasa de esas
/// unidades a píxeles de la imagen capturada (1.0 salvo en pantallas Retina).
#[derive(Clone, Copy)]
pub struct Screen {
    /// Identificador del monitor (sin uso en Windows: se captura el escritorio virtual).
    #[cfg_attr(windows, allow(dead_code))]
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
}

impl Screen {
    /// Convierte una coordenada en píxeles de la captura a unidades del SO.
    pub fn len_to_os(&self, px: u32) -> i32 {
        (px as f64 / self.scale).round() as i32
    }

    /// Convierte un punto en píxeles de la captura a coordenadas globales del SO.
    pub fn point_to_os(&self, px: u32, py: u32) -> (i32, i32) {
        (self.x + self.len_to_os(px), self.y + self.len_to_os(py))
    }
}
