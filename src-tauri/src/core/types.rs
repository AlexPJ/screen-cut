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

    /// Reduce la imagen para que quepa en `max_w × max_h` (nunca la amplía),
    /// promediando cada bloque de píxeles para que el texto no se vea dentado.
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> RawImage {
        let ratio = (max_w as f64 / self.width as f64).min(max_h as f64 / self.height as f64);
        if ratio >= 1.0 {
            return self.clone();
        }
        let w = ((self.width as f64 * ratio).round() as u32).max(1);
        let h = ((self.height as f64 * ratio).round() as u32).max(1);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for ty in 0..h {
            let (y0, y1) = (ty * self.height / h, ((ty + 1) * self.height / h).max(ty * self.height / h + 1));
            for tx in 0..w {
                let (x0, x1) = (tx * self.width / w, ((tx + 1) * self.width / w).max(tx * self.width / w + 1));
                let mut sum = [0u32; 4];
                for y in y0..y1 {
                    let row = self.row(y);
                    for x in x0..x1 {
                        let px = &row[(x * 4) as usize..(x * 4 + 4) as usize];
                        for (acc, &v) in sum.iter_mut().zip(px) {
                            *acc += v as u32;
                        }
                    }
                }
                let n = (y1 - y0) * (x1 - x0);
                out.extend(sum.iter().map(|v| (v / n) as u8));
            }
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

#[cfg(test)]
mod tests {
    use super::RawImage;

    #[test]
    fn thumbnail_keeps_aspect_and_averages() {
        // 4×2: mitad izquierda negra, mitad derecha blanca.
        let mut bgra = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                bgra.extend([v, v, v, 255]);
            }
        }
        let img = RawImage::new(4, 2, bgra);
        let t = img.thumbnail(2, 2);
        assert_eq!((t.width, t.height), (2, 1));
        assert_eq!(&t.bgra, &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(img.thumbnail(100, 100).width, 4);
    }
}
