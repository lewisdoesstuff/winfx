//! Drive the WinFX APO through its shared memory block, as the app will.
//!   apo_ctl push current            chain from %APPDATA%\winfx\config.json
//!   apo_ctl push <preset.json>      WinFX / EasyEffects preset
//!   apo_ctl push gain:<dB>          a single Gain effect (for testing)
//!   apo_ctl push empty              no effects
//!   apo_ctl bypass on|off
//!   apo_ctl status [seconds]        instances, heartbeat, meters
use engine::chain::{ChainConfig, EffectConfig, EffectSlot};
use engine::shared::{Mapping, default_path};
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;

fn db(v: f32) -> String {
    if v <= 1e-9 { "-inf".into() } else { format!("{:.1}", 20.0 * v.log10()) }
}

fn load(arg: &str) -> ChainConfig {
    if arg == "current" {
        let p = PathBuf::from(std::env::var("APPDATA").unwrap()).join("winfx").join("config.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        return serde_json::from_value(v["engine"]["chain"].clone()).unwrap();
    }
    if arg == "empty" {
        return ChainConfig { slots: vec![] };
    }
    if let Some(g) = arg.strip_prefix("gain:") {
        let gain_db: f32 = g.parse().unwrap();
        return ChainConfig {
            slots: vec![EffectSlot {
                id: 1,
                bypass: false,
                effect: EffectConfig::Gain(engine::dsp::GainParams { gain_db }),
            }],
        };
    }
    match engine::import::import(&std::fs::read_to_string(arg).unwrap()).unwrap() {
        engine::import::Imported::Chain(c, _) => c,
        engine::import::Imported::Eq(eq, _) => {
            ChainConfig { slots: vec![EffectSlot { id: 1, bypass: false, effect: EffectConfig::Eq(eq) }] }
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let m = Mapping::open(&default_path()).expect("open shared block");
    let sh = m.get();
    sh.init();
    match a.first().map(String::as_str) {
        Some("push") => {
            let chain = load(a.get(1).map(String::as_str).unwrap_or("current"));
            let json = serde_json::to_vec(&chain).unwrap();
            assert!(sh.write_config(&json), "config too large");
            let names: Vec<&str> = chain.slots.iter().map(|s| s.effect.name()).collect();
            println!("pushed seq {} ({} bytes): {}", sh.config_seq(), json.len(), names.join(", "));
            let t = std::time::Instant::now();
            while t.elapsed().as_secs_f32() < 2.0 && sh.applied_seq.load(Relaxed) != sh.config_seq() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if sh.applied_seq.load(Relaxed) == sh.config_seq() {
                println!("applied by the APO after {:.0} ms", t.elapsed().as_secs_f32() * 1000.0);
            } else {
                println!("not applied yet (no live APO stream?)");
            }
        }
        Some("bypass") => {
            let on = a.get(1).map(String::as_str) == Some("on");
            sh.bypass.store(on as u32, Relaxed);
            println!("bypass {}", if on { "on" } else { "off" });
        }
        _ => {
            let secs: f32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            let t = std::time::Instant::now();
            let mut last = sh.frames.load(Relaxed);
            let _ = (
                sh.meters.in_peak.take(),
                sh.meters.out_peak.take(),
                sh.meters.in_rms.take(),
                sh.meters.out_rms.take(),
            );
            while t.elapsed().as_secs_f32() < secs {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let f = sh.frames.load(Relaxed);
                let gr: Vec<String> =
                    (0..8).map(|i| sh.meters.gr[i].take()).filter(|g| *g > 0.05).map(|g| format!("{g:.1}")).collect();
                println!(
                    "instances {}  {} Hz {} ch  +{} frames  cfg {}/{}  bypass {}  in {} / {} dB  out {} / {} dB  GR [{}]",
                    sh.instances.load(Relaxed),
                    sh.rate.load(Relaxed),
                    sh.channels.load(Relaxed),
                    f - last,
                    sh.applied_seq.load(Relaxed),
                    sh.config_seq(),
                    sh.bypass.load(Relaxed),
                    db(sh.meters.in_peak.take()),
                    db(sh.meters.in_rms.take()),
                    db(sh.meters.out_peak.take()),
                    db(sh.meters.out_rms.take()),
                    gr.join(" ")
                );
                last = f;
            }
        }
    }
}
