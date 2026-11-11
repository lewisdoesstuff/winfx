//! De-esser: tames sibilance by turning down the band above a split
//! frequency when it gets loud. "Split" mode only touches that band, using a
//! Linkwitz-Riley (LR4) crossover whose bands stay in phase and sum flat;
//! "Wide" turns down the whole signal.

use super::{db_to_lin, time_coeff, Biquad, Coeffs, FilterKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeesserMode {
    #[default]
    Split,
    Wide,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeesserParams {
    /// Sibilance starts above this frequency.
    pub freq_hz: f32,
    pub threshold_db: f32,
    pub ratio: f32,
    /// Most reduction applied (positive dB).
    pub range_db: f32,
    pub release_ms: f32,
    #[serde(default)]
    pub mode: DeesserMode,
}

impl Default for DeesserParams {
    fn default() -> Self {
        DeesserParams { freq_hz: 6000.0, threshold_db: -30.0, ratio: 4.0, range_db: 12.0, release_ms: 60.0, mode: DeesserMode::Split }
    }
}

const DETECT_MS: f32 = 2.0;
const ATTACK_MS: f32 = 0.5;

#[derive(Clone, Debug)]
pub struct Deesser {
    /// LR4 = two cascaded Butterworth sections per band.
    lp: [Biquad; 2],
    hp: [Biquad; 2],
    thr: f32,
    slope: f32,
    range: f32,
    split: bool,
    det_c: f32,
    att_c: f32,
    rel_c: f32,
    ms: f32,
    gr_db: f32,
}

impl Deesser {
    pub fn new(p: &DeesserParams, sr: f32) -> Self {
        let mut d = Deesser {
            lp: [Biquad::default(), Biquad::default()],
            hp: [Biquad::default(), Biquad::default()],
            thr: 0.0,
            slope: 0.0,
            range: 0.0,
            split: true,
            det_c: 0.0,
            att_c: 0.0,
            rel_c: 0.0,
            ms: 0.0,
            gr_db: 0.0,
        };
        d.set_params(p, sr);
        d
    }

    pub fn set_params(&mut self, p: &DeesserParams, sr: f32) {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let (lo, hi) = (
            Coeffs::design(FilterKind::LowPass, p.freq_hz, q, 0.0, sr),
            Coeffs::design(FilterKind::HighPass, p.freq_hz, q, 0.0, sr),
        );
        self.lp.iter_mut().for_each(|f| f.set_coeffs(lo));
        self.hp.iter_mut().for_each(|f| f.set_coeffs(hi));
        self.thr = p.threshold_db;
        self.slope = 1.0 - 1.0 / p.ratio.max(1.0);
        self.range = p.range_db.max(0.0);
        self.split = p.mode == DeesserMode::Split;
        self.det_c = time_coeff(DETECT_MS, sr);
        self.att_c = time_coeff(ATTACK_MS, sr);
        self.rel_c = time_coeff(p.release_ms, sr);
    }

    pub fn reduction_db(&self) -> f32 {
        -self.gr_db
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let h1 = self.hp[0].process(x);
        let h = self.hp[1].process(h1);
        let l1 = self.lp[0].process(x);
        let l = self.lp[1].process(l1);
        self.ms = self.ms * self.det_c + h * h * (1.0 - self.det_c);
        let level = 10.0 * (self.ms + 1e-12).log10();
        let target = if level > self.thr { -((level - self.thr) * self.slope).min(self.range) } else { 0.0 };
        let c = if target < self.gr_db { self.att_c } else { self.rel_c };
        self.gr_db = self.gr_db * c + target * (1.0 - c);
        let g = db_to_lin(self.gr_db);
        if self.split { l + g * h } else { (l + h) * g }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin()).collect()
    }

    fn peak_tail(d: &mut Deesser, x: &[f32]) -> f32 {
        let y: Vec<f32> = x.iter().map(|&s| d.process(s)).collect();
        y[y.len() / 2..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn cuts_loud_sibilance_but_not_lows() {
        let p = DeesserParams::default();
        // Loud 8 kHz "ess" (−6 dBFS) gets turned down.
        let mut d = Deesser::new(&p, 48_000.0);
        let ess = peak_tail(&mut d, &tone(8000.0, 0.5, 9600));
        assert!(ess < 0.5 * 0.5, "ess peak {ess}");
        assert!(d.reduction_db() > 6.0);
        // A loud 200 Hz vowel passes untouched in split mode.
        let mut d = Deesser::new(&p, 48_000.0);
        let low = peak_tail(&mut d, &tone(200.0, 0.5, 9600));
        assert!((low - 0.5).abs() < 0.02, "low peak {low}");
    }
}
