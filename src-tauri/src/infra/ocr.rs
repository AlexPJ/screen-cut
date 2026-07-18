//! OCR con Tesseract (binario externo) + preprocesado de imagen.
//! Se llama a `tesseract.exe` como proceso (sin FFI ni linkado de C/C++, para
//! mantener el build ligero). Si Tesseract no está disponible, se cae al motor
//! nativo de Windows (`Windows.Media.Ocr`).

use crate::core::types::{OcrLine, OcrResult, RawImage};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// Evita que aparezca una ventana de consola al invocar tesseract.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn recognize(img: &RawImage) -> Result<OcrResult, String> {
    match tesseract_path() {
        Some(exe) => {
            let prepared = preprocess(img);
            recognize_tesseract(&prepared, exe)
        }
        None => recognize_native(img),
    }
}

// ============================ Tesseract ============================

fn recognize_tesseract(img: &RawImage, exe: &Path) -> Result<OcrResult, String> {
    let png = crate::infra::png_io::encode_png(img)?;
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let mut tmp = std::env::temp_dir();
    tmp.push(format!(
        "screencut-ocr-{}-{}.png",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, &png).map_err(|e| format!("temp OCR: {e}"))?;

    let l = langs(exe);
    let out = Command::new(exe)
        .arg(&tmp)
        .arg("stdout")
        .arg("-l")
        .arg(&l)
        .arg("--psm")
        .arg("6") // bloque uniforme de texto: bueno para terminales/capturas
        .arg("--oem")
        .arg("1") // motor LSTM (mejor precisión)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("Tesseract: {e}"));
    let _ = std::fs::remove_file(&tmp);
    let out = out?;

    if !out.status.success() {
        return Err(format!(
            "Tesseract falló: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let text = String::from_utf8_lossy(&out.stdout)
        .replace("\r\n", "\n")
        .trim_end()
        .to_string();
    let lines = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| OcrLine { text: l.to_string() })
        .collect();

    Ok(OcrResult {
        text,
        lines,
        language: format!("Tesseract · {l}"),
    })
}

/// Localiza `tesseract.exe`. Orden: junto al .exe de la app (bundle/sidecar),
/// PATH, instalación estándar del sistema, y carpeta de usuario. Cacheado.
fn tesseract_path() -> Option<&'static Path> {
    static P: OnceLock<Option<PathBuf>> = OnceLock::new();
    P.get_or_init(|| {
        // 1. Junto al ejecutable de la app (para distribución empaquetada).
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for rel in ["tesseract\\tesseract.exe", "tesseract.exe"] {
                    let p = dir.join(rel);
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
        // 2. PATH.
        if let Ok(out) = Command::new("where")
            .arg("tesseract")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            if out.status.success() {
                if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                    let p = PathBuf::from(line.trim());
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
        // 3. Rutas de instalación conocidas (sistema y usuario).
        let mut candidates = vec![
            PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe"),
        ];
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join(r"Programs\Tesseract-OCR\tesseract.exe"));
        }
        candidates.into_iter().find(|p| p.exists())
    })
    .as_deref()
}

/// Idiomas a usar: preferimos inglés + español si están instalados. Cacheado.
fn langs(exe: &Path) -> String {
    static L: OnceLock<String> = OnceLock::new();
    L.get_or_init(|| {
        let mut avail = Vec::new();
        if let Ok(out) = Command::new(exe)
            .arg("--list-langs")
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            for l in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
                avail.push(l.trim().to_string());
            }
        }
        let chosen: Vec<&str> = ["eng", "spa"]
            .into_iter()
            .filter(|w| avail.iter().any(|a| a == w))
            .collect();
        if chosen.is_empty() {
            "eng".into()
        } else {
            chosen.join("+")
        }
    })
    .clone()
}

// ============================ Preprocesado ============================
// Escala de grises, inversión automática si el fondo es oscuro, estirado de
// contraste y ampliación ×2 cuando el texto es pequeño. Tesseract binariza
// internamente (Otsu/adaptativo), así que le damos un gris limpio y grande.

