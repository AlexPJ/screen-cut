//! Captura con scroll (vertical u horizontal) al estilo Snagit:
//! envía eventos de rueda a la ventana bajo la región, captura fotogramas
//! y los cose detectando el desplazamiento por coincidencia de píxeles.
//! El usuario decide cuándo terminar (botón flotante) o se detiene sola
//! al llegar al final del contenido.

use crate::core::types::RawImage;
use crate::infra::capture::{self, Screen};
use crate::infra::input;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::Duration;

const MAX_STEPS: usize = 400;
/// Píxeles del borde final (barra de scroll) excluidos del cotejo.
const SCROLLBAR_MARGIN: u32 = 24;
/// Pasos consecutivos sin movimiento antes de dar por terminado el contenido.
const IDLE_STEPS_TO_STOP: usize = 4;
/// Espera mínima tras enviar la rueda antes de empezar a sondear el asiento.
const KICK_MS: u64 = 90;
/// Intervalo entre sondeos para detectar que el scroll (con inercia) ya paró.
const SETTLE_POLL_MS: u64 = 120;
/// Máximo de sondeos: cubre inercias largas (~1.8 s) sin colgarse.
const SETTLE_MAX_POLLS: usize = 15;

#[derive(Clone, Copy, PartialEq)]
pub enum Direction {
    Down,
    Right,
}

/// Región `x/y/width/height` en píxeles de la captura, relativos a `screen`.
#[allow(clippy::too_many_arguments)]
pub fn scrolling_capture(
    screen: &Screen,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    dir: Direction,
    stop: &AtomicBool,
    mut on_progress: impl FnMut(usize, u32),
) -> Result<RawImage, String> {
    // El cursor debe estar sobre la región para que el sistema enrute la rueda
    // a la ventana correcta (en Windows, "scroll de ventanas inactivas").
    let (cx, cy) = screen.point_to_os(x + width / 2, y + height / 2);
    input::move_cursor(cx, cy);
    sleep(Duration::from_millis(150));

    // Longitud de la región en el eje del scroll, en unidades del SO.
    let region_len = screen.len_to_os(if dir == Direction::Down { height } else { width });
    let grab = || capture::capture_rect(screen, x, y, width, height);

    let first = grab()?;
    let mut stitched = first.clone();
    let mut prev = first;
    let mut idle = 0usize;
    // Sentido invertido de la rueda (ver más abajo).
    let mut reverse = false;

    for step in 0..MAX_STEPS {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        input::scroll_step(dir == Direction::Right, reverse, region_len)?;
        sleep(Duration::from_millis(KICK_MS));

        // Espera activa a que el scroll (con inercia en apps Electron/Chrome)
        // se detenga: capturamos hasta que dos fotogramas consecutivos sean
        // idénticos. Solo entonces medimos el desplazamiento contra `prev`.
        // Esto evita medir "en pleno vuelo" y perder el solapamiento.
        let mut frame = grab()?;
        for _ in 0..SETTLE_MAX_POLLS {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            sleep(Duration::from_millis(SETTLE_POLL_MS));
            let next = grab()?;
            let stable = frames_equal(&frame, &next);
            frame = next;
            if stable {
                break;
            }
        }

        let matched = match dir {
            Direction::Down => find_offset_vertical(&prev, &frame),
            Direction::Right => find_offset_horizontal(&prev, &frame),
        }
        .filter(|&o| o > 0);

        match matched {
            Some(offset) => {
                idle = 0;
                stitched = match dir {
                    Direction::Down => append_vertical(&stitched, &frame, offset),
                    Direction::Right => append_horizontal(&stitched, &frame, offset),
                };
                prev = frame;
                let total = if dir == Direction::Down {
                    stitched.height
                } else {
                    stitched.width
                };
                on_progress(step + 1, total);
            }
            // macOS: con el "scroll natural" activado, la rueda simulada puede ir
            // al revés. Si el primer paso no avanza, se prueba el sentido contrario.
            None if cfg!(target_os = "macos") && step == 0 => reverse = true,
            None => {
                idle += 1;
                if idle >= IDLE_STEPS_TO_STOP {
                    break; // fin del contenido
                }
            }
        }

        // Límite de seguridad de memoria (~120 MP).
        if (stitched.width as u64 * stitched.height as u64) > 120_000_000 {
            break;
        }
    }

    Ok(stitched)
}

