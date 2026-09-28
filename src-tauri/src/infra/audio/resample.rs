//! Conversión a 16 kHz mono, el formato que usa Whisper. Para voz basta con un
//! paso bajo FIR (sinc con ventana de Blackman, corte a 7,2 kHz) seguido de
//! interpolación lineal: la entrada ya está filtrada muy por debajo de la
//! frecuencia de Nyquist de la salida, así que no aparece aliasing.

pub const OUT_RATE: u32 = 16_000;
const CUTOFF_HZ: f64 = 7_200.0;

pub struct ToMono16k {
    /// Paso entre muestras de salida, en muestras de entrada.
    step: f64,
    taps: Vec<f32>,
    /// Últimas entradas (mono) para continuar el filtro entre bloques.
    history: Vec<f32>,
    /// Última muestra filtrada del bloque anterior.
    prev: f32,
    /// Posición de la próxima salida; 0 = `prev`, 1 = primera muestra del bloque.
    t: f64,
}

impl ToMono16k {
    pub fn new(in_rate: u32) -> Self {
        let taps = if in_rate > OUT_RATE { lowpass(CUTOFF_HZ / in_rate as f64, 97) } else { vec![1.0] };
        Self {
            step: in_rate as f64 / OUT_RATE as f64,
            history: vec![0.0; taps.len() - 1],
            taps,
            prev: 0.0,
            t: 1.0,
        }
    }

    /// Convierte un bloque intercalado de `channels` canales y añade la salida a `out`.
    pub fn process(&mut self, interleaved: &[f32], channels: u16, out: &mut Vec<f32>) {
        let ch = channels.max(1) as usize;
        let mono = interleaved.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32);
        self.history.extend(mono);
        let n_taps = self.taps.len();
        let filtered: Vec<f32> = self
            .history
            .windows(n_taps)
            .map(|w| w.iter().rev().zip(&self.taps).map(|(x, h)| x * h).sum())
            .collect();
        let keep = self.history.len() - (n_taps - 1);
        self.history.drain(..keep);

        // b[0] = prev, b[1..] = filtered
        let len = filtered.len() as f64;
        let at = |i: usize| if i == 0 { self.prev } else { filtered[i - 1] };
        while self.t <= len {
            let i = self.t.floor() as usize;
            let frac = (self.t - i as f64) as f32;
            let a = at(i);
            let b = if (i as f64) < len { at(i + 1) } else { a };
            out.push(a + (b - a) * frac);
            self.t += self.step;
        }
        if let Some(&last) = filtered.last() {
            self.prev = last;
            self.t -= len;
        }
    }
}

/// Paso bajo de `n` coeficientes (impar) con corte normalizado `fc` (ciclos por muestra).
fn lowpass(fc: f64, n: usize) -> Vec<f32> {
    use std::f64::consts::PI;
    let m = (n - 1) as f64;
    let mut taps: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * x).sin() / (PI * x) };
            let window = 0.42 - 0.5 * (2.0 * PI * i as f64 / m).cos() + 0.08 * (4.0 * PI * i as f64 / m).cos();
            sinc * window
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= sum);
    taps.into_iter().map(|t| t as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: u32, secs: f64, channels: u16) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32;
                std::iter::repeat_n(v, channels as usize)
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn keeps_speech_and_length_across_blocks() {
        for rate in [48_000, 44_100] {
            let input = tone(1_000.0, rate, 1.0, 2);
            let mut r = ToMono16k::new(rate);
            let mut out = Vec::new();
            for block in input.chunks(2 * 441) {
                r.process(block, 2, &mut out);
            }
            assert!((out.len() as i64 - 16_000).abs() <= 2, "{rate}: {} muestras", out.len());
            let level = rms(&out[1_000..]);
            assert!((level - 0.707).abs() < 0.02, "{rate}: rms {level}");
        }
    }

    #[test]
    fn removes_frequencies_above_nyquist() {
        let mut r = ToMono16k::new(48_000);
        let mut out = Vec::new();
        r.process(&tone(12_000.0, 48_000, 1.0, 1), 1, &mut out);
        assert!(rms(&out[1_000..]) < 0.01);
    }
}
