//! WASAPI capture and render threads.
//!
//! Capture: mic → mono f32 → chain → ring buffer.
//! Render:  ring buffer → drift resampler → all output channels → cable.
//! Both run event-driven in shared mode, at MMCSS "Pro Audio" priority.
//! Neither locks or allocates once streaming.

use crate::chain::Chain;
use crate::devices::ensure_com;
use crate::engine::{InputChannel, StreamInfo};
use crate::meters::{block_levels, Meters};
use crate::resample::DriftResampler;
use crate::{Error, Result, SAMPLE_RATE};
use rtrb::{Consumer, Producer};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex};
use wasapi::{
    AudioClient, Device, DeviceEnumerator, Direction, SampleType, StreamMode, TimingMode, WaveFormat,
};

/// How long a wait for a device event may take before we count it as a stall.
const EVENT_TIMEOUT_MS: u32 = 200;
/// Consecutive stalls before the stream is declared dead and restarted.
const MAX_STALLS: u32 = 10;
const STARTUP_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// Shared between the control thread and one capture+render pair.
#[derive(Default)]
pub(crate) struct Session {
    pub stop: AtomicBool,
    pub failed: AtomicBool,
    pub error: Mutex<Option<String>>,
    /// Capture period in frames, so render can size its buffer target.
    pub cap_period: AtomicU32,
    /// Minimum extra safety buffering in frames on top of the device periods.
    /// Render grows its own margin above this when dropouts happen.
    pub extra_frames: AtomicU32,
    /// Latency test: capture mutes its output, then injects a click and
    /// records the click's capture timestamp (QPC, 100 ns units).
    pub inject_request: AtomicBool,
    pub inject_qpc: std::sync::atomic::AtomicU64,
}

impl Session {
    fn fail(&self, e: Error) {
        let mut slot = self.error.lock().unwrap();
        if slot.is_none() {
            *slot = Some(e.0);
        }
        self.failed.store(true, Relaxed);
    }
}

/// Frames to keep in the ring between capture and render.
pub(crate) fn target_frames(cap_period: u32, ren_period: u32, extra: u32) -> usize {
    let (big, small) = (cap_period.max(ren_period), cap_period.min(ren_period));
    (big + small / 2 + extra) as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Input,
    Output,
}

pub(crate) type Ready = mpsc::Sender<(Side, Result<StreamInfo>)>;

struct Opened {
    client: AudioClient,
    format: WaveFormat,
    sample: SampleFmt,
    channels: usize,
    period_frames: u32,
    info: StreamInfo,
}

/// On-the-wire sample encoding. Shared mode is always F32; exclusive mode
/// uses whatever the device natively accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleFmt {
    F32,
    I16,
    I24,
    /// 32-bit container, left-justified (covers 24-in-32).
    I32,
}

impl SampleFmt {
    fn bytes(self) -> usize {
        match self {
            SampleFmt::I16 => 2,
            SampleFmt::I24 => 3,
            SampleFmt::F32 | SampleFmt::I32 => 4,
        }
    }

    #[inline]
    fn write(self, x: f32, out: &mut [u8]) {
        let x = x.clamp(-1.0, 1.0);
        match self {
            SampleFmt::F32 => out[..4].copy_from_slice(&x.to_le_bytes()),
            SampleFmt::I16 => out[..2].copy_from_slice(&((x * 32_767.0) as i16).to_le_bytes()),
            SampleFmt::I24 => out[..3].copy_from_slice(&((x as f64 * 8_388_607.0) as i32).to_le_bytes()[..3]),
            SampleFmt::I32 => out[..4].copy_from_slice(&((x as f64 * 2_147_483_647.0) as i32).to_le_bytes()),
        }
    }

    #[inline]
    fn read(self, b: &[u8]) -> f32 {
        match self {
            SampleFmt::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            SampleFmt::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
            SampleFmt::I24 => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
            SampleFmt::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        }
    }
}

/// Desired exclusive-mode period. The driver's minimum wins if it's larger.
const EXCLUSIVE_PERIOD_HNS: i64 = 20_000; // 2 ms

