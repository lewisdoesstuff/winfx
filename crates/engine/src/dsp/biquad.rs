//! RBJ "Audio EQ Cookbook" biquads, transposed direct form II in f64.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterKind {
    HighPass,
    LowPass,
    LowShelf,
    HighShelf,
    Peak,
    Notch,
    BandPass,
    AllPass,
}

impl FilterKind {
    /// Does this filter type have a gain parameter?
    pub fn has_gain(self) -> bool {
        matches!(self, FilterKind::Peak | FilterKind::LowShelf | FilterKind::HighShelf)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    pub const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    pub fn design(kind: FilterKind, freq: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        let sr = sr as f64;
        let freq = (freq as f64).clamp(1.0, sr * 0.49);
        let q = (q as f64).max(0.01);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let w0 = 2.0 * PI * freq / sr;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            FilterKind::HighPass => {
                let b = (1.0 + cos) / 2.0;
                (b, -(1.0 + cos), b, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            FilterKind::LowPass => {
                let b = (1.0 - cos) / 2.0;
                (b, 1.0 - cos, b, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
            }
            FilterKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::AllPass => (1.0 - alpha, -2.0 * cos, 1.0 + alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::Peak => {
                (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
            }
            FilterKind::LowShelf => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - sq),
                    (a + 1.0) + (a - 1.0) * cos + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - sq,
                )
            }
            FilterKind::HighShelf => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - sq),
                    (a + 1.0) - (a - 1.0) * cos + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - sq,
                )
            }
        };
        Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    /// Magnitude response in dB at `freq`; used for drawing EQ curves.
    pub fn magnitude_db(&self, freq: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * freq as f64 / sr as f64;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        // H(e^jw) with z^-1 = e^-jw
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let mag2 = (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300);
        (10.0 * mag2.max(1e-30).log10()) as f32
    }
}

#[derive(Clone, Debug)]
pub struct Biquad {
    c: Coeffs,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Biquad { c: Coeffs::IDENTITY, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    pub fn new(c: Coeffs) -> Self {
        Biquad { c, z1: 0.0, z2: 0.0 }
    }

    /// Change coefficients, keeping filter state (no click on parameter tweaks).
    pub fn set_coeffs(&mut self, c: Coeffs) {
        self.c = c;
    }

    pub fn coeffs(&self) -> &Coeffs {
        &self.c
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = x as f64;
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn peak_hits_gain_at_center() {
        let c = Coeffs::design(FilterKind::Peak, 5800.0, 1.5, -4.0, SR);
        assert!((c.magnitude_db(5800.0, SR) + 4.0).abs() < 0.01);
        assert!(c.magnitude_db(100.0, SR).abs() < 0.1);
    }

    #[test]
    fn shelves_reach_gain() {
        let lo = Coeffs::design(FilterKind::LowShelf, 160.0, 0.707, 3.0, SR);
        assert!((lo.magnitude_db(20.0, SR) - 3.0).abs() < 0.2);
        assert!(lo.magnitude_db(5000.0, SR).abs() < 0.1);
        let hi = Coeffs::design(FilterKind::HighShelf, 9000.0, 0.707, -2.0, SR);
        assert!((hi.magnitude_db(20000.0, SR) + 2.0).abs() < 0.2);
        assert!(hi.magnitude_db(500.0, SR).abs() < 0.1);
    }

    #[test]
    fn notch_bandpass_allpass() {
        let n = Coeffs::design(FilterKind::Notch, 1000.0, 5.0, 0.0, SR);
        assert!(n.magnitude_db(1000.0, SR) < -40.0);
        assert!(n.magnitude_db(100.0, SR).abs() < 0.1);
        let bp = Coeffs::design(FilterKind::BandPass, 1000.0, 1.0, 0.0, SR);
        assert!(bp.magnitude_db(1000.0, SR).abs() < 0.01);
        assert!(bp.magnitude_db(50.0, SR) < -20.0);
        let ap = Coeffs::design(FilterKind::AllPass, 1000.0, 0.7, 0.0, SR);
        for f in [50.0, 1000.0, 15_000.0] {
            assert!(ap.magnitude_db(f, SR).abs() < 0.01);
        }
    }

    #[test]
    fn highpass_is_minus_3db_at_cutoff_and_matches_response() {
        let c = Coeffs::design(FilterKind::HighPass, 80.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, SR);
        assert!((c.magnitude_db(80.0, SR) + 3.01).abs() < 0.05);
        assert!(c.magnitude_db(20.0, SR) < -20.0);
        // Run a 1 kHz sine through it: passband, unity.
        let mut f = Biquad::new(c);
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let y = f.process((2.0 * std::f32::consts::PI * 1000.0 * n as f32 / SR).sin());
            if n > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - 1.0).abs() < 0.01, "peak {peak}");
    }
}
