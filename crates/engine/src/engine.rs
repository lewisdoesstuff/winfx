//! Control thread: owns the current config, starts/stops the capture+render
//! pair (and the optional monitor stream), restarts them when a device fails
//! or disappears, and forwards chain edits to the audio thread.

use crate::chain::{Chain, ChainConfig};
use crate::devices;
use crate::dsp::db_to_lin;
use crate::meters::{AtomicF32, Meters};
use crate::stream::{self, CaptureArgs, RenderArgs, Session, Side};
use crate::{Error, Result, SAMPLE_RATE};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSel {
    /// Input: the Windows default recording device. Output: VB-Cable's "CABLE Input".
    /// Monitor: the Windows default playback device.
    #[default]
    Default,
    /// A specific endpoint. Matched by ID first; if that's gone (some drivers,
    /// Audient's included, re-create endpoints with new IDs when the sample rate
    /// changes), by exact friendly name.
    Device { id: String, name: String },
}

impl DeviceSel {
    pub fn device(d: &devices::DeviceInfo) -> Self {
        DeviceSel::Device { id: d.id.clone(), name: d.name.clone() }
    }
}

fn resolve(dir: devices::Dir, id: &str, name: &str) -> Result<devices::DeviceInfo> {
    let all: Vec<_> = devices::list(dir)?.into_iter().filter(|d| d.active).collect();
    all.iter()
        .find(|d| d.id == id)
        .or_else(|| all.iter().find(|d| d.name == name))
        .cloned()
        .ok_or_else(|| Error(format!("\"{name}\" not found (unplugged or disabled?)")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputChannel {
    /// Average all channels of the input endpoint.
    Mix,
    /// Take one channel (0-based). For a mic on EVO 4 input 1 that's 0.
    Channel(u16),
}

impl Default for InputChannel {
    fn default() -> Self {
        InputChannel::Channel(0)
    }
}

/// Hear the processed mic on a playback device (headphones).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonitorConfig {
    pub enabled: bool,
    pub device: DeviceSel,
    pub volume_db: f32,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        MonitorConfig { enabled: false, device: DeviceSel::Default, volume_db: -6.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(default)]
    pub input: DeviceSel,
    #[serde(default)]
    pub output: DeviceSel,
    #[serde(default)]
    pub input_channel: InputChannel,
    /// Minimum extra buffering on top of the device periods, in ms. The render
    /// side adds more by itself when dropouts happen and relaxes back later.
    #[serde(default = "default_min_safety_ms")]
    pub min_safety_ms: f32,
    /// Open the mic in exclusive mode for lower latency (falls back to shared).
    #[serde(default = "yes")]
    pub exclusive_input: bool,
    /// Open the virtual cable in exclusive mode (only WinFX plays into it).
    #[serde(default = "yes")]
    pub exclusive_output: bool,
    #[serde(default)]
    pub monitor: MonitorConfig,
    /// Direct mode: the chain runs in the WinFX APO on the input device itself
    /// (see `apo`); WinFX only listens to the processed input, for meters and
    /// the monitor. Off: capture, process and play into the virtual cable.
    #[serde(default)]
    pub direct: bool,
    pub chain: ChainConfig,
}

/// Starting margin; the adaptive margin grows from here on real dropouts.
fn default_min_safety_ms() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            input: DeviceSel::Default,
            output: DeviceSel::Default,
            input_channel: InputChannel::default(),
            min_safety_ms: default_min_safety_ms(),
            exclusive_input: true,
            exclusive_output: true,
            monitor: MonitorConfig::default(),
            direct: false,
            chain: ChainConfig::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    pub id: String,
    pub name: String,
    pub channels: u16,
    /// The device's own shared-mode rate (WASAPI converts if ≠ 48 kHz).
    pub device_rate: u32,
    pub period_frames: u32,
    /// Opened through IAudioClient3 with a reduced engine period, or exclusive.
    pub low_latency: bool,
    /// Opened in exclusive mode (input only).
    pub exclusive: bool,
    /// Exclusive mode was requested but failed; this is why (now running shared).
    pub exclusive_fallback: Option<String>,
    pub stream_latency_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Starting,
    Running,
    /// A device failed or is missing; retrying periodically.
    Retrying,
    Stopped,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub state: RunState,
    pub input: Option<StreamInfo>,
    pub output: Option<StreamInfo>,
    pub last_error: Option<String>,
    /// Estimated added latency from mic to cable, in ms.
    pub latency_ms: f32,
    /// Monitor stream, when enabled and running.
    pub monitor: Option<StreamInfo>,
    pub monitor_error: Option<String>,
    /// Estimated mic → headphones latency, in ms.
    pub monitor_latency_ms: f32,
    pub latency_test: LatencyTest,
    /// Where an audition is playing (the monitor device), or why it can't.
    pub audition_device: Option<String>,
    pub audition_error: Option<String>,
    /// Direct mode only: what the APO is doing.
    pub direct: Option<DirectStatus>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirectStatus {
    /// The APO processed audio in the last second.
    pub active: bool,
    /// Streams the APO is running on (WinFX's own listener counts as one).
    pub instances: u32,
    /// The APO has picked up the latest chain.
    pub in_sync: bool,
    /// The shared memory block couldn't be opened; why.
    pub error: Option<String>,
}

/// Measured mic → virtual-mic latency (click injected after capture, timed at
/// CABLE Output with the hardware timestamps of both streams).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum LatencyTest {
    #[default]
    Idle,
    Running,
    Done {
        /// Capture timestamp of the click → its arrival at CABLE Output, median.
        path_ms: f32,
        min_ms: f32,
        max_ms: f32,
        /// Latency the chain itself adds (limiter look-ahead), not seen by the test.
        chain_ms: f32,
        trials: usize,
    },
    Failed(String),
}

enum Cmd {
    SetChain(ChainConfig),
    SetConfig(EngineConfig),
    SetMonitor(MonitorConfig),
    MeasureLatency,
    Audition(bool),
    Shutdown,
}

pub struct Engine {
    tx: mpsc::Sender<Cmd>,
    meters: Arc<Meters>,
    status: Arc<Mutex<Status>>,
    monitor_gain: Arc<AtomicF32>,
    tap: Arc<crate::tap::Tap>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(config: EngineConfig) -> Engine {
        Self::start_inner(config, false)
    }

    /// No audio devices are opened; status reports a plausible running state
    /// and meters show synthetic levels. For UI previews/screenshots.
    pub fn start_preview(config: EngineConfig) -> Engine {
        Self::start_inner(config, true)
    }

    fn start_inner(config: EngineConfig, preview: bool) -> Engine {
        let (tx, rx) = mpsc::channel();
        let meters = Arc::new(Meters::default());
        let status = Arc::new(Mutex::new(Status::default()));
        let monitor_gain = Arc::new(AtomicF32::default());
        monitor_gain.store(db_to_lin(config.monitor.volume_db));
        let tap = Arc::new(crate::tap::Tap::default());
        let thread = {
            let ctx = Ctl {
                meters: meters.clone(),
                status: status.clone(),
                monitor_gain: monitor_gain.clone(),
                tap: tap.clone(),
            };
            std::thread::Builder::new()
                .name("winfx-control".into())
                .spawn(move || if preview { preview_loop(config, rx, ctx) } else { control_loop(config, rx, ctx) })
                .expect("spawn control thread")
        };
        Engine { tx, meters, status, monitor_gain, tap, thread: Some(thread) }
    }

    /// Update effect parameters/order. Applied without restarting audio.
    pub fn set_chain(&self, chain: ChainConfig) {
        let _ = self.tx.send(Cmd::SetChain(chain));
    }

    /// Change devices or other settings. Restarts the streams.
    pub fn set_config(&self, config: EngineConfig) {
        let _ = self.tx.send(Cmd::SetConfig(config));
    }

    /// Turn the monitor on/off or change its device. Doesn't touch the main path.
    pub fn set_monitor(&self, monitor: MonitorConfig) {
        self.monitor_gain.store(db_to_lin(monitor.volume_db));
        let _ = self.tx.send(Cmd::SetMonitor(monitor));
    }

    /// Send a few clicks through the virtual mic and time them. Results land
    /// in `Status::latency_test`. Anyone listening to the virtual mic hears the clicks.
    pub fn measure_latency(&self) {
        let _ = self.tx.send(Cmd::MeasureLatency);
    }

    /// Recorder / scope / A/B controls.
    pub fn tap(&self) -> &Arc<crate::tap::Tap> {
        &self.tap
    }

    /// Loop the recording through the chain into the monitor device (started
    /// temporarily if the monitor is off). The virtual mic gets silence meanwhile.
    pub fn set_audition(&self, on: bool) {
        let _ = self.tx.send(Cmd::Audition(on));
    }

    /// Live monitor volume, no restart.
    pub fn set_monitor_volume(&self, db: f32) {
        self.monitor_gain.store(db_to_lin(db));
    }

    pub fn meters(&self) -> &Arc<Meters> {
        &self.meters
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Ctl {
    meters: Arc<Meters>,
    status: Arc<Mutex<Status>>,
    monitor_gain: Arc<AtomicF32>,
    tap: Arc<crate::tap::Tap>,
}

struct MonitorRun {
    session: Arc<Session>,
    thread: JoinHandle<()>,
}

impl MonitorRun {
    fn stop(self) {
        self.session.stop.store(true, Relaxed);
        let _ = self.thread.join();
    }
}

struct Running {
    session: Arc<Session>,
    threads: Vec<JoinHandle<()>>,
    chain_tx: Producer<Box<Chain>>,
    trash_rx: Consumer<Box<Chain>>,
    monitor_tx: Producer<Option<Producer<f32>>>,
    monitor_trash_rx: Consumer<Producer<f32>>,
    monitor: Option<MonitorRun>,
    input_period: u32,
    safety_frames: u32,
    output_name: String,
}

impl Running {
    fn stop(mut self) {
        self.stop_monitor();
        self.session.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        while self.trash_rx.pop().is_ok() {}
        while self.monitor_trash_rx.pop().is_ok() {}
    }

    fn stop_monitor(&mut self) {
        if let Some(m) = self.monitor.take() {
            // Detach from capture first so it stops writing, then stop the render.
            let _ = self.monitor_tx.push(None);
            m.stop();
        }
    }

    fn start_monitor(&mut self, cfg: &MonitorConfig, ctl: &Ctl) -> Result<StreamInfo> {
        self.stop_monitor();
        let device = match &cfg.device {
            DeviceSel::Device { id, name } => resolve(devices::Dir::Output, id, name)?,
            DeviceSel::Default => devices::default_device(devices::Dir::Output, devices::Role::Console)
                .ok_or("no default playback device")?,
        };
        if is_virtual_cable(&device.name) {
            return Err(Error(format!(
                "\"{}\" is a virtual cable, so monitoring would feed the output back into \
                 itself. Choose another device.",
                device.name
            )));
        }

        let session = Arc::new(Session::default());
        session.cap_period.store(self.input_period, Relaxed);
        session.extra_frames.store(self.safety_frames, Relaxed);
        let (ring_tx, ring_rx) = RingBuffer::<f32>::new(SAMPLE_RATE as usize / 2);
        let (ready_tx, ready_rx) = mpsc::channel();
        let args = RenderArgs {
            device_id: device.id,
            ring: ring_rx,
            meters: ctl.meters.clone(),
            session: session.clone(),
            monitor_gain: Some(ctl.monitor_gain.clone()),
            exclusive: false,
        };
        let thread = spawn("winfx-monitor", move || stream::render_thread(args, ready_tx));
        let run = MonitorRun { session, thread };

        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok((_, Ok(info))) => {
                if self.monitor_tx.push(Some(ring_tx)).is_err() {
                    run.stop();
                    return Err("couldn't attach monitor".into());
                }
                self.monitor = Some(run);
                Ok(info)
            }
            Ok((_, Err(e))) => {
                run.stop();
                Err(e)
            }
            Err(_) => {
                run.stop();
                Err("timed out opening monitor device".into())
            }
        }
    }
}

const RETRY_EVERY: Duration = Duration::from_secs(2);
const READY_TIMEOUT: Duration = Duration::from_secs(5);

fn ms(frames: u32) -> f32 {
    frames as f32 * 1000.0 / SAMPLE_RATE as f32
}

/// Direct mode's link to the APO: pushes the chain, mirrors its meters into
/// ours (so the UI reads one set) and watches its heartbeat.
struct DirectLink {
    map: Arc<crate::shared::Mapping>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DirectLink {
    /// While the test clip records, the APO passes the raw input through (so
    /// the clip is unprocessed); while it plays, our capture thread runs the
    /// chain and meters, so mirroring pauses.
    fn open(ctl: &Ctl) -> Result<DirectLink> {
        crate::apo::ensure_data_dir().map_err(|e| Error(format!("data folder: {e}")))?;
        let map = Arc::new(crate::shared::Mapping::open(&crate::shared::default_path()).map_err(|e| Error(e.to_string()))?);
        map.get().init();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = {
            let (map, stop, meters, status, tap) =
                (map.clone(), stop.clone(), ctl.meters.clone(), ctl.status.clone(), ctl.tap.clone());
            spawn("winfx-direct", move || {
                let sh = map.get();
                let mut last_frames = sh.frames.load(Relaxed);
                let mut last_progress = Instant::now();
                let mut tick = 0u32;
                while !stop.load(Relaxed) {
                    std::thread::sleep(Duration::from_millis(15));
                    sh.bypass.store(tap.is_recording() as u32, Relaxed);
                    let m = &sh.meters;
                    if tap.is_auditioning() {
                        continue;
                    }
                    meters.in_peak.raise(m.in_peak.take());
                    meters.in_rms.raise(m.in_rms.take());
                    meters.out_peak.raise(m.out_peak.take());
                    meters.out_rms.raise(m.out_rms.take());
                    for i in 0..crate::meters::MAX_SLOTS {
                        meters.gr[i].raise(m.gr[i].take());
                        meters.det[i].raise(m.det[i].take());
                        meters.aux[i].store(m.aux[i].load());
                    }
                    tick += 1;
                    if tick.is_multiple_of(10) {
                        let f = sh.frames.load(Relaxed);
                        if f != last_frames {
                            last_frames = f;
                            last_progress = Instant::now();
                        }
                        let d = DirectStatus {
                            active: last_progress.elapsed() < Duration::from_secs(1),
                            instances: sh.instances.load(Relaxed),
                            in_sync: sh.applied_seq.load(Relaxed) == sh.config_seq(),
                            error: None,
                        };
                        let mut s = status.lock().unwrap();
                        if s.direct.as_ref() != Some(&d) {
                            s.direct = Some(d);
                        }
                    }
                }
            })
        };
        Ok(DirectLink { map, stop, thread: Some(thread) })
    }

    fn push(&self, config: &EngineConfig) {
        let sh = self.map.get();
        sh.input_channel.store(
            match config.input_channel {
                InputChannel::Mix => crate::shared::MIX_CHANNELS,
                InputChannel::Channel(c) => c as u32,
            },
            Relaxed,
        );
        if let Ok(json) = serde_json::to_vec(&config.chain) {
            sh.write_config(&json);
        }
    }
}

impl Drop for DirectLink {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Not in direct mode: if the APO is installed on the mic, make it pass audio
/// through so other apps don't get a stale chain.
fn release_apo() {
    if crate::shared::default_path().exists()
        && let Ok(m) = crate::shared::Mapping::open(&crate::shared::default_path())
            && m.get().valid() {
                m.get().bypass.store(1, Relaxed);
            }
}

fn control_loop(mut config: EngineConfig, rx: mpsc::Receiver<Cmd>, ctl: Ctl) {
    devices::ensure_com();
    let mut running: Option<Running> = None;
    let mut direct: Option<DirectLink> = None;
    if !config.direct {
        release_apo();
    }
    let mut retry_at = Instant::now();
    let mut monitor_retry_at: Option<Instant> = None;
    // A monitor started only for an audition, to stop when it ends.
    let mut temp_monitor = false;

    loop {
        if config.direct && direct.is_none() {
            match DirectLink::open(&ctl) {
                Ok(l) => {
                    l.push(&config);
                    direct = Some(l);
                }
                Err(e) => {
                    let mut s = ctl.status.lock().unwrap();
                    s.direct = Some(DirectStatus { error: Some(e.0), ..Default::default() });
                }
            }
        }
        if running.is_none() && Instant::now() >= retry_at {
            match start(&config, &ctl.meters, &ctl.tap) {
                Ok((r, input, output)) => {
                    let mut s = ctl.status.lock().unwrap();
                    s.latency_ms = match &output {
                        Some(output) => {
                            let target =
                                stream::target_frames(input.period_frames, output.period_frames, r.safety_frames);
                            ms(input.period_frames) + ms(target as u32) + output.stream_latency_ms.max(ms(output.period_frames))
                        }
                        // The APO runs inside the audio engine's own pass: only look-ahead adds delay.
                        None => config.chain.latency_ms(),
                    };
                    s.state = RunState::Running;
                    s.input = Some(input);
                    s.output = output;
                    s.last_error = None;
                    ctl.meters.reset_counters();
                    // A fresh session has no audition monitor.
                    ctl.tap.set_auditioning(false);
                    temp_monitor = false;
                    s.audition_device = None;
                    running = Some(r);
                    monitor_retry_at = config.monitor.enabled.then(Instant::now);
                }
                Err(e) => {
                    let mut s = ctl.status.lock().unwrap();
                    s.state = RunState::Retrying;
                    s.last_error = Some(e.0);
                    s.monitor = None;
                    retry_at = Instant::now() + RETRY_EVERY;
                }
            }
        }

        if let Some(r) = &mut running {
            while r.trash_rx.pop().is_ok() {}
            while r.monitor_trash_rx.pop().is_ok() {}

            if r.session.failed.load(Relaxed) {
                let err = r.session.error.lock().unwrap().take();
                running.take().unwrap().stop();
                let mut s = ctl.status.lock().unwrap();
                s.state = RunState::Retrying;
                s.last_error = err;
                s.monitor = None;
                retry_at = Instant::now() + Duration::from_millis(500);
            } else {
                // Monitor failures only restart the monitor.
                if r.monitor.as_ref().is_some_and(|m| m.session.failed.load(Relaxed)) {
                    let err = r.monitor.as_ref().and_then(|m| m.session.error.lock().unwrap().take());
                    r.stop_monitor();
                    let mut s = ctl.status.lock().unwrap();
                    s.monitor = None;
                    s.monitor_error = err;
                    monitor_retry_at = Some(Instant::now() + RETRY_EVERY);
                }
                if config.monitor.enabled && monitor_retry_at.is_some_and(|t| Instant::now() >= t) {
                    let res = r.start_monitor(&config.monitor, &ctl);
                    let mut s = ctl.status.lock().unwrap();
                    match res {
                        Ok(info) => {
                            let target = stream::target_frames(r.input_period, info.period_frames, r.safety_frames);
                            s.monitor_latency_ms = ms(r.input_period)
                                + ms(target as u32)
                                + info.stream_latency_ms.max(ms(info.period_frames));
                            s.monitor = Some(info);
                            s.monitor_error = None;
                            monitor_retry_at = None;
                        }
                        Err(e) => {
                            s.monitor = None;
                            s.monitor_error = Some(e.0);
                            monitor_retry_at = Some(Instant::now() + RETRY_EVERY);
                        }
                    }
                }
            }
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::SetChain(chain)) => {
                // Direct mode keeps a copy for the test clip.
                if let Some(r) = &mut running {
                    let _ = r.chain_tx.push(Box::new(Chain::build(&chain, SAMPLE_RATE as f32)));
                }
                config.chain = chain;
                if let Some(l) = &direct {
                    l.push(&config);
                    ctl.status.lock().unwrap().latency_ms = config.chain.latency_ms();
                }
            }
            Ok(Cmd::SetConfig(c)) => {
                config = c;
                if let Some(r) = running.take() {
                    r.stop();
                }
                if !config.direct && direct.take().is_some() {
                    release_apo();
                }
                let mut s = ctl.status.lock().unwrap();
                s.state = RunState::Starting;
                s.monitor = None;
                s.direct = None;
                s.latency_test = LatencyTest::Idle;
                retry_at = Instant::now();
            }
            Ok(Cmd::SetMonitor(m)) => {
                config.monitor = m;
                if let Some(r) = &mut running {
                    r.stop_monitor();
                }
                let mut s = ctl.status.lock().unwrap();
                s.monitor = None;
                s.monitor_error = None;
                monitor_retry_at = config.monitor.enabled.then(Instant::now);
            }
            Ok(Cmd::Audition(on)) => {
                ctl.tap.set_auditioning(on);
                let mut s = ctl.status.lock().unwrap();
                s.audition_error = None;
                if !on {
                    s.audition_device = None;
                    if temp_monitor {
                        if let Some(r) = &mut running {
                            r.stop_monitor();
                        }
                        temp_monitor = false;
                    }
                } else if let Some(r) = &mut running {
                    if r.monitor.is_some() {
                        s.audition_device = s.monitor.as_ref().map(|m| m.name.clone());
                    } else {
                        drop(s);
                        let cfg = MonitorConfig { enabled: true, ..config.monitor.clone() };
                        let res = r.start_monitor(&cfg, &ctl);
                        let mut s = ctl.status.lock().unwrap();
                        match res {
                            Ok(info) => {
                                s.audition_device = Some(info.name);
                                temp_monitor = true;
                            }
                            Err(e) => {
                                s.audition_error = Some(e.0);
                                ctl.tap.set_auditioning(false);
                            }
                        }
                    }
                } else {
                    s.audition_error = Some("audio isn't running".into());
                    ctl.tap.set_auditioning(false);
                }
            }
            Ok(Cmd::MeasureLatency) => {
                let mut s = ctl.status.lock().unwrap();
                match &running {
                    _ if s.latency_test == LatencyTest::Running => {}
                    None => s.latency_test = LatencyTest::Failed("audio isn't running".into()),
                    Some(_) if config.direct => {
                        s.latency_test = LatencyTest::Failed("not needed in direct mode".into())
                    }
                    Some(r) => {
                        s.latency_test = LatencyTest::Running;
                        let (session, out_name, status) = (r.session.clone(), r.output_name.clone(), ctl.status.clone());
                        let chain_ms = config.chain.latency_ms();
                        spawn("winfx-latency", move || measure_latency(session, out_name, status, chain_ms));
                    }
                }
            }
            Ok(Cmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    if let Some(r) = running.take() {
        r.stop();
    }
    // In direct mode the APO keeps processing after WinFX exits, with the last chain.
    drop(direct);
    let mut s = ctl.status.lock().unwrap();
    s.state = RunState::Stopped;
    s.monitor = None;
}

/// Preview mode: no devices, a believable running status, and meters driven by
/// a synthetic speech-like envelope so the UI can be developed and screenshotted
/// while the real app holds the mic.
fn preview_loop(mut config: EngineConfig, rx: mpsc::Receiver<Cmd>, ctl: Ctl) {
    let fake = |name: &str, ch: u16, period: u32, exclusive: bool| StreamInfo {
        id: format!("preview:{name}"),
        name: name.into(),
        channels: ch,
        device_rate: SAMPLE_RATE,
        period_frames: period,
        low_latency: true,
        exclusive,
        exclusive_fallback: None,
        stream_latency_ms: 0.0,
    };
    let monitor_info = || fake("Speakers", 2, 480, false);
    // Show the selected device, so the preview matches the device picker.
    let input_info = |c: &EngineConfig| match &c.input {
        DeviceSel::Device { name, .. } => fake(name, 1, 144, true),
        DeviceSel::Default => fake("Microphone", 1, 144, true),
    };
    {
        let mut s = ctl.status.lock().unwrap();
        s.state = RunState::Running;
        s.input = Some(input_info(&config));
        if config.direct {
            s.output = None;
            s.latency_ms = config.chain.latency_ms();
            s.direct = Some(DirectStatus { active: true, instances: 1, in_sync: true, error: None });
        } else {
            s.output = Some(fake("CABLE Input (VB-Audio Virtual Cable)", 2, 128, false));
            s.latency_ms = 14.0;
        }
        s.monitor = config.monitor.enabled.then(monitor_info);
        s.monitor_latency_ms = 28.0;
    }
    let started = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Cmd::SetChain(c)) => config.chain = c,
            Ok(Cmd::SetConfig(c)) => {
                let mut s = ctl.status.lock().unwrap();
                s.input = Some(input_info(&c));
                if c.direct {
                    s.output = None;
                    s.latency_ms = c.chain.latency_ms();
                    s.direct = Some(DirectStatus { active: true, instances: 1, in_sync: true, error: None });
                } else {
                    s.output = Some(fake("CABLE Input (VB-Audio Virtual Cable)", 2, 128, false));
                    s.latency_ms = 14.0;
                    s.direct = None;
                }
                config = c;
            }
            Ok(Cmd::SetMonitor(m)) => {
                config.monitor = m;
                ctl.status.lock().unwrap().monitor = config.monitor.enabled.then(monitor_info);
            }
            Ok(Cmd::MeasureLatency) => {
                ctl.status.lock().unwrap().latency_test =
                    LatencyTest::Failed("not available in preview mode".into());
            }
            Ok(Cmd::Audition(_)) => {
                ctl.status.lock().unwrap().audition_error = Some("not available in preview mode".into());
            }
            Ok(Cmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Syllables ~4 Hz inside phrases ~0.3 Hz, with a quiet room floor.
        let t = started.elapsed().as_secs_f32();
        let phrase = ((t * 0.3 * std::f32::consts::TAU).sin() * 0.5 + 0.5).powf(0.5);
        let syl = ((t * 4.1 * std::f32::consts::TAU).sin() * 0.5 + 0.5).powi(2);
        let talking = phrase > 0.35;
        let in_db = if talking { -40.0 + 26.0 * syl * phrase } else { -66.0 };
        let lin = crate::dsp::db_to_lin(in_db);
        let m = &ctl.meters;
        m.in_peak.raise(lin * 1.4);
        m.in_rms.raise(lin);
        let out = if talking { crate::dsp::db_to_lin(in_db + 18.0).min(0.89) } else { 0.0005 };
        m.out_peak.raise(out * 1.3);
        m.out_rms.raise(out);
        for (i, slot) in config.chain.slots.iter().enumerate().take(crate::meters::MAX_SLOTS) {
            let gr = match (&slot.effect, slot.bypass) {
                (_, true) => 0.0,
                (crate::chain::EffectConfig::Gate(g), _) => if talking { 0.0 } else { g.range_db },
                (crate::chain::EffectConfig::Compressor(_), _) => if talking { 6.0 * syl * phrase } else { 0.0 },
                (crate::chain::EffectConfig::Limiter(_), _) if talking => { 1.2 * syl * syl },
                _ => 0.0,
            };
            m.gr[i].raise(gr);
        }
    }
    ctl.status.lock().unwrap().state = RunState::Stopped;
}

fn start(
    config: &EngineConfig,
    meters: &Arc<Meters>,
    tap: &Arc<crate::tap::Tap>,
) -> Result<(Running, StreamInfo, Option<StreamInfo>)> {
    // Resolve the input up front so we can refuse to record our own output:
    // with CABLE Output as the input, the chain would feed back into itself.
    let input = match &config.input {
        DeviceSel::Default => devices::default_device(devices::Dir::Input, devices::Role::Console)
            .ok_or("no default recording device")?,
        DeviceSel::Device { id, name } => resolve(devices::Dir::Input, id, name)?,
    };
    if is_virtual_cable(&input.name) {
        return Err(Error(format!(
            "\"{}\" is a virtual cable, so the output would loop back into the input. \
             Choose another input.",
            input.name
        )));
    }
    let input_id = Some(input.id);
    let output_id = match &config.output {
        _ if config.direct => String::new(),
        DeviceSel::Device { id, name } => resolve(devices::Dir::Output, id, name)?.id,
        DeviceSel::Default => {
            devices::find_cable_input()?
                .ok_or("no output device. Install a virtual audio cable, such as VB-Audio Virtual Cable, or choose an output")?
                .id
        }
    };

    let safety_frames = (config.min_safety_ms.max(0.0) * SAMPLE_RATE as f32 / 1000.0) as u32;
    let session = Arc::new(Session::default());
    session.extra_frames.store(safety_frames, Relaxed);
    let (ring_tx, ring_rx) = RingBuffer::<f32>::new(SAMPLE_RATE as usize / 2);
    let (chain_tx, chain_rx) = RingBuffer::<Box<Chain>>::new(8);
    let (trash_tx, trash_rx) = RingBuffer::<Box<Chain>>::new(8);
    let (monitor_tx, monitor_rx) = RingBuffer::<Option<Producer<f32>>>::new(4);
    let (monitor_trash_tx, monitor_trash_rx) = RingBuffer::<Producer<f32>>::new(4);
    let (ready_tx, ready_rx) = mpsc::channel();

    // Direct mode listens to the processed input in shared mode (exclusive
    // would bypass the APO) and only runs the chain for the test clip.
    let cap = CaptureArgs {
        device_id: input_id,
        channel: config.input_channel,
        exclusive: config.exclusive_input && !config.direct,
        chain: Box::new(Chain::build(&config.chain, SAMPLE_RATE as f32)),
        chain_rx,
        trash_tx,
        ring: ring_tx,
        monitor_rx,
        monitor_trash: monitor_trash_tx,
        meters: meters.clone(),
        session: session.clone(),
        tap: tap.clone(),
        discard: config.direct,
    };
    let ren = RenderArgs {
        device_id: output_id,
        ring: ring_rx,
        meters: meters.clone(),
        session: session.clone(),
        monitor_gain: None,
        exclusive: config.exclusive_output,
    };
    let mut threads = vec![spawn("winfx-capture", {
        let r = ready_tx.clone();
        move || stream::capture_thread(cap, r)
    })];
    if !config.direct {
        threads.push(spawn("winfx-render", move || stream::render_thread(ren, ready_tx)));
    } else {
        drop((ren, ready_tx));
    }
    let mut running = Running {
        session,
        threads,
        chain_tx,
        trash_rx,
        monitor_tx,
        monitor_trash_rx,
        monitor: None,
        input_period: 0,
        safety_frames,
        output_name: String::new(),
    };

    let (mut input, mut output) = (None, None);
    let deadline = Instant::now() + READY_TIMEOUT;
    while input.is_none() || (output.is_none() && !config.direct) {
        let left = deadline.saturating_duration_since(Instant::now());
        let msg = match ready_rx.recv_timeout(left) {
            Ok(m) => m,
            Err(_) => {
                running.stop();
                return Err("timed out opening audio devices".into());
            }
        };
        match msg {
            (side, Err(e)) => {
                running.stop();
                let which = if side == Side::Input { "input" } else { "output" };
                return Err(Error(format!("{which}: {e}")));
            }
            (Side::Input, Ok(i)) => input = Some(i),
            (Side::Output, Ok(o)) => output = Some(o),
        }
    }
    let input = input.unwrap();
    running.input_period = input.period_frames;
    running.output_name = output.as_ref().map(|o| o.name.clone()).unwrap_or_default();
    Ok((running, input, output))
}

/// Runs on its own thread: find the cable's recording side, listen for the
/// clicks the capture thread injects, and publish the result.
fn measure_latency(session: Arc<Session>, out_name: String, status: Arc<Mutex<Status>>, chain_ms: f32) {
    devices::ensure_com();
    let result = (|| -> Result<Vec<f32>> {
        // VB-Cable pairs "CABLE Input (…)" (playback) with "CABLE Output (…)" (recording).
        let rec_name = out_name.replacen("Input", "Output", 1);
        let rec = devices::list(devices::Dir::Input)?
            .into_iter()
            .find(|d| d.active && d.name == rec_name)
            .ok_or_else(|| Error(format!("couldn't find \"{rec_name}\", the recording side of the output")))?;
        stream::listen_for_clicks(&rec.id, &session, 5)
    })();
    let mut s = status.lock().unwrap();
    s.latency_test = match result {
        Ok(mut v) if !v.is_empty() => {
            v.sort_by(f32::total_cmp);
            LatencyTest::Done {
                path_ms: v[v.len() / 2],
                min_ms: v[0],
                max_ms: v[v.len() - 1],
                chain_ms,
                trials: v.len(),
            }
        }
        Ok(_) => LatencyTest::Failed("no clicks arrived at the output".into()),
        Err(e) => LatencyTest::Failed(e.0),
    };
}

fn is_virtual_cable(name: &str) -> bool {
    name.contains("VB-Audio")
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn audio thread")
}
