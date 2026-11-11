//! Zero-latency convolution with an impulse response (WAV file).
//!
//! The first `P` taps run as a direct FIR; the rest as uniformly partitioned
//! overlap-save FFT convolution with block size `P`. The FFT part has `P`
//! samples of latency, which exactly matches the tail starting at tap `P`,
//! so the two join seamlessly with no added delay.
//!
//! Kernels (resampled, normalized, transformed) are cached by path and file
//! timestamp, so rebuilding the chain on every slider move doesn't reload them.

use super::db_to_lin;
use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Direct-FIR head length = FFT block size.
const P: usize = 128;
/// Longest IR accepted, seconds (longer files are truncated).
pub const MAX_IR_SECONDS: f32 = 3.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ConvolverParams {
    /// Path to a .wav impulse response; empty = pass-through.
    #[serde(default)]
    pub ir_path: String,
    /// Wet amount, 0..1 (dry is 1 − mix).
    pub mix: f32,
    pub gain_db: f32,
}

/// A prepared IR.
pub struct Kernel {
    head: Vec<f32>,
    /// Spectra of tail partitions (each `P` taps, zero-padded to 2P).
    parts: Vec<Vec<Complex<f32>>>,
    pub seconds: f32,
    pub source_rate: u32,
    pub channels: u16,
}

type Cache = Mutex<HashMap<(String, Option<std::time::SystemTime>), Result<Arc<Kernel>, String>>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Load (or fetch the cached) kernel for `path` at sample rate `sr`.
pub fn kernel(path: &str, sr: f32) -> Result<Arc<Kernel>, String> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let key = (path.to_owned(), mtime);
    if let Some(k) = cache().lock().unwrap().get(&key) {
        return k.clone();
    }
    let k = load(path, sr).map(Arc::new);
    cache().lock().unwrap().insert(key, k.clone());
    k
}

fn load(path: &str, sr: f32) -> Result<Kernel, String> {
    let (mut ir, info) = crate::wav::read_mono(std::path::Path::new(path), sr, MAX_IR_SECONDS)
        .map_err(|e| format!("the IR: {e}"))?;
    let out_len = ir.len();
    let spec = hound::WavSpec { channels: info.channels, sample_rate: info.source_rate, bits_per_sample: 0, sample_format: hound::SampleFormat::Float };
    // Unity power gain, so swapping IRs doesn't jump the level.
    let energy: f32 = ir.iter().map(|v| v * v).sum::<f32>().sqrt();
    if energy < 1e-9 {
        return Err("the IR is silent".into());
    }
    ir.iter_mut().for_each(|v| *v /= energy);

    let head = ir[..ir.len().min(P)].to_vec();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(2 * P);
    let parts = ir
        .get(P..)
        .unwrap_or(&[])
        .chunks(P)
        .map(|chunk| {
            let mut buf = vec![0.0f32; 2 * P];
            buf[..chunk.len()].copy_from_slice(chunk);
            let mut spec = fft.make_output_vec();
            fft.process(&mut buf, &mut spec).expect("fft size");
            spec
        })
        .collect();
    Ok(Kernel { head, parts, seconds: out_len as f32 / sr, source_rate: spec.sample_rate, channels: spec.channels })
}

pub struct Convolver {
    kernel: Option<Arc<Kernel>>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    /// Direct-FIR history (ring of the last P inputs).
    hist: Vec<f32>,
    hist_pos: usize,
    /// Overlap-save input: previous block + current block.
    block: Vec<f32>,
    fill: usize,
    /// Frequency-domain delay line of past input spectra.
    fdl: Vec<Vec<Complex<f32>>>,
    fdl_pos: usize,
    acc: Vec<Complex<f32>>,
    time: Vec<f32>,
    spec: Vec<Complex<f32>>,
    scratch_f: Vec<Complex<f32>>,
    scratch_i: Vec<Complex<f32>>,
    /// Tail output for the current block (computed at the end of the previous one).
    tail: Vec<f32>,
    wet: f32,
    dry: f32,
}

