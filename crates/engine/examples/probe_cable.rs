//! Record a few seconds from a capture endpoint (default: CABLE Output) in
//! shared mode and report level, packet timing and timestamp health.
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

fn main() {
    let _ = wasapi::initialize_mta();
    let want = std::env::args().nth(1).unwrap_or_else(|| "CABLE Output".into());
    let en = DeviceEnumerator::new().unwrap();
    let dev = (&en.get_device_collection(Direction::Capture).unwrap())
        .into_iter()
        .filter_map(Result::ok)
        .find(|d| d.get_friendlyname().map(|n| n.starts_with(&want)).unwrap_or(false))
        .expect("device not found");
    println!("device: {}", dev.get_friendlyname().unwrap());
    let mut client = dev.get_iaudioclient().unwrap();
    let mix = client.get_mixformat().unwrap();
    println!("mix format: {} Hz, {} ch, {} bit", mix.get_samplespersec(), mix.get_nchannels(), mix.get_bitspersample());
    let ch = mix.get_nchannels() as usize;
    let fmt = WaveFormat::new(32, 32, SampleType::Float, 48_000, ch, None);
    let (def, _min) = client.get_device_period().unwrap();
    client.initialize_client(&fmt, Direction::Capture, StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: def }, None).unwrap();
    let cap = client.get_audiocaptureclient().unwrap();
    let ev = client.set_get_eventhandle().unwrap();
    let mut buf = vec![0u8; client.get_buffer_size().unwrap() as usize * 4 * ch];
    client.start_stream().unwrap();
    let t0 = std::time::Instant::now();
    let (mut frames, mut peak, mut sumsq, mut packets, mut zero_ts, mut ts_err, mut silent) = (0usize, 0f32, 0f64, 0, 0, 0, 0);
    let mut first_ts = None;
    let mut last_ts = 0u64;
    while t0.elapsed().as_secs_f32() < 3.0 {
        if ev.wait_for_event(500).is_err() {
            println!("  (event timeout)");
            continue;
        }
        loop {
            if let Ok(Some(0)) = cap.get_next_packet_size() {
                break;
            }
            let Ok((n, info)) = cap.read_from_device(&mut buf) else { break };
            if n == 0 {
                break;
            }
            packets += 1;
            if info.timestamp == 0 { zero_ts += 1; }
            if info.flags.timestamp_error { ts_err += 1; }
            if info.flags.silent { silent += 1; }
            first_ts.get_or_insert(info.timestamp);
            last_ts = info.timestamp;
            for f in buf[..n as usize * 4 * ch].chunks_exact(4 * ch) {
                let x = f32::from_le_bytes([f[0], f[1], f[2], f[3]]);
                peak = peak.max(x.abs());
                sumsq += (x * x) as f64;
            }
            frames += n as usize;
        }
    }
    let _ = client.stop_stream();
    let rms = (sumsq / frames.max(1) as f64).sqrt();
    println!("frames {frames} ({:.2} s)  packets {packets}  peak {:.1} dBFS  rms {:.1} dBFS", frames as f32 / 48_000.0, 20.0 * (peak.max(1e-9)).log10(), 20.0 * rms.max(1e-9).log10());
    println!("timestamps: zero {zero_ts}, error-flagged {ts_err}, silent-flagged {silent}, span {:.2} s", (last_ts - first_ts.unwrap_or(last_ts)) as f64 / 1e7);
}
