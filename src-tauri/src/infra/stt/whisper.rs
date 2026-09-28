//! Motor de transcripción: whisper.cpp con el modelo cargado una sola vez y
//! compartido. Las llamadas se hacen de una en una (un modelo grande ocupa
//! cientos de MB y ya usa todos los núcleos o la GPU).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Un fragmento reconocido, con tiempos relativos al audio recibido.
#[derive(Debug, Clone)]
pub struct Piece {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub lang: Option<String>,
}

static ENGINE: Mutex<Option<(PathBuf, WhisperContext)>> = Mutex::new(None);

/// Transcribe `samples` (16 kHz mono). `language` es "auto" o un código ISO.
/// `vad` es el modelo Silero; sin él se transcribe todo el audio.
pub fn transcribe(model: &Path, vad: Option<&Path>, language: &str, samples: &[f32]) -> Result<Vec<Piece>, String> {
    let mut engine = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    if engine.as_ref().map(|(p, _)| p.as_path()) != Some(model) {
        *engine = None; // libera el anterior antes de cargar otro
        whisper_rs::install_logging_hooks(); // whisper.cpp escribe mucho por stderr
        let path = model.to_str().ok_or("Ruta del modelo no válida")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .map_err(|e| format!("No se pudo cargar el modelo de transcripción: {e}"))?;
        *engine = Some((model.to_path_buf(), ctx));
    }
    let (_, ctx) = engine.as_ref().expect("cargado arriba");
    let mut state = ctx.create_state().map_err(|e| format!("Error del motor de transcripción: {e}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // La mitad de los núcleos: la videollamada tiene que seguir fluida.
    let threads = std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(2).clamp(1, 8);
    params.set_n_threads(threads as i32);
    params.set_language(Some(language));
    params.set_translate(false);
    // Cada trozo va por separado: así un error no se arrastra al siguiente.
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    if let Some(vad) = vad.and_then(Path::to_str) {
        params.set_vad_model_path(Some(vad));
        params.enable_vad(true);
    }

    state.full(params, samples).map_err(|e| format!("Error transcribiendo: {e}"))?;
    let lang = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
    let pieces = state
        .as_iter()
        .filter_map(|seg| {
            let text = seg.to_str_lossy().ok()?.trim().to_string();
            if seg.no_speech_probability() > 0.8 || is_noise(&text) {
                return None;
            }
            Some(Piece {
                // Whisper da los tiempos en centésimas de segundo.
                start_ms: seg.start_timestamp().max(0) as u64 * 10,
                end_ms: seg.end_timestamp().max(0) as u64 * 10,
                text,
                lang: lang.clone(),
            })
        })
        .collect();
    Ok(pieces)
}

/// Libera el modelo (antes de borrarlo del disco o al quedarse sin trabajo).
/// Hay que hacerlo también antes de salir: el backend de Metal aborta en su
/// limpieza final si sigue cargado.
pub fn unload() {
    *ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Como `unload`, pero sin esperar si hay una transcripción en marcha.
pub fn try_unload() {
    if let Ok(mut engine) = ENGINE.try_lock() {
        *engine = None;
    }
}

/// Lo que Whisper produce con ruido o música: "[Música]", "(risas)", "♪♪"…
fn is_noise(text: &str) -> bool {
    let t = text.trim();
    let bracketed = |open: char, close: char| t.starts_with(open) && t.ends_with(close);
    t.is_empty()
        || bracketed('[', ']')
        || bracketed('(', ')')
        || bracketed('*', '*')
        || t.chars().all(|c| !c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::is_noise;

    #[test]
    fn drops_sound_tags() {
        assert!(is_noise("[Música]"));
        assert!(is_noise("(risas)"));
        assert!(is_noise(" ♪ ♪ "));
        assert!(is_noise("..."));
        assert!(!is_noise("Buenos días, doctora."));
    }
}
