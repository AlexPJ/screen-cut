use crate::app::state::AppState;
use crate::core::types::{CaptureInfo, OcrResult, RawImage};
use crate::infra::{capture, clipboard, ocr, png_io, scroll};
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------- Atajos globales ----------

fn region_hotkey() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyX)
}
fn prtsc_hotkey() -> Shortcut {
    Shortcut::new(None, Code::PrintScreen)
}

/// Registra el atajo por defecto (Ctrl+Shift+X) al arrancar.
pub fn register_default_hotkey(app: &AppHandle) {
    let _ = app.global_shortcut().register(region_hotkey());
}

/// Activa/desactiva Impr Pant como disparador de captura de región. Al activarlo
/// desactiva (best-effort) el mapeo de Windows de Impr Pant a "Recortes" para
/// que gane nuestro atajo; al desactivarlo lo restaura.
#[tauri::command]
pub fn set_prtsc_shortcut(app: AppHandle, enabled: bool) -> Result<(), String> {
    let gs = app.global_shortcut();
    let sc = prtsc_hotkey();
    if enabled {
        if !gs.is_registered(sc.clone()) {
            gs.register(sc).map_err(|e| e.to_string())?;
        }
        set_windows_snip_key(false);
    } else {
        if gs.is_registered(sc.clone()) {
            gs.unregister(sc).map_err(|e| e.to_string())?;
        }
        set_windows_snip_key(true);
    }
    Ok(())
}

/// Registra/actualiza un atajo personalizado (formato "Ctrl+Shift+X").
#[tauri::command]
pub fn set_hotkey_shortcut(app: AppHandle, accelerator: String) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister(region_hotkey());
    let sc: Shortcut = accelerator.parse().map_err(|_| "Atajo inválido")?;
    gs.register(sc).map_err(|e| e.to_string())
}

