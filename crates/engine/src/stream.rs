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
    /// Extra safety buffering in frames on top of the device periods.
    pub extra_frames: AtomicU32,
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

/// Open the mic in exclusive mode at 48 kHz in whatever sample format the
/// device takes natively. Errors if the device is busy, disallows exclusive
/// access, or can't do 48 kHz.
fn open_exclusive_capture(device: &Device) -> Result<Opened> {
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
    if let Err(e) = client.initialize_client(&format, Direction::Capture, mode(period), None) {
        let unaligned = matches!(&e, wasapi::WasapiError::Windows(w) if w.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED);
        if !unaligned {
            return Err(e.into());
        }
        // Documented recovery: use the size the driver rounded to, on a fresh client.
        let frames = client.get_buffer_size()?;
        period = wasapi::calculate_period_100ns(frames as i64, sr as i64);
        client = device.get_iaudioclient()?;
        client.initialize_client(&format, Direction::Capture, mode(period), None)?;
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
}

pub(crate) fn capture_thread(a: CaptureArgs, ready: Ready) {
    let session = a.session.clone();
    run_thread(Side::Input, &session, ready, move |ready| capture(a, ready));
}

fn capture(mut a: CaptureArgs, ready: &mut dyn FnMut(StreamInfo)) -> Result<()> {
    let device = get_device(a.device_id.as_deref(), Direction::Capture)?;
    let mut exclusive_err = None;
    let op = if a.exclusive {
        match open_exclusive_capture(&device) {
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

    a.session.cap_period.store(op.period_frames, Relaxed);
    ready(op.info.clone());
    op.client.start_stream()?;

    let mut stalls = 0;
    let mut first_packet = true;
    let mut monitor: Option<Producer<f32>> = None;
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

            let (peak, rms) = block_levels(block);
            a.meters.in_peak.raise(peak);
            a.meters.in_rms.raise(rms);
            a.chain.process(block, &a.meters);

            let free = a.ring.slots();
            if free < n {
                a.meters.overflows.fetch_add(1, Relaxed);
            }
            if let Ok(chunk) = a.ring.write_chunk_uninit(n.min(free)) {
                chunk.fill_from_iter(block.iter().copied());
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
}

pub(crate) fn render_thread(a: RenderArgs, ready: Ready) {
    let session = a.session.clone();
    run_thread(Side::Output, &session, ready, move |ready| render(a, ready));
}

fn render(mut a: RenderArgs, ready: &mut dyn FnMut(StreamInfo)) -> Result<()> {
    let device = get_device(Some(&a.device_id), Direction::Render)?;
    let op = open(&device, Direction::Render)?;
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

        rs.set_target(target_frames(
            a.session.cap_period.load(Relaxed),
            op.period_frames,
            a.session.extra_frames.load(Relaxed),
        ));
        let avail = op.client.get_available_space_in_frames()? as usize;
        if avail == 0 {
            continue;
        }
        let block = &mut mono[..avail];
        let stats = rs.render(&mut a.ring, block);
        // The first moments after start are settling, not dropouts worth reporting.
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
            let b = x.to_le_bytes();
            for c in 0..ch {
                frame[c * 4..c * 4 + 4].copy_from_slice(&b);
            }
        }
        rc.write_to_device(out, None)?;

        if !is_monitor {
            a.meters.fill_frames.store(a.ring.slots() as u32, Relaxed);
            a.meters.drift_ppm.store(rs.ratio_ppm() as f32);
        }
    }
    let _ = op.client.stop_stream();
    Ok(())
}
