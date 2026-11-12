//! Space and character effects: reverb, delay, crusher.

use super::{Biquad, Coeffs, FilterKind, db_to_lin};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ reverb

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReverbParams {
    /// 0..1: decay length.
    pub room_size: f32,
    /// 0..1: how quickly highs die away.
    pub damping: f32,
    pub predelay_ms: f32,
    /// Wet amount 0..1 (dry stays at full level).
    pub mix: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        ReverbParams { room_size: 0.5, damping: 0.5, predelay_ms: 10.0, mix: 0.15 }
    }
}

/// Freeverb's tunings (at 44.1 kHz), scaled to the engine rate.
const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const MAX_PREDELAY_MS: f32 = 200.0;

#[derive(Clone, Debug)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

#[derive(Clone, Debug)]
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

#[derive(Clone, Debug)]
pub struct Reverb {
    combs: Vec<Comb>,
    aps: Vec<Allpass>,
    pre: Vec<f32>,
    pre_pos: usize,
    pre_len: usize,
    feedback: f32,
    damp: f32,
    wet: f32,
    sr: f32,
}

impl Reverb {
    pub fn new(p: &ReverbParams, sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let mut r = Reverb {
            combs: COMBS
                .iter()
                .map(|&n| Comb { buf: vec![0.0; (n as f32 * scale) as usize], pos: 0, store: 0.0 })
                .collect(),
            aps: ALLPASSES.iter().map(|&n| Allpass { buf: vec![0.0; (n as f32 * scale) as usize], pos: 0 }).collect(),
            pre: vec![0.0; (MAX_PREDELAY_MS * 0.001 * sr) as usize + 1],
            pre_pos: 0,
            pre_len: 0,
            feedback: 0.0,
            damp: 0.0,
            wet: 0.0,
            sr,
        };
        r.set_params(p, sr);
        r
    }

    pub fn set_params(&mut self, p: &ReverbParams, _sr: f32) {
        self.feedback = 0.7 + 0.28 * p.room_size.clamp(0.0, 1.0);
        self.damp = 0.4 * p.damping.clamp(0.0, 1.0);
        self.wet = p.mix.clamp(0.0, 1.0);
        self.pre_len = ((p.predelay_ms.clamp(0.0, MAX_PREDELAY_MS) * 0.001 * self.sr) as usize).min(self.pre.len() - 1);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // Pre-delay.
        self.pre[self.pre_pos] = x;
        let n = self.pre.len();
        let input = self.pre[(self.pre_pos + n - self.pre_len) % n] * 0.015;
        self.pre_pos = (self.pre_pos + 1) % n;

        let mut out = 0.0;
        for c in &mut self.combs {
            let y = c.buf[c.pos];
            c.store = y * (1.0 - self.damp) + c.store * self.damp;
            c.buf[c.pos] = input + c.store * self.feedback;
            c.pos = (c.pos + 1) % c.buf.len();
            out += y;
        }
        for a in &mut self.aps {
            let b = a.buf[a.pos];
            a.buf[a.pos] = out + b * 0.5;
            a.pos = (a.pos + 1) % a.buf.len();
            out = b - out;
        }
        x + out * self.wet * 3.0
    }
}

// ------------------------------------------------------------------ delay

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DelayParams {
    pub time_ms: f32,
    /// 0..0.95
    pub feedback: f32,
    pub mix: f32,
    /// Repeats lose highs above this, and lows below `low_cut_hz`.
    pub high_cut_hz: f32,
    pub low_cut_hz: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        DelayParams { time_ms: 280.0, feedback: 0.35, mix: 0.2, high_cut_hz: 5000.0, low_cut_hz: 150.0 }
    }
}

const MAX_DELAY_MS: f32 = 2000.0;

#[derive(Clone, Debug)]
pub struct Delay {
    buf: Vec<f32>,
    pos: usize,
    len: usize,
    fb: f32,
    mix: f32,
    hc: Biquad,
    lc: Biquad,
    sr: f32,
}

