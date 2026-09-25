use crate::app::settings::Settings;
use crate::app::state::AppState;
use crate::core::types::{CaptureInfo, OcrResult, RawImage};
use crate::infra::capture::{self, Screen};
use crate::infra::{clipboard, ocr, png_io, scroll};
use serde::Serialize;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    State, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

// ---------- Atajos globales ----------

/// Ctrl+Shift+X en Windows y Linux; ⇧⌘X en macOS, donde los atajos de sistema
/// usan ⌘ (como ⇧⌘4 para las capturas nativas).
fn region_hotkey() -> Shortcut {
    let modifier = if cfg!(target_os = "macos") { Modifiers::SUPER } else { Modifiers::CONTROL };
    Shortcut::new(Some(modifier | Modifiers::SHIFT), Code::KeyX)
}
fn prtsc_hotkey() -> Shortcut {
    Shortcut::new(None, Code::PrintScreen)
}

/// Registra el atajo por defecto (Ctrl+Shift+X, ⇧⌘X en macOS) al arrancar.
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

#[cfg(windows)]
fn set_windows_snip_key(enabled_for_snip: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let val = if enabled_for_snip { "1" } else { "0" };
    let _ = std::process::Command::new("reg")
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

/// Fuera de Windows no hay un mapeo del sistema que desactivar.
#[cfg(not(windows))]
fn set_windows_snip_key(_enabled_for_snip: bool) {}

fn to_info(img: &RawImage) -> Result<CaptureInfo, String> {
    Ok(CaptureInfo {
        width: img.width,
        height: img.height,
        png_base64: png_io::encode_png_base64(img)?,
        saved_path: None,
    })
}

/// Marca de tiempo local "AAAA-MM-DD_HH-mm-ss-mmm" sin depender de crates de
/// fecha/hora: usa la hora local del sistema vía WinAPI.
#[cfg(windows)]
fn local_timestamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let st = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{:03}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
    )
}

/// Igual que la versión de Windows, con `localtime_r` de libc.
#[cfg(not(windows))]
fn local_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        now.subsec_millis()
    )
}

