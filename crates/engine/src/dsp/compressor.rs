//! Feed-forward RMS compressor with soft knee.

use super::{db_to_lin, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub rms_ms: f32,
    #[serde(default)]
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        CompressorParams {
            threshold_db: -28.0,
            ratio: 3.0,
            attack_ms: 5.0,
            release_ms: 120.0,
            knee_db: 6.0,
            rms_ms: 10.0,
            makeup_db: 0.0,
        }
    }
}

/// Static gain curve: gain change in dB (≤ 0) for an input level in dB.
pub fn gain_computer(level_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let slope = 1.0 / ratio.max(1.0) - 1.0;
    let over = level_db - threshold_db;
    if knee_db > 0.0 && 2.0 * over.abs() <= knee_db {
        slope * (over + knee_db / 2.0).powi(2) / (2.0 * knee_db)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

#[derive(Clone, Debug)]
pub struct Compressor {
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    makeup: f32,
    rms_c: f32,
    att_c: f32,
    rel_c: f32,
    ms: f32,
    gr_db: f32,
}

impl Compressor {
    pub fn new(p: &CompressorParams, sr: f32) -> Self {
        let mut c = Compressor {
            threshold_db: 0.0,
            ratio: 1.0,
            knee_db: 0.0,
            makeup: 1.0,
            rms_c: 0.0,
            att_c: 0.0,
            rel_c: 0.0,
            ms: 0.0,
            gr_db: 0.0,
        };
        c.set_params(p, sr);
        c
    }

    pub fn set_params(&mut self, p: &CompressorParams, sr: f32) {
        self.threshold_db = p.threshold_db;
        self.ratio = p.ratio.max(1.0);
        self.knee_db = p.knee_db.max(0.0);
        self.makeup = db_to_lin(p.makeup_db);
        self.rms_c = time_coeff(p.rms_ms.max(0.01), sr);
        self.att_c = time_coeff(p.attack_ms, sr);
        self.rel_c = time_coeff(p.release_ms, sr);
    }

    /// Current gain reduction in positive dB.
    pub fn reduction_db(&self) -> f32 {
        -self.gr_db
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.ms = self.ms * self.rms_c + x * x * (1.0 - self.rms_c);
        let level_db = 10.0 * (self.ms + 1e-12).log10();
        let target = gain_computer(level_db, self.threshold_db, self.ratio, self.knee_db);
        let c = if target < self.gr_db { self.att_c } else { self.rel_c };
        self.gr_db = self.gr_db * c + target * (1.0 - c);
        x * db_to_lin(self.gr_db) * self.makeup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_curve() {
        // Well below: no change. Well above: (1/3 - 1) * over.
        assert_eq!(gain_computer(-60.0, -28.0, 3.0, 6.0), 0.0);
        assert!((gain_computer(-10.0, -28.0, 3.0, 6.0) - (-12.0)).abs() < 1e-4);
        // Knee is continuous at its edges.
        let lo = gain_computer(-31.0, -28.0, 3.0, 6.0);
        let hi = gain_computer(-25.0, -28.0, 3.0, 6.0);
        assert!(lo.abs() < 1e-5);
        assert!((hi - (-2.0)).abs() < 1e-4);
    }

    #[test]
    fn settles_to_static_curve_on_steady_tone() {
        let sr = 48_000.0;
        let mut c = Compressor::new(&CompressorParams::default(), sr);
        // Sine at peak 0.5 → RMS ≈ -9.03 dBFS → 19 dB over → ≈ -12.7 dB.
        for n in 0..48_000 {
            c.process(0.5 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / sr).sin());
        }
        let expected = -gain_computer(-9.03, -28.0, 3.0, 6.0);
        assert!((c.reduction_db() - expected).abs() < 0.3, "{} vs {expected}", c.reduction_db());
    }
}
