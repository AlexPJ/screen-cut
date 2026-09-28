//! Audio de las sesiones: micrófono ("Tú") y audio del sistema ("Otros"), cada
//! uno en su propio WAV de 16 kHz mono alineado con el reloj de la sesión.

pub(crate) mod device;
#[cfg(any(windows, test))]
pub mod mix;
mod resample;
mod track;
pub(crate) mod wav;

pub use device::Source;
pub use wav::{available_samples, read_samples};

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Instant;
use track::Track;

/// Bloque de audio tal como llega del dispositivo.
pub struct PcmBlock {
    /// Momento en que empezó el bloque.
    pub at: Instant,
    pub rate: u32,
    pub channels: u16,
    /// Muestras intercaladas, en [-1, 1].
    pub samples: Vec<f32>,
}

/// Una fuente grabándose a disco. El escritor va en su propio hilo para no
/// hacer E/S dentro del callback de audio.
pub struct TrackRecorder {
    stream: device::AudioStream,
    writer: JoinHandle<Result<u64, String>>,
}

impl TrackRecorder {
    pub fn start(source: Source, path: PathBuf, session_start: Instant) -> Result<Self, String> {
        let mut track = Track::create(&path, session_start).map_err(|e| format!("No se pudo crear {}: {e}", path.display()))?;
        let (tx, rx) = mpsc::channel::<PcmBlock>();
        let stream = device::start(source, move |block| {
            let _ = tx.send(block);
        })?;
        let writer = std::thread::spawn(move || {
            // El canal se cierra cuando se detiene la captura.
            for block in rx {
                track.push(&block).map_err(|e| format!("Error escribiendo el audio: {e}"))?;
            }
            track.finish(Instant::now()).map_err(|e| e.to_string())
        });
        Ok(Self { stream, writer })
    }

    /// Detiene la captura, cierra el WAV y devuelve su duración en ms.
    pub fn stop(self) -> Result<u64, String> {
        self.stream.stop();
        self.writer.join().map_err(|_| "El escritor de audio falló".to_string())?
    }
}

