//! Preferencias persistentes del usuario (carpeta de capturas, etc.).
//! Se guardan como JSON en la carpeta de configuración de la app.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// `#[serde(default)]` a nivel de struct: un campo que falte en el JSON (p. ej.
/// uno añadido en una versión nueva) toma su valor por defecto en vez de hacer
/// fallar todo el parseo y perder el resto de preferencias.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Carpeta donde se autoguarda cada captura (además de mostrarse en la app).
    pub screenshots_dir: PathBuf,
    /// Carpeta de las sesiones. `None` = `<screenshots_dir>/Sesiones`.
    pub sessions_dir: Option<PathBuf>,
    /// Idioma de la transcripción: "auto" o un código ISO 639-1 ("es", "en"…).
    pub transcription_language: String,
    /// Si es `true`, no se pregunta el idioma al empezar cada sesión.
    pub remember_language: bool,
    /// Modelo de Whisper (id del catálogo de modelos).
    pub whisper_model: String,
    /// Transcribir durante la sesión (`false` = todo al terminar, para PCs lentos).
    pub transcribe_live: bool,
    /// Duración máxima de cada imagen en el visor. `None` = hasta la siguiente.
    pub max_image_secs: Option<u32>,
    /// Grabación de vídeo: incluir el audio del sistema y el micrófono.
    pub rec_system_audio: bool,
    pub rec_mic: bool,
    /// Micrófono elegido. `None` = el predeterminado del sistema.
    pub mic_device: Option<String>,
    /// Conservar el audio de la sesión tras transcribirlo (para re-transcribir).
    pub keep_session_audio: bool,
    /// Sesiones: grabar el micrófono ("Tú") y el audio del sistema ("Otros").
    pub session_mic: bool,
    pub session_system_audio: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self::default_for_platform()
    }
}

impl Settings {
    /// Carpeta de capturas por defecto según el sistema operativo:
    /// `Imágenes/Screenshots` (o el equivalente localizado de "Imágenes").
    /// `dirs` resuelve la carpeta de imágenes de forma nativa en Windows/macOS/Linux,
    /// dejando el terreno preparado para un futuro puerto a esas plataformas.
    pub fn default_for_platform() -> Self {
        let base = dirs::picture_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            screenshots_dir: base.join("Screenshots"),
            sessions_dir: None,
            transcription_language: "auto".into(),
            remember_language: false,
            whisper_model: default_whisper_model().into(),
            transcribe_live: true,
            max_image_secs: None,
            rec_system_audio: true,
            rec_mic: false,
            mic_device: None,
            keep_session_audio: true,
            session_mic: true,
            session_system_audio: true,
        }
    }

    /// Carpeta efectiva de las sesiones.
    pub fn sessions_dir(&self) -> PathBuf {
        self.sessions_dir.clone().unwrap_or_else(|| self.screenshots_dir.join("Sesiones"))
    }

    fn config_path(app: &AppHandle) -> Option<PathBuf> {
        app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
    }

    pub fn load(app: &AppHandle) -> Self {
        Self::config_path(app)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::config_path(app).ok_or("No se pudo resolver la carpeta de configuración")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())
    }
}

/// Con GPU (Metal en Apple Silicon) el modelo grande va sobrado; en el resto
/// se parte del rápido para no competir por la CPU con la videollamada.
fn default_whisper_model() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "large-v3-turbo-q5_0"
    } else {
        "small-q5_1"
    }
}

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_keep_their_folder() {
        let s: Settings = serde_json::from_str(r#"{"screenshots_dir":"/tmp/caps"}"#).unwrap();
        assert_eq!(s.screenshots_dir, PathBuf::from("/tmp/caps"));
        assert_eq!(s.transcription_language, "auto");
        assert!(!s.remember_language);
        assert_eq!(s.sessions_dir(), PathBuf::from("/tmp/caps/Sesiones"));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let s: Settings =
            serde_json::from_str(r#"{"screenshots_dir":"/tmp/caps","from_the_future":1}"#).unwrap();
        assert_eq!(s.screenshots_dir, PathBuf::from("/tmp/caps"));
    }
}
