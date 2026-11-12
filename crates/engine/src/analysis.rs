//! Offline analysis: run a recording through a chain exactly as the live
//! engine would, and measure what comes out.

use crate::chain::{Chain, ChainConfig};
use crate::dsp::{lin_to_db, Biquad, Coeffs, FilterKind};
use crate::meters::{Meters, MAX_SLOTS};
use crate::SAMPLE_RATE;
use realfft::RealFftPlanner;

/// Live engine's typical capture block (EVO 4, exclusive).
const BLOCK: usize = 144;
/// Window for level statistics.
const WIN: usize = SAMPLE_RATE as usize / 20; // 50 ms

pub struct Rendered {
    pub output: Vec<f32>,
    /// Per chain slot: (effect name, gain reduction per block, dB).
    pub gr: Vec<(String, Vec<f32>)>,
}

/// Process `input` through `chain` in live-sized blocks.
pub fn render(chain: &ChainConfig, input: &[f32]) -> Rendered {
    let sr = SAMPLE_RATE as f32;
    let mut c = Chain::build(chain, sr);
    let meters = Meters::default();
    let mut output = Vec::with_capacity(input.len());
    let n_slots = chain.slots.len().min(MAX_SLOTS);
    let mut gr: Vec<Vec<f32>> = vec![Vec::new(); n_slots];
    for block in input.chunks(BLOCK) {
        let mut buf = block.to_vec();
        c.process(&mut buf, &meters);
        output.extend_from_slice(&buf);
        for (i, g) in gr.iter_mut().enumerate() {
            g.push(meters.gr[i].take());
        }
        meters.out_peak.take();
        meters.out_rms.take();
    }
    // Remove the chain's own latency so input and output line up.
    let shift = (chain.latency_ms() * 0.001 * sr).round() as usize;
    if shift > 0 && shift < output.len() {
        output.drain(..shift);
        output.extend(std::iter::repeat_n(0.0, shift));
    }
    Rendered { output, gr: chain.slots.iter().zip(gr).map(|(s, g)| (s.effect.name().to_owned(), g)).collect() }
}

#[derive(Debug, Clone)]
pub struct Report {
    /// EBU R128 integrated loudness (gated), LUFS.
    pub lufs: f32,
    pub peak_db: f32,
    /// 10th / 90th percentile of 50 ms RMS: room noise and speech level.
    pub noise_db: f32,
    pub speech_db: f32,
    /// Octave bands (Hz, dB relative to the 1 kHz band) over speech only.
    pub bands: Vec<(f32, f32)>,
    /// 5–10 kHz energy relative to 1–4 kHz, average and worst 5 % of speech, dB.
    pub sibilance_db: f32,
    pub sibilance_p95_db: f32,
    /// Share of the recording that is speech (window above noise + 12 dB).
    pub speech_share: f32,
    /// EBU R128 loudness range: spread of 3 s short-term loudness (LU).
    pub lra: f32,
    /// Octave bands of the pauses (Hz, absolute dBFS per band): what the noise is made of.
    pub noise_bands: Vec<(f32, f32)>,
    /// Tonal peaks in the pauses (Hz, dB above the surrounding noise), strongest first.
    pub noise_tones: Vec<(f32, f32)>,
}

/// Indices of 50 ms windows that are speech, judged on `reference` (the raw input).
pub fn speech_windows(reference: &[f32]) -> Vec<bool> {
    let rms: Vec<f32> = reference.chunks(WIN).map(|w| lin_to_db(rms(w))).collect();
    let noise = percentile(&rms, 0.10);
    rms.iter().map(|r| *r > noise + 12.0).collect()
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt() as f32
}