/// Autoguarda la captura en la carpeta configurada por el usuario.
/// Devuelve la ruta final si tiene éxito.
fn auto_save(app: &AppHandle, img: &RawImage) -> Result<PathBuf, String> {
    let dir = {
        let state: State<AppState> = app.state();
        let guard = state.settings.lock().unwrap();
        guard.screenshots_dir.clone()
    };
    crate::app::settings::ensure_dir(&dir)?;
    let path = dir.join(format!("Captura_{}.png", local_timestamp()));
    let bytes = png_io::encode_png(img)?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Guarda la captura en el estado, la copia al portapapeles y la autoguarda
/// en disco (ambos "best-effort": si fallan, no impiden mostrar la captura).
fn store_and_notify(app: &AppHandle, img: RawImage) -> Result<CaptureInfo, String> {
    let mut info = to_info(&img)?;

    let _ = clipboard::copy_image(&img);

    match auto_save(app, &img) {
        Ok(path) => info.saved_path = Some(path.to_string_lossy().to_string()),
        Err(e) => {
            let _ = app.emit("capture-error", format!("No se pudo autoguardar la captura: {e}"));
        }
    }

    let state: State<AppState> = app.state();
    *state.last_capture.lock().unwrap() = Some(img);
    let _ = app.emit("capture-ready", info.clone());
    Ok(info)
}

#[tauri::command]
pub fn capture_fullscreen(app: AppHandle) -> Result<(), String> {
    // Todo el trabajo va a un hilo aparte: no se puede bloquear el event loop.
    std::thread::spawn(move || {
        if hide_main(&app) {
            sleep(Duration::from_millis(400));
        }
        let result = capture::capture_screen().map(|(img, _)| img);
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
    hide_main(app);
    sleep(Duration::from_millis(400));

    let (img, screen) = capture::capture_screen()?;
    {
        let state: State<AppState> = app.state();
        *state.overlay_capture.lock().unwrap() = Some((img, screen));
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

    place_window(&win, screen.x, screen.y, screen.width, screen.height)?;
    #[cfg(target_os = "macos")]
    crate::infra::macos::raise_overlay(&win);
    // En Linux el gestor de ventanas puede recolocar la ventana (y en Wayland
    // ignora la posición): pantalla completa garantiza que cubra el monitor.
    #[cfg(target_os = "linux")]
    win.set_fullscreen(true).map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Coloca una ventana en coordenadas del SO (ver `capture::Screen`): puntos en
/// macOS, píxeles físicos en Windows y Linux.
fn place_window(win: &tauri::WebviewWindow, x: i32, y: i32, width: i32, height: i32) -> Result<(), String> {
    let result = if cfg!(target_os = "macos") {
        // Primero el tamaño: `setContentSize:` de Cocoa mantiene fija la esquina
        // inferior izquierda, así que redimensionar después de colocar haría
        // crecer la ventana hacia arriba y la sacaría de la pantalla.
        win.set_size(LogicalSize::new(width as f64, height as f64)).and_then(|_| {
            win.set_position(LogicalPosition::new(x as f64, y as f64))
        })
    } else {
        win.set_position(PhysicalPosition::new(x, y))
            .and_then(|_| win.set_size(PhysicalSize::new(width as u32, height as u32)))
    };
    result.map_err(|e| e.to_string())
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
        let (img, _) = guard.as_ref().ok_or("No hay captura de overlay")?;
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

/// El overlay llama aquí con la región elegida (en píxeles de la captura,
/// relativos a la pantalla que cubre el overlay) y el modo: "region", "scroll-down" o "scroll-right".
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
    let screen = {
        let state: State<AppState> = app.state();
        let guard = state.overlay_capture.lock().unwrap();
        let (_, screen) = guard.as_ref().ok_or("No hay captura de overlay")?;
        *screen
    };
    close_overlay(app);

    if mode == "region" {
        let img = {
            let state: State<AppState> = app.state();
            let guard = state.overlay_capture.lock().unwrap();
            let (img, _) = guard.as_ref().ok_or("No hay captura de overlay")?;
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
    open_scroll_control(app, &screen, x, y, height)?;

    let app2 = app.clone();
    std::thread::spawn(move || {
        sleep(Duration::from_millis(350)); // deja desaparecer el overlay
        let result = scroll::scrolling_capture(
            &screen,
            x,
            y,
            width,
            height,
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

/// Ventanita flotante con el botón "Terminar", colocada fuera de la región
/// (dada en píxeles de la captura, relativos a `screen`).
fn open_scroll_control(app: &AppHandle, screen: &Screen, x: u32, y: u32, height: u32) -> Result<(), String> {
    // Tamaño en unidades del SO.
    const W: i32 = 360;
    const H: i32 = 64;
    let (rx, ry) = screen.point_to_os(x, y);
    let rh = screen.len_to_os(height);
    // Encima de la región si hay hueco; si no, debajo; si tampoco, esquina superior.
    let cy = if ry - screen.y > H + 24 {
        ry - H - 16
    } else if (screen.y + screen.height) - (ry + rh) > H + 24 {
        ry + rh + 16
    } else {
        screen.y + 16
    };
    let cx = rx.max(screen.x + 8);

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
    place_window(&win, cx, cy, W, H)?;
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

/// Quita la ventana principal de en medio antes de capturar. Devuelve si estaba.
/// En Windows se minimiza (sigue en la barra de tareas); en macOS y Linux se
/// oculta, porque minimizar anima la ventana hacia el Dock y tarda más.
fn hide_main(app: &AppHandle) -> bool {
    let Some(w) = app.get_webview_window("main") else { return false };
    if cfg!(windows) {
        let _ = w.minimize();
    } else {
        let _ = w.hide();
    }
    true
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

// --- Carpeta de autoguardado de capturas (configurable en Ajustes) ---

#[tauri::command]
pub fn get_screenshots_dir(state: State<AppState>) -> String {
    state.settings.lock().unwrap().screenshots_dir.to_string_lossy().to_string()
}

#[tauri::command]
pub fn get_default_screenshots_dir() -> String {
    Settings::default_for_platform().screenshots_dir.to_string_lossy().to_string()
}

#[tauri::command]
pub fn set_screenshots_dir(app: AppHandle, state: State<AppState>, path: String) -> Result<(), String> {
    let dir = PathBuf::from(path);
    crate::app::settings::ensure_dir(&dir)?;
    let mut settings = state.settings.lock().unwrap();
    settings.screenshots_dir = dir;
    settings.save(&app)
}
