//! Adaptadores específicos de macOS: OCR con Vision y ajustes de NSWindow que
//! Tauri no expone.

use crate::core::types::{OcrLine, OcrResult, RawImage};
use objc2::AnyThread;
use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

// ============================ Ventanas ============================

/// Pone el overlay de selección por encima de la barra de menús y del Dock, y
/// hace que aparezca también sobre apps a pantalla completa. `always_on_top`
/// de Tauri solo llega al nivel "flotante", que queda por debajo de ambos.
pub fn raise_overlay(window: &tauri::WebviewWindow) {
    let Ok(ptr) = window.ns_window() else { return };
    let ptr = ptr as usize;
    let _ = window.run_on_main_thread(move || {
        // SAFETY: Tauri devuelve un NSWindow válido mientras exista la ventana,
        // y AppKit solo se toca desde el hilo principal.
        let ns_window = unsafe { &*(ptr as *const NSWindow) };
        ns_window.setLevel(NSStatusWindowLevel + 1);
        ns_window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
    });
}

// ============================ OCR (Vision) ============================

/// OCR con el framework Vision (el mismo que usa "Texto en vivo"). Viene con
/// el sistema, así que en macOS el OCR funciona sin instalar Tesseract.
pub fn recognize_text(img: &RawImage) -> Result<OcrResult, String> {
    let image = cg_image(img)?;

    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    let languages = NSArray::from_retained_slice(&[NSString::from_str("es-ES"), NSString::from_str("en-US")]);
    request.setRecognitionLanguages(&languages);

    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &image,
            &NSDictionary::new(),
        )
    };
    let as_request: &VNRequest = request.as_ref();
    handler
        .performRequests_error(&NSArray::from_slice(&[as_request]))
        .map_err(|e| format!("OCR: {}", e.localizedDescription()))?;

    // Vision devuelve las observaciones en orden de lectura.
    let lines: Vec<OcrLine> = request
        .results()
        .map(|observations| {
            observations
                .iter()
                .filter_map(|obs| obs.topCandidates(1).firstObject())
                .map(|candidate| OcrLine { text: candidate.string().to_string() })
                .filter(|line| !line.text.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");

    Ok(OcrResult { text, lines, language: "Vision (macOS)".into() })
}

fn cg_image(img: &RawImage) -> Result<CFRetained<CGImage>, String> {
    let data = CFData::from_bytes(&img.bgra);
    let provider = CGDataProvider::with_cf_data(Some(&data)).ok_or("CGDataProvider falló")?;
    let space = CGColorSpace::new_device_rgb().ok_or("CGColorSpace falló")?;
    // BGRA en memoria = ARGB little-endian; el alfa se ignora (la captura es opaca).
    let info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipFirst.0 | CGImageByteOrderInfo::Order32Little.0);
    unsafe {
        CGImage::new(
            img.width as usize,
            img.height as usize,
            8,
            32,
            img.width as usize * 4,
            Some(&space),
            info,
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or_else(|| "CGImage falló".into())
}
