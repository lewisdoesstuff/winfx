//! Pitch effects built on a granular delay-line shifter: two read heads
//! sweep through a short delay at a rate set by the pitch ratio, crossfaded
//! with a sine window so one is always mid-grain while the other wraps.
//! Natural on voice; adds about half a window of latency.
//!
//! - [`PitchShift`]: fixed shift.
//! - [`Harmonizer`]: two shifted voices added to the dry voice.
//! - [`Autotune`]: YIN pitch detection → nearest note in a scale → shifter.

use super::{db_to_lin, time_coeff};
use serde::{Deserialize, Serialize};

const BUF: usize = 16_384;
/// Smallest delay a head may read at (samples).
const D_MIN: f32 = 48.0;
/// How far a restarting head searches for an in-phase splice: covers one
/// period down to ~65 Hz.
const SEARCH: usize = 740;
/// Waveform length compared when searching.
const MATCH: usize = 384;

#[derive(Clone, Copy, Debug, Default)]
struct Head {
    delay: f32,
    age: usize,
}

/// Two-head granular shifter with in-phase splicing (WSOLA-style): each head
/// plays a Hann-windowed grain while its delay drifts by (1 − ratio) per
/// sample; when a grain ends, the head restarts at the delay whose waveform
/// best lines up with the other head, so overlaps add instead of cancelling.
#[derive(Clone, Debug)]
pub struct Shifter {
    buf: Vec<f32>,
    pos: usize,
    heads: [Head; 2],
    grain: usize,
}

impl Shifter {
    pub fn new(window_ms: f32, sr: f32) -> Self {
        let mut s = Shifter { buf: vec![0.0; BUF], pos: 0, heads: [Head::default(); 2], grain: 1 };
        s.set_window(window_ms, sr);
        // Stagger the heads by half a grain.
        s.heads = [Head { delay: D_MIN, age: 0 }, Head { delay: D_MIN, age: s.grain / 2 }];
        s
    }

    pub fn set_window(&mut self, window_ms: f32, sr: f32) {
        self.grain = ((window_ms * 0.001 * sr) as usize).clamp(256, BUF / 4);
    }

    #[inline]
    fn at(&self, delay: usize) -> f32 {
        self.buf[(self.pos + BUF - delay % BUF) % BUF]
    }

    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let d = delay.max(1.0);
        let i = d.floor() as usize;
        let t = d - i as f32;
        let (a, b) = (self.at(i), self.at(i + 1));
        a + (b - a) * t
    }

    /// Start head `i` on a new grain, in phase with the other head.
    fn restart(&mut self, i: usize, ratio: f32) {
        let other = self.heads[1 - i].delay.round() as usize;
        // Pitching up shrinks the delay over a grain; leave room for that.
        let base = D_MIN as usize + ((ratio - 1.0).max(0.0) * self.grain as f32) as usize;
        let max_delay = BUF - MATCH - 2;
        let mut best = (base, f32::MIN);
        for d in base..(base + SEARCH).min(max_delay) {
            let mut c = 0.0;
            for j in (0..MATCH).step_by(2) {
                c += self.at(d + j) * self.at(other + j);
            }
            if c > best.1 {
                best = (d, c);
            }
        }
        self.heads[i] = Head { delay: best.0 as f32, age: 0 };
    }

    /// `ratio` 2.0 = up an octave, 0.5 = down an octave.
    #[inline]
    pub fn process(&mut self, x: f32, ratio: f32) -> f32 {
        self.buf[self.pos] = x;
        let mut y = 0.0;
        for i in 0..2 {
            if self.heads[i].age >= self.grain {
                self.restart(i, ratio);
            }
            let h = self.heads[i];
            // Hann windows half a grain apart sum to exactly 1.
            let w = (std::f32::consts::PI * h.age as f32 / self.grain as f32).sin().powi(2);
            y += self.read(h.delay) * w;
            self.heads[i].delay = (h.delay + 1.0 - ratio).clamp(1.0, (BUF - 2) as f32);
            self.heads[i].age += 1;
        }
        self.pos = (self.pos + 1) % BUF;
        y
    }
}

fn semis(s: f32) -> f32 {
    2f32.powf(s / 12.0)
}

// ------------------------------------------------------------------ pitch shift

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PitchParams {
    pub semitones: f32,
    pub cents: f32,
    /// Wet amount 0..1.
    pub mix: f32,
    pub window_ms: f32,
}

impl Default for PitchParams {
    fn default() -> Self {
        PitchParams { semitones: -3.0, cents: 0.0, mix: 1.0, window_ms: 40.0 }
    }
}

#[derive(Clone, Debug)]
pub struct PitchShift {
    sh: Shifter,
    ratio: f32,
    mix: f32,
}

impl PitchShift {
    pub fn new(p: &PitchParams, sr: f32) -> Self {
        let mut s = PitchShift { sh: Shifter::new(p.window_ms, sr), ratio: 1.0, mix: 1.0 };
        s.set_params(p, sr);
        s
    }
    pub fn set_params(&mut self, p: &PitchParams, sr: f32) {
        self.sh.set_window(p.window_ms, sr);
        self.ratio = semis(p.semitones + p.cents / 100.0);
        self.mix = p.mix.clamp(0.0, 1.0);
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let w = self.sh.process(x, self.ratio);
        x + (w - x) * self.mix
    }
}