fn percentile(v: &[f32], q: f32) -> f32 {
    if v.is_empty() {
        return -120.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[((s.len() - 1) as f32 * q).round() as usize]
}

/// EBU Tech 3342 loudness range: 10th–95th percentile of gated 3 s short-term loudness.
pub fn loudness_range(x: &[f32]) -> f32 {
    let sr = SAMPLE_RATE as f32;
    let mut shelf = Biquad::new(Coeffs::design(FilterKind::HighShelf, 1681.97, std::f32::consts::FRAC_1_SQRT_2, 4.0, sr));
    let mut hpf = Biquad::new(Coeffs::design(FilterKind::HighPass, 38.13, 0.5003, 0.0, sr));
    let k: Vec<f32> = x.iter().map(|&s| hpf.process(shelf.process(s))).collect();
    let (block, hop) = ((3.0 * sr) as usize, (0.1 * sr) as usize);
    let mut st = vec![];
    let mut i = 0;
    while i + block <= k.len() {
        let ms = k[i..i + block].iter().map(|v| (v * v) as f64).sum::<f64>() / block as f64;
        st.push((-0.691 + 10.0 * ms.max(1e-20).log10()) as f32);
        i += hop;
    }
    let abs: Vec<f32> = st.into_iter().filter(|l| *l > -70.0).collect();
    if abs.len() < 2 {
        return 0.0;
    }
    let mean_ms = abs.iter().map(|l| 10f64.powf((*l as f64 + 0.691) / 10.0)).sum::<f64>() / abs.len() as f64;
    let gate = (-0.691 + 10.0 * mean_ms.log10()) as f32 - 20.0;
    let rel: Vec<f32> = abs.into_iter().filter(|l| *l > gate).collect();
    percentile(&rel, 0.95) - percentile(&rel, 0.10)
}

/// BS.1770 integrated loudness with absolute (−70) and relative (−10 LU) gates.
pub fn integrated_lufs(x: &[f32]) -> f32 {
    let sr = SAMPLE_RATE as f32;
    let mut shelf = Biquad::new(Coeffs::design(FilterKind::HighShelf, 1681.97, std::f32::consts::FRAC_1_SQRT_2, 4.0, sr));
    let mut hpf = Biquad::new(Coeffs::design(FilterKind::HighPass, 38.13, 0.5003, 0.0, sr));
    let k: Vec<f32> = x.iter().map(|&s| hpf.process(shelf.process(s))).collect();
    let (block, hop) = ((0.4 * sr) as usize, (0.1 * sr) as usize);
    let mut ms = vec![];
    let mut i = 0;
    while i + block <= k.len() {
        ms.push(k[i..i + block].iter().map(|v| (v * v) as f64).sum::<f64>() / block as f64);
        i += hop;
    }
    let lufs = |m: f64| -0.691 + 10.0 * m.max(1e-20).log10();
    let abs: Vec<f64> = ms.into_iter().filter(|m| lufs(*m) > -70.0).collect();
    if abs.is_empty() {
        return -70.0;
    }
    let rel_gate = lufs(abs.iter().sum::<f64>() / abs.len() as f64) - 10.0;
    let rel: Vec<f64> = abs.into_iter().filter(|m| lufs(*m) > rel_gate).collect();
    if rel.is_empty() {
        return -70.0;
    }
    lufs(rel.iter().sum::<f64>() / rel.len() as f64) as f32
}

pub fn analyze(x: &[f32], speech: &[bool]) -> Report {
    let rms_db: Vec<f32> = x.chunks(WIN).map(|w| lin_to_db(rms(w))).collect();
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));

    // Average spectrum over speech windows (4096-pt Hann FFTs).
    const N: usize = 4096;
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N);
    let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos()).collect();
    let mut power = vec![0.0f64; N / 2 + 1];
    let mut npower = vec![0.0f64; N / 2 + 1];
    let mut nframes = 0;
    let (mut sib, mut presence) = (vec![], vec![]);
    let mut frames = 0;
    let mut start = 0;
    while start + N <= x.len() {
        let mid = (start + N / 2) / WIN;
        let quiet = (start / WIN..(start + N) / WIN).all(|w| !speech.get(w).copied().unwrap_or(true));
        if quiet {
            let mut buf: Vec<f32> = x[start..start + N].iter().zip(&win).map(|(a, b)| a * b).collect();
            let mut spec = fft.make_output_vec();
            if fft.process(&mut buf, &mut spec).is_ok() {
                for (p, c) in npower.iter_mut().zip(&spec) {
                    *p += c.norm_sqr() as f64;
                }
                nframes += 1;
            }
        }
        if speech.get(mid).copied().unwrap_or(false) {
            let mut buf: Vec<f32> = x[start..start + N].iter().zip(&win).map(|(a, b)| a * b).collect();
            let mut spec = fft.make_output_vec();
            if fft.process(&mut buf, &mut spec).is_ok() {
                let band = |lo: f32, hi: f32| -> f64 {
                    let bin = |f: f32| (f * N as f32 / SAMPLE_RATE as f32) as usize;
                    spec[bin(lo)..bin(hi).min(spec.len())].iter().map(|c| c.norm_sqr() as f64).sum()
                };
                sib.push(10.0 * (band(5000.0, 10_000.0) / band(1000.0, 4000.0).max(1e-20)).log10() as f32);
                presence.push(band(1000.0, 4000.0));
                for (p, c) in power.iter_mut().zip(&spec) {
                    *p += c.norm_sqr() as f64;
                }
                frames += 1;
            }
        }
        start += N / 2;
    }
    let _ = presence;
    let bin_hz = SAMPLE_RATE as f32 / N as f32;
    let octave = |fc: f32| -> f64 {
        let (lo, hi) = (fc / 2f32.sqrt(), fc * 2f32.sqrt());
        power[(lo / bin_hz) as usize..((hi / bin_hz) as usize).min(power.len())].iter().sum::<f64>().max(1e-30)
    };
    let centers = [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0];
    let ref_1k = octave(1000.0);
    let bands = if frames > 0 {
        centers.iter().map(|&f| (f, 10.0 * (octave(f) / ref_1k).log10() as f32)).collect()
    } else {
        vec![]
    };
    let speech_share = speech.iter().filter(|s| **s).count() as f32 / speech.len().max(1) as f32;
    // Absolute band levels of the pauses, normalised so a full-scale sine reads 0 dBFS.
    let wsum: f32 = win.iter().sum();
    let noise_bands = if nframes > 0 {
        centers
            .iter()
            .map(|&fc| {
                let (lo, hi) = (fc / 2f32.sqrt(), fc * 2f32.sqrt());
                let e: f64 = npower[(lo / bin_hz) as usize..((hi / bin_hz) as usize).min(npower.len())].iter().sum::<f64>() / nframes as f64;
                (fc, (10.0 * (e.max(1e-30)).log10() + 20.0 * (2.0 / wsum as f64).log10() - 3.01) as f32)
            })
            .collect()
    } else {
        vec![]
    };
    Report {
        lufs: integrated_lufs(x),
        peak_db: lin_to_db(peak),
        noise_db: percentile(&rms_db, 0.10),
        speech_db: percentile(&rms_db, 0.90),
        bands,
        sibilance_db: if sib.is_empty() { -99.0 } else { sib.iter().sum::<f32>() / sib.len() as f32 },
        sibilance_p95_db: percentile(&sib, 0.95),
        speech_share,
        lra: loudness_range(x),
        noise_bands,
        noise_tones: tones(&npower, bin_hz),
    }
}

