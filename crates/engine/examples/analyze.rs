//! Compare presets on a recording:
//!   cargo run -p engine --example analyze -- <recording.wav> [raw|current|<preset.json>]...
//! "current" = the chain in %APPDATA%\winfx\config.json. With --write, saves each
//! processed version next to the recording as "<name> - <preset>.wav".

use engine::analysis::{analyze, gr_split, render, speech_windows};
use engine::import::{import, Imported};
use engine::{ChainConfig, SAMPLE_RATE};
use std::path::{Path, PathBuf};

fn load_chain(arg: &str) -> Result<(String, ChainConfig), String> {
    match arg {
        "raw" => Ok(("raw".into(), ChainConfig { slots: vec![] })),
        "current" => {
            let p = PathBuf::from(std::env::var("APPDATA").unwrap_or_default()).join("winfx").join("config.json");
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let chain = serde_json::from_value(v["engine"]["chain"].clone()).map_err(|e| e.to_string())?;
            Ok(("current".into(), chain))
        }
        path => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let name = Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
            match import(&text)? {
                Imported::Chain(c, _) => Ok((name, c)),
                Imported::Eq(eq, _) => Ok((
                    name,
                    ChainConfig { slots: vec![engine::EffectSlot { id: 1, bypass: false, effect: engine::EffectConfig::Eq(eq) }] },
                )),
            }
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    args.retain(|a| a != "--write");
    let Some(rec) = args.first() else {
        eprintln!("usage: analyze <recording.wav> [raw|current|preset.json]...");
        std::process::exit(2);
    };
    let (input, info) = engine::wav::read_mono(Path::new(rec), SAMPLE_RATE as f32, 600.0).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "{rec}: {:.1} s (source {} Hz, {} ch)\n",
        input.len() as f32 / SAMPLE_RATE as f32,
        info.source_rate,
        info.channels
    );
    let speech = speech_windows(&input);
    let presets: Vec<&str> = if args.len() > 1 { args[1..].iter().map(String::as_str).collect() } else { vec!["raw", "current"] };

    let mut reports = vec![];
    for p in &presets {
        let (name, chain) = load_chain(p).unwrap_or_else(|e| panic!("{e}"));
        let r = render(&chain, &input);
        let out_path = Path::new(rec).with_file_name(format!(
            "{} - {}.wav",
            Path::new(rec).file_stem().unwrap().to_string_lossy(),
            name
        ));
        if write && name != "raw" {
            let _ = engine::wav::write_mono(&out_path, &r.output, SAMPLE_RATE);
        }
        reports.push((name, analyze(&r.output, &speech), r.gr));
    }

    let w = 14;
    print!("{:<26}", "");
    for (n, _, _) in &reports {
        print!("{:>w$}", if n.len() > w - 1 { &n[..w - 1] } else { n });
    }
    println!();
    let row = |label: &str, f: &dyn Fn(&engine::analysis::Report) -> String| {
        print!("{label:<26}");
        for (_, r, _) in &reports {
            print!("{:>w$}", f(r));
        }
        println!();
    };
    row("Loudness (LUFS)", &|r| format!("{:.1}", r.lufs));
    row("Peak (dBFS)", &|r| format!("{:.1}", r.peak_db));
    row("Loudness range (LU)", &|r| format!("{:.1}", r.lra));
    row("Speech level (dBFS)", &|r| format!("{:.1}", r.speech_db));
    row("Noise floor (dBFS)", &|r| format!("{:.1}", r.noise_db));
    row("Speech − noise (dB)", &|r| format!("{:.1}", r.speech_db - r.noise_db));
    row("Sibilance avg (dB)", &|r| format!("{:.1}", r.sibilance_db));
    row("Sibilance worst 5% (dB)", &|r| format!("{:.1}", r.sibilance_p95_db));
    println!("Tonal balance vs 1 kHz (dB, speech only):");
    if let Some((_, r, _)) = reports.first() {
        for (i, (hz, _)) in r.bands.iter().enumerate() {
            let label = if *hz >= 1000.0 { format!("  {:.0}k", hz / 1000.0) } else { format!("  {hz:.0}") };
            print!("{label:<26}");
            for (_, r, _) in &reports {
                print!("{:>w$}", r.bands.get(i).map(|b| format!("{:+.1}", b.1)).unwrap_or_default());
            }
            println!();
        }
    }
    for (name, r, _) in &reports {
        if !r.noise_tones.is_empty() {
            let t: Vec<String> = r.noise_tones.iter().map(|(f, d)| format!("{f:.1} Hz (+{d:.0} dB)")).collect();
            println!("Tones in pauses [{name}]: {}", t.join(", "));
        }
    }
    println!("Pauses — noise per octave band (dBFS):");
    if let Some((_, r, _)) = reports.first() {
        for (i, (hz, _)) in r.noise_bands.iter().enumerate() {
            let label = if *hz >= 1000.0 { format!("  {:.0}k", hz / 1000.0) } else { format!("  {hz:.0}") };
            print!("{label:<26}");
            for (_, r, _) in &reports {
                print!("{:>w$}", r.noise_bands.get(i).map(|b| format!("{:.1}", b.1)).unwrap_or_default());
            }
            println!();
        }
    }
    println!("\nGain reduction (dB) — talking / pauses / max:");
    for (name, _, gr) in &reports {
        for (fx, g) in gr {
            if g.iter().any(|v| *v > 0.05) {
                let (t, q, m) = gr_split(g, &speech);
                println!("  {name:<14} {fx:<14} {t:5.1} / {q:5.1} / {m:5.1}");
            }
        }
    }
    println!("\nspeech is {:.0}% of the recording", reports[0].1.speech_share * 100.0);
}