// ------------------------------------------------------------------ harmonizer

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonizerParams {
    pub voice1_semitones: f32,
    pub voice1_db: f32,
    pub voice2_semitones: f32,
    pub voice2_db: f32,
    pub dry_db: f32,
}

impl Default for HarmonizerParams {
    fn default() -> Self {
        HarmonizerParams { voice1_semitones: 4.0, voice1_db: -6.0, voice2_semitones: 7.0, voice2_db: -9.0, dry_db: 0.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Harmonizer {
    v1: Shifter,
    v2: Shifter,
    r1: f32,
    r2: f32,
    g1: f32,
    g2: f32,
    dry: f32,
}

impl Harmonizer {
    pub fn new(p: &HarmonizerParams, sr: f32) -> Self {
        let mut h = Harmonizer { v1: Shifter::new(35.0, sr), v2: Shifter::new(45.0, sr), r1: 1.0, r2: 1.0, g1: 0.0, g2: 0.0, dry: 1.0 };
        h.set_params(p, sr);
        h
    }
    pub fn set_params(&mut self, p: &HarmonizerParams, _sr: f32) {
        self.r1 = semis(p.voice1_semitones);
        self.r2 = semis(p.voice2_semitones);
        self.g1 = db_to_lin(p.voice1_db);
        self.g2 = db_to_lin(p.voice2_db);
        self.dry = db_to_lin(p.dry_db);
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        x * self.dry + self.v1.process(x, self.r1) * self.g1 + self.v2.process(x, self.r2) * self.g2
    }
}

// ------------------------------------------------------------------ autotune

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    #[default]
    Chromatic,
    Major,
    Minor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutotuneParams {
    /// 0 = C … 11 = B.
    pub key: u8,
    #[serde(default)]
    pub scale: Scale,
    /// How fast notes snap: 0 = instant (robotic), ~100 ms = natural.
    pub speed_ms: f32,
    pub mix: f32,
}

impl Default for AutotuneParams {
    fn default() -> Self {
        AutotuneParams { key: 0, scale: Scale::Chromatic, speed_ms: 30.0, mix: 1.0 }
    }
}

const YIN_DECIM: usize = 2;
const YIN_LEN: usize = 512; // at sr / 2: ~21 ms
const YIN_HOP: usize = 128;
const YIN_THRESHOLD: f32 = 0.15;
const MIN_HZ: f32 = 70.0;
const MAX_HZ: f32 = 800.0;

#[derive(Clone, Debug)]
pub struct Autotune {
    sh: Shifter,
    sr: f32,
    hist: Vec<f32>,
    hist_pos: usize,
    decim_acc: f32,
    decim_n: usize,
    hop: usize,
    diff: Vec<f32>,
    allowed: [bool; 12],
    speed_c: f32,
    target_ratio: f32,
    ratio: f32,
    mix: f32,
    /// Last detected pitch (Hz), 0 when unvoiced.
    pub detected_hz: f32,
}

impl Autotune {
    pub fn new(p: &AutotuneParams, sr: f32) -> Self {
        let mut a = Autotune {
            sh: Shifter::new(25.0, sr),
            sr,
            hist: vec![0.0; YIN_LEN * 2],
            hist_pos: 0,
            decim_acc: 0.0,
            decim_n: 0,
            hop: 0,
            diff: vec![0.0; YIN_LEN],
            allowed: [true; 12],
            speed_c: 0.0,
            target_ratio: 1.0,
            ratio: 1.0,
            mix: 1.0,
            detected_hz: 0.0,
        };
        a.set_params(p, sr);
        a
    }

    pub fn set_params(&mut self, p: &AutotuneParams, sr: f32) {
        const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
        const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
        self.allowed = match p.scale {
            Scale::Chromatic => [true; 12],
            Scale::Major | Scale::Minor => {
                let steps = if p.scale == Scale::Major { MAJOR } else { MINOR };
                let mut a = [false; 12];
                for s in steps {
                    a[((s + p.key) % 12) as usize] = true;
                }
                a
            }
        };
        self.speed_c = time_coeff(p.speed_ms.max(0.0), sr);
        self.mix = p.mix.clamp(0.0, 1.0);
    }

    /// Nearest allowed note's frequency to `f`.
    fn snap(&self, f: f32) -> f32 {
        let midi = 69.0 + 12.0 * (f / 440.0).log2();
        let base = midi.round() as i32;
        (0..=6)
            .flat_map(|d| [base + d, base - d])
            .find(|m| self.allowed[m.rem_euclid(12) as usize])
            .map(|m| 440.0 * 2f32.powf((m as f32 - 69.0) / 12.0))
            .unwrap_or(f)
    }

    /// YIN over the decimated history; returns f0 or 0 when unvoiced.
    fn detect(&mut self) -> f32 {
        let sr = self.sr / YIN_DECIM as f32;
        let (tmin, tmax) = ((sr / MAX_HZ) as usize, ((sr / MIN_HZ) as usize).min(YIN_LEN - 1));
        let n = self.hist.len();
        let at = |i: usize| self.hist[(self.hist_pos + i) % n];
        let w = YIN_LEN;
        let mut running = 0.0;
        self.diff[0] = 1.0;
        for tau in 1..=tmax {
            let mut d = 0.0;
            for j in 0..w {
                let a = at(j) - at(j + tau);
                d += a * a;
            }
            running += d;
            self.diff[tau] = if running > 0.0 { d * tau as f32 / running } else { 1.0 };
        }
        let mut tau = tmin.max(2);
        while tau < tmax {
            if self.diff[tau] < YIN_THRESHOLD {
                while tau + 1 < tmax && self.diff[tau + 1] < self.diff[tau] {
                    tau += 1;
                }
                // Parabolic interpolation around the dip.
                let (a, b, c) = (self.diff[tau - 1], self.diff[tau], self.diff[tau + 1]);
                let den = a - 2.0 * b + c;
                let t = if den.abs() > 1e-9 { tau as f32 + 0.5 * (a - c) / den } else { tau as f32 };
                return sr / t;
            }
            tau += 1;
        }
        0.0
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // Decimate by 2 (box average is enough for a pitch tracker).
        self.decim_acc += x;
        self.decim_n += 1;
        if self.decim_n == YIN_DECIM {
            let n = self.hist.len();
            self.hist[self.hist_pos] = self.decim_acc / YIN_DECIM as f32;
            self.hist_pos = (self.hist_pos + 1) % n;
            self.decim_acc = 0.0;
            self.decim_n = 0;
            self.hop += 1;
            if self.hop == YIN_HOP {
                self.hop = 0;
                let f = self.detect();
                self.detected_hz = f;
                self.target_ratio = if f > 0.0 { self.snap(f) / f } else { 1.0 };
            }
        }
        self.ratio = self.ratio * self.speed_c + self.target_ratio * (1.0 - self.speed_c);
        let w = self.sh.process(x, self.ratio);
        x + (w - x) * self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zero-crossing frequency estimate over the second half.
    fn freq_of(y: &[f32]) -> f32 {
        let h = &y[y.len() / 2..];
        let crossings = h.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f32 * 48_000.0 / h.len() as f32
    }

    fn sine(f: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / 48_000.0).sin()).collect()
    }

    /// Strongest frequency between `lo` and `hi` (1 Hz steps, DFT over the second half).
    fn peak_freq(y: &[f32], lo: f32, hi: f32) -> f32 {
        let h = &y[y.len() / 2..];
        let mag = |f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &v) in h.iter().enumerate() {
                let w = 2.0 * std::f64::consts::PI * f as f64 * n as f64 / 48_000.0;
                re += v as f64 * w.cos();
                im -= v as f64 * w.sin();
            }
            re * re + im * im
        };
        (lo as i32..=hi as i32).map(|f| f as f32).max_by(|a, b| mag(*a).total_cmp(&mag(*b))).unwrap()
    }

