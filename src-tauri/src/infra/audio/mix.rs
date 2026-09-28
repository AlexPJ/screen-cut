//! Mezcla varias fuentes (micrófono y audio del sistema) en una sola pista
//! estéreo de 48 kHz para los vídeos. Se consume al ritmo del reloj: lo que
//! falta se rellena con silencio (el loopback de Windows no entrega nada
//! mientras no suena nada) y si una fuente acumula demasiado se descarta lo
//! más antiguo, para que el audio no se desfase del vídeo.

use super::PcmBlock;
use std::collections::VecDeque;

pub const RATE: u32 = 48_000;
/// Retraso máximo que se tolera en una fuente antes de descartar audio.
const MAX_BACKLOG_FRAMES: usize = RATE as usize / 5; // 200 ms

pub struct Mixer {
    inputs: Vec<Input>,
}

struct Input {
    /// Muestras estéreo intercaladas a 48 kHz.
    queue: VecDeque<f32>,
    resampler: ToStereo48k,
}

impl Mixer {
    pub fn new(inputs: usize) -> Self {
        Self { inputs: (0..inputs).map(|_| Input { queue: VecDeque::new(), resampler: ToStereo48k::default() }).collect() }
    }

    /// Añade un bloque de la fuente `input`.
    pub fn push(&mut self, input: usize, block: &PcmBlock) {
        let Some(input) = self.inputs.get_mut(input) else { return };
        input.resampler.process(block.rate, block.channels, &block.samples, &mut input.queue);
        let excess = input.queue.len().saturating_sub(MAX_BACKLOG_FRAMES * 2);
        input.queue.drain(..excess - excess % 2);
    }

    /// Descarta todo lo acumulado.
    pub fn clear(&mut self) {
        for input in &mut self.inputs {
            input.queue.clear();
        }
    }

    /// Saca `frames` fotogramas de audio mezclados, en PCM de 16 bits estéreo.
    pub fn pull(&mut self, frames: usize) -> Vec<i16> {
        let mut mix = vec![0f32; frames * 2];
        for input in &mut self.inputs {
            let n = input.queue.len().min(mix.len());
            for (out, s) in mix.iter_mut().zip(input.queue.drain(..n)) {
                *out += s;
            }
        }
        mix.into_iter().map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).collect()
    }
}

/// Pasa cualquier formato a estéreo de 48 kHz con interpolación lineal (de
/// sobra para voz y para el audio de una videollamada).
#[derive(Default)]
struct ToStereo48k {
    /// Posición del siguiente fotograma de salida, en fotogramas de entrada,
    /// relativa al primer fotograma del bloque actual (puede ser negativa: -1
    /// es el último del bloque anterior).
    pos: f64,
    last: (f32, f32),
    rate: u32,
}

impl ToStereo48k {
    fn process(&mut self, rate: u32, channels: u16, samples: &[f32], out: &mut VecDeque<f32>) {
        if rate == 0 || channels == 0 {
            return;
        }
        if rate != self.rate {
            *self = Self { rate, ..Self::default() };
        }
        let ch = channels as usize;
        let frame = |i: usize| -> (f32, f32) {
            let f = &samples[i * ch..i * ch + ch];
            if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) }
        };
        let frames = samples.len() / ch;
        if frames == 0 {
            return;
        }
        let step = rate as f64 / RATE as f64;
        // Se interpola entre `a` (fotograma floor(pos)) y el siguiente.
        while self.pos < frames as f64 - 1.0 {
            let i = self.pos.floor();
            let t = (self.pos - i) as f32;
            let a = if i < 0.0 { self.last } else { frame(i as usize) };
            let b = frame((i + 1.0) as usize);
            out.push_back(a.0 + (b.0 - a.0) * t);
            out.push_back(a.1 + (b.1 - a.1) * t);
            self.pos += step;
        }
        self.last = frame(frames - 1);
        self.pos -= frames as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn block(rate: u32, channels: u16, frames: usize, value: f32) -> PcmBlock {
        PcmBlock { at: Instant::now(), rate, channels, samples: vec![value; frames * channels as usize] }
    }

    #[test]
    fn resamples_to_48k_stereo_without_drift() {
        // 1 s a 44,1 kHz mono en bloques de 10 ms, consumido según llega.
        let mut m = Mixer::new(1);
        let mut total = 0;
        for _ in 0..100 {
            m.push(0, &block(44_100, 1, 441, 0.25));
            total += m.pull(m.inputs[0].queue.len() / 2).len() / 2;
        }
        assert!((total as i64 - 48_000).abs() <= 2, "{total}");
    }

    #[test]
    fn mixes_sources_and_fills_gaps_with_silence() {
        let mut m = Mixer::new(2);
        m.push(0, &block(48_000, 2, 100, 0.25));
        m.push(1, &block(48_000, 1, 50, 0.5));
        let out = m.pull(100);
        assert_eq!(out.len(), 200);
        let v = |s: i16| s as f32 / i16::MAX as f32;
        assert!((v(out[0]) - 0.75).abs() < 0.01, "las dos fuentes suman");
        assert!((v(out[150]) - 0.25).abs() < 0.01, "solo la primera");
        assert_eq!(m.pull(10), vec![0; 20], "sin datos, silencio");
        m.push(0, &block(48_000, 2, 100, 0.25));
        m.clear();
        assert_eq!(m.pull(10), vec![0; 20], "lo acumulado se descarta");
    }

    #[test]
    fn drops_old_audio_when_a_source_runs_ahead() {
        let mut m = Mixer::new(1);
        m.push(0, &block(48_000, 2, RATE as usize, 0.1)); // 1 s de golpe
        assert_eq!(m.inputs[0].queue.len(), MAX_BACKLOG_FRAMES * 2);
    }
}