/// Open a device in exclusive mode at 48 kHz in whatever sample format it
/// takes natively. Errors if the device is busy, disallows exclusive access,
/// or can't do 48 kHz.
fn open_exclusive(device: &Device, dir: Direction) -> Result<Opened> {
    use windows::Win32::Media::Audio::AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED;

    let id = device.get_id()?;
    let name = device.get_friendlyname().unwrap_or_default();
    let mut client = device.get_iaudioclient()?;
    let channels = device
        .get_device_format()
        .map(|f| f.get_nchannels() as usize)
        .or_else(|_| client.get_mixformat().map(|f| f.get_nchannels() as usize))?;

    let sr = SAMPLE_RATE as usize;
    let candidates = [
        (SampleFmt::I32, WaveFormat::new(32, 24, SampleType::Int, sr, channels, None)),
        (SampleFmt::I24, WaveFormat::new(24, 24, SampleType::Int, sr, channels, None)),
        (SampleFmt::F32, WaveFormat::new(32, 32, SampleType::Float, sr, channels, None)),
        (SampleFmt::I32, WaveFormat::new(32, 32, SampleType::Int, sr, channels, None)),
        (SampleFmt::I16, WaveFormat::new(16, 16, SampleType::Int, sr, channels, None)),
    ];
    let (sample, format) = candidates
        .iter()
        .find_map(|(s, f)| client.is_supported_exclusive_with_quirks(f).ok().map(|f| (*s, f)))
        .ok_or("device doesn't accept 48 kHz in exclusive mode")?;

    let mut period = client.calculate_aligned_period_near(EXCLUSIVE_PERIOD_HNS, None, &format)?;
    let mode = |p| StreamMode::EventsExclusive { period_hns: p };
    if let Err(e) = client.initialize_client(&format, dir, mode(period), None) {
        let unaligned = matches!(&e, wasapi::WasapiError::Windows(w) if w.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED);
        if !unaligned {
            return Err(e.into());
        }
        // Documented recovery: use the size the driver rounded to, on a fresh client.
        let frames = client.get_buffer_size()?;
        period = wasapi::calculate_period_100ns(frames as i64, sr as i64);
        client = device.get_iaudioclient()?;
        client.initialize_client(&format, dir, mode(period), None)?;
    }

    let period_frames = client.get_buffer_size()?;
    let info = StreamInfo {
        id,
        name,
        channels: channels as u16,
        device_rate: SAMPLE_RATE,
        period_frames,
        low_latency: true,
        exclusive: true,
        exclusive_fallback: None,
        stream_latency_ms: client
            .get_stream_latency()
            .map(|hns| hns as f32 / 10_000.0)
            .unwrap_or(0.0),
    };
    Ok(Opened { client, format, sample, channels, period_frames, info })
}

fn get_device(id: Option<&str>, dir: Direction) -> Result<Device> {
    let en = DeviceEnumerator::new()?;
    match id {
        Some(id) => en
            .get_device(id)
            .map_err(|e| Error(format!("device {id} unavailable: {e}"))),
        None => Ok(en.get_default_device(dir)?),
    }
}

/// Open a shared-mode, event-driven stream at 48 kHz f32. Prefer the
/// low-latency IAudioClient3 path when the device's mix format already
/// matches; otherwise let WASAPI convert (handles 44.1/96 kHz devices etc).
fn open(device: &Device, dir: Direction) -> Result<Opened> {
    let id = device.get_id()?;
    let name = device.get_friendlyname().unwrap_or_default();
    let mut client = device.get_iaudioclient()?;
    let mix = client.get_mixformat()?;
    let channels = mix.get_nchannels() as usize;
    let mix_rate = mix.get_samplespersec();
    let is_f32_48k = mix_rate == SAMPLE_RATE
        && matches!(mix.get_subformat(), Ok(SampleType::Float))
        && mix.get_bitspersample() == 32;

    let make_info = |client: &AudioClient, period: u32, low_latency: bool| StreamInfo {
        id: id.clone(),
        name: name.clone(),
        channels: channels as u16,
        device_rate: mix_rate,
        period_frames: period,
        low_latency,
        exclusive: false,
        exclusive_fallback: None,
        stream_latency_ms: client
            .get_stream_latency()
            .map(|hns| hns as f32 / 10_000.0)
            .unwrap_or(0.0),
    };

    if is_f32_48k {
        if let Ok(p) = client.get_shared_mode_engine_period(&mix) {
            if client
                .initialize_shared_audio_stream(&mix, TimingMode::Events, p.min)
                .is_ok()
            {
                let info = make_info(&client, p.min, true);
                return Ok(Opened {
                    client,
                    format: mix,
                    sample: SampleFmt::F32,
                    channels,
                    period_frames: p.min,
                    info,
                });
            }
            // A failed Initialize leaves the client unusable; start over.
            client = device.get_iaudioclient()?;
        }
    }

    let fmt = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE as usize, channels, None);
    let (def_hns, min_hns) = client.get_device_period()?;
    client.initialize_client(
        &fmt,
        dir,
        StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min_hns },
        None,
    )?;
    // In classic shared mode the engine runs at its default period.
    let period = (def_hns as u64 * SAMPLE_RATE as u64 / 10_000_000) as u32;
    let info = make_info(&client, period, false);
    Ok(Opened { client, format: fmt, sample: SampleFmt::F32, channels, period_frames: period, info })
}

