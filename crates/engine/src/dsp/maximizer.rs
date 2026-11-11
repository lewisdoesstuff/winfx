//! Maximizer: push the level up by `-threshold` dB into a look-ahead peak
//! limiter, so the voice gets louder and denser without clipping.

use super::{db_to_lin, Limiter, LimiterParams};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaximizerParams {
    /// Signal above this (dBFS) gets limited; the gain is −threshold.
    pub threshold_db: f32,
    pub ceiling_db: f32,
    pub release_ms: f32,
}

impl Default for MaximizerParams {
    fn default() -> Self {
        MaximizerParams { threshold_db: -6.0, ceiling_db: -0.5, release_ms: 80.0 }
    }
}

const LOOKAHEAD_MS: f32 = 1.0;

#[derive(Clone, Debug)]
pub struct Maximizer {
    boost: f32,
    limiter: Limiter,
}

impl Maximizer {
    fn limiter_params(p: &MaximizerParams) -> LimiterParams {
        LimiterParams { ceiling_db: p.ceiling_db.min(0.0), release_ms: p.release_ms, lookahead_ms: LOOKAHEAD_MS }
    }

    pub fn new(p: &MaximizerParams, sr: f32) -> Self {
        Maximizer { boost: db_to_lin(-p.threshold_db.min(0.0)), limiter: Limiter::new(&Self::limiter_params(p), sr) }
    }

    pub fn set_params(&mut self, p: &MaximizerParams, sr: f32) {
        self.boost = db_to_lin(-p.threshold_db.min(0.0));
        self.limiter.set_params(&Self::limiter_params(p), sr);
    }

    pub fn reduction_db(&self) -> f32 {
        self.limiter.reduction_db()
    }

    pub fn latency_ms() -> f32 {
        LOOKAHEAD_MS
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.limiter.process(x * self.boost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn louder_but_never_over_the_ceiling() {
        let p = MaximizerParams::default();
        let mut m = Maximizer::new(&p, 48_000.0);
        let ceil = db_to_lin(p.ceiling_db);
        let mut peak = 0.0f32;
        for i in 0..48_000 {
            let y = m.process(0.6 * (2.0 * std::f32::consts::PI * 300.0 * i as f32 / 48_000.0).sin());
            peak = peak.max(y.abs());
        }
        assert!(peak <= ceil + 1e-6);
        assert!(peak > 0.6, "got louder: {peak}");
    }
}