fn set_windows_snip_key(enabled_for_snip: bool) {
    let val = if enabled_for_snip { "1" } else { "0" };
    let _ = Command::new("reg")
        .args([
            "add",
            r"HKCU\Control Panel\Keyboard",
            "/v",
            "PrintScreenKeyForSnippingEnabled",
            "/t",
            "REG_DWORD",
            "/d",
            val,
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

fn to_info(img: &RawImage) -> Result<CaptureInfo, String> {
    Ok(CaptureInfo {
        width: img.width,
        height: img.height,
        png_base64: png_io::encode_png_base64(img)?,
    })
}

fn store_and_notify(app: &AppHandle, img: RawImage) -> Result<CaptureInfo, String> {
    let info = to_info(&img)?;
    let state: State<AppState> = app.state();
    *state.last_capture.lock().unwrap() = Some(img);
    let _ = app.emit("capture-ready", info.clone());
    Ok(info)
}

#[tauri::command]
pub fn capture_fullscreen(app: AppHandle) -> Result<(), String> {
    // Todo el trabajo va a un hilo aparte: no se puede bloquear el event loop.
    std::thread::spawn(move || {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.minimize();
            sleep(Duration::from_millis(400));
        }
        let result = capture::capture_virtual_screen().map(|(img, _)| img);
        show_main(&app);
        match result {
            Ok(img) => {
                let _ = store_and_notify(&app, img);
            }
            Err(e) => {
                let _ = app.emit("capture-error", e);
            }
        }
    });
    Ok(())
}

/// Abre el overlay de selección de región (también usado por el atajo global).
pub fn open_region_overlay(app: AppHandle) -> Result<(), String> {
    std::thread::spawn(move || {
        if let Err(e) = open_region_overlay_inner(&app) {
            show_main(&app);
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

fn open_region_overlay_inner(app: &AppHandle) -> Result<(), String> {
    if app.get_webview_window("overlay").is_some() {
        return Ok(());
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
    sleep(Duration::from_millis(400));

    let (img, vs) = capture::capture_virtual_screen()?;
    {
        let state: State<AppState> = app.state();
        *state.overlay_capture.lock().unwrap() = Some((img, vs.x, vs.y));
    }

    let win = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
        .title("Selecciona una región")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .position(0.0, 0.0)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;

    win.set_position(tauri::PhysicalPosition::new(vs.x, vs.y))
        .map_err(|e| e.to_string())?;
    win.set_size(tauri::PhysicalSize::new(vs.width as u32, vs.height as u32))
        .map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn start_region_selection(app: AppHandle) -> Result<(), String> {
    open_region_overlay(app)
}

/// Devuelve el PNG del escritorio congelado para pintar el overlay.
#[tauri::command]
pub fn get_capture_png(app: AppHandle, which: String) -> Result<CaptureInfo, String> {
    let state: State<AppState> = app.state();
    if which == "overlay" {
        let guard = state.overlay_capture.lock().unwrap();
        let (img, _, _) = guard.as_ref().ok_or("No hay captura de overlay")?;
        to_info(img)
    } else {
        let guard = state.last_capture.lock().unwrap();
        let img = guard.as_ref().ok_or("No hay captura")?;
        to_info(img)
    }
}

#[derive(Serialize, Clone)]
struct ScrollProgress {
    step: usize,
    total_px: u32,
}

/// El overlay llama aquí con la región elegida (en píxeles físicos, relativos
/// al escritorio virtual) y el modo: "region", "scroll-down" o "scroll-right".
#[tauri::command]
pub fn finish_region_selection(
    app: AppHandle,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    mode: String,
) -> Result<(), String> {
    // En un hilo aparte: cerrar/crear ventanas desde el event loop bloquearía la app.
    std::thread::spawn(move || {
        if let Err(e) = finish_region_selection_inner(&app, x, y, width, height, &mode) {
            show_main(&app);
            let _ = app.emit("capture-error", e);
        }
    });
    Ok(())
}

fn finish_region_selection_inner(
    app: &AppHandle,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    mode: &str,
) -> Result<(), String> {
    let (vx, vy) = {
        let state: State<AppState> = app.state();
        let guard = state.overlay_capture.lock().unwrap();
        let (_, vx, vy) = guard.as_ref().ok_or("No hay captura de overlay")?;
        (*vx, *vy)
    };
    close_overlay(app);

    if mode == "region" {
        let img = {
            let state: State<AppState> = app.state();
            let guard = state.overlay_capture.lock().unwrap();
            let (img, _, _) = guard.as_ref().ok_or("No hay captura de overlay")?;
            img.crop(x, y, width, height)
        };
        show_main(app);
        store_and_notify(app, img)?;
        return Ok(());
    }

    // Captura con scroll: se hace en vivo sobre la pantalla real.
    let dir = if mode == "scroll-right" {
        scroll::Direction::Right
    } else {
        scroll::Direction::Down
    };

    let stop = {
        let state: State<AppState> = app.state();
        let flag = state.scroll_stop.clone();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
        flag
    };
    open_scroll_control(app, vx + x as i32, vy + y as i32, width as i32, height as i32)?;

    let app2 = app.clone();
    std::thread::spawn(move || {
        sleep(Duration::from_millis(350)); // deja desaparecer el overlay
        let result = scroll::scrolling_capture(
            vx + x as i32,
            vy + y as i32,
            width as i32,
            height as i32,
            dir,
            &stop,
            |step, total_px| {
                let _ = app2.emit("scroll-progress", ScrollProgress { step, total_px });
            },
        );
        if let Some(w) = app2.get_webview_window("scrollctl") {
            let _ = w.close();
        }
        show_main(&app2);
        match result {
            Ok(img) => {
                let _ = store_and_notify(&app2, img);
            }
            Err(e) => {
                let _ = app2.emit("capture-error", e);
            }
        }
    });
    Ok(())
}

/// Ventanita flotante con el botón "Terminar", colocada fuera de la región.
fn open_scroll_control(
    app: &AppHandle,
    rx: i32,
    ry: i32,
    _rw: i32,
    rh: i32,
) -> Result<(), String> {
    const W: u32 = 360;
    const H: u32 = 64;
    let vs = capture::virtual_screen();
    // Encima de la región si hay hueco; si no, debajo; si tampoco, esquina superior.
    let cy = if ry - vs.y > (H as i32 + 24) {
        ry - H as i32 - 16
    } else if (vs.y + vs.height) - (ry + rh) > (H as i32 + 24) {
        ry + rh + 16
    } else {
        vs.y + 16
    };
    let cx = (rx).max(vs.x + 8);

    let win = WebviewWindowBuilder::new(
        app,
        "scrollctl",
        WebviewUrl::App("scrollctl.html".into()),
    )
    .title("Captura con scroll")
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(cx, cy))
        .map_err(|e| e.to_string())?;
    win.set_size(tauri::PhysicalSize::new(W, H))
        .map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn stop_scrolling(state: State<AppState>) {
    state
        .scroll_stop
        .store(true, std::sync::atomic::Ordering::Relaxed);
}

#[tauri::command]
pub fn cancel_region_selection(app: AppHandle) {
    std::thread::spawn(move || {
        close_overlay(&app);
        show_main(&app);
    });
}

fn close_overlay(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.close();
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
pub fn run_ocr(state: State<AppState>) -> Result<OcrResult, String> {
    let img = {
        let guard = state.last_capture.lock().unwrap();
        guard.clone().ok_or("No hay ninguna captura")?
    };
    ocr::recognize(&img)
}

#[tauri::command]
pub fn scrolling_capture() -> Result<(), String> {
    // La captura con scroll se inicia desde el overlay (finish_region_selection).
    Err("Usa la selección de región con modo scroll".into())
}

#[tauri::command]
pub fn copy_capture_to_clipboard(state: State<AppState>) -> Result<(), String> {
    let guard = state.last_capture.lock().unwrap();
    let img = guard.as_ref().ok_or("No hay ninguna captura")?;
    clipboard::copy_image(img)
}

#[tauri::command]
pub fn save_capture_png(state: State<AppState>, path: String) -> Result<(), String> {
    let guard = state.last_capture.lock().unwrap();
    let img = guard.as_ref().ok_or("No hay ninguna captura")?;
    let bytes = png_io::encode_png(img)?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

// --- Comandos que operan sobre la imagen editada (base + anotaciones/crop) que
// el editor del frontend aplana a un PNG. Así el copiar/guardar/OCR reflejan
// exactamente lo que el usuario ve. ---

fn decode_data_url(png_base64: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    // Acepta tanto "data:image/png;base64,XXXX" como el base64 pelado.
    let b64 = png_base64
        .rsplit_once(',')
        .map(|(_, b)| b)
        .unwrap_or(png_base64);
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("base64 inválido: {e}"))
}

#[tauri::command]
pub fn copy_png(png_base64: String) -> Result<(), String> {
    let bytes = decode_data_url(&png_base64)?;
    let img = png_io::decode_png_to_bgra(&bytes)?;
    clipboard::copy_image(&img)
}

#[tauri::command]
pub fn save_png(path: String, png_base64: String) -> Result<(), String> {
    let bytes = decode_data_url(&png_base64)?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ocr_png(png_base64: String) -> Result<OcrResult, String> {
    let bytes = decode_data_url(&png_base64)?;
    let img = png_io::decode_png_to_bgra(&bytes)?;
    ocr::recognize(&img)
}