/// Raise to MMCSS "Pro Audio" and flush denormals to zero.
fn setup_rt_thread() {
    use windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
    let mut task_index = 0u32;
    // Failure just means normal priority; keep going.
    let _ = unsafe { AvSetMmThreadCharacteristicsW(windows::core::w!("Pro Audio"), &mut task_index) };
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040); // FTZ | DAZ
    }
}

/// Runs `body`, reporting setup failure via `ready` and later failures via the session.
fn run_thread(
    side: Side,
    session: &Session,
    ready: Ready,
    body: impl FnOnce(&mut dyn FnMut(StreamInfo)) -> Result<()>,
) {
    ensure_com();
    setup_rt_thread();
    let mut sent = false;
    let res = body(&mut |info| {
        sent = true;
        let _ = ready.send((side, Ok(info)));
    });
    if let Err(e) = res {
        if sent {
            session.fail(e);
        } else {
            let _ = ready.send((side, Err(e)));
        }
    }
}

pub(crate) struct CaptureArgs {
    pub device_id: Option<String>,
    pub channel: InputChannel,
    pub exclusive: bool,
    pub chain: Box<Chain>,
    pub chain_rx: Consumer<Box<Chain>>,
    pub trash_tx: Producer<Box<Chain>>,
    pub ring: Producer<f32>,
    /// Monitor ring producers arrive here (None = detach); old ones go back via trash.
    pub monitor_rx: Consumer<Option<Producer<f32>>>,
    pub monitor_trash: Producer<Producer<f32>>,
    pub meters: Arc<Meters>,
    pub session: Arc<Session>,
    pub tap: Arc<crate::tap::Tap>,
}

pub(crate) fn capture_thread(a: CaptureArgs, ready: Ready) {
    let session = a.session.clone();
    run_thread(Side::Input, &session, ready, move |ready| capture(a, ready));
}

