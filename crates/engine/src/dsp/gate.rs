//! Noise gate with hysteresis, hold, range and a high-passed sidechain.

use super::{Biquad, Coeffs, FilterKind, db_to_lin, lin_to_db, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GateParams {
    /// Opens when the sidechain level rises above this.
    pub threshold_db: f32,
    /// Closes only once the level falls this far below the threshold.
    pub hysteresis_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    /// Attenuation when closed (positive dB).
    pub range_db: f32,
    /// High-pass on the detector so rumble doesn't open the gate. 0 = off.
    pub sidechain_hpf_hz: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        GateParams {
            threshold_db: -50.0,
            hysteresis_db: 6.0,
            attack_ms: 1.0,
            hold_ms: 80.0,
            release_ms: 200.0,
            range_db: 24.0,
            sidechain_hpf_hz: 100.0,
        }
    }
}

const DETECTOR_RELEASE_MS: f32 = 10.0;

#[derive(Clone, Debug)]
pub struct Gate {
    sc: Biquad,
    sc_on: bool,
    env: f32,
    env_c: f32,
    open_thr: f32,
    close_thr: f32,
    hold_samples: u32,
    hold_left: u32,
    open: bool,
    floor: f32,
    att_c: f32,
    rel_c: f32,
    gain: f32,
}

impl Gate {
    pub fn new(p: &GateParams, sr: f32) -> Self {
        let mut g = Gate {
            sc: Biquad::default(),
            sc_on: false,
            env: 0.0,
            env_c: 0.0,
            open_thr: 0.0,
            close_thr: 0.0,
            hold_samples: 0,
            hold_left: 0,
            open: false,
            floor: 1.0,
            att_c: 0.0,
            rel_c: 0.0,
            gain: 1.0,
        };
        g.set_params(p, sr);
        g.gain = g.floor;
        g
    }

    pub fn set_params(&mut self, p: &GateParams, sr: f32) {
        self.sc_on = p.sidechain_hpf_hz > 0.0;
        if self.sc_on {
            self.sc.set_coeffs(Coeffs::design(
                FilterKind::HighPass,
                p.sidechain_hpf_hz,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                sr,
            ));
        }
        self.env_c = time_coeff(DETECTOR_RELEASE_MS, sr);
        self.open_thr = db_to_lin(p.threshold_db);
        self.close_thr = db_to_lin(p.threshold_db - p.hysteresis_db.max(0.0));
        self.hold_samples = (p.hold_ms.max(0.0) * 0.001 * sr) as u32;
        self.floor = db_to_lin(-p.range_db.max(0.0));
        self.att_c = time_coeff(p.attack_ms, sr);
        self.rel_c = time_coeff(p.release_ms, sr);
    }

    /// Current gain reduction in positive dB.
    pub fn reduction_db(&self) -> f32 {
        -lin_to_db(self.gain)
    }

    /// The detector envelope (linear) that's compared against the thresholds.
    pub fn detector_level(&self) -> f32 {
        self.env
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let s = if self.sc_on { self.sc.process(x) } else { x }.abs();
        self.env = if s > self.env { s } else { self.env * self.env_c + s * (1.0 - self.env_c) };

        if self.env >= self.open_thr {
            self.open = true;
            self.hold_left = self.hold_samples;
        } else if self.open {
            if self.env < self.close_thr {
                if self.hold_left > 0 {
                    self.hold_left -= 1;
                } else {
                    self.open = false;
                }
            } else {
                self.hold_left = self.hold_samples;
            }
        }

        let target = if self.open { 1.0 } else { self.floor };
        let c = if target > self.gain { self.att_c } else { self.rel_c };
        self.gain = self.gain * c + target * (1.0 - c);
        x * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(n: usize, amp: f32) -> f32 {
        amp * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin()
    }

    #[test]
    fn opens_on_loud_and_closes_after_hold_and_release() {
        let mut g = Gate::new(&GateParams::default(), SR);
        // Quiet (-70 dB): stays closed at -24 dB.
        for n in 0..4800 {
            g.process(sine(n, db_to_lin(-70.0)));
        }
        assert!(!g.is_open());
        assert!((g.reduction_db() - 24.0).abs() < 0.5);
        // Loud (-20 dB): opens fast.
        for n in 0..480 {
            g.process(sine(n, 0.1));
        }
        assert!(g.is_open());
        assert!(g.reduction_db() < 0.5);
        // -53 dB is between close (-56) and open (-50): hysteresis keeps it open.
        for n in 0..48_000 {
            g.process(sine(n, db_to_lin(-53.0)));
        }
        assert!(g.is_open());
        // Silence: closes after hold (80 ms) and fades over release.
        for _ in 0..(48 * 60) {
            g.process(0.0);
        }
        assert!(g.is_open(), "should still be holding");
        for _ in 0..(48 * 1000) {
            g.process(0.0);
        }
        assert!(!g.is_open());
        assert!(g.reduction_db() > 23.0);
    }
}
