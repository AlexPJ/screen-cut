use serde::Serialize;

/// Imagen en memoria, BGRA de 8 bits por canal (formato nativo de GDI),
/// filas de arriba hacia abajo.
#[derive(Clone)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl RawImage {
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Self {
        debug_assert_eq!(bgra.len(), (width * height * 4) as usize);
        Self { width, height, bgra }
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let stride = (self.width * 4) as usize;
        let start = y as usize * stride;
        &self.bgra[start..start + stride]
    }

    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> RawImage {
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        let w = w.min(self.width - x).max(1);
        let h = h.min(self.height - y).max(1);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        let stride = (self.width * 4) as usize;
        for row in y..y + h {
            let start = row as usize * stride + (x * 4) as usize;
            out.extend_from_slice(&self.bgra[start..start + (w * 4) as usize]);
        }
        RawImage::new(w, h, out)
    }
}

#[derive(Serialize, Clone)]
pub struct CaptureInfo {
    pub width: u32,
    pub height: u32,
    /// PNG codificado en base64 (data URL sin prefijo).
    pub png_base64: String,
    /// Ruta donde se autoguardó la captura, si el autoguardado tuvo éxito.
    pub saved_path: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct OcrLine {
    pub text: String,
}

#[derive(Serialize, Clone)]
pub struct OcrResult {
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub language: String,
}
