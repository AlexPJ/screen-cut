//! Escritor WAV mínimo: PCM de 16 bits, mono. La cabecera se reescribe cada
//! pocos segundos, así que si la app se cierra de golpe el archivo sigue siendo
//! válido y solo se pierden los últimos segundos.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub struct WavWriter {
    out: BufWriter<File>,
    rate: u32,
    samples: u64,
    since_header: u64,
}

impl WavWriter {
    pub fn create(path: &Path, rate: u32) -> io::Result<Self> {
        let mut w = Self { out: BufWriter::new(File::create(path)?), rate, samples: 0, since_header: 0 };
        w.write_header()?;
        Ok(w)
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            self.out.write_all(&v.to_le_bytes())?;
        }
        self.samples += samples.len() as u64;
        self.since_header += samples.len() as u64;
        if self.since_header >= self.rate as u64 * 5 {
            self.write_header()?;
        }
        Ok(())
    }

    pub fn write_silence(&mut self, count: u64) -> io::Result<()> {
        const ZEROS: [f32; 1024] = [0.0; 1024];
        let mut left = count;
        while left > 0 {
            let n = left.min(ZEROS.len() as u64) as usize;
            self.write(&ZEROS[..n])?;
            left -= n as u64;
        }
        Ok(())
    }

    fn write_header(&mut self) -> io::Result<()> {
        let data = (self.samples * 2).min(u32::MAX as u64 - 36) as u32;
        let pos = self.out.stream_position()?;
        self.out.seek(SeekFrom::Start(0))?;
        let mut h = Vec::with_capacity(44);
        h.extend(b"RIFF");
        h.extend((36 + data).to_le_bytes());
        h.extend(b"WAVEfmt ");
        h.extend(16u32.to_le_bytes()); // tamaño del bloque fmt
        h.extend(1u16.to_le_bytes()); // PCM
        h.extend(1u16.to_le_bytes()); // mono
        h.extend(self.rate.to_le_bytes());
        h.extend((self.rate * 2).to_le_bytes()); // bytes por segundo
        h.extend(2u16.to_le_bytes()); // bytes por muestra
        h.extend(16u16.to_le_bytes()); // bits
        h.extend(b"data");
        h.extend(data.to_le_bytes());
        self.out.write_all(&h)?;
        if pos > 0 {
            self.out.seek(SeekFrom::Start(pos))?;
        }
        self.out.flush()?;
        self.since_header = 0;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<u64> {
        self.write_header()?;
        Ok(self.samples)
    }
}

/// Lee hasta `max` muestras a partir de la muestra `from` de un WAV escrito por
/// `WavWriter`, aunque se siga grabando: se fía del tamaño del archivo y no de
/// la cabecera, que va unos segundos por detrás.
pub fn read_samples(path: &Path, from: u64, max: usize) -> io::Result<Vec<f32>> {
    use std::io::Read;
    let mut file = File::open(path)?;
    let available = file.metadata()?.len().saturating_sub(HEADER) / 2;
    let count = available.saturating_sub(from).min(max as u64) as usize;
    file.seek(SeekFrom::Start(HEADER + from * 2))?;
    let mut bytes = vec![0u8; count * 2];
    file.read_exact(&mut bytes)?;
    Ok(bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
        .collect())
}

/// Muestras que hay ya en disco.
pub fn available_samples(path: &Path) -> io::Result<u64> {
    Ok(std::fs::metadata(path)?.len().saturating_sub(HEADER) / 2)
}

const HEADER: u64 = 44;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_header() {
        let path = std::env::temp_dir().join(format!("screencut-wav-test-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path, 16_000).unwrap();
        w.write(&[0.0, 0.5, -1.0]).unwrap();
        w.write_silence(2_000).unwrap();
        assert_eq!(w.finish().unwrap(), 2_003);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 2_003 * 2);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 2_003 * 2);
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), i16::MAX / 2);
    }

    #[test]
    fn reads_back_a_range() {
        let path = std::env::temp_dir().join(format!("screencut-wav-read-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path, 16_000).unwrap();
        w.write(&[0.0, 0.5, -0.5, 0.25]).unwrap();
        w.finish().unwrap();
        assert_eq!(available_samples(&path).unwrap(), 4);
        let got = read_samples(&path, 1, 10).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(got.len(), 3);
        assert!((got[0] - 0.5).abs() < 1e-3 && (got[2] - 0.25).abs() < 1e-3);
    }
}
