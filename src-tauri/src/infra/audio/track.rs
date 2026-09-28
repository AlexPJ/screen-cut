//! Una pista de la sesión (micrófono o audio del sistema) en un WAV de 16 kHz
//! mono alineado con el reloj de la sesión: la muestra `n` corresponde siempre
//! al instante `n / 16000` s desde el inicio, así que imágenes y transcripción
//! se pueden cruzar sin desfases.
//!
//! Para mantenerlo, se rellena con silencio lo que falte (el hueco hasta el
//! primer bloque, o los de WASAPI loopback, que no entrega nada mientras no
//! suena nada) y se recorta si el reloj del dispositivo va adelantado.

use super::resample::{ToMono16k, OUT_RATE};
use super::wav::WavWriter;
use super::PcmBlock;
use std::io;
use std::path::Path;
use std::time::Instant;

/// Desfase que se tolera antes de corregir, en muestras de salida (250 ms).
const TOLERANCE: u64 = OUT_RATE as u64 / 4;

pub struct Track {
    session_start: Instant,
    wav: WavWriter,
    resampler: Option<(u32, ToMono16k)>,
    buf: Vec<f32>,
}

impl Track {
    pub fn create(path: &Path, session_start: Instant) -> io::Result<Self> {
        Ok(Self { session_start, wav: WavWriter::create(path, OUT_RATE)?, resampler: None, buf: Vec::new() })
    }

    pub fn push(&mut self, block: &PcmBlock) -> io::Result<()> {
        if self.resampler.as_ref().map(|(r, _)| *r) != Some(block.rate) {
            self.resampler = Some((block.rate, ToMono16k::new(block.rate)));
        }
        self.buf.clear();
        let (_, resampler) = self.resampler.as_mut().expect("recién creado");
        resampler.process(&block.samples, block.channels, &mut self.buf);

        let expected = block.at.saturating_duration_since(self.session_start).as_secs_f64();
        let expected = (expected * OUT_RATE as f64) as u64;
        let written = self.wav.samples();
        if expected > written + TOLERANCE {
            self.wav.write_silence(expected - written)?;
        } else if written > expected + TOLERANCE {
            // Vamos adelantados: se descarta lo que sobra de este bloque.
            let skip = ((written - expected) as usize).min(self.buf.len());
            return self.wav.write(&self.buf[skip..]);
        }
        self.wav.write(&self.buf)
    }

    /// Cierra el WAV, completado con silencio hasta `end` (si al final no
    /// sonaba nada, no llegaron bloques), y devuelve su duración en ms.
    pub fn finish(mut self, end: Instant) -> io::Result<u64> {
        let expected = (end.saturating_duration_since(self.session_start).as_secs_f64() * OUT_RATE as f64) as u64;
        let written = self.wav.samples();
        if expected > written {
            self.wav.write_silence(expected - written)?;
        }
        Ok(self.wav.finish()? * 1000 / OUT_RATE as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn stays_aligned_with_the_session_clock() {
        let path = std::env::temp_dir().join(format!("screencut-track-test-{}.wav", std::process::id()));
        let t0 = Instant::now();
        let mut track = Track::create(&path, t0).unwrap();
        let block = |at_ms: u64| PcmBlock {
            at: t0 + Duration::from_millis(at_ms),
            rate: 48_000,
            channels: 1,
            samples: vec![0.1; 4_800], // 100 ms
        };
        // Empieza a los 2 s: los primeros 2 s son silencio.
        track.push(&block(2_000)).unwrap();
        track.push(&block(2_100)).unwrap();
        // Hueco de ~3 s sin audio (WASAPI loopback cuando no suena nada).
        track.push(&block(5_200)).unwrap();
        // Termina a los 8 s sin más audio: se completa con silencio.
        let ms = track.finish(t0 + Duration::from_secs(8)).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(ms, 8_000);
    }
}
