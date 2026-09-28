//! Captura de pantalla vía GDI (BitBlt sobre el escritorio virtual).

use super::{Screen, ScreenInfo};
use crate::core::types::RawImage;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, EnumDisplayMonitors,
    GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CAPTUREBLT, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

fn virtual_screen() -> Screen {
    unsafe {
        Screen {
            id: 0,
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            scale: 1.0,
        }
    }
}

/// Captura un rectángulo de `screen`, en píxeles relativos a su origen.
pub fn capture_rect(screen: &Screen, x: u32, y: u32, width: u32, height: u32) -> Result<RawImage, String> {
    blit(screen.x + x as i32, screen.y + y as i32, width as i32, height as i32)
}

/// Captura un rectángulo en coordenadas de pantalla (físicas).
fn blit(x: i32, y: i32, width: i32, height: i32) -> Result<RawImage, String> {
    render_to_image(width, height, "BitBlt falló", |mem_dc, screen_dc| unsafe {
        BitBlt(mem_dc, 0, 0, width, height, screen_dc, x, y, SRCCOPY | CAPTUREBLT).is_ok()
    })
}

/// Crea un bitmap en memoria de `width × height`, deja que `paint` dibuje en
/// él (recibe el DC en memoria y el de la pantalla) y lo devuelve como BGRA.
pub(crate) fn render_to_image(
    width: i32,
    height: i32,
    paint_error: &str,
    paint: impl FnOnce(HDC, HDC) -> bool,
) -> Result<RawImage, String> {
    if width <= 0 || height <= 0 {
        return Err("Región de captura vacía".into());
    }
    unsafe {
        let screen_dc = GetDC(HWND::default());
        if screen_dc.is_invalid() {
            return Err("GetDC falló".into());
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        let old = SelectObject(mem_dc, bitmap);

        let mut result = Err(paint_error.into());
        if paint(mem_dc, screen_dc) {
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (width * height * 4) as usize];
            let scan = GetDIBits(
                mem_dc,
                bitmap,
                0,
                height as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut info,
                DIB_RGB_COLORS,
            );
            if scan == height {
                // GDI deja el canal alfa a 0; lo forzamos a opaco.
                for px in buf.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                result = Ok(RawImage::new(width as u32, height as u32, buf));
            } else {
                result = Err("GetDIBits falló".into());
            }
        }

        SelectObject(mem_dc, old);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);
        result
    }
}

/// Captura todo el escritorio virtual (todos los monitores).
pub fn capture_screen() -> Result<(RawImage, Screen), String> {
    let vs = virtual_screen();
    let img = blit(vs.x, vs.y, vs.width, vs.height)?;
    Ok((img, vs))
}

/// Captura un monitor entero.
pub fn capture_monitor(screen: &Screen) -> Result<RawImage, String> {
    blit(screen.x, screen.y, screen.width, screen.height)
}

/// Todos los monitores (en píxeles físicos), el principal primero.
pub fn list_screens() -> Result<Vec<ScreenInfo>, String> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<ScreenInfo>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO).as_bool() {
            let r = info.monitorInfo.rcMonitor;
            let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            list.push(ScreenInfo {
                screen: Screen {
                    id: list.len() as u32,
                    x: r.left,
                    y: r.top,
                    width: r.right - r.left,
                    height: r.bottom - r.top,
                    scale: 1.0,
                },
                // "\\.\DISPLAY1" → "DISPLAY1"
                name: String::from_utf16_lossy(&info.szDevice[..len]).trim_start_matches(r"\\.\").to_string(),
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            });
        }
        BOOL(1)
    }

    let mut list: Vec<ScreenInfo> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(HDC::default(), None, Some(collect), LPARAM(&mut list as *mut _ as isize))
    };
    if !ok.as_bool() {
        return Err("No se pudieron listar los monitores".into());
    }
    list.sort_by_key(|s| !s.primary);
    Ok(list)
}
