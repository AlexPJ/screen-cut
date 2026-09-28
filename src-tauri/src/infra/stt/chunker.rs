//! Dónde partir el audio para transcribirlo por trozos: Whisper trabaja con
//! ventanas de 30 s y corta peor las palabras si el trozo acaba a media frase,
//! así que se corta en un silencio a partir de los 15 s.

/// Frecuencia de las pistas de la sesión.
pub const RATE: usize = 16_000;
const MIN: usize = 15 * RATE;
const MAX: usize = 30 * RATE;
/// Tramos de 100 ms; un silencio son 5 seguidos (medio segundo).
const FRAME: usize = RATE / 10;
const SILENCE_FRAMES: usize = 5;
/// RMS por debajo del cual un tramo cuenta como silencio (≈ -40 dBFS).
const QUIET: f32 = 0.01;

/// Posición (en muestras desde el inicio de `pending`) donde cortar el
/// siguiente trozo, o `None` si hay que esperar más audio. Con `finished` se
/// devuelve todo lo que quede.
pub fn cut_point(pending: &[f32], finished: bool) -> Option<usize> {
    let len = pending.len();
    if len == 0 {
        return None;
    }
    if finished && len <= MAX {
        return Some(len);
    }
    if len < MIN {
        return None;
    }
    let end = len.min(MAX);
    let rms: Vec<f32> = pending[MIN..end].chunks(FRAME).map(frame_rms).collect();
    // El tramo de medio segundo más silencioso entre los 15 y los 30 s.
    let quietest = (0..rms.len().saturating_sub(SILENCE_FRAMES - 1))
        .map(|i| (i, rms[i..i + SILENCE_FRAMES].iter().cloned().fold(0.0, f32::max)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    match quietest {
        // Se corta en mitad del silencio.
        Some((i, level)) if level < QUIET => Some(MIN + (i + SILENCE_FRAMES / 2) * FRAME),
        // Sin silencio: se espera hasta los 30 s y entonces se corta donde menos se hable.
        Some((i, _)) if len >= MAX => Some(MIN + (i + SILENCE_FRAMES / 2) * FRAME),
        _ if len >= MAX => Some(MAX),
        _ => None,
    }
}

/// Un trozo sin nada audible no merece despertar al modelo.
pub fn is_silent(samples: &[f32]) -> bool {
    samples.chunks(FRAME).all(|f| frame_rms(f) < QUIET / 3.0)
}

/// Muestras de silencio al principio de un trozo (dejando 200 ms de margen).
/// Whisper tiende a fechar la primera frase al inicio del trozo aunque empiece
/// más tarde, así que se le quita el silencio inicial.
pub fn leading_silence(samples: &[f32]) -> usize {
    let first = samples.chunks(FRAME).position(|f| frame_rms(f) >= QUIET).unwrap_or(0);
    first.saturating_sub(2) * FRAME
}

fn frame_rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * RATE as f32) as usize).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect()
    }

    #[test]
    fn waits_for_enough_audio() {
        assert_eq!(cut_point(&tone(10.0), false), None);
        assert_eq!(cut_point(&tone(10.0), true), Some(10 * RATE));
        assert_eq!(cut_point(&[], true), None);
    }

    #[test]
    fn cuts_in_the_silence_after_15_seconds() {
        let mut audio = tone(18.0);
        audio.extend(vec![0.0; RATE]); // silencio de 18 a 19 s
        audio.extend(tone(3.0));
        let cut = cut_point(&audio, false).unwrap();
        assert!((18 * RATE..19 * RATE).contains(&cut), "corte en {cut}");
    }

    #[test]
    fn never_exceeds_30_seconds() {
        assert!(cut_point(&tone(20.0), false).is_none(), "sin silencio espera");
        let cut = cut_point(&tone(40.0), false).unwrap();
        assert!((MIN..=MAX).contains(&cut));
        let cut = cut_point(&tone(40.0), true).unwrap();
        assert!(cut <= MAX);
    }

    #[test]
    fn skips_leading_silence() {
        let mut audio = vec![0.0; 2 * RATE];
        audio.extend(tone(1.0));
        assert_eq!(leading_silence(&audio), 2 * RATE - 2 * FRAME);
        assert_eq!(leading_silence(&tone(1.0)), 0);
    }

    #[test]
    fn detects_silence() {
        assert!(is_silent(&vec![0.0; RATE]));
        assert!(!is_silent(&tone(1.0)));
    }
}
