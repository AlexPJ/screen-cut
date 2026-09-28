//! Grabación de vídeo (MP4: H.264 + AAC) con las APIs nativas de cada sistema,
//! sin ffmpeg. macOS: ScreenCaptureKit + AVAssetWriter. Windows:
//! Windows.Graphics.Capture + Media Foundation. Linux: GStreamer (con WebM de
//! respaldo si faltan los codificadores de MP4).

use crate::infra::capture::Screen;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Recording;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::Recording;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Recording;

/// Qué grabar.
#[derive(Clone, Debug)]
#[cfg_attr(not(any(target_os = "macos", windows, target_os = "linux")), allow(dead_code))]
pub enum Source {
    /// Un monitor entero o un rectángulo suyo. `rect` va en píxeles de la
    /// captura del monitor (como el overlay); `None` = el monitor entero.
    Display { screen: Screen, rect: Option<Rect> },
    /// Una ventana (CGWindowID en macOS, HWND en Windows, XID en X11).
    Window { id: u64 },
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(any(target_os = "macos", windows, target_os = "linux")), allow(dead_code))]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(any(target_os = "macos", windows, target_os = "linux")), allow(dead_code))]
pub struct Options {
    pub source: Source,
    pub path: PathBuf,
    pub system_audio: bool,
    pub microphone: bool,
}

/// Tamaño de salida en píxeles: pares (H.264 trabaja con bloques de 2×2) y
/// sin pasar de 4K, el máximo que admiten los codificadores por hardware. En
/// Windows no se usa: su codificador recorta en vez de escalar, así que allí
/// se graba al tamaño nativo.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub fn output_size(width: f64, height: f64) -> (usize, usize) {
    let fit = (3840.0 / width).min(2160.0 / height).min(1.0);
    let even = |v: f64| ((v * fit).round() as usize & !1).max(2);
    (even(width), even(height))
}

/// Bitrate de vídeo razonable para contenido de pantalla (texto nítido sin
/// archivos enormes): unos 6 Mbit/s en 1080p.
#[cfg_attr(not(any(target_os = "macos", windows, target_os = "linux")), allow(dead_code))]
pub fn video_bitrate(width: usize, height: usize) -> usize {
    (width * height * 3).clamp(1_500_000, 16_000_000)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub struct Recording;

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
impl Recording {
    pub fn start(_opts: Options, _on_error: impl Fn(String) + Send + Sync + 'static) -> Result<(Self, Vec<String>), String> {
        Err("La grabación de vídeo todavía no está disponible en este sistema".into())
    }

    pub fn stop(self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_size_is_even_and_at_most_4k() {
        assert_eq!(output_size(1001.0, 601.0), (1000, 600));
        let (w, h) = output_size(5120.0, 2880.0);
        assert!(w <= 3840 && h <= 2160 && w % 2 == 0 && h % 2 == 0);
        assert_eq!(output_size(1.0, 1.0), (2, 2));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod probe {
    use super::*;

    /// Graba 3 s de la pantalla principal: `SCREENCUT_OUT=… cargo test probe_record -- --ignored`.
    #[test]
    #[ignore]
    fn probe_record() {
        let screen = crate::infra::capture::list_screens().unwrap().remove(0).screen;
        let region = std::env::var("SCREENCUT_REGION").is_ok();
        let rect = region.then_some(Rect { x: 200.0, y: 200.0, width: 1282.0, height: 802.0 });
        let opts = Options {
            source: Source::Display { screen, rect },
            path: std::env::var("SCREENCUT_OUT").unwrap().into(),
            system_audio: true,
            microphone: std::env::var("SCREENCUT_MIC").is_ok(),
        };
        let (rec, warnings) = Recording::start(opts, |e| eprintln!("on_error: {e}")).unwrap();
        println!("avisos: {warnings:?}");
        std::thread::sleep(std::time::Duration::from_secs(3));
        rec.stop().unwrap();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod probe {
    use super::*;

    /// Graba 3 s de la pantalla (X11, p. ej. con Xvfb) y comprueba que sale un
    /// vídeo: `SCREENCUT_OUT=… cargo test probe_record_linux -- --ignored`. En
    /// CI no hay servidor de sonido, así que también prueba que sin audio se
    /// graba al menos la imagen.
    #[test]
    #[ignore]
    fn probe_record_linux() {
        let screen = crate::infra::capture::list_screens().unwrap().remove(0).screen;
        let rect = Some(Rect { x: 10.0, y: 10.0, width: 641.0, height: 481.0 });
        let opts = Options {
            source: Source::Display { screen, rect },
            path: std::env::var("SCREENCUT_OUT").unwrap().into(),
            system_audio: true,
            microphone: true,
        };
        let (rec, warnings) = Recording::start(opts, |e| panic!("on_error: {e}")).unwrap();
        println!("avisos: {warnings:?}");
        let path = rec.path().to_path_buf();
        std::thread::sleep(std::time::Duration::from_secs(3));
        rec.stop().unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        println!("{} ({size} bytes)", path.display());
        assert!(size > 10_000, "vídeo vacío");
    }
}