    #[test]
    fn shifts_up_an_octave() {
        let mut p = PitchShift::new(&PitchParams { semitones: 12.0, cents: 0.0, mix: 1.0, window_ms: 40.0 }, 48_000.0);
        let y: Vec<f32> = sine(220.0, 48_000).into_iter().map(|s| p.process(s)).collect();
        let f = peak_freq(&y, 300.0, 600.0);
        assert!((f - 440.0).abs() <= 2.0, "{f}");
    }

    #[test]
    fn shifts_down_a_fifth() {
        let mut p = PitchShift::new(&PitchParams { semitones: -7.0, cents: 0.0, mix: 1.0, window_ms: 40.0 }, 48_000.0);
        let y: Vec<f32> = sine(440.0, 48_000).into_iter().map(|s| p.process(s)).collect();
        let f = peak_freq(&y, 200.0, 400.0);
        assert!((f - 293.7).abs() <= 2.0, "{f}");
    }

    #[test]
    fn autotune_detects_and_pulls_to_the_nearest_note() {
        // 450 Hz is between A4 (440) and A#4 (466): chromatic snaps it to 440.
        let mut a = Autotune::new(&AutotuneParams { speed_ms: 5.0, ..Default::default() }, 48_000.0);
        let y: Vec<f32> = sine(450.0, 96_000).into_iter().map(|s| a.process(s)).collect();
        assert!((a.detected_hz - 450.0).abs() < 5.0, "detected {}", a.detected_hz);
        let f = freq_of(&y);
        assert!((f - 440.0).abs() < 12.0, "out {f}");
    }

    #[test]
    fn snap_respects_the_scale() {
        // C major has no C#: 277 Hz (C#4) snaps to C4 or D4.
        let a = Autotune::new(&AutotuneParams { key: 0, scale: Scale::Major, ..Default::default() }, 48_000.0);
        let s = a.snap(277.2);
        assert!((s - 261.6).abs() < 1.0 || (s - 293.7).abs() < 1.0, "{s}");
    }
}
