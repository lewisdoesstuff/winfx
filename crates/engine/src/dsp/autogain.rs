//! Auto-gain: steers loudness toward a target, measured like a broadcast
//! meter (ITU-R BS.1770 K-weighting, 400 ms "momentary" window). Pauses
//! below the silence threshold freeze the gain, so room noise isn't pumped up.

use super::{Biquad, Coeffs, FilterKind, db_to_lin, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutogainParams {
    /// Loudness to aim for, LUFS.
    pub target_lufs: f32,
    /// Below this the input counts as silence and the gain holds.
    pub silence_lufs: f32,
    pub max_boost_db: f32,
    pub max_cut_db: f32,
    /// How quickly the gain follows, seconds.
    pub speed_s: f32,
}

impl Default for AutogainParams {
    fn default() -> Self {
        AutogainParams { target_lufs: -18.0, silence_lufs: -45.0, max_boost_db: 12.0, max_cut_db: 12.0, speed_s: 2.0 }
    }
}

const MOMENTARY_MS: f32 = 400.0;

#[derive(Clone, Debug)]
pub struct Autogain {
    shelf: Biquad,
    hpf: Biquad,
    ms_c: f32,
    ms: f32,
    target: f32,
    silence: f32,
    boost: f32,
    cut: f32,
    follow_c: f32,
    gain_db: f32,
}

impl Autogain {
    pub fn new(p: &AutogainParams, sr: f32) -> Self {
        let mut a = Autogain {
            // K-weighting: the BS.1770 pre-filter (high shelf) and RLB high-pass.
            shelf: Biquad::new(Coeffs::design(
                FilterKind::HighShelf,
                1681.97,
                std::f32::consts::FRAC_1_SQRT_2,
                4.0,
                sr,
            )),
            hpf: Biquad::new(Coeffs::design(FilterKind::HighPass, 38.13, 0.5003, 0.0, sr)),
            ms_c: time_coeff(MOMENTARY_MS, sr),
            ms: 0.0,
            target: 0.0,
            silence: 0.0,
            boost: 0.0,
            cut: 0.0,
            follow_c: 0.0,
            gain_db: 0.0,
        };
        a.set_params(p, sr);
        a
    }

    pub fn set_params(&mut self, p: &AutogainParams, sr: f32) {
        self.target = p.target_lufs;
        self.silence = p.silence_lufs;
        self.boost = p.max_boost_db.max(0.0);
        self.cut = p.max_cut_db.max(0.0);
        self.follow_c = time_coeff(p.speed_s.max(0.05) * 1000.0, sr);
    }

    /// Gain currently applied (signed dB).
    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }

    /// Momentary loudness of the input, LUFS.
    pub fn loudness(&self) -> f32 {
        -0.691 + 10.0 * (self.ms + 1e-12).log10()
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let k = self.hpf.process(self.shelf.process(x));
        self.ms = self.ms * self.ms_c + k * k * (1.0 - self.ms_c);
        let lufs = self.loudness();
        if lufs > self.silence {
            let want = (self.target - lufs).clamp(-self.cut, self.boost);
            self.gain_db = self.gain_db * self.follow_c + want * (1.0 - self.follow_c);
        }
        x * db_to_lin(self.gain_db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brings_a_quiet_voice_up_toward_target_and_holds_in_silence() {
        let p = AutogainParams::default();
        let mut a = Autogain::new(&p, 48_000.0);
        // 1 kHz at −30 dBFS RMS-ish: well under −18 LUFS.
        for i in 0..48_000 * 8 {
            a.process(0.045 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin());
        }
        let g = a.gain_db();
        assert!(g > 8.0 && g <= 12.0, "gain {g}");
        // Silence: the gain must not drift.
        for _ in 0..48_000 * 4 {
            a.process(0.0);
        }
        assert!((a.gain_db() - g).abs() < 0.5);
    }
}
