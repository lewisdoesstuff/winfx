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
    channels: usize,
    period_frames: u32,
    info: StreamInfo,
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
                return Ok(Opened { client, format: mix, channels, period_frames: p.min, info });
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
    Ok(Opened { client, format: fmt, channels, period_frames: period, info })
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
    pub chain: Box<Chain>,
    pub chain_rx: Consumer<Box<Chain>>,
    pub trash_tx: Producer<Box<Chain>>,
    pub ring: Producer<f32>,
    pub meters: Arc<Meters>,
    pub session: Arc<Session>,
}

pub(crate) fn capture_thread(a: CaptureArgs, ready: Ready) {
    let session = a.session.clone();
    run_thread(Side::Input, &session, ready, move |ready| capture(a, ready));
}

fn capture(mut a: CaptureArgs, ready: &mut dyn FnMut(StreamInfo)) -> Result<()> {
    let device = get_device(a.device_id.as_deref(), Direction::Capture)?;
    let op = open(&device, Direction::Capture)?;
    let cap = op.client.get_audiocaptureclient()?;
    let event = op.client.set_get_eventhandle()?;
    let buf_frames = op.client.get_buffer_size()? as usize;
    let block_align = op.format.get_blockalign() as usize;
    let ch = op.channels;
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
                let s = |c: usize| f32::from_le_bytes(frame[c * 4..c * 4 + 4].try_into().unwrap());
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

    ready(op.info.clone());

    // Start on a buffer of silence, per the WASAPI rendering guidance.
    let avail = op.client.get_available_space_in_frames()? as usize;
    rc.write_to_device(&bytes[..avail * block_align], None)?;
    op.client.start_stream()?;

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
        if stats.underrun {
            a.meters.underruns.fetch_add(1, Relaxed);
        }
        if stats.dropped > 0 {
            a.meters.overflows.fetch_add(1, Relaxed);
        }

        let out = &mut bytes[..avail * block_align];
        for (frame, &x) in out.chunks_exact_mut(block_align).zip(block.iter()) {
            let b = x.to_le_bytes();
            for c in 0..ch {
                frame[c * 4..c * 4 + 4].copy_from_slice(&b);
            }
        }
        rc.write_to_device(out, None)?;

        a.meters.fill_frames.store(a.ring.slots() as u32, Relaxed);
        a.meters.drift_ppm.store(rs.ratio_ppm() as f32);
    }
    let _ = op.client.stop_stream();
    Ok(())
}