fn preprocess(img: &RawImage) -> RawImage {
    let (w, h) = (img.width, img.height);
    let n = (w * h) as usize;

    // 1. Luminancia + media + histograma.
    let mut lum = vec![0u8; n];
    let mut hist = [0u32; 256];
    let mut sum = 0u64;
    for (i, px) in img.bgra.chunks_exact(4).enumerate() {
        let l =
            (px[2] as u32 * 299 + px[1] as u32 * 587 + px[0] as u32 * 114) / 1000;
        lum[i] = l as u8;
        hist[l as usize] += 1;
        sum += l as u64;
    }
    let mean = (sum / n as u64) as u8;
    let invert = mean < 128; // fondo oscuro → invertimos a texto oscuro sobre claro

    // 2. Estirado de contraste con percentiles robustos (2%–98%).
    let total = n as u32;
    let lo = percentile(&hist, total, 2);
    let hi = percentile(&hist, total, 98).max(lo + 1);
    let span = (hi as i32 - lo as i32).max(1);
    for l in lum.iter_mut() {
        let mut v = (((*l as i32 - lo as i32) * 255) / span).clamp(0, 255) as u8;
        if invert {
            v = 255 - v;
        }
        *l = v;
    }

    // 3. Ampliación ×2 si la imagen es pequeña (mejora el reconocimiento).
    let (gw, gh, gray) = if w.max(h) < 1600 {
        upscale2x(&lum, w, h)
    } else {
        (w, h, lum)
    };

    // 4. De vuelta a BGRA gris (para reutilizar el codificador PNG).
    let mut bgra = Vec::with_capacity((gw * gh * 4) as usize);
    for &v in &gray {
        bgra.extend_from_slice(&[v, v, v, 255]);
    }
    RawImage::new(gw, gh, bgra)
}

fn percentile(hist: &[u32; 256], total: u32, p: u32) -> u8 {
    let target = (total as u64 * p as u64 / 100) as u32;
    let mut acc = 0u32;
    for (i, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= target {
            return i as u8;
        }
    }
    255
}

fn upscale2x(src: &[u8], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let nw = w * 2;
    let nh = h * 2;
    let mut out = vec![0u8; (nw * nh) as usize];
    for y in 0..nh {
        let fy = y as f32 / 2.0;
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(h - 1);
        let dy = fy - y0 as f32;
        for x in 0..nw {
            let fx = x as f32 / 2.0;
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(w - 1);
            let dx = fx - x0 as f32;
            let p00 = src[(y0 * w + x0) as usize] as f32;
            let p10 = src[(y0 * w + x1) as usize] as f32;
            let p01 = src[(y1 * w + x0) as usize] as f32;
            let p11 = src[(y1 * w + x1) as usize] as f32;
            let top = p00 + (p10 - p00) * dx;
            let bot = p01 + (p11 - p01) * dx;
            out[(y * nw + x) as usize] = (top + (bot - top) * dy).round() as u8;
        }
    }
    (nw, nh, out)
}

// ============================ Fallback nativo ============================

fn recognize_native(img: &RawImage) -> Result<OcrResult, String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Security::Cryptography::CryptographicBuffer;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("No se pudo crear el motor OCR: {e}"))?;

    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(2600);
    let img_scaled;
    let img = if img.width > max_dim || img.height > max_dim {
        img_scaled = downscale(img, max_dim);
        &img_scaled
    } else {
        img
    };

    let buffer = CryptographicBuffer::CreateFromByteArray(&img.bgra)
        .map_err(|e| format!("Buffer OCR: {e}"))?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        img.width as i32,
        img.height as i32,
    )
    .map_err(|e| format!("SoftwareBitmap: {e}"))?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("OCR: {e}"))?
        .get()
        .map_err(|e| format!("OCR: {e}"))?;

    let mut lines = Vec::new();
    let mut full = String::new();
    if let Ok(ocr_lines) = result.Lines() {
        for line in ocr_lines {
            if let Ok(text) = line.Text() {
                let text = text.to_string();
                if !full.is_empty() {
                    full.push('\n');
                }
                full.push_str(&text);
                lines.push(OcrLine { text });
            }
        }
    }

    let language = engine
        .RecognizerLanguage()
        .and_then(|l| l.DisplayName())
        .map(|s| s.to_string())
        .unwrap_or_default();

    Ok(OcrResult { text: full, lines, language })
}

fn downscale(img: &RawImage, max_dim: u32) -> RawImage {
    let scale = (max_dim as f64 / img.width.max(img.height) as f64).min(1.0);
    let nw = ((img.width as f64 * scale) as u32).max(1);
    let nh = ((img.height as f64 * scale) as u32).max(1);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        let sy = (y as u64 * img.height as u64 / nh as u64) as u32;
        let row = img.row(sy);
        for x in 0..nw {
            let sx = (x as u64 * img.width as u64 / nw as u64) as usize * 4;
            out.extend_from_slice(&row[sx..sx + 4]);
        }
    }
    RawImage::new(nw, nh, out)
}
