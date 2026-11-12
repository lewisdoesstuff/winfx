//! Built-in effects. Each processor is sample-rate aware, keeps its state
//! across parameter changes (`set_params`), and never allocates while processing.

pub mod autogain;
pub mod biquad;
pub mod compressor;
pub mod convolver;
pub mod deesser;
pub mod enhance;
pub mod expander;
pub mod gain;
pub mod gate;
pub mod limiter;
pub mod maximizer;
pub mod pitch;
pub mod space;

pub use autogain::{Autogain, AutogainParams};
pub use biquad::{Biquad, Coeffs, FilterKind};
pub use compressor::{Compressor, CompressorParams};
pub use convolver::{Convolver, ConvolverParams};
pub use deesser::{Deesser, DeesserMode, DeesserParams};
pub use enhance::{BassEnhancer, BassEnhancerParams, Exciter, ExciterParams};
pub use expander::{Expander, ExpanderParams};
pub use gain::{Gain, GainParams};
pub use gate::{Gate, GateParams};
pub use limiter::{Limiter, LimiterParams};
pub use maximizer::{Maximizer, MaximizerParams};
pub use pitch::{Autotune, AutotuneParams, Harmonizer, HarmonizerParams, PitchParams, PitchShift, Scale};
pub use space::{Crusher, CrusherParams, Delay, DelayParams, Reverb, ReverbParams};

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
    if ms <= 0.0 { 0.0 } else { (-1.0 / (ms * 0.001 * sr)).exp() }
}
