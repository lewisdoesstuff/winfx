//! Audio engine: WASAPI capture/render, DSP chain, clock-drift compensation.
//!
//! UI-agnostic. The app drives it through [`Engine`] (commands over a channel)
//! and reads [`Meters`] (atomics) and [`Status`] (mutex, control-rate only).
//! Nothing on the audio threads locks or allocates in steady state.

pub mod analysis;
pub mod chain;
pub mod devices;
pub mod dsp;
mod engine;
pub mod import;
pub mod meters;
pub mod resample;
pub mod shared;
pub mod tap;
pub mod wav;
mod stream;

pub use chain::{Chain, ChainConfig, EffectConfig, EffectSlot};
pub use engine::{
    DeviceSel, Engine, EngineConfig, InputChannel, LatencyTest, MonitorConfig, RunState, Status,
    StreamInfo,
};
pub use meters::Meters;

/// The engine's internal processing rate. WASAPI converts to/from it when a
/// device runs at something else.
pub const SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<wasapi::WasapiError> for Error {
    fn from(e: wasapi::WasapiError) -> Self {
        Error(e.to_string())
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error(s.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