fn capture(mut a: CaptureArgs, ready: &mut dyn FnMut(StreamInfo)) -> Result<()> {
    let device = get_device(a.device_id.as_deref(), Direction::Capture)?;
    let mut exclusive_err = None;
    let op = if a.exclusive {
        match open_exclusive(&device, Direction::Capture) {
            Ok(op) => Some(op),
            Err(e) => {
                exclusive_err = Some(e.0);
                None
            }
        }
    } else {
        None
    };
    let mut op = match op {
        Some(op) => op,
        None => open(&device, Direction::Capture)?,
    };
    op.info.exclusive_fallback = exclusive_err;
    let cap = op.client.get_audiocaptureclient()?;
    let event = op.client.set_get_eventhandle()?;
    let buf_frames = op.client.get_buffer_size()? as usize;
    let block_align = op.format.get_blockalign() as usize;
    let ch = op.channels;
    let sample = op.sample;
    let sbytes = sample.bytes();
    let pick = match a.channel {
        InputChannel::Channel(c) if (c as usize) < ch => Some(c as usize),
        InputChannel::Channel(_) => Some(0),
        InputChannel::Mix => None,
    };
    let mut bytes = vec![0u8; buf_frames * block_align];
    let mut mono = vec![0f32; buf_frames];
    let mut dry = vec![0f32; buf_frames];
    // A/B crossfade toward the original (0 = processed, 1 = original).
    let mut ab = 0.0f32;
    let ab_c = crate::dsp::time_coeff(5.0, SAMPLE_RATE as f32);

    a.session.cap_period.store(op.period_frames, Relaxed);
    ready(op.info.clone());
    op.client.start_stream()?;

    let mut stalls = 0;
    let mut first_packet = true;
    let mut monitor: Option<Producer<f32>> = None;
    // Latency test state: frames of silence left before / after the click.
    let mut mute_before = 0usize;
    let mut mute_after = 0usize;
    while !a.session.stop.load(Relaxed) {
        if event.wait_for_event(EVENT_TIMEOUT_MS).is_err() {
            stalls += 1;
            if stalls >= MAX_STALLS {
                return Err("input device stopped delivering audio".into());
            }
            continue;
        }
        stalls = 0;

        // Swap in a new chain if the control thread sent one.
        while let Ok(mut new) = a.chain_rx.pop() {
            new.adopt_state(&mut a.chain);
            let old = std::mem::replace(&mut a.chain, new);
            let _ = a.trash_tx.push(old);
        }
        while let Ok(m) = a.monitor_rx.pop() {
            if let Some(old) = std::mem::replace(&mut monitor, m) {
                let _ = a.monitor_trash.push(old);
            }
        }

        loop {
            match cap.get_next_packet_size()? {
                Some(0) => break,
                _ => {}
            }
            let (n, info) = cap.read_from_device(&mut bytes)?;
            let n = n as usize;
            if n == 0 {
                break;
            }
            if info.flags.data_discontinuity && !first_packet {
                a.meters.discontinuities.fetch_add(1, Relaxed);
            }
            first_packet = false;

            let block = &mut mono[..n];
            for (dst, frame) in block.iter_mut().zip(bytes[..n * block_align].chunks_exact(block_align)) {
                let s = |c: usize| sample.read(&frame[c * sbytes..]);
                *dst = match pick {
                    Some(c) => s(c),
                    None => (0..ch).map(s).sum::<f32>() / ch as f32,
                };
            }

            // Tuning tools: record the raw mic; while auditioning, the recording replaces it.
            let auditioning = a.tap.input(block);

            let (peak, rms) = block_levels(block);
            a.meters.in_peak.raise(peak);
            a.meters.in_rms.raise(rms);
            dry[..n].copy_from_slice(block);
            a.chain.process(block, &a.meters);

            // A/B: crossfade to the (level-matched) original.
            let (ab_target, dry_gain) = match a.tap.original_gain() {
                Some(g) => (1.0, g),
                None => (0.0, 1.0),
            };
            if ab > 1e-4 || ab_target > 0.0 {
                for (x, d) in block.iter_mut().zip(&dry[..n]) {
                    ab = ab * ab_c + ab_target * (1.0 - ab_c);
                    *x += (d * dry_gain - *x) * ab;
                }
            }

            if a.session.inject_request.swap(false, Relaxed) {
                mute_before = SAMPLE_RATE as usize * 150 / 1000;
            }
            if mute_before > 0 || mute_after > 0 {
                block.fill(0.0);
                if mute_before > 0 {
                    mute_before = mute_before.saturating_sub(n);
                    if mute_before == 0 {
                        // Click at the start of the next packet; its timestamp is that packet's.
                        mute_after = SAMPLE_RATE as usize * 100 / 1000 + 1;
                    }
                } else if mute_after == SAMPLE_RATE as usize * 100 / 1000 + 1 {
                    for (i, x) in block.iter_mut().take(48).enumerate() {
                        *x = if i % 2 == 0 { 0.8 } else { -0.8 };
                    }
                    a.session.inject_qpc.store(info.timestamp, Relaxed);
                    mute_after -= 1;
                } else {
                    mute_after = mute_after.saturating_sub(n);
                }
            }

            a.tap.scope_write(block);

            let free = a.ring.slots();
            if free < n {
                a.meters.overflows.fetch_add(1, Relaxed);
            }
            if let Ok(chunk) = a.ring.write_chunk_uninit(n.min(free)) {
                if auditioning {
                    // Nobody on a call should hear the audition loop.
                    chunk.fill_from_iter(std::iter::repeat(0.0));
                } else {
                    chunk.fill_from_iter(block.iter().copied());
                }
            }
            if let Some(m) = &mut monitor {
                let k = n.min(m.slots());
                if let Ok(chunk) = m.write_chunk_uninit(k) {
                    chunk.fill_from_iter(block.iter().copied());
                }
            }
        }
    }
    let _ = op.client.stop_stream();
    Ok(())
}

