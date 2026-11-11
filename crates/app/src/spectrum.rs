//! Spectrum analyzer for the EQ graph: FFT of the engine's output scope,
//! mapped onto the graph's log-frequency columns, with peak-fall smoothing.

use engine::tap::Tap;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

const N_FFT: usize = 4096;
/// Display columns, matching the EQ curve's resolution.
pub const COLUMNS: usize = 240;
/// dB per second the display falls back by.
const FALL_DB_PER_S: f32 = 30.0;
pub const FLOOR_DB: f32 = -96.0;

pub struct Analyzer {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    norm: f32,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Smoothed level per column, dB re full scale.
    pub db: Vec<f32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT);
        let window: Vec<f32> = (0..N_FFT)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
            .collect();
        // A full-scale sine reads 0 dB.
        let norm = 2.0 / window.iter().sum::<f32>();
        Analyzer {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            window,
            norm,
            db: vec![FLOOR_DB; COLUMNS],
        }
    }
}

/// Column i's centre frequency on the graph's 20 Hz–20 kHz log axis.
pub fn column_freq(i: f32) -> f32 {
    20.0 * 1000f32.powf(i / (COLUMNS - 1) as f32)
}

impl Analyzer {
    pub fn update(&mut self, tap: &Tap, dt: f32) {
        tap.read_scope(&mut self.input);
        for (x, w) in self.input.iter_mut().zip(&self.window) {
            *x *= w;
        }
        if self.fft.process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch).is_err() {
            return;
        }
        let sr = engine::SAMPLE_RATE as f32;
        let bin_hz = sr / N_FFT as f32;
        let mag_db = |k: usize| {
            let m = self.output[k.min(self.output.len() - 1)].norm() * self.norm;
            20.0 * m.max(1e-9).log10()
        };
        let fall = FALL_DB_PER_S * dt;
        for i in 0..COLUMNS {
            let lo = column_freq(i as f32 - 0.5) / bin_hz;
            let hi = column_freq(i as f32 + 0.5) / bin_hz;
            let (k0, k1) = (lo.floor() as usize, hi.ceil() as usize);
            // Wide columns: strongest bin inside. Narrow (low frequencies): nearest bin.
            let level = if k1 > k0 + 1 {
                (k0 + 1..=k1.min(self.output.len() - 1)).map(mag_db).fold(FLOOR_DB, f32::max)
            } else {
                mag_db(((lo + hi) / 2.0).round() as usize)
            }
            .max(FLOOR_DB);
            let d = &mut self.db[i];
            *d = if level > *d { level } else { (*d - fall).max(level) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_peaks_in_its_column_at_its_level() {
        let tap = Tap::default();
        let sr = engine::SAMPLE_RATE as f32;
        let block: Vec<f32> = (0..8192).map(|n| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / sr).sin()).collect();
        // Fill the scope through the same path the capture thread uses.
        tap.scope_write_for_test(&block);
        let mut a = Analyzer::default();
        a.update(&tap, 0.033);
        let (peak_col, peak_db) = a.db.iter().enumerate().fold((0, FLOOR_DB), |m, (i, d)| if *d > m.1 { (i, *d) } else { m });
        let f = column_freq(peak_col as f32);
        assert!((900.0..1100.0).contains(&f), "peak at {f} Hz");
        assert!((peak_db + 6.0).abs() < 1.0, "0.5 amplitude ≈ −6 dBFS, got {peak_db}");
        // Far from the tone the window's sidelobes are well down.
        assert!(a.db[0] < -60.0 && a.db[COLUMNS - 1] < -60.0);
    }
}
