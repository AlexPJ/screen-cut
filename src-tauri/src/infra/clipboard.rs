//! Copia de imágenes al portapapeles. En Windows como CF_DIB (Win32 puro); en
//! macOS y Linux vía arboard.

use crate::core::types::RawImage;

pub use imp::copy_image;

#[cfg(windows)]
mod imp {
    use super::RawImage;
    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BI_RGB};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB: u32 = 8;

    pub fn copy_image(img: &RawImage) -> Result<(), String> {
        let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
        let data_size = img.bgra.len();
        unsafe {
            OpenClipboard(HWND::default()).map_err(|e| format!("OpenClipboard: {e}"))?;
            let result = (|| -> Result<(), String> {
                EmptyClipboard().map_err(|e| format!("EmptyClipboard: {e}"))?;
                let hmem = GlobalAlloc(GMEM_MOVEABLE, header_size + data_size)
                    .map_err(|e| format!("GlobalAlloc: {e}"))?;
                let ptr = GlobalLock(hmem) as *mut u8;
                if ptr.is_null() {
                    return Err("GlobalLock falló".into());
                }
                let header = BITMAPINFOHEADER {
                    biSize: header_size as u32,
                    biWidth: img.width as i32,
                    biHeight: -(img.height as i32), // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    biSizeImage: data_size as u32,
                    ..Default::default()
                };
                std::ptr::copy_nonoverlapping(
                    &header as *const _ as *const u8,
                    ptr,
                    header_size,
                );
                std::ptr::copy_nonoverlapping(img.bgra.as_ptr(), ptr.add(header_size), data_size);
                let _ = GlobalUnlock(hmem);
                SetClipboardData(CF_DIB, HANDLE(hmem.0))
                    .map_err(|e| format!("SetClipboardData: {e}"))?;
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::RawImage;
    use std::borrow::Cow;
    use std::sync::{Mutex, OnceLock};

    /// En Linux (X11/Wayland) el contenido del portapapeles lo sirve el propio
    /// proceso: si se destruye el `Clipboard`, la imagen desaparece. Por eso se
    /// mantiene uno vivo durante toda la ejecución.
    fn clipboard() -> Result<&'static Mutex<arboard::Clipboard>, String> {
        static CB: OnceLock<Mutex<arboard::Clipboard>> = OnceLock::new();
        if let Some(cb) = CB.get() {
            return Ok(cb);
        }
        let cb = arboard::Clipboard::new().map_err(|e| format!("Portapapeles: {e}"))?;
        Ok(CB.get_or_init(|| Mutex::new(cb)))
    }

    pub fn copy_image(img: &RawImage) -> Result<(), String> {
        let mut rgba = img.bgra.clone();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2); // BGRA -> RGBA
        }
        let data = arboard::ImageData {
            width: img.width as usize,
            height: img.height as usize,
            bytes: Cow::Owned(rgba),
        };
        clipboard()?
            .lock()
            .unwrap()
            .set_image(data)
            .map_err(|e| format!("Portapapeles: {e}"))
    }
}