/// ¿Son (prácticamente) idénticos dos fotogramas? Se usa para detectar que el
/// scroll con inercia ya se ha detenido. Muestrea filas a lo alto de la imagen.
fn frames_equal(a: &RawImage, b: &RawImage) -> bool {
    if a.width != b.width || a.height != b.height {
        return false;
    }
    let h = a.height;
    let step = (h / 48).max(1);
    let mut y = 0;
    while y < h {
        if row_diff(a.row(y), b.row(y)) > 3 {
            return false;
        }
        y += step;
    }
    true
}

/// Diferencia media absoluta entre dos filas (muestreada), ignorando el
/// margen final donde suele vivir la barra de scroll.
fn row_diff(a: &[u8], b: &[u8]) -> u64 {
    let usable = a.len().saturating_sub((SCROLLBAR_MARGIN * 4) as usize).max(4);
    let mut sum = 0u64;
    let mut count = 0u64;
    let step = ((usable / 4 / 64).max(1)) * 4;
    let mut i = 0;
    while i + 3 < usable {
        sum += a[i].abs_diff(b[i]) as u64
            + a[i + 1].abs_diff(b[i + 1]) as u64
            + a[i + 2].abs_diff(b[i + 2]) as u64;
        count += 3;
        i += step;
    }
    if count == 0 { 0 } else { sum / count }
}

/// Umbral de diferencia media (por fila muestreada) para aceptar un encaje.
const MATCH_TOLERANCE: u64 = 10;

/// Busca cuántos píxeles se ha desplazado `next` hacia arriba respecto a `prev`.
/// La banda inferior de `prev` debe reaparecer más arriba en `next`. Elegimos el
/// desplazamiento con MENOR diferencia global (no el primero que pasa el umbral):
/// esto evita falsos encajes en contenido repetitivo, donde un desplazamiento
/// menor "casi" coincide salvo por detalles (p.ej. dígitos que cambian).
fn find_offset_vertical(prev: &RawImage, next: &RawImage) -> Option<u32> {
    let h = prev.height;
    let band = (h / 6).clamp(24, 140).min(h);
    let anchor_start = h - band; // banda inferior de prev
    let max_offset = anchor_start;

    let mut samples = 0u64;
    let mut best_score = u64::MAX;
    let mut best_offset = 0u32;

    for offset in 0..=max_offset {
        let target = anchor_start - offset;
        let mut score = 0u64;
        let mut n = 0u64;
        let mut i = 0;
        while i < band {
            score += row_diff(prev.row(anchor_start + i), next.row(target + i));
            n += 1;
            i += 3;
            // Poda: si ya superamos el mejor, abandona este offset.
            if score >= best_score {
                score = u64::MAX;
                break;
            }
        }
        if score < best_score {
            best_score = score;
            best_offset = offset;
            samples = n;
        }
    }

    let avg = if samples == 0 { u64::MAX } else { best_score / samples };
    if avg <= MATCH_TOLERANCE {
        Some(best_offset)
    } else {
        None
    }
}

/// "Energía" horizontal de una fila (suma de gradiente entre píxeles vecinos).
/// Alta en líneas de texto, baja en huecos/espacios en blanco.
fn row_energy(img: &RawImage, y: u32) -> u64 {
    let row = img.row(y);
    let mut e = 0u64;
    let mut x = 4;
    while x < row.len() {
        let cur = row[x] as i32 + row[x + 1] as i32 + row[x + 2] as i32;
        let prev = row[x - 4] as i32 + row[x - 3] as i32 + row[x - 2] as i32;
        e += (cur - prev).unsigned_abs() as u64;
        x += 4;
    }
    e
}