pub(crate) struct RenderArgs {
    pub device_id: String,
    pub ring: Consumer<f32>,
    pub meters: Arc<Meters>,
    pub session: Arc<Session>,
    /// Some = this is the monitor stream: apply this live gain (linear) and
    /// keep its stats out of the main meters.
    pub monitor_gain: Option<Arc<crate::meters::AtomicF32>>,
    /// Try exclusive mode first (only WinFX should be playing into the cable).
    pub exclusive: bool,
}

/// Adaptive buffering: +1 ms per dropout (up to 10 ms), −0.5 ms after each
/// clean stretch, never below the configured minimum.
const MARGIN_STEP: u32 = SAMPLE_RATE / 1000;
const MARGIN_MAX: u32 = SAMPLE_RATE / 100;
const MARGIN_RELAX_AFTER: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) fn render_thread(a: RenderArgs, ready: Ready) {
    let session = a.session.clone();
    run_thread(Side::Output, &session, ready, move |ready| render(a, ready));
}

fn render(mut a: RenderArgs, ready: &mut dyn FnMut(StreamInfo)) -> Result<()> {
    let device = get_device(Some(&a.device_id), Direction::Render)?;
    let mut exclusive_err = None;
    let op = if a.exclusive {
        match open_exclusive(&device, Direction::Render) {
            Ok(op) => Some(op),
            Err(e) => {
                exclusive_err = Some(e.0);
                None
            }
        }
    } else {
        None
    };
    let mut op = match op {
        Some(op) => op,
        None => open(&device, Direction::Render)?,
    };
    op.info.exclusive_fallback = exclusive_err;
    let sample = op.sample;
    let sbytes = sample.bytes();
    let rc = op.client.get_audiorenderclient()?;
    let event = op.client.set_get_eventhandle()?;
    let buf_frames = op.client.get_buffer_size()? as usize;
    let block_align = op.format.get_blockalign() as usize;
    let ch = op.channels;
    let mut bytes = vec![0u8; buf_frames * block_align];
    let mut mono = vec![0f32; buf_frames];
    let mut rs = DriftResampler::new(SAMPLE_RATE);
    let is_monitor = a.monitor_gain.is_some();
    let gain_c = crate::dsp::time_coeff(10.0, SAMPLE_RATE as f32);
    let mut gain = a.monitor_gain.as_ref().map_or(1.0, |g| g.load());

    ready(op.info.clone());

    // Start on a buffer of silence, per the WASAPI rendering guidance.
    let avail = op.client.get_available_space_in_frames()? as usize;
    rc.write_to_device(&bytes[..avail * block_align], None)?;
    op.client.start_stream()?;
    let started = std::time::Instant::now();
    let mut margin = a.session.extra_frames.load(Relaxed);
    let mut clean_since = std::time::Instant::now();

    let mut stalls = 0;
    while !a.session.stop.load(Relaxed) {
        if event.wait_for_event(EVENT_TIMEOUT_MS).is_err() {
            stalls += 1;
            if stalls >= MAX_STALLS {
                return Err("output device stopped requesting audio".into());
            }
            continue;
        }
        stalls = 0;

        let min_margin = a.session.extra_frames.load(Relaxed);
        if clean_since.elapsed() > MARGIN_RELAX_AFTER && margin > min_margin {
            margin = margin.saturating_sub(MARGIN_STEP / 2).max(min_margin);
            clean_since = std::time::Instant::now();
        }
        margin = margin.max(min_margin);
        let target = target_frames(a.session.cap_period.load(Relaxed), op.period_frames, margin);
        rs.set_target(target);
        let avail = op.client.get_available_space_in_frames()? as usize;
        if avail == 0 {
            continue;
        }
        let block = &mut mono[..avail];
        let stats = rs.render(&mut a.ring, block);
        // The first moments after start are settling, not dropouts worth reporting.
        if stats.underrun && started.elapsed() > STARTUP_GRACE {
            margin = (margin + MARGIN_STEP).min(MARGIN_MAX.max(min_margin));
            clean_since = std::time::Instant::now();
        }
        if is_monitor {
            if stats.underrun && started.elapsed() > STARTUP_GRACE {
                a.meters.monitor_underruns.fetch_add(1, Relaxed);
            }
            let target = a.monitor_gain.as_ref().map_or(1.0, |g| g.load());
            for x in block.iter_mut() {
                gain = gain * gain_c + target * (1.0 - gain_c);
                *x *= gain;
            }
        } else {
            if stats.underrun && started.elapsed() > STARTUP_GRACE {
                a.meters.underruns.fetch_add(1, Relaxed);
            }
            if stats.dropped > 0 {
                a.meters.overflows.fetch_add(1, Relaxed);
            }
        }

        let out = &mut bytes[..avail * block_align];
        for (frame, &x) in out.chunks_exact_mut(block_align).zip(block.iter()) {
            for c in 0..ch {
                sample.write(x, &mut frame[c * sbytes..]);
            }
        }
        rc.write_to_device(out, None)?;

        if !is_monitor {
            a.meters.fill_frames.store(a.ring.slots() as u32, Relaxed);
            a.meters.drift_ppm.store(rs.ratio_ppm() as f32);
            a.meters.margin_frames.store(margin, Relaxed);
            a.meters.target_frames.store(target as u32, Relaxed);
        }
    }
    let _ = op.client.stop_stream();
    Ok(())
}

