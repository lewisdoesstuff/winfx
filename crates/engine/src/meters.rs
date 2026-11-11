//! Lock-free meters written by the audio threads and read by the UI.
//!
//! Level and gain-reduction values are "max since last read": the audio thread
//! `raise`s them and the reader `take`s (reads and resets). Non-negative f32s
//! order the same as their bit patterns, so `fetch_max` on the bits works.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

pub const MAX_SLOTS: usize = 32;

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
    /// Raise to at least `v` (v must be ≥ 0).
    pub fn raise(&self, v: f32) {
        self.0.fetch_max(v.max(0.0).to_bits(), Relaxed);
    }
    /// Read and reset to 0.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Relaxed))
    }
}

#[derive(Default)]
pub struct Meters {
    /// Linear peak / RMS of the raw input (before the chain).
    pub in_peak: AtomicF32,
    pub in_rms: AtomicF32,
    /// Linear peak / RMS after the chain (what goes to the cable).
    pub out_peak: AtomicF32,
    pub out_rms: AtomicF32,
    /// Gain reduction in positive dB, per chain slot index (0 for non-dynamics slots).
    pub gr: [AtomicF32; MAX_SLOTS],
    /// Frames buffered between capture and render.
    pub fill_frames: AtomicU32,
    /// Drift-correction ratio, as parts-per-million away from 1.0 (signed, stored as f32).
    pub drift_ppm: AtomicF32,
    /// Render had nothing to play (input stalled or too little buffering).
    pub underruns: AtomicU64,
    /// Buffer overflowed and samples were dropped (render stalled).
    pub overflows: AtomicU64,
    /// Input packets flagged as discontinuous by WASAPI.
    pub discontinuities: AtomicU64,
}

impl Meters {
    pub fn reset_counters(&self) {
        self.underruns.store(0, Relaxed);
        self.overflows.store(0, Relaxed);
        self.discontinuities.store(0, Relaxed);
    }
}

/// Peak and RMS of a block.
pub fn block_levels(buf: &[f32]) -> (f32, f32) {
    if buf.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum = 0.0f32;
    for &x in buf {
        peak = peak.max(x.abs());
        sum += x * x;
    }
    (peak, (sum / buf.len() as f32).sqrt())
}
