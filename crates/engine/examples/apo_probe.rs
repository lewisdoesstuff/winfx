//! Record a few seconds from a capture endpoint in shared mode (so Windows'
//! effects run) and report level plus the strength of a 1 kHz tone, which the
//! APO spike mixes in.
//!   cargo run -p engine --example apo_probe -- "Mic | Line | Instrument 1" [seconds]
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

fn goertzel(x: &[f32], hz: f32, rate: f32) -> f32 {
    let w = std::f32::consts::TAU * hz / rate;
    let (c, mut s1, mut s2) = (2.0 * w.cos(), 0.0f32, 0.0f32);
    for &v in x {
        let s0 = v + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let p = s1 * s1 + s2 * s2 - c * s1 * s2;
    // Amplitude of a sine at `hz`.
    2.0 * p.max(0.0).sqrt() / x.len().max(1) as f32
}

fn db(v: f32) -> f32 {
    20.0 * v.max(1e-9).log10()
}

fn main() {
    let _ = wasapi::initialize_mta();
    let want = std::env::args().nth(1).unwrap_or_else(|| "Mic | Line | Instrument 1".into());
    let secs: f32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let en = DeviceEnumerator::new().unwrap();
    let dev = (&en.get_device_collection(Direction::Capture).unwrap())
        .into_iter()
        .filter_map(Result::ok)
        .find(|d| d.get_friendlyname().map(|n| n.starts_with(&want)).unwrap_or(false))
        .expect("device not found");
    println!("device: {} ({})", dev.get_friendlyname().unwrap(), dev.get_id().unwrap_or_default());
    let mut client = dev.get_iaudioclient().unwrap();
    let mix = client.get_mixformat().unwrap();
    let ch = mix.get_nchannels() as usize;
    let rate = mix.get_samplespersec() as f32;
    println!("mix format: {} Hz, {} ch", mix.get_samplespersec(), ch);
    let fmt = WaveFormat::new(32, 32, SampleType::Float, rate as usize, ch, None);
    let (def, _min) = client.get_device_period().unwrap();
    client
        .initialize_client(
            &fmt,
            Direction::Capture,
            StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: def },
            None,
        )
        .unwrap();
    let cap = client.get_audiocaptureclient().unwrap();
    let ev = client.set_get_eventhandle().unwrap();
    let mut buf = vec![0u8; client.get_buffer_size().unwrap() as usize * 4 * ch];
    let mut mono: Vec<f32> = vec![];
    client.start_stream().unwrap();
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f32() < secs {
        if ev.wait_for_event(500).is_err() {
            continue;
        }
        loop {
            if let Ok(Some(0)) = cap.get_next_packet_size() {
                break;
            }
            let Ok((n, _)) = cap.read_from_device(&mut buf) else { break };
            if n == 0 {
                break;
            }
            for f in buf[..n as usize * 4 * ch].chunks_exact(4 * ch) {
                mono.push(f32::from_le_bytes([f[0], f[1], f[2], f[3]]));
            }
        }
    }
    let _ = client.stop_stream();
    // Skip the first 100 ms (stream start-up).
    let x = &mono[(rate as usize / 10).min(mono.len())..];
    let rms = (x.iter().map(|v| (v * v) as f64).sum::<f64>() / x.len().max(1) as f64).sqrt() as f32;
    let peak = x.iter().fold(0f32, |m, v| m.max(v.abs()));
    let tone = goertzel(x, 1000.0, rate);
    let side = (goertzel(x, 870.0, rate) + goertzel(x, 1130.0, rate)) / 2.0;
    println!("{:.2} s  rms {:.1} dBFS  peak {:.1} dBFS", x.len() as f32 / rate, db(rms), db(peak));
    println!(
        "1 kHz tone {:.1} dBFS (neighbours {:.1} dBFS) -> {}",
        db(tone),
        db(side),
        if tone > 0.01 && tone > side * 10.0 { "APO ACTIVE" } else { "no tone" }
    );
}