/// Latency test listener: open the cable's recording side, ask the capture
/// thread for `trials` clicks, and time each from its capture timestamp to its
/// arrival here. Both timestamps are WASAPI's QPC positions (100 ns units).
pub(crate) fn listen_for_clicks(device_id: &str, session: &Session, trials: usize) -> Result<Vec<f32>> {
    let device = get_device(Some(device_id), Direction::Capture)?;
    let op = open(&device, Direction::Capture)?;
    let cap = op.client.get_audiocaptureclient()?;
    let event = op.client.set_get_eventhandle()?;
    let buf_frames = op.client.get_buffer_size()? as usize;
    let block_align = op.format.get_blockalign() as usize;
    let mut bytes = vec![0u8; buf_frames * block_align];
    op.client.start_stream()?;

    let mut results = vec![];
    // Anything but exact zeros at all? (A misconfigured cable delivers pure silence.)
    let mut heard_anything = false;
    for _ in 0..trials {
        session.inject_qpc.store(0, Relaxed);
        session.inject_request.store(true, Relaxed);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let mut found = None;
        'trial: while found.is_none() && std::time::Instant::now() < deadline {
            if session.stop.load(Relaxed) {
                break;
            }
            if event.wait_for_event(100).is_err() {
                continue;
            }
            loop {
                if let Some(0) = cap.get_next_packet_size()? {
                    break;
                }
                let (n, info) = cap.read_from_device(&mut bytes)?;
                if n == 0 {
                    break;
                }
                if !heard_anything {
                    heard_anything = bytes[..n as usize * block_align]
                        .chunks_exact(4)
                        .any(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0.0);
                }
                let injected = session.inject_qpc.load(Relaxed);
                if injected == 0 {
                    continue; // still in the silence before the click; just drain
                }
                if info.timestamp == 0 || info.flags.timestamp_error {
                    return Err("the virtual cable doesn't report timestamps".into());
                }
                for (i, frame) in bytes[..n as usize * block_align].chunks_exact(block_align).enumerate() {
                    let arrival = info.timestamp + (i as u64 * 10_000_000) / SAMPLE_RATE as u64;
                    if arrival > injected && f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]).abs() > 0.3 {
                        found = Some((arrival - injected) as f32 / 10_000.0);
                        break 'trial;
                    }
                }
            }
        }
        results.extend(found);
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let _ = op.client.stop_stream();
    if !heard_anything {
        return Err("the virtual cable isn't passing audio (CABLE Output is pure silence). In VB-Cable's control panel,                     the latency must be larger than a Windows audio period: use 3×256 smp or more"
            .into());
    }
    Ok(results)
}