impl Delay {
    pub fn new(p: &DelayParams, sr: f32) -> Self {
        let mut d = Delay {
            buf: vec![0.0; (MAX_DELAY_MS * 0.001 * sr) as usize + 1],
            pos: 0,
            len: 1,
            fb: 0.0,
            mix: 0.0,
            hc: Biquad::default(),
            lc: Biquad::default(),
            sr,
        };
        d.set_params(p, sr);
        d
    }

    pub fn set_params(&mut self, p: &DelayParams, sr: f32) {
        self.len = ((p.time_ms.clamp(1.0, MAX_DELAY_MS) * 0.001 * self.sr) as usize).clamp(1, self.buf.len() - 1);
        self.fb = p.feedback.clamp(0.0, 0.95);
        self.mix = p.mix.clamp(0.0, 1.0);
        let q = std::f32::consts::FRAC_1_SQRT_2;
        self.hc.set_coeffs(Coeffs::design(FilterKind::LowPass, p.high_cut_hz, q, 0.0, sr));
        self.lc.set_coeffs(Coeffs::design(FilterKind::HighPass, p.low_cut_hz, q, 0.0, sr));
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len();
        let echo = self.buf[(self.pos + n - self.len) % n];
        let filtered = self.lc.process(self.hc.process(echo));
        self.buf[self.pos] = x + filtered * self.fb;
        self.pos = (self.pos + 1) % n;
        x + echo * self.mix
    }
}

// ------------------------------------------------------------------ crusher

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrusherParams {
    pub bits: f32,
    /// Hold each sample this many times (sample-rate reduction).
    pub downsample: f32,
    pub mix: f32,
    pub gain_db: f32,
}

impl Default for CrusherParams {
    fn default() -> Self {
        CrusherParams { bits: 8.0, downsample: 4.0, mix: 1.0, gain_db: 0.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Crusher {
    levels: f32,
    hold: f32,
    count: f32,
    held: f32,
    mix: f32,
    gain: f32,
}

impl Crusher {
    pub fn new(p: &CrusherParams, sr: f32) -> Self {
        let mut c = Crusher { levels: 0.0, hold: 1.0, count: 0.0, held: 0.0, mix: 1.0, gain: 1.0 };
        c.set_params(p, sr);
        c
    }

    pub fn set_params(&mut self, p: &CrusherParams, _sr: f32) {
        self.levels = 2f32.powf(p.bits.clamp(1.0, 16.0) - 1.0);
        self.hold = p.downsample.clamp(1.0, 64.0);
        self.mix = p.mix.clamp(0.0, 1.0);
        self.gain = db_to_lin(p.gain_db);
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.count += 1.0;
        if self.count >= self.hold {
            self.count -= self.hold;
            self.held = (x * self.levels).round() / self.levels;
        }
        (x + (self.held - x) * self.mix) * self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverb_rings_on_and_decays() {
        let mut r = Reverb::new(&ReverbParams { mix: 1.0, ..Default::default() }, 48_000.0);
        let mut y = vec![r.process(1.0)];
        y.extend((0..96_000).map(|_| r.process(0.0)));
        let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>();
        let early = energy(&y[2_000..12_000]);
        let late = energy(&y[80_000..90_000]);
        assert!(early > 1e-4, "tail exists: {early}");
        assert!(late < early * 0.05, "and decays: {late} vs {early}");
    }

    #[test]
    fn delay_repeats_at_the_set_time() {
        let mut d = Delay::new(
            &DelayParams { time_ms: 100.0, feedback: 0.5, mix: 1.0, high_cut_hz: 20_000.0, low_cut_hz: 10.0 },
            48_000.0,
        );
        let y: Vec<f32> = (0..20_000).map(|n| d.process(if n == 0 { 1.0 } else { 0.0 })).collect();
        let first = (1..20_000).max_by(|&a, &b| y[a].abs().total_cmp(&y[b].abs())).unwrap();
        assert!((first as i32 - 4800).abs() <= 2, "echo at {first}");
    }

    #[test]
    fn crusher_quantizes() {
        let mut c = Crusher::new(&CrusherParams { bits: 2.0, downsample: 1.0, mix: 1.0, gain_db: 0.0 }, 48_000.0);
        assert_eq!(c.process(0.3), 0.5);
        assert_eq!(c.process(0.1), 0.0);
    }
}
