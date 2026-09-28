//! Ventanas en Windows vía Win32.

use super::{pickable, tidy, WindowInfo};
use crate::core::types::RawImage;
use crate::infra::capture::windows::render_to_image;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindow, IsWindowVisible, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
};

/// Dibuja también el contenido acelerado por GPU (navegadores, vídeo, Teams).
/// No viene definido en el crate `windows`.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

fn hwnd(id: u64) -> HWND {
    HWND(id as usize as *mut _)
}

/// Ventanas de primer nivel visibles, de la de delante a la de atrás, sin las
/// de ScreenCut. Deja fuera las herramientas, las ventanas con dueño (diálogos)
/// y las "ocultas" por DWM (apps de la Store suspendidas, otros escritorios).
pub fn list() -> Result<Vec<WindowInfo>, String> {
    unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<WindowInfo>);
        if let Some(info) = describe(hwnd) {
            list.push(info);
        }
        BOOL(1)
    }

    let mut list: Vec<WindowInfo> = Vec::new();
    unsafe { EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize)) }
        .map_err(|e| format!("No se pudieron listar las ventanas: {e}"))?;
    Ok(list)
}

unsafe fn describe(hwnd: HWND) -> Option<WindowInfo> {
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return None;
    }
    if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
        return None;
    }
    if GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
        return None;
    }
    let mut cloaked = 0u32;
    let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
    if cloaked != 0 {
        return None;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
    if pid == std::process::id() {
        return None;
    }

    let rect = frame_bounds(hwnd)?;
    let (width, height) = ((rect.right - rect.left) as u32, (rect.bottom - rect.top) as u32);
    let mut buf = [0u16; 512];
    let len = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
    let title = tidy(String::from_utf16_lossy(&buf[..len]));
    let app = process_name(pid).unwrap_or_default();
    pickable(&title, &app, width, height).then_some(WindowInfo {
        id: hwnd.0 as usize as u64,
        title,
        app,
        width,
        height,
    })
}

/// Rectángulo visible de la ventana, sin los bordes invisibles de
/// redimensionado que Windows 10/11 incluye en `GetWindowRect`.
unsafe fn frame_bounds(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let size = std::mem::size_of::<RECT>() as u32;
    if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut _ as *mut _, size).is_ok() {
        return Some(rect);
    }
    GetWindowRect(hwnd, &mut rect).ok().map(|_| rect)
}

/// "Teams.exe" a partir del PID.
unsafe fn process_name(pid: u32) -> Option<String> {
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid).ok()?;
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
    let _ = CloseHandle(process);
    ok.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(path.rsplit('\\').next().unwrap_or(&path).to_string())
}

/// Captura el contenido de una ventana con `PrintWindow`, aunque esté tapada.
pub fn capture(id: u64) -> Result<RawImage, String> {
    let hwnd = hwnd(id);
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err("La ventana ya no está abierta".into());
        }
        if IsIconic(hwnd).as_bool() {
            return Err("La ventana está minimizada".into());
        }
        let mut outer = RECT::default();
        GetWindowRect(hwnd, &mut outer).map_err(|e| format!("No se pudo leer la ventana: {e}"))?;
        let visible = frame_bounds(hwnd).unwrap_or(outer);
        let (w, h) = (outer.right - outer.left, outer.bottom - outer.top);
        let img = render_to_image(w, h, "No se pudo capturar la ventana", |mem_dc, _| {
            PrintWindow(hwnd, mem_dc, PW_RENDERFULLCONTENT).as_bool()
        })?;
        // PrintWindow pinta el rectángulo completo; quitamos los bordes invisibles.
        Ok(img.crop(
            (visible.left - outer.left).max(0) as u32,
            (visible.top - outer.top).max(0) as u32,
            (visible.right - visible.left) as u32,
            (visible.bottom - visible.top) as u32,
        ))
    }
}
