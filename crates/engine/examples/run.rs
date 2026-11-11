//! Headless test harness for the engine.
//!
//!   cargo run -p engine --example run -- --list
//!   cargo run -p engine --example run -- [--input <id|name>] [--output <id|name>]
//!                                        [--channel <n>|mix] [--shared] [--bypass] [--seconds <n>]
//!
//! Defaults: Windows default mic → VB-Cable "CABLE Input", SM57 voice chain.

use engine::devices::{self, Dir, Role};
use engine::dsp::lin_to_db;
use engine::{DeviceSel, Engine, EngineConfig, InputChannel, RunState};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
    };

    if args.iter().any(|a| a == "--list") {
        list();
        return;
    }

    let mut config = EngineConfig::default();
    if let Some(q) = get("--input") {
        let d = devices::find(Dir::Input, &q).unwrap().unwrap_or_else(|| die(&format!("no input matches {q:?}")));
        config.input = DeviceSel::device(&d);
    }
    if let Some(q) = get("--output") {
        let d = devices::find(Dir::Output, &q).unwrap().unwrap_or_else(|| die(&format!("no output matches {q:?}")));
        config.output = DeviceSel::device(&d);
    }
    if let Some(q) = get("--monitor") {
        let d = devices::find(Dir::Output, &q).unwrap().unwrap_or_else(|| die(&format!("no output matches {q:?}")));
        config.monitor = engine::MonitorConfig { enabled: true, device: DeviceSel::device(&d), volume_db: -6.0 };
    }
    if let Some(c) = get("--channel") {
        config.input_channel = if c == "mix" {
            InputChannel::Mix
        } else {
            InputChannel::Channel(c.parse().unwrap_or_else(|_| die("--channel takes a number or 'mix'")))
        };
    }
    if let Some(ms) = get("--safety").and_then(|s| s.parse().ok()) {
        config.safety_ms = ms;
    }
    if args.iter().any(|a| a == "--shared") {
        config.exclusive_input = false;
    }
    if args.iter().any(|a| a == "--bypass") {
        for s in &mut config.chain.slots {
            s.bypass = true;
        }
    }
    let seconds: Option<u64> = get("--seconds").and_then(|s| s.parse().ok());

    let slot_names: Vec<&str> = config.chain.slots.iter().map(|s| s.effect.name()).collect();
    let engine = Engine::start(config);
    let started = Instant::now();
    let mut last_state = None;

    loop {
        std::thread::sleep(Duration::from_millis(250));
        let st = engine.status();
        if last_state != Some(st.state) {
            last_state = Some(st.state);
            println!();
            match st.state {
                RunState::Running => {
                    let i = st.input.as_ref().unwrap();
                    let o = st.output.as_ref().unwrap();
                    println!("RUNNING  ~{:.1} ms added latency", st.latency_ms);
                    for (label, s) in [("in ", i), ("out", o)] {
                        println!(
                            "  {label}: {}  [{} ch, device {} Hz, period {} frames ({:.2} ms){}, stream latency {:.1} ms]",
                            s.name,
                            s.channels,
                            s.device_rate,
                            s.period_frames,
                            s.period_frames as f32 / 48.0,
                            if s.exclusive { ", EXCLUSIVE" } else if s.low_latency { ", low-latency" } else { "" },
                            s.stream_latency_ms
                        );
                        if let Some(why) = &s.exclusive_fallback {
                            println!("       exclusive mode failed, using shared: {why}");
                        }
                    }
                }
                other => println!("{other:?}: {}", st.last_error.as_deref().unwrap_or("")),
            }
        }

        if st.state == RunState::Running {
            let m = engine.meters();
            let db = |x: f32| lin_to_db(x).max(-99.0);
            let mut gr = String::new();
            for (i, name) in slot_names.iter().enumerate() {
                let v = m.gr[i].take();
                if matches!(*name, "Gate" | "Compressor" | "Limiter") {
                    gr.push_str(&format!(" {}:{:4.1}", &name[..4], v));
                }
            }
            let mon = match (&st.monitor, &st.monitor_error) {
                (Some(_), _) => format!(" | mon ~{:.0} ms xrun {}", st.monitor_latency_ms, m.monitor_underruns.load(Relaxed)),
                (None, Some(e)) => format!(" | mon ERR {e}"),
                _ => String::new(),
            };
            print!(
                "\r in {:6.1} dB | out {:6.1} dB | GR{} | buf {:5.2} ms {:+6.0} ppm | xrun {} ovf {} disc {}{mon}   ",
                db(m.in_peak.take()),
                db(m.out_peak.take()),
                gr,
                m.fill_frames.load(Relaxed) as f32 / 48.0,
                m.drift_ppm.load(),
                m.underruns.load(Relaxed),
                m.overflows.load(Relaxed),
                m.discontinuities.load(Relaxed),
            );
            let _ = std::io::Write::flush(&mut std::io::stdout());
            m.in_rms.take();
            m.out_rms.take();
        }

        if seconds.is_some_and(|s| started.elapsed() >= Duration::from_secs(s)) {
            println!();
            break;
        }
    }
}

fn list() {
    for dir in [Dir::Input, Dir::Output] {
        println!("{dir:?} devices:");
        for d in devices::list(dir).unwrap() {
            println!("  {} {}  {}", if d.active { "*" } else { " " }, d.name, d.id);
        }
    }
    for (label, role) in [("default", Role::Console), ("communications", Role::Communications)] {
        if let Some(d) = devices::default_device(Dir::Input, role) {
            println!("{label} input: {}", d.name);
        }
    }
    match devices::find_cable_input().unwrap() {
        Some(d) => println!("cable: {}  {}", d.name, d.id),
        None => println!("cable: NOT FOUND"),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