/// Añade el contenido nuevo de `frame` al mosaico. En lugar de cortar exactamente
/// en `offset`, desliza el punto de unión hacia una fila de baja energía (un hueco
/// entre líneas) dentro de la zona solapada, y aplica un breve fundido. Así la
/// costura cae en espacio en blanco y no parte ninguna línea de texto (donde el
/// anti-aliasing difiere entre capturas y se vería un fantasma).
fn append_vertical(stitched: &RawImage, frame: &RawImage, offset: u32) -> RawImage {
    let w = frame.width;
    let h = frame.height;
    let rowb = (w * 4) as usize;

    // Filas solapadas presentes en ambas imágenes (mismo contenido capturado dos veces).
    let overlap = (h - offset).min(stitched.height);
    // Cuánto podemos "retroceder" la unión hacia arriba buscando un hueco.
    let window = overlap.min(48);

    // Elige el retroceso `r` cuya fila de unión en `frame` tenga menor energía.
    let mut best_r = 0u32;
    let mut best_e = u64::MAX;
    for r in 0..=window {
        let frow = h - offset - r; // primera fila NUEVA tomada de frame tras la unión
        let e = row_energy(frame, frow);
        if e < best_e {
            best_e = e;
            best_r = r;
        }
    }
    let r = best_r;

    let keep = stitched.height - r; // filas conservadas del mosaico
    let take_from = h - offset - r; // primera fila tomada de frame
    let new_h = stitched.height + offset;

    let mut bgra = vec![0u8; rowb * new_h as usize];
    bgra[..rowb * keep as usize].copy_from_slice(&stitched.bgra[..rowb * keep as usize]);
    bgra[rowb * keep as usize..]
        .copy_from_slice(&frame.bgra[rowb * take_from as usize..rowb * h as usize]);

    // Fundido: mezcla las `feather` filas anteriores a la unión (que existen en
    // ambas capturas) para suavizar cualquier diferencia de tono/anti-aliasing.
    let feather = r.min(6).min(keep).min(take_from);
    for k in 0..feather {
        let srow = keep - feather + k; // fila destino (venía del mosaico)
        let frow = take_from - feather + k; // misma fila en frame
        let a = (k + 1) as f32 / (feather + 1) as f32;
        let so = rowb * srow as usize;
        let fo = rowb * frow as usize;
        for c in 0..rowb {
            let ov = bgra[so + c] as f32;
            let fv = frame.bgra[fo + c] as f32;
            bgra[so + c] = (ov * (1.0 - a) + fv * a).round() as u8;
        }
    }

    RawImage::new(w, new_h, bgra)
}

/// Versión horizontal: trabaja sobre la imagen transpuesta conceptualmente.
fn find_offset_horizontal(prev: &RawImage, next: &RawImage) -> Option<u32> {
    let pt = transpose(prev);
    let nt = transpose(next);
    find_offset_vertical(&pt, &nt)
}

fn append_horizontal(stitched: &RawImage, frame: &RawImage, offset: u32) -> RawImage {
    let new_cols = frame.crop(frame.width - offset, 0, offset, frame.height);
    let new_width = stitched.width + offset;
    let mut bgra = vec![0u8; (new_width * stitched.height * 4) as usize];
    let dst_stride = (new_width * 4) as usize;
    let old_stride = (stitched.width * 4) as usize;
    let add_stride = (offset * 4) as usize;
    for y in 0..stitched.height as usize {
        let dst = y * dst_stride;
        bgra[dst..dst + old_stride]
            .copy_from_slice(&stitched.bgra[y * old_stride..(y + 1) * old_stride]);
        bgra[dst + old_stride..dst + old_stride + add_stride]
            .copy_from_slice(&new_cols.bgra[y * add_stride..(y + 1) * add_stride]);
    }
    RawImage::new(new_width, stitched.height, bgra)
}

fn transpose(img: &RawImage) -> RawImage {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let row = img.row(y as u32);
        for x in 0..w {
            let dst = (x * h + y) * 4;
            out[dst..dst + 4].copy_from_slice(&row[x * 4..x * 4 + 4]);
        }
    }
    RawImage::new(img.height, img.width, out)
}