impl Convolver {
    pub fn new(p: &ConvolverParams, sr: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(2 * P);
        let inv = planner.plan_fft_inverse(2 * P);
        let kernel = if p.ir_path.is_empty() { None } else { kernel(&p.ir_path, sr).ok() };
        let n_parts = kernel.as_ref().map_or(0, |k| k.parts.len());
        let mut c = Convolver {
            fdl: vec![fwd.make_output_vec(); n_parts.max(1)],
            fdl_pos: 0,
            acc: fwd.make_output_vec(),
            time: inv.make_output_vec(),
            spec: fwd.make_output_vec(),
            scratch_f: fwd.make_scratch_vec(),
            scratch_i: inv.make_scratch_vec(),
            fwd,
            inv,
            kernel,
            hist: vec![0.0; P],
            hist_pos: 0,
            block: vec![0.0; 2 * P],
            fill: 0,
            tail: vec![0.0; P],
            wet: 0.0,
            dry: 1.0,
        };
        c.set_params(p, sr);
        c
    }

    /// Same IR (same cached kernel)? Then state can carry over. Pure pointer
    /// compare: safe on the audio thread (no locks, no file access).
    pub fn same_kernel(&self, other: &Convolver) -> bool {
        match (&self.kernel, &other.kernel) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn set_params(&mut self, p: &ConvolverParams, _sr: f32) {
        let mix = p.mix.clamp(0.0, 1.0);
        let g = db_to_lin(p.gain_db);
        self.wet = mix * g;
        self.dry = (1.0 - mix) * g;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let Some(k) = self.kernel.as_ref() else { return x };
        // Head: direct FIR over the last P inputs.
        self.hist[self.hist_pos] = x;
        let mut head = 0.0;
        let mut idx = self.hist_pos;
        for &h in &k.head {
            head += h * self.hist[idx];
            idx = if idx == 0 { P - 1 } else { idx - 1 };
        }
        self.hist_pos = (self.hist_pos + 1) % P;

        let y = head + self.tail[self.fill];
        self.block[P + self.fill] = x;
        self.fill += 1;
        if self.fill == P {
            self.fill = 0;
            self.run_tail();
        }
        x * self.dry + y * self.wet
    }

    /// End of a block: transform [prev, cur], push into the FDL, sum the
    /// partition products and keep the last P samples as the next block's tail.
    fn run_tail(&mut self) {
        let Some(k) = self.kernel.clone() else { return };
        let n = k.parts.len();
        if n > 0 {
            self.time.copy_from_slice(&self.block);
            let _ = self.fwd.process_with_scratch(&mut self.time, &mut self.fdl[self.fdl_pos], &mut self.scratch_f);
            self.acc.iter_mut().for_each(|a| *a = Complex::new(0.0, 0.0));
            for (j, h) in k.parts.iter().enumerate() {
                let x = &self.fdl[(self.fdl_pos + n - j) % n];
                for ((a, xv), hv) in self.acc.iter_mut().zip(x).zip(h) {
                    *a += xv * hv;
                }
            }
            self.spec.copy_from_slice(&self.acc);
            // Real IFFT needs the DC and Nyquist bins' imaginary parts to be zero.
            self.spec[0].im = 0.0;
            if let Some(last) = self.spec.last_mut() {
                last.im = 0.0;
            }
            let _ = self.inv.process_with_scratch(&mut self.spec, &mut self.time, &mut self.scratch_i);
            let norm = 1.0 / (2 * P) as f32;
            for (t, v) in self.tail.iter_mut().zip(&self.time[P..]) {
                *t = v * norm;
            }
            self.fdl_pos = (self.fdl_pos + 1) % n;
        }
        // Slide: current block becomes "previous".
        self.block.copy_within(P.., 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_ir(path: &std::path::Path, ir: &[f32], rate: u32) {
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for &s in ir {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn matches_direct_convolution_with_no_latency() {
        let dir = std::env::temp_dir().join(format!("winfx-conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ir.wav");
        // A decaying, sign-alternating IR well past several partitions.
        let ir: Vec<f32> = (0..1000).map(|n| 0.97f32.powi(n) * if n % 3 == 0 { 1.0 } else { -0.5 }).collect();
        write_ir(&path, &ir, 48_000);
        let norm: f32 = ir.iter().map(|v| v * v).sum::<f32>().sqrt();

        let p = ConvolverParams { ir_path: path.to_string_lossy().into(), mix: 1.0, gain_db: 0.0 };
        let mut c = Convolver::new(&p, 48_000.0);
        let x: Vec<f32> = (0..3000).map(|n| ((n * 7919) % 101) as f32 / 50.0 - 1.0).collect();
        let y: Vec<f32> = x.iter().map(|&s| c.process(s)).collect();
        for n in 0..3000 {
            let want: f32 = (0..ir.len().min(n + 1)).map(|k| ir[k] / norm * x[n - k]).sum();
            assert!((y[n] - want).abs() < 1e-3, "n={n}: {} vs {want}", y[n]);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
