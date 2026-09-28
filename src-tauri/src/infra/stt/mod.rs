//! Transcripción local con whisper.cpp. El audio nunca sale del equipo: solo
//! se descargan los pesos del modelo.

pub mod catalog;
pub mod chunker;
mod whisper;

pub use whisper::{transcribe, try_unload, unload};

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct Language {
    /// Código ISO 639-1 que entiende Whisper ("es", "en"…).
    pub code: &'static str,
    /// Nombre en inglés según whisper.cpp ("spanish"); la interfaz lo traduce.
    pub name: &'static str,
}

/// Versión de whisper.cpp enlazada.
pub fn engine_version() -> &'static str {
    whisper_rs::get_whisper_version()
}

/// Idiomas que reconoce el modelo, en el orden de whisper.cpp (por frecuencia).
pub fn languages() -> Vec<Language> {
    (0..=whisper_rs::get_lang_max_id())
        .filter_map(|id| {
            Some(Language {
                code: whisper_rs::get_lang_str(id)?,
                name: whisper_rs::get_lang_str_full(id)?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whisper_is_linked_and_multilingual() {
        assert!(!engine_version().is_empty());
        let langs = languages();
        assert!(langs.len() >= 99, "solo {} idiomas", langs.len());
        assert!(langs.iter().any(|l| l.code == "es" && l.name == "spanish"));
    }
}
