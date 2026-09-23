//! Simulación de ratón para la captura con scroll: mover el cursor sobre la
//! región y enviar pasos de rueda a la ventana que hay debajo.
//! Las coordenadas van en unidades del SO (ver `capture::Screen`).

pub use imp::*;

#[cfg(windows)]
mod imp {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

    const WHEEL_DELTA: i32 = 120;
    /// Nº de "clics" de rueda por paso. 1 mantiene el avance por debajo del solape
    /// detectable incluso con inercia, evitando saltarse contenido.
    const CLICKS_PER_STEP: i32 = 1;

    pub fn move_cursor(x: i32, y: i32) {
        unsafe {
            let _ = SetCursorPos(x, y);
        }
    }

    /// Un paso de rueda hacia abajo (o a la derecha; hacia arriba/izquierda si
    /// `reverse`). `_region_len` no se usa: en Windows el paso es un clic de rueda.
    pub fn scroll_step(horizontal: bool, reverse: bool, _region_len: i32) -> Result<(), String> {
        let delta = if reverse { 1 } else { -1 } * WHEEL_DELTA * CLICKS_PER_STEP;
        let flags = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
        // En HWHEEL el signo positivo desplaza a la derecha.
        let data = if horizontal { -delta } else { delta };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: data as u32,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_foundation::CGPoint;
    use objc2_core_graphics::{
        CGEvent, CGEventTapLocation, CGScrollEventUnit, CGWarpMouseCursorPosition,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const NSDictionary<NSString, NSNumber>) -> bool;
    }

    /// Enviar eventos de ratón requiere el permiso de Accesibilidad. Si falta,
    /// macOS descarta los eventos en silencio, así que lo comprobamos antes.
    fn ensure_accessibility() -> Result<(), String> {
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let prompt = NSNumber::new_bool(true);
        let options = NSDictionary::from_slices(&[&*key], &[&*prompt]);
        if unsafe { AXIsProcessTrustedWithOptions(&*options) } {
            Ok(())
        } else {
            Err("La captura con scroll necesita permiso de Accesibilidad. Actívalo en Ajustes del Sistema → \
                 Privacidad y seguridad → Accesibilidad y vuelve a intentarlo."
                .into())
        }
    }

    pub fn cursor_position() -> Option<(i32, i32)> {
        let event = CGEvent::new(None)?;
        let p = CGEvent::location(Some(&event));
        Some((p.x as i32, p.y as i32))
    }

    pub fn move_cursor(x: i32, y: i32) {
        let _ = CGWarpMouseCursorPosition(CGPoint { x: x as f64, y: y as f64 });
    }

    /// Un paso de rueda hacia abajo (o a la derecha; hacia arriba/izquierda si
    /// `reverse`), en píxeles: el 40 % de la región, para que siempre quede
    /// solape suficiente para coser.
    pub fn scroll_step(horizontal: bool, reverse: bool, region_len: i32) -> Result<(), String> {
        ensure_accessibility()?;
        let px = ((region_len as f64 * 0.4) as i32).max(20) * if reverse { -1 } else { 1 };
        let (vertical, horizontal) = if horizontal { (0, -px) } else { (-px, 0) };
        let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 2, vertical, horizontal, 0)
            .ok_or("No se pudo crear el evento de scroll")?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    //! X11 vía la extensión XTEST. En Wayland no hay forma estándar de simular
    //! eventos: solo llegarán a las apps que corran sobre XWayland.
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt as _, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
    };
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    fn connect() -> Result<(RustConnection, u32), String> {
        let (conn, screen) = x11rb::connect(None)
            .map_err(|_| "La captura con scroll necesita una sesión X11".to_string())?;
        let root = conn.setup().roots[screen].root;
        Ok((conn, root))
    }

    pub fn cursor_position() -> Option<(i32, i32)> {
        let (conn, root) = connect().ok()?;
        let reply = conn.query_pointer(root).ok()?.reply().ok()?;
        Some((reply.root_x as i32, reply.root_y as i32))
    }

    pub fn move_cursor(x: i32, y: i32) {
        if let Ok((conn, root)) = connect() {
            let _ = conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 0, x11rb::CURRENT_TIME, root, x as i16, y as i16, 0);
            let _ = conn.sync();
        }
    }

    /// Un clic de rueda hacia abajo (botón 5) o a la derecha (botón 7); con
    /// `reverse`, hacia arriba (4) o a la izquierda (6).
    pub fn scroll_step(horizontal: bool, reverse: bool, _region_len: i32) -> Result<(), String> {
        let (conn, _) = connect()?;
        let button = match (horizontal, reverse) {
            (false, false) => 5,
            (false, true) => 4,
            (true, false) => 7,
            (true, true) => 6,
        };
        for kind in [BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT] {
            conn.xtest_fake_input(kind, button, x11rb::CURRENT_TIME, x11rb::NONE, 0, 0, 0)
                .map_err(|e| format!("XTEST: {e}"))?;
        }
        conn.sync().map_err(|e| format!("XTEST: {e}"))
    }
}
