//! WAV in/out: any rate and channel count in, mono at the engine rate out.

use std::path::Path;

pub struct WavInfo {
    pub source_rate: u32,
    pub channels: u16,
}

/// Read a WAV as mono (channels averaged), resampled to `sr` with cubic
/// interpolation, at most `max_seconds` long.
pub fn read_mono(path: &Path, sr: f32, max_seconds: f32) -> Result<(Vec<f32>, WavInfo), String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| format!("couldn't open the file: {e}"))?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 * scale)).collect::<Result<_, _>>()
        }
    }
    .map_err(|e| format!("couldn't read the file: {e}"))?;
    let mono: Vec<f32> = raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    if mono.is_empty() {
        return Err("the file is empty".into());
    }
    let ratio = spec.sample_rate as f64 / sr as f64;
    let out_len = ((mono.len() as f64 / ratio) as usize).min((max_seconds * sr) as usize).max(1);
    let out = if spec.sample_rate as f32 == sr {
        mono[..out_len.min(mono.len())].to_vec()
    } else {
        let at = |i: isize| mono[i.clamp(0, mono.len() as isize - 1) as usize];
        (0..out_len)
            .map(|n| {
                let pos = n as f64 * ratio;
                let i = pos.floor() as isize;
                let t = (pos - i as f64) as f32;
                let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
                let c1 = 0.5 * (c - a);
                let c2 = a - 2.5 * b + 2.0 * c - 0.5 * d;
                let c3 = 0.5 * (d - a) + 1.5 * (b - c);
                ((c3 * t + c2) * t + c1) * t + b
            })
            .collect()
    };
    Ok((out, WavInfo { source_rate: spec.sample_rate, channels: spec.channels }))
}

/// Write mono 32-bit float at `sr`.
pub fn write_mono(path: &Path, samples: &[f32], sr: u32) -> Result<(), String> {
    let spec =
        hound::WavSpec { channels: 1, sample_rate: sr, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("couldn't create the file: {e}"))?;
    for &s in samples {
        w.write_sample(s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}
