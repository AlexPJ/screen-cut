//! Copia de imágenes al portapapeles como CF_DIB (Win32 puro).

use crate::core::types::RawImage;
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
