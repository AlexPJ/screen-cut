//! Listado y captura de ventanas para el selector de objetivo. xcap en macOS y
//! Linux (X11); en Windows, Win32 con `PrintWindow`, que captura la ventana
//! aunque otra la tape.

use serde::Serialize;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{capture, list};

#[cfg(not(windows))]
mod portable;
#[cfg(not(windows))]
pub use self::portable::{capture, list};

#[derive(Serialize, Clone)]
pub struct WindowInfo {
    /// Identificador del sistema: CGWindowID, XID o HWND.
    pub id: u64,
    pub title: String,
    /// Nombre de la aplicación (o del ejecutable en Windows).
    pub app: String,
    pub width: u32,
    pub height: u32,
}

/// Descarta lo que no tiene sentido capturar: ventanas diminutas (iconos de la
/// barra de menús, tooltips) y las que no tienen ni título ni aplicación.
fn pickable(title: &str, app: &str, width: u32, height: u32) -> bool {
    width >= 120 && height >= 80 && !(title.trim().is_empty() && app.trim().is_empty())
}

/// Quita espacios y marcas invisibles de dirección de texto (WhatsApp, p. ej.,
/// pone un U+200E delante de su nombre).
fn tidy(s: String) -> String {
    let t = s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{200e}' | '\u{200f}'));
    if t.len() == s.len() { s } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::{pickable, tidy};

    #[test]
    fn tidies_titles() {
        assert_eq!(tidy("\u{200e}WhatsApp ".into()), "WhatsApp");
    }

    #[test]
    fn filters_tiny_and_anonymous_windows() {
        assert!(pickable("Zoom Meeting", "zoom.us", 1280, 720));
        assert!(!pickable("Item-0", "Control Center", 40, 24));
        assert!(!pickable("", " ", 800, 600));
    }
}

