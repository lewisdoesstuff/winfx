//! Smoothed gain stage.

use super::{db_to_lin, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GainParams {
    pub gain_db: f32,
}

impl Default for GainParams {
    fn default() -> Self {
        GainParams { gain_db: 0.0 }
    }
}

const SMOOTH_MS: f32 = 10.0;

#[derive(Clone, Debug)]
pub struct Gain {
    target: f32,
    current: f32,
    c: f32,
}

impl Gain {
    pub fn new(p: &GainParams, sr: f32) -> Self {
        let target = db_to_lin(p.gain_db);
        Gain { target, current: target, c: time_coeff(SMOOTH_MS, sr) }
    }

    pub fn set_params(&mut self, p: &GainParams, sr: f32) {
        self.target = db_to_lin(p.gain_db);
        self.c = time_coeff(SMOOTH_MS, sr);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.current = self.current * self.c + self.target * (1.0 - self.c);
        x * self.current
    }
}
