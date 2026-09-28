//! Modelos que la app sabe descargar, con su tamaño y SHA-256 para comprobar
//! que el archivo llegó entero y es el que esperamos.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug)]
pub struct ModelSpec {
    /// Identificador estable (se guarda en los ajustes).
    pub id: &'static str,
    /// Nombre corto para la interfaz.
    pub label: &'static str,
    pub description: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Repositorio de Hugging Face del que se descarga.
    pub repo: &'static str,
}

const WHISPER: &str = "ggerganov/whisper.cpp";

/// Modelos de Whisper, del más preciso al más ligero. Todos son multilingües.
pub const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "large-v3-turbo-q5_0",
        label: "Preciso",
        description: "Large v3 Turbo. La mejor calidad; ideal con GPU (Apple Silicon).",
        file: "ggml-large-v3-turbo-q5_0.bin",
        size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        repo: WHISPER,
    },
    ModelSpec {
        id: "medium-q5_0",
        label: "Equilibrado",
        description: "Medium. Buena calidad, más lento que el preciso sin GPU.",
        file: "ggml-medium-q5_0.bin",
        size: 539_212_467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        repo: WHISPER,
    },
    ModelSpec {
        id: "small-q5_1",
        label: "Rápido",
        description: "Small. Va bien en casi cualquier PC sin quitar CPU a la videollamada.",
        file: "ggml-small-q5_1.bin",
        size: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        repo: WHISPER,
    },
    ModelSpec {
        id: "base-q5_1",
        label: "Mínimo",
        description: "Base. Para equipos muy justos; comete bastantes más errores.",
        file: "ggml-base-q5_1.bin",
        size: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        repo: WHISPER,
    },
];

/// Detector de voz (Silero). Se descarga junto con el primer modelo; sirve para
/// no transcribir silencios, que es donde Whisper se inventa frases.
pub const VAD: ModelSpec = ModelSpec {
    id: "silero-vad",
    label: "Detector de voz",
    description: "Silero VAD v6.2",
    file: "ggml-silero-v6.2.0.bin",
    size: 885_098,
    sha256: "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987",
    repo: "ggml-org/whisper-vad",
};

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().chain(std::iter::once(&VAD)).find(|m| m.id == id)
}

impl ModelSpec {
    pub fn download_url(&self) -> String {
        format!("https://huggingface.co/{}/resolve/main/{}", self.repo, self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for m in MODELS.iter().chain(std::iter::once(&VAD)) {
            assert_eq!(m.sha256.len(), 64, "{}", m.id);
            assert!(m.download_url().ends_with(m.file), "{}", m.id);
            assert!(m.download_url().starts_with("https://huggingface.co/"), "{}", m.id);
        }
        assert!(find("small-q5_1").is_some());
        assert!(find("../etc").is_none());
    }
}
