//! Look-ahead peak limiter. A sliding-window minimum over the required gain
//! lets the gain ramp down before a peak leaves the delay line; a final clamp
//! guarantees the ceiling.

use super::{db_to_lin, lin_to_db, time_coeff};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimiterParams {
    pub ceiling_db: f32,
    pub release_ms: f32,
    pub lookahead_ms: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        LimiterParams { ceiling_db: -1.0, release_ms: 60.0, lookahead_ms: 1.5 }
    }
}

#[derive(Clone, Debug)]
pub struct Limiter {
    ceiling: f32,
    rel_c: f32,
    att_c: f32,
    /// Delay line of input samples, length `len`.
    delay: Vec<f32>,
    /// Monotonic deque (indices into a virtual sample counter) for sliding min of required gain.
    dq_val: Vec<f32>,
    dq_idx: Vec<u64>,
    dq_head: usize,
    dq_len: usize,
    pos: usize,
    n: u64,
    gain: f32,
}

impl Limiter {
    pub fn new(p: &LimiterParams, sr: f32) -> Self {
        let len = Self::lookahead_samples(p, sr) + 1;
        let mut l = Limiter {
            ceiling: 1.0,
            rel_c: 0.0,
            att_c: 0.0,
            delay: vec![0.0; len],
            dq_val: vec![1.0; len],
            dq_idx: vec![0; len],
            dq_head: 0,
            dq_len: 0,
            pos: 0,
            n: 0,
            gain: 1.0,
        };
        l.set_params(p, sr);
        l
    }

    fn lookahead_samples(p: &LimiterParams, sr: f32) -> usize {
        ((p.lookahead_ms.clamp(0.1, 20.0) * 0.001 * sr).round() as usize).max(1)
    }

    /// Whether `set_params` can apply `p` in place (lookahead decides buffer size).
    pub fn compatible(&self, p: &LimiterParams, sr: f32) -> bool {
        Self::lookahead_samples(p, sr) + 1 == self.delay.len()
    }

    pub fn latency_samples(&self) -> usize {
        self.delay.len() - 1
    }

    pub fn set_params(&mut self, p: &LimiterParams, sr: f32) {
        self.ceiling = db_to_lin(p.ceiling_db.min(0.0));
        self.rel_c = time_coeff(p.release_ms, sr);
        // Reach ~99% of the target within the lookahead.
        let la = (self.delay.len() - 1) as f32;
        self.att_c = (-5.0 / la).exp();
    }

    pub fn reduction_db(&self) -> f32 {
        -lin_to_db(self.gain)
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let cap = self.delay.len();
        let a = x.abs();
        let req = if a > self.ceiling { self.ceiling / a } else { 1.0 };

        // Sliding window min over the last `cap` samples.
        while self.dq_len > 0 {
            let back = (self.dq_head + self.dq_len - 1) % cap;
            if self.dq_val[back] >= req {
                self.dq_len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.dq_head + self.dq_len) % cap;
        self.dq_val[slot] = req;
        self.dq_idx[slot] = self.n;
        self.dq_len += 1;
        if self.n >= cap as u64 && self.dq_idx[self.dq_head] <= self.n - cap as u64 {
            self.dq_head = (self.dq_head + 1) % cap;
            self.dq_len -= 1;
        }
        let target = self.dq_val[self.dq_head];
        self.n += 1;

        let c = if target < self.gain { self.att_c } else { self.rel_c };
        self.gain = self.gain * c + target * (1.0 - c);

        let delayed = self.delay[self.pos];
        self.delay[self.pos] = x;
        self.pos = (self.pos + 1) % cap;
        (delayed * self.gain).clamp(-self.ceiling, self.ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_ceiling_and_passes_quiet_signal() {
        let sr = 48_000.0;
        let p = LimiterParams::default();
        let mut l = Limiter::new(&p, sr);
        let ceil = db_to_lin(p.ceiling_db);
        let mut max_out = 0.0f32;
        let mut loud_peak = 0.0f32;
        for n in 0..48_000 {
            let amp = if (10_000..20_000).contains(&n) { 4.0 } else { 0.3 };
            let x = amp * (2.0 * std::f32::consts::PI * 440.0 * n as f32 / sr).sin();
            let y = l.process(x);
            max_out = max_out.max(y.abs());
            if (12_000..20_000).contains(&n) {
                loud_peak = loud_peak.max(y.abs());
            }
        }
        assert!(max_out <= ceil + 1e-6);
        // The gain envelope itself (not just the safety clamp) is doing the work:
        // a +12 dB burst comes out right at the ceiling.
        assert!(loud_peak > ceil * 0.9, "{loud_peak}");
        // After release, a quiet tone is untouched.
        assert!(l.reduction_db() < 0.05, "{}", l.reduction_db());
    }
}
