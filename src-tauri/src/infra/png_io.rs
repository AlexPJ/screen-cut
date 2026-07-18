//! Codificación PNG (sin dependencias pesadas de `image`).

use crate::core::types::RawImage;
use base64::Engine;

pub fn encode_png(img: &RawImage) -> Result<Vec<u8>, String> {
    let mut rgba = img.bgra.clone();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2); // BGRA -> RGBA
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, img.width, img.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

pub fn encode_png_base64(img: &RawImage) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(encode_png(img)?))
}

/// Decodifica un PNG (RGB o RGBA) a `RawImage` en BGRA.
pub fn decode_png_to_bgra(bytes: &[u8]) -> Result<RawImage, String> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let data = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    match info.color_type {
        png::ColorType::Rgba => {
            for px in data.chunks_exact(4) {
                bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        png::ColorType::Rgb => {
            for px in data.chunks_exact(3) {
                bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        other => return Err(format!("Formato PNG no soportado: {other:?}")),
    }
    Ok(RawImage::new(w, h, bgra))
}