/// Narrow peaks (hum, whine) in a power spectrum: bins ≥ 8 dB above the
/// median of their ±30-bin neighbourhood, 40 Hz–12 kHz, local maxima only.
fn tones(power: &[f64], bin_hz: f32) -> Vec<(f32, f32)> {
    let mut out = vec![];
    let (lo, hi) = ((40.0 / bin_hz) as usize, ((12_000.0 / bin_hz) as usize).min(power.len().saturating_sub(31)));
    for k in lo.max(31)..hi {
        if power[k] < power[k - 1] || power[k] < power[k + 1] {
            continue;
        }
        let mut nb: Vec<f64> = power[k - 30..k + 31].to_vec();
        nb.sort_by(f64::total_cmp);
        let above = 10.0 * (power[k] / nb[30].max(1e-30)).log10();
        if above >= 8.0 {
            // Parabolic interpolation for the exact frequency.
            let (a, b, c) = (power[k - 1].max(1e-30).ln(), power[k].max(1e-30).ln(), power[k + 1].max(1e-30).ln());
            let off = 0.5 * (a - c) / (a - 2.0 * b + c);
            out.push(((k as f64 + off) as f32 * bin_hz, above as f32));
        }
    }
    out.sort_by(|x, y| y.1.total_cmp(&x.1));
    out.truncate(6);
    out
}

/// Mean gain reduction (dB) while talking and in pauses, per slot.
pub fn gr_split(gr: &[f32], speech: &[bool]) -> (f32, f32, f32) {
    let per_win = WIN / BLOCK;
    let (mut talk, mut quiet, mut nt, mut nq) = (0.0, 0.0, 0, 0);
    let mut max = 0.0f32;
    for (i, g) in gr.iter().enumerate() {
        max = max.max(*g);
        if speech.get(i * BLOCK / WIN).copied().unwrap_or(false) {
            talk += g;
            nt += 1;
        } else {
            quiet += g;
            nq += 1;
        }
    }
    let _ = per_win;
    (talk / nt.max(1) as f32, quiet / nq.max(1) as f32, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lufs_of_a_reference_tone() {
        // A 1 kHz sine at −20 dBFS peak reads about −23 LUFS (−3 dB RMS, ≈ −0.7 K-weighting + offset).
        let x: Vec<f32> = (0..48_000 * 5).map(|n| 0.1 * (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin()).collect();
        let l = integrated_lufs(&x);
        assert!((l - (-23.0)).abs() < 0.5, "{l}");
    }
}
