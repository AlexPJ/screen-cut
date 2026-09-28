//! Contenido de `session.json`: todo lo que el visor necesita para mostrar la
//! sesión. Los campos nuevos llevan `#[serde(default)]` para poder abrir
//! sesiones de versiones anteriores.

use crate::app::target::CaptureTarget;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// En curso (o la app se cerró sin terminarla, ver `Interrupted`).
    Recording,
    Complete,
    /// Se encontró en `Recording` al arrancar la app: se cerró a mitad.
    Interrupted,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    pub version: u32,
    /// Nombre de la carpeta: la marca de tiempo local del inicio.
    pub id: String,
    /// Inicio en milisegundos desde 1970 (UTC); el visor lo muestra en hora local.
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub status: Status,
    pub target: CaptureTarget,
    /// Duración máxima de cada imagen en el visor. `None` = hasta la siguiente.
    #[serde(default)]
    pub max_image_secs: Option<u32>,
    #[serde(default)]
    pub images: Vec<SessionImage>,
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// Pistas de audio grabadas (WAV de 16 kHz mono, alineados con el inicio).
    #[serde(default)]
    pub audio: Vec<AudioTrack>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioTrack {
    /// "me" (micrófono) u "others" (audio del sistema).
    pub speaker: String,
    /// Ruta relativa a la carpeta de la sesión.
    pub file: String,
    pub duration_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SessionImage {
    /// Número de captura, desde 1.
    pub n: u32,
    /// Rutas relativas a la carpeta de la sesión.
    pub file: String,
    pub thumb: String,
    /// Momento de la captura, en ms desde el inicio de la sesión.
    pub t_ms: u64,
    /// Inicio y fin editados a mano en el visor. `None` = calculado: desde la
    /// captura hasta la siguiente (con la duración máxima, si la hay).
    #[serde(default)]
    pub start_ms: Option<u64>,
    #[serde(default)]
    pub end_ms: Option<u64>,
    pub width: u32,
    pub height: u32,
}

/// Un fragmento de la transcripción (se rellena a partir del M4).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    /// "me" (micrófono) u "others" (audio del sistema).
    pub speaker: String,
    #[serde(default)]
    pub lang: Option<String>,
    pub text: String,
}

/// Resumen para la lista de sesiones.
#[derive(Serialize, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub status: Status,
    pub images: usize,
    pub target: String,
}

impl CaptureTarget {
    /// Descripción corta para listas: "Ventana: Zoom Meeting", "Pantalla"…
    pub fn label(&self) -> String {
        match self {
            CaptureTarget::Screen { .. } => "Pantalla completa".into(),
            CaptureTarget::Region { width, height, .. } => format!("Región de {width} × {height}"),
            CaptureTarget::Window { title, app, .. } => {
                format!("Ventana: {}", if title.is_empty() { app } else { title })
            }
        }
    }
}
