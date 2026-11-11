//! Control thread: owns the current config, starts/stops the capture+render
//! pair, restarts it when a device fails or disappears, and forwards chain
//! edits to the audio thread.

use crate::chain::{Chain, ChainConfig};
use crate::devices;
use crate::meters::Meters;
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    #[serde(default)]
    pub input: DeviceSel,
    #[serde(default)]
    pub output: DeviceSel,
    #[serde(default)]
    pub input_channel: InputChannel,
    /// Extra buffering on top of the device periods, in ms. Raise if you hear dropouts.
    #[serde(default = "default_safety_ms")]
    pub safety_ms: f32,
    pub chain: ChainConfig,
}

fn default_safety_ms() -> f32 {
    2.0
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            input: DeviceSel::Default,
            output: DeviceSel::Default,
            input_channel: InputChannel::default(),
            safety_ms: default_safety_ms(),
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
    /// Opened through IAudioClient3 with a reduced engine period.
    pub low_latency: bool,
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
}

enum Cmd {
    SetChain(ChainConfig),
    SetConfig(EngineConfig),
    Shutdown,
}

pub struct Engine {
    tx: mpsc::Sender<Cmd>,
    meters: Arc<Meters>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(config: EngineConfig) -> Engine {
        let (tx, rx) = mpsc::channel();
        let meters = Arc::new(Meters::default());
        let status = Arc::new(Mutex::new(Status::default()));
        let thread = {
            let (meters, status) = (meters.clone(), status.clone());
            std::thread::Builder::new()
                .name("winfx-control".into())
                .spawn(move || control_loop(config, rx, meters, status))
                .expect("spawn control thread")
        };
        Engine { tx, meters, status, thread: Some(thread) }
    }

    /// Update effect parameters/order. Applied without restarting audio.
    pub fn set_chain(&self, chain: ChainConfig) {
        let _ = self.tx.send(Cmd::SetChain(chain));
    }

