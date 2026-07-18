#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod infra;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::ShortcutState;

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    // Todos los atajos registrados (Ctrl+Shift+X y, si el usuario
                    // lo activa, Impr Pant) disparan la captura de región.
                    if event.state == ShortcutState::Pressed {
                        let _ = app::commands::open_region_overlay(app.clone());
                    }
                })
                .build(),
        )
        .manage(app::state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            app::commands::capture_fullscreen,
            app::commands::start_region_selection,
            app::commands::finish_region_selection,
            app::commands::cancel_region_selection,
            app::commands::run_ocr,
            app::commands::scrolling_capture,
            app::commands::stop_scrolling,
            app::commands::copy_capture_to_clipboard,
            app::commands::save_capture_png,
            app::commands::get_capture_png,
            app::commands::copy_png,
            app::commands::save_png,
            app::commands::ocr_png,
            app::commands::set_prtsc_shortcut,
            app::commands::set_hotkey_shortcut,
        ])
        .setup(|app| {
            // Atajo por defecto: Ctrl+Shift+X (siempre activo).
            app::commands::register_default_hotkey(app.handle());

            // Icono de bandeja: mantiene la app viva para responder a Impr Pant.
            let capture_i =
                MenuItem::with_id(app, "capture", "Capturar región", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Mostrar ventana", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Salir", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&capture_i, &show_i, &PredefinedMenuItem::separator(app)?, &quit_i],
            )?;

            TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("ScreenCut")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "capture" => {
                        let _ = app::commands::open_region_overlay(app.clone());
                    }
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Cerrar la ventana principal la oculta en la bandeja (la app sigue
            // disponible para el atajo global). Para salir del todo: bandeja → Salir.
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error al iniciar ScreenCut");
}
