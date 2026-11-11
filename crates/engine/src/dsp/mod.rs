//! Built-in effects. Each processor is sample-rate aware, keeps its state
//! across parameter changes (`set_params`), and never allocates while processing.

pub mod biquad;
pub mod compressor;
pub mod gain;
pub mod gate;
pub mod limiter;

pub use biquad::{Biquad, Coeffs, FilterKind};
pub use compressor::{Compressor, CompressorParams};
pub use gain::{Gain, GainParams};
pub use gate::{Gate, GateParams};
pub use limiter::{Limiter, LimiterParams};

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
/// `y = c*y + (1-c)*x` reaches ~63% of a step after `ms`.
#[inline]
pub fn time_coeff(ms: f32, sr: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sr)).exp()
    }
}
