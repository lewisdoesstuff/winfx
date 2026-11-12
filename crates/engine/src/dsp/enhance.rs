//! Harmonic enhancers: generate harmonics from one band and blend them back.
//!
//! - **Exciter** (presence / air): band above `freq_hz` → asymmetric soft
//!   saturation → band-limited back to that band → mixed in.
//! - **Bass enhancer** (warmth): band below `freq_hz` → saturation → keep the
//!   new harmonics (up to a few times `freq_hz`, above a subsonic floor) → mixed in.

use super::{Biquad, Coeffs, FilterKind, db_to_lin};
use serde::{Deserialize, Serialize};

const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// Small offset into tanh makes it asymmetric → even harmonics too (warmer).
const BIAS: f32 = 0.2;

#[inline]
fn saturate(x: f32, drive: f32) -> f32 {
    ((x * drive + BIAS).tanh() - BIAS.tanh()) / drive.tanh().max(1e-3)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExciterParams {
    /// Harmonics level relative to the input (dB).
    pub amount_db: f32,
    pub drive: f32,
    /// Harmonics are generated from, and kept above, this frequency.
    pub freq_hz: f32,
    /// Harmonics are rolled off above this (tames fizz).
    pub ceiling_hz: f32,
}

impl Default for ExciterParams {
    fn default() -> Self {
        ExciterParams { amount_db: -14.0, drive: 4.0, freq_hz: 4000.0, ceiling_hz: 15_000.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Exciter {
    pre: [Biquad; 2],
    post: [Biquad; 2],
    top: Biquad,
    amount: f32,
    drive: f32,
}

impl Exciter {
    pub fn new(p: &ExciterParams, sr: f32) -> Self {
        let mut e = Exciter {
            pre: Default::default(),
            post: Default::default(),
            top: Biquad::default(),
            amount: 0.0,
            drive: 1.0,
        };
        e.set_params(p, sr);
        e
    }

    pub fn set_params(&mut self, p: &ExciterParams, sr: f32) {
        let hp = Coeffs::design(FilterKind::HighPass, p.freq_hz, Q, 0.0, sr);
        self.pre.iter_mut().for_each(|f| f.set_coeffs(hp));
        self.post.iter_mut().for_each(|f| f.set_coeffs(hp));
        self.top.set_coeffs(Coeffs::design(FilterKind::LowPass, p.ceiling_hz.max(p.freq_hz * 1.5), Q, 0.0, sr));
        self.amount = db_to_lin(p.amount_db);
        self.drive = p.drive.clamp(0.5, 20.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let b = self.pre[0].process(x);
        let band = self.pre[1].process(b);
        let h = saturate(band, self.drive) - band; // only what saturation added
        let h = self.post[0].process(h);
        let h = self.post[1].process(h);
        x + self.top.process(h) * self.amount
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BassEnhancerParams {
    pub amount_db: f32,
    pub drive: f32,
    /// Harmonics are generated from the band below this.
    pub freq_hz: f32,
    /// Subsonic floor: nothing is added below this.
    pub floor_hz: f32,
}

impl Default for BassEnhancerParams {
    fn default() -> Self {
        BassEnhancerParams { amount_db: -12.0, drive: 3.0, freq_hz: 120.0, floor_hz: 40.0 }
    }
}

#[derive(Clone, Debug)]
pub struct BassEnhancer {
    pre: [Biquad; 2],
    keep_lo: Biquad,
    keep_hi: Biquad,
    amount: f32,
    drive: f32,
}

impl BassEnhancer {
    pub fn new(p: &BassEnhancerParams, sr: f32) -> Self {
        let mut b = BassEnhancer {
            pre: Default::default(),
            keep_lo: Biquad::default(),
            keep_hi: Biquad::default(),
            amount: 0.0,
            drive: 1.0,
        };
        b.set_params(p, sr);
        b
    }

    pub fn set_params(&mut self, p: &BassEnhancerParams, sr: f32) {
        let lp = Coeffs::design(FilterKind::LowPass, p.freq_hz, Q, 0.0, sr);
        self.pre.iter_mut().for_each(|f| f.set_coeffs(lp));
        // Keep the generated harmonics: from the floor up to ~4× the band.
        self.keep_lo.set_coeffs(Coeffs::design(FilterKind::HighPass, p.floor_hz.max(10.0), Q, 0.0, sr));
        self.keep_hi.set_coeffs(Coeffs::design(FilterKind::LowPass, (p.freq_hz * 4.0).min(sr * 0.45), Q, 0.0, sr));
        self.amount = db_to_lin(p.amount_db);
        self.drive = p.drive.clamp(0.5, 20.0);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let b = self.pre[0].process(x);
        let band = self.pre[1].process(b);
        let h = saturate(band, self.drive) - band;
        x + self.keep_hi.process(self.keep_lo.process(h)) * self.amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Energy at `f` in `y` (single-bin DFT), dB.
    fn level_at(y: &[f32], f: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &v) in y.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * f as f64 * n as f64 / 48_000.0;
            re += v as f64 * w.cos();
            im -= v as f64 * w.sin();
        }
        20.0 * ((re * re + im * im).sqrt() * 2.0 / y.len() as f64).max(1e-12).log10() as f32
    }

    fn run(mut f: impl FnMut(f32) -> f32, freq: f32) -> Vec<f32> {
        let x: Vec<f32> =
            (0..48_000).map(|n| 0.3 * (2.0 * std::f32::consts::PI * freq * n as f32 / 48_000.0).sin()).collect();
        x.into_iter().map(&mut f).collect::<Vec<_>>()[24_000..].to_vec()
    }

    #[test]
    fn exciter_adds_harmonics_above_its_band_only() {
        let mut e = Exciter::new(&ExciterParams { amount_db: -6.0, ..Default::default() }, 48_000.0);
        let y = run(|s| e.process(s), 5000.0);
        assert!(level_at(&y, 10_000.0) > -45.0, "2nd harmonic {}", level_at(&y, 10_000.0));
        // A low tone is outside the band: nothing is added.
        let mut e = Exciter::new(&ExciterParams { amount_db: -6.0, ..Default::default() }, 48_000.0);
        let y = run(|s| e.process(s), 200.0);
        assert!(level_at(&y, 400.0) < -80.0);
    }

    #[test]
    fn bass_enhancer_adds_low_harmonics() {
        let mut b = BassEnhancer::new(&BassEnhancerParams { amount_db: -6.0, ..Default::default() }, 48_000.0);
        let y = run(|s| b.process(s), 80.0);
        assert!(level_at(&y, 160.0) > -45.0, "2nd harmonic {}", level_at(&y, 160.0));
        assert!(level_at(&y, 240.0) > -60.0, "3rd harmonic {}", level_at(&y, 240.0));
    }
}