    /// Change devices or other settings. Restarts the streams.
    pub fn set_config(&self, config: EngineConfig) {
        let _ = self.tx.send(Cmd::SetConfig(config));
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

struct Running {
    session: Arc<Session>,
    threads: Vec<JoinHandle<()>>,
    chain_tx: Producer<Box<Chain>>,
    trash_rx: Consumer<Box<Chain>>,
}

impl Running {
    fn stop(mut self) {
        self.session.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        while self.trash_rx.pop().is_ok() {}
    }
}

const RETRY_EVERY: Duration = Duration::from_secs(2);
const READY_TIMEOUT: Duration = Duration::from_secs(5);

fn control_loop(
    mut config: EngineConfig,
    rx: mpsc::Receiver<Cmd>,
    meters: Arc<Meters>,
    status: Arc<Mutex<Status>>,
) {
    devices::ensure_com();
    let mut running: Option<Running> = None;
    let mut retry_at = Instant::now();

    loop {
        if running.is_none() && Instant::now() >= retry_at {
            match start(&config, &meters) {
                Ok((r, input, output)) => {
                    let ext = (config.safety_ms.max(0.0) * SAMPLE_RATE as f32 / 1000.0) as u32;
                    let target =
                        stream::target_frames(input.period_frames, output.period_frames, ext);
                    let ms = |f: u32| f as f32 * 1000.0 / SAMPLE_RATE as f32;
                    let mut s = status.lock().unwrap();
                    s.latency_ms = ms(input.period_frames)
                        + ms(target as u32)
                        + output.stream_latency_ms.max(ms(output.period_frames));
                    s.state = RunState::Running;
                    s.input = Some(input);
                    s.output = Some(output);
                    s.last_error = None;
                    meters.reset_counters();
                    running = Some(r);
                }
                Err(e) => {
                    let mut s = status.lock().unwrap();
                    s.state = RunState::Retrying;
                    s.last_error = Some(e.0);
                    retry_at = Instant::now() + RETRY_EVERY;
                }
            }
        }

        if let Some(r) = &mut running {
            while r.trash_rx.pop().is_ok() {}
            if r.session.failed.load(Relaxed) {
                let err = r.session.error.lock().unwrap().take();
                running.take().unwrap().stop();
                let mut s = status.lock().unwrap();
                s.state = RunState::Retrying;
                s.last_error = err;
                retry_at = Instant::now() + Duration::from_millis(500);
            }
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::SetChain(chain)) => {
                if let Some(r) = &mut running {
                    let _ = r.chain_tx.push(Box::new(Chain::build(&chain, SAMPLE_RATE as f32)));
                }
                config.chain = chain;
            }
            Ok(Cmd::SetConfig(c)) => {
                config = c;
                if let Some(r) = running.take() {
                    r.stop();
                }
                status.lock().unwrap().state = RunState::Starting;
                retry_at = Instant::now();
            }
            Ok(Cmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    if let Some(r) = running.take() {
        r.stop();
    }
    status.lock().unwrap().state = RunState::Stopped;
}

fn start(config: &EngineConfig, meters: &Arc<Meters>) -> Result<(Running, StreamInfo, StreamInfo)> {
    // Resolve the input up front so we can refuse to record our own output:
    // with CABLE Output as the input, the chain would feed back into itself.
    let input = match &config.input {
        DeviceSel::Default => devices::default_device(devices::Dir::Input, devices::Role::Console)
            .ok_or("no default recording device")?,
        DeviceSel::Device { id, name } => resolve(devices::Dir::Input, id, name)?,
    };
    if is_virtual_cable(&input.name) {
        return Err(Error(format!(
            "input is \"{}\", the virtual cable this app plays into — that would loop back on \
             itself. Pick your microphone as the input.",
            input.name
        )));
    }
    let input_id = Some(input.id);
    let output_id = match &config.output {
        DeviceSel::Device { id, name } => resolve(devices::Dir::Output, id, name)?.id,
        DeviceSel::Default => {
            devices::find_cable_input()?
                .ok_or("VB-Cable \"CABLE Input\" not found — is VB-Audio Virtual Cable installed?")?
                .id
        }
    };

    let session = Arc::new(Session::default());
    session.extra_frames.store(
        (config.safety_ms.max(0.0) * SAMPLE_RATE as f32 / 1000.0) as u32,
        Relaxed,
    );
    let (ring_tx, ring_rx) = RingBuffer::<f32>::new(SAMPLE_RATE as usize / 2);
    let (chain_tx, chain_rx) = RingBuffer::<Box<Chain>>::new(8);
    let (trash_tx, trash_rx) = RingBuffer::<Box<Chain>>::new(8);
    let (ready_tx, ready_rx) = mpsc::channel();

    let cap = CaptureArgs {
        device_id: input_id,
        channel: config.input_channel,
        chain: Box::new(Chain::build(&config.chain, SAMPLE_RATE as f32)),
        chain_rx,
        trash_tx,
        ring: ring_tx,
        meters: meters.clone(),
        session: session.clone(),
    };
    let ren = RenderArgs {
        device_id: output_id,
        ring: ring_rx,
        meters: meters.clone(),
        session: session.clone(),
    };
    let threads = vec![
        spawn("winfx-capture", {
            let r = ready_tx.clone();
            move || stream::capture_thread(cap, r)
        }),
        spawn("winfx-render", move || stream::render_thread(ren, ready_tx)),
    ];
    let running = Running { session, threads, chain_tx, trash_rx };

    let (mut input, mut output) = (None, None);
    let deadline = Instant::now() + READY_TIMEOUT;
    while input.is_none() || output.is_none() {
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
    Ok((running, input.unwrap(), output.unwrap()))
}

fn is_virtual_cable(name: &str) -> bool {
    name.contains("VB-Audio")
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> JoinHandle<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn audio thread")
}
