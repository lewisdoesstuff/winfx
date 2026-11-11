//! Lock-free taps between the capture thread and the UI, for tuning tools:
//!
//! - **scope**: the last few thousand output samples (spectrum analyzer).
//! - **recorder**: up to `REC_SECONDS` of raw mic input.
//! - **audition**: loop the recording through the chain instead of the mic.
//! - **A/B**: hear the original instead of the processed signal (level-matched).
//!
//! Everything is preallocated atomics: the audio thread never locks or
//! allocates, and the UI tolerates the odd torn read in display data.

use crate::meters::AtomicF32;
use crate::SAMPLE_RATE;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};

pub const SCOPE_LEN: usize = 8192;
pub const REC_SECONDS: usize = 30;

pub struct Tap {
    scope: Box<[AtomicU32]>,
    scope_pos: AtomicUsize,

    rec: Box<[AtomicU32]>,
    rec_len: AtomicUsize,
    recording: AtomicBool,

    auditioning: AtomicBool,
    play_pos: AtomicUsize,

    /// A/B: true = hear the original (dry) signal.
    original: AtomicBool,
    /// Gain applied to the original in A/B so loudness doesn't bias the comparison.
    original_gain: AtomicF32,
}

fn atomics(n: usize) -> Box<[AtomicU32]> {
    (0..n).map(|_| AtomicU32::new(0)).collect()
}

impl Default for Tap {
    fn default() -> Self {
        Tap {
            scope: atomics(SCOPE_LEN),
            scope_pos: AtomicUsize::new(0),
            rec: atomics(REC_SECONDS * SAMPLE_RATE as usize),
            rec_len: AtomicUsize::new(0),
            recording: AtomicBool::new(false),
            auditioning: AtomicBool::new(false),
            play_pos: AtomicUsize::new(0),
            original: AtomicBool::new(false),
            original_gain: AtomicF32::default(),
        }
    }
}

impl Tap {
    // ---------- control side (UI / engine) ----------

    pub fn start_recording(&self) {
        self.auditioning.store(false, Relaxed);
        self.rec_len.store(0, Relaxed);
        self.recording.store(true, Relaxed);
    }

    pub fn stop_recording(&self) {
        self.recording.store(false, Relaxed);
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Relaxed)
    }

    pub fn recorded_seconds(&self) -> f32 {
        self.rec_len.load(Relaxed) as f32 / SAMPLE_RATE as f32
    }

    pub fn has_recording(&self) -> bool {
        self.rec_len.load(Relaxed) > SAMPLE_RATE as usize / 4
    }

    pub fn set_auditioning(&self, on: bool) {
        if on {
            self.recording.store(false, Relaxed);
            self.play_pos.store(0, Relaxed);
        }
        self.auditioning.store(on && self.has_recording(), Relaxed);
    }

    pub fn is_auditioning(&self) -> bool {
        self.auditioning.load(Relaxed)
    }

    pub fn play_position_seconds(&self) -> f32 {
        self.play_pos.load(Relaxed) as f32 / SAMPLE_RATE as f32
    }

    pub fn set_original(&self, on: bool, gain: f32) {
        self.original_gain.store(gain);
        self.original.store(on, Relaxed);
    }

    pub fn is_original(&self) -> bool {
        self.original.load(Relaxed)
    }

    /// The most recent `out.len()` output samples, oldest first.
    pub fn read_scope(&self, out: &mut [f32]) {
        let n = out.len().min(SCOPE_LEN);
        let end = self.scope_pos.load(Relaxed);
        for (i, o) in out[..n].iter_mut().enumerate() {
            let idx = (end + SCOPE_LEN - n + i) % SCOPE_LEN;
            *o = f32::from_bits(self.scope[idx].load(Relaxed));
        }
    }

    /// RMS (linear) of the recording, raw.
    pub fn recording_rms(&self) -> f32 {
        let n = self.rec_len.load(Relaxed);
        if n == 0 {
            return 0.0;
        }
        let sum: f64 = self.rec[..n]
            .iter()
            .map(|a| {
                let x = f32::from_bits(a.load(Relaxed)) as f64;
                x * x
            })
            .sum();
        (sum / n as f64).sqrt() as f32
    }

    // ---------- audio side (capture thread) ----------

    /// Record raw input (if recording), then replace it with the recording
    /// (if auditioning). Returns true while auditioning.
    #[inline]
    pub(crate) fn input(&self, block: &mut [f32]) -> bool {
        if self.recording.load(Relaxed) {
            let mut len = self.rec_len.load(Relaxed);
            for &x in block.iter() {
                if len >= self.rec.len() {
                    self.recording.store(false, Relaxed);
                    break;
                }
                self.rec[len].store(x.to_bits(), Relaxed);
                len += 1;
            }
            self.rec_len.store(len, Relaxed);
        }
        if !self.auditioning.load(Relaxed) {
            return false;
        }
        let len = self.rec_len.load(Relaxed);
        if len == 0 {
            return false;
        }
        let mut pos = self.play_pos.load(Relaxed) % len;
        for x in block.iter_mut() {
            *x = f32::from_bits(self.rec[pos].load(Relaxed));
            pos = (pos + 1) % len;
        }
        self.play_pos.store(pos, Relaxed);
        true
    }

    #[inline]
    pub(crate) fn original_gain(&self) -> Option<f32> {
        self.original.load(Relaxed).then(|| self.original_gain.load().max(0.0))
    }

    /// Test hook for other crates (the app's analyzer tests).
    #[doc(hidden)]
    pub fn scope_write_for_test(&self, block: &[f32]) {
        self.scope_write(block)
    }

    #[inline]
    pub(crate) fn scope_write(&self, block: &[f32]) {
        let mut pos = self.scope_pos.load(Relaxed);
        for &x in block {
            self.scope[pos].store(x.to_bits(), Relaxed);
            pos = (pos + 1) % SCOPE_LEN;
        }
        self.scope_pos.store(pos, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_then_loop() {
        let tap = Tap::default();
        tap.start_recording();
        let mut a: Vec<f32> = (0..24_000).map(|i| i as f32).collect();
        tap.input(&mut a);
        tap.stop_recording();
        assert_eq!(tap.recorded_seconds(), 0.5);
        tap.set_auditioning(true);
        let mut b = vec![0.0; 30_000];
        assert!(tap.input(&mut b));
        assert_eq!(b[0], 0.0);
        assert_eq!(b[23_999], 23_999.0);
        assert_eq!(b[24_000], 0.0, "loops");
    }

    #[test]
    fn scope_returns_latest_in_order() {
        let tap = Tap::default();
        let block: Vec<f32> = (0..10_000).map(|i| i as f32).collect();
        tap.scope_write(&block);
        let mut out = vec![0.0; 4];
        tap.read_scope(&mut out);
        assert_eq!(out, [9996.0, 9997.0, 9998.0, 9999.0]);
    }
}
