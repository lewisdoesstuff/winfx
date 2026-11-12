//! Downward expander: a gentler gate. Below the threshold the level drops
//! by `ratio` (2:1 → 1 dB under = 2 dB under), with a soft knee and a floor.

use super::{db_to_lin, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExpanderParams {
    pub threshold_db: f32,
    pub ratio: f32,
    /// Most reduction applied (positive dB).
    pub range_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub rms_ms: f32,
}

impl Default for ExpanderParams {
    fn default() -> Self {
        ExpanderParams {
            threshold_db: -45.0,
            ratio: 2.0,
            range_db: 20.0,
            attack_ms: 2.0,
            release_ms: 150.0,
            knee_db: 6.0,
            rms_ms: 10.0,
        }
    }
}

/// Gain change (≤ 0 dB) for a level, threshold, ratio, knee.
pub fn gain_computer(level_db: f32, threshold_db: f32, ratio: f32, knee_db: f32) -> f32 {
    let k = ratio.max(1.0) - 1.0;
    let over = level_db - threshold_db;
    if knee_db > 0.0 && 2.0 * over.abs() <= knee_db {
        -k * (over - knee_db / 2.0).powi(2) / (2.0 * knee_db)
    } else if over < 0.0 {
        k * over
    } else {
        0.0
    }
}

#[derive(Clone, Debug)]
pub struct Expander {
    thr: f32,
    ratio: f32,
    knee: f32,
    range: f32,
    rms_c: f32,
    att_c: f32,
    rel_c: f32,
    ms: f32,
    gr_db: f32,
}

impl Expander {
    pub fn new(p: &ExpanderParams, sr: f32) -> Self {
        let mut e = Expander {
            thr: 0.0,
            ratio: 1.0,
            knee: 0.0,
            range: 0.0,
            rms_c: 0.0,
            att_c: 0.0,
            rel_c: 0.0,
            ms: 0.0,
            gr_db: 0.0,
        };
        e.set_params(p, sr);
        e.gr_db = -e.range;
        e
    }

    pub fn set_params(&mut self, p: &ExpanderParams, sr: f32) {
        self.thr = p.threshold_db;
        self.ratio = p.ratio.max(1.0);
        self.knee = p.knee_db.max(0.0);
        self.range = p.range_db.max(0.0);
        self.rms_c = time_coeff(p.rms_ms.max(0.1), sr);
        self.att_c = time_coeff(p.attack_ms, sr);
        self.rel_c = time_coeff(p.release_ms, sr);
    }

    pub fn reduction_db(&self) -> f32 {
        -self.gr_db
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.ms = self.ms * self.rms_c + x * x * (1.0 - self.rms_c);
        let level = 10.0 * (self.ms + 1e-12).log10();
        let target = gain_computer(level, self.thr, self.ratio, self.knee).max(-self.range);
        // Opening (gain rising) uses attack; closing uses release.
        let c = if target > self.gr_db { self.att_c } else { self.rel_c };
        self.gr_db = self.gr_db * c + target * (1.0 - c);
        x * db_to_lin(self.gr_db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_is_continuous_and_bounded() {
        assert_eq!(gain_computer(-20.0, -45.0, 2.0, 6.0), 0.0);
        assert!((gain_computer(-55.0, -45.0, 2.0, 6.0) - (-10.0)).abs() < 1e-4);
        assert!(gain_computer(-42.0, -45.0, 2.0, 6.0).abs() < 1e-5);
        assert!((gain_computer(-48.0, -45.0, 2.0, 6.0) - (-3.0)).abs() < 1e-4);
    }

    #[test]
    fn quiet_signal_is_turned_down_by_range_at_most() {
        let p = ExpanderParams::default();
        let mut e = Expander::new(&p, 48_000.0);
        for i in 0..48_000 {
            e.process(0.0001 * (i as f32 * 0.05).sin()); // ≈ −83 dB RMS: 38 dB under → clamped to range
        }
        assert!((e.reduction_db() - p.range_db).abs() < 0.5, "{}", e.reduction_db());
        for i in 0..48_000 {
            e.process(0.3 * (i as f32 * 0.05).sin());
        }
        assert!(e.reduction_db() < 0.1);
    }
}
