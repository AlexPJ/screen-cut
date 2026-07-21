//! Preferencias persistentes del usuario (carpeta de capturas, etc.).
//! Se guardan como JSON en la carpeta de configuración de la app.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    /// Carpeta donde se autoguarda cada captura (además de mostrarse en la app).
    pub screenshots_dir: PathBuf,
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
        Self { screenshots_dir: base.join("Screenshots") }
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

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())
}
