//! Effect chain: serializable config (presets) and the realtime processor built from it.
//!
//! The control thread builds a new [`Chain`] whenever the config changes and
//! hands it to the audio thread, which calls [`Chain::adopt_state`] so filters,
//! envelopes and delay lines carry over (no clicks), then sends the old chain
//! back to be dropped off the audio thread.

use crate::dsp::{
    Autogain, AutogainParams, BassEnhancer, BassEnhancerParams, Biquad, Coeffs, Compressor,
    CompressorParams, Convolver, ConvolverParams, Deesser, DeesserParams, Exciter, ExciterParams,
    Expander, ExpanderParams, FilterKind, Gain, GainParams, Gate, GateParams, Limiter, LimiterParams,
    Maximizer, MaximizerParams, time_coeff,
};
use crate::meters::{block_levels, Meters, MAX_SLOTS};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterParams {
    pub kind: FilterKind,
    pub freq_hz: f32,
    pub q: f32,
    /// Number of cascaded biquads: 1 = 12 dB/oct, 2 = 24 dB/oct, …
    #[serde(default = "one")]
    pub slope: u8,
}

impl FilterParams {
    pub fn stages(&self) -> usize {
        self.slope.clamp(1, MAX_SLOPE) as usize
    }
    pub fn coeffs(&self, sr: f32) -> Coeffs {
        Coeffs::design(self.kind, self.freq_hz, self.q, 0.0, sr)
    }
}

/// Steepest slope offered (8 × 12 dB/oct).
pub const MAX_SLOPE: u8 = 8;

fn one() -> u8 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    #[serde(default = "yes")]
    pub enabled: bool,
    pub kind: FilterKind,
    pub freq_hz: f32,
    #[serde(default)]
    pub gain_db: f32,
    pub q: f32,
    /// Cascaded stages; for gain types each stage gets gain / stages.
    #[serde(default = "one")]
    pub slope: u8,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqParams {
    pub bands: Vec<EqBand>,
    /// Overall gain after the bands (e.g. an imported "Preamp").
    #[serde(default)]
    pub gain_db: f32,
}

impl EqParams {
    pub fn total_stages(&self) -> usize {
        self.bands.iter().map(EqBand::stages).sum()
    }
}

impl EqBand {
    pub fn stages(&self) -> usize {
        self.slope.clamp(1, MAX_SLOPE) as usize
    }

    /// Coefficients of ONE stage; the band is `stages()` of these in series.
    pub fn coeffs(&self, sr: f32) -> Coeffs {
        if self.enabled {
            let g = if self.kind.has_gain() { self.gain_db / self.stages() as f32 } else { 0.0 };
            Coeffs::design(self.kind, self.freq_hz, self.q, g, sr)
        } else {
            Coeffs::IDENTITY
        }
    }

    /// The band's full magnitude response in dB.
    pub fn response_db(&self, freq: f32, sr: f32) -> f32 {
        self.coeffs(sr).magnitude_db(freq, sr) * self.stages() as f32
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectConfig {
    Filter(FilterParams),
    Eq(EqParams),
    Gate(GateParams),
    Compressor(CompressorParams),
    Gain(GainParams),
    Limiter(LimiterParams),
    Deesser(DeesserParams),
    Expander(ExpanderParams),
    Autogain(AutogainParams),
    Maximizer(MaximizerParams),
    Exciter(ExciterParams),
    BassEnhancer(BassEnhancerParams),
    Convolver(ConvolverParams),
}

impl EffectConfig {
    pub fn name(&self) -> &'static str {
        match self {
            EffectConfig::Filter(_) => "Filter",
            EffectConfig::Eq(_) => "Equalizer",
            EffectConfig::Gate(_) => "Gate",
            EffectConfig::Compressor(_) => "Compressor",
            EffectConfig::Gain(_) => "Gain",
            EffectConfig::Limiter(_) => "Limiter",
            EffectConfig::Deesser(_) => "De-esser",
            EffectConfig::Expander(_) => "Expander",
            EffectConfig::Autogain(_) => "Auto-gain",
            EffectConfig::Maximizer(_) => "Maximizer",
            EffectConfig::Exciter(_) => "Exciter",
            EffectConfig::BassEnhancer(_) => "Bass enhancer",
            EffectConfig::Convolver(_) => "Convolver",
        }
    }

    /// Magnitude response in dB at `freq` for filter-type effects (for the EQ curve).
    pub fn response_db(&self, freq: f32, sr: f32) -> Option<f32> {
        match self {
            EffectConfig::Filter(p) => Some(p.coeffs(sr).magnitude_db(freq, sr) * p.stages() as f32),
            EffectConfig::Eq(p) => {
                Some(p.gain_db + p.bands.iter().map(|b| b.response_db(freq, sr)).sum::<f32>())
            }
            EffectConfig::Gain(p) => Some(p.gain_db),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    /// Stable identity across edits/reorders, so state follows the effect.
    pub id: u32,
    #[serde(default)]
    pub bypass: bool,
    #[serde(flatten)]
    pub effect: EffectConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainConfig {
    pub slots: Vec<EffectSlot>,
}

impl ChainConfig {
    /// The SM57 → "SM7B-ish" voice chain from the handoff.
    pub fn sm57_voice() -> Self {
        use FilterKind::*;
        let band = |kind, freq_hz, gain_db, q| EqBand { enabled: true, kind, freq_hz, gain_db, q, slope: 1 };
        let slots = vec![
            EffectConfig::Filter(FilterParams { kind: HighPass, freq_hz: 80.0, q: 0.707, slope: 1 }),
            EffectConfig::Eq(EqParams {
                bands: vec![
                    band(LowShelf, 160.0, 3.0, 0.707),
                    band(Peak, 350.0, -1.5, 1.2),
                    band(Peak, 2200.0, 1.0, 0.9),
                    band(Peak, 5800.0, -4.0, 1.5),
                    band(HighShelf, 9000.0, -2.0, 0.707),
                ],
                gain_db: 0.0,
            }),
            EffectConfig::Gate(GateParams::default()),
            EffectConfig::Compressor(CompressorParams::default()),
            EffectConfig::Gain(GainParams { gain_db: 6.0 }),
            EffectConfig::Limiter(LimiterParams::default()),
        ];
        ChainConfig {
            slots: slots
                .into_iter()
                .enumerate()
                .map(|(i, effect)| EffectSlot { id: i as u32 + 1, bypass: false, effect })
                .collect(),
        }
    }

    /// Delay the chain itself adds, in ms (limiter look-ahead; the filters are minimum-phase).
    pub fn latency_ms(&self) -> f32 {
        self.slots
            .iter()
            .filter(|s| !s.bypass)
            .map(|s| match &s.effect {
                EffectConfig::Maximizer(_) => Maximizer::latency_ms(),
                EffectConfig::Limiter(l) => {
                    let frames = (l.lookahead_ms.clamp(0.1, 20.0) * 0.001 * crate::SAMPLE_RATE as f32).round().max(1.0);
                    frames * 1000.0 / crate::SAMPLE_RATE as f32
                }
                _ => 0.0,
            })
            .sum()
    }

    pub fn next_id(&self) -> u32 {
        self.slots.iter().map(|s| s.id).max().unwrap_or(0) + 1
    }
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self::sm57_voice()
    }
}

enum Proc {
    Filter(Vec<Biquad>),
    /// All bands' stages flattened in order, plus the output gain.
    Eq(Vec<Biquad>, Gain),
    Gate(Gate),
    Compressor(Compressor),
    Gain(Gain),
    Limiter(Limiter),
    Deesser(Deesser),
    Expander(Expander),
    Autogain(Autogain),
    Maximizer(Maximizer),
    Exciter(Exciter),
    BassEnhancer(BassEnhancer),
    Convolver(Box<Convolver>),
}

impl Proc {
    fn build(cfg: &EffectConfig, sr: f32) -> Proc {
        match cfg {
            EffectConfig::Filter(p) => Proc::Filter(vec![Biquad::new(p.coeffs(sr)); p.stages()]),
            EffectConfig::Eq(p) => Proc::Eq(
                p.bands
                    .iter()
                    .flat_map(|b| std::iter::repeat_n(Biquad::new(b.coeffs(sr)), b.stages()))
                    .collect(),
                Gain::new(&GainParams { gain_db: p.gain_db }, sr),
            ),
            EffectConfig::Gate(p) => Proc::Gate(Gate::new(p, sr)),
            EffectConfig::Compressor(p) => Proc::Compressor(Compressor::new(p, sr)),
            EffectConfig::Gain(p) => Proc::Gain(Gain::new(p, sr)),
            EffectConfig::Limiter(p) => Proc::Limiter(Limiter::new(p, sr)),
            EffectConfig::Deesser(p) => Proc::Deesser(Deesser::new(p, sr)),
            EffectConfig::Expander(p) => Proc::Expander(Expander::new(p, sr)),
            EffectConfig::Autogain(p) => Proc::Autogain(Autogain::new(p, sr)),
            EffectConfig::Maximizer(p) => Proc::Maximizer(Maximizer::new(p, sr)),
            EffectConfig::Exciter(p) => Proc::Exciter(Exciter::new(p, sr)),
            EffectConfig::BassEnhancer(p) => Proc::BassEnhancer(BassEnhancer::new(p, sr)),
            EffectConfig::Convolver(p) => Proc::Convolver(Box::new(Convolver::new(p, sr))),
        }
    }

    /// Can this processor take `cfg` in place, without reallocating?
    fn compatible(&self, cfg: &EffectConfig, sr: f32) -> bool {
        match (self, cfg) {
            (Proc::Filter(f), EffectConfig::Filter(p)) => f.len() == p.stages(),
            (Proc::Gate(_), EffectConfig::Gate(_))
            | (Proc::Compressor(_), EffectConfig::Compressor(_))
            | (Proc::Gain(_), EffectConfig::Gain(_)) => true,
            (Proc::Eq(f, _), EffectConfig::Eq(p)) => f.len() == p.total_stages(),
            (Proc::Limiter(l), EffectConfig::Limiter(p)) => l.compatible(p, sr),
            (Proc::Deesser(_), EffectConfig::Deesser(_))
            | (Proc::Expander(_), EffectConfig::Expander(_))
            | (Proc::Autogain(_), EffectConfig::Autogain(_))
            | (Proc::Maximizer(_), EffectConfig::Maximizer(_))
            | (Proc::Exciter(_), EffectConfig::Exciter(_))
            | (Proc::BassEnhancer(_), EffectConfig::BassEnhancer(_)) => true,
            // Decided in `can_adopt` by comparing the loaded kernels.
            (Proc::Convolver(_), EffectConfig::Convolver(_)) => false,
            _ => false,
        }
    }

    fn set_params(&mut self, cfg: &EffectConfig, sr: f32) {
        match (self, cfg) {
            (Proc::Filter(fs), EffectConfig::Filter(p)) => {
                let c = p.coeffs(sr);
                fs.iter_mut().for_each(|f| f.set_coeffs(c));
            }
            (Proc::Eq(fs, gain), EffectConfig::Eq(p)) => {
                let mut it = fs.iter_mut();
                for b in &p.bands {
                    let c = b.coeffs(sr);
                    for f in it.by_ref().take(b.stages()) {
                        f.set_coeffs(c);
                    }
                }
                gain.set_params(&GainParams { gain_db: p.gain_db }, sr);
            }
            (Proc::Gate(g), EffectConfig::Gate(p)) => g.set_params(p, sr),
            (Proc::Compressor(c), EffectConfig::Compressor(p)) => c.set_params(p, sr),
            (Proc::Gain(g), EffectConfig::Gain(p)) => g.set_params(p, sr),
            (Proc::Limiter(l), EffectConfig::Limiter(p)) => l.set_params(p, sr),
            (Proc::Deesser(d), EffectConfig::Deesser(p)) => d.set_params(p, sr),
            (Proc::Expander(e), EffectConfig::Expander(p)) => e.set_params(p, sr),
            (Proc::Autogain(a), EffectConfig::Autogain(p)) => a.set_params(p, sr),
            (Proc::Maximizer(m), EffectConfig::Maximizer(p)) => m.set_params(p, sr),
            (Proc::Exciter(e), EffectConfig::Exciter(p)) => e.set_params(p, sr),
            (Proc::BassEnhancer(b), EffectConfig::BassEnhancer(p)) => b.set_params(p, sr),
            (Proc::Convolver(c), EffectConfig::Convolver(p)) => c.set_params(p, sr),
            _ => {}
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        match self {
            Proc::Filter(fs) => fs.iter_mut().fold(x, |x, f| f.process(x)),
            Proc::Eq(fs, gain) => gain.process(fs.iter_mut().fold(x, |x, f| f.process(x))),
            Proc::Gate(g) => g.process(x),
            Proc::Compressor(c) => c.process(x),
            Proc::Gain(g) => g.process(x),
            Proc::Limiter(l) => l.process(x),
            Proc::Deesser(d) => d.process(x),
            Proc::Expander(e) => e.process(x),
            Proc::Autogain(a) => a.process(x),
            Proc::Maximizer(m) => m.process(x),
            Proc::Exciter(e) => e.process(x),
            Proc::BassEnhancer(b) => b.process(x),
            Proc::Convolver(c) => c.process(x),
        }
    }

    fn reduction_db(&self) -> f32 {
        match self {
            Proc::Gate(g) => g.reduction_db(),
            Proc::Compressor(c) => c.reduction_db(),
            Proc::Limiter(l) => l.reduction_db(),
            Proc::Deesser(d) => d.reduction_db(),
            Proc::Expander(e) => e.reduction_db(),
            Proc::Maximizer(m) => m.reduction_db(),
            _ => 0.0,
        }
    }
}

struct SlotProc {
    id: u32,
    cfg: EffectConfig,
    bypass: bool,
    /// Wet amount, ramps toward 0 (bypassed) or 1 to avoid clicks.
    mix: f32,
    proc: Proc,
}

pub struct Chain {
    slots: Vec<SlotProc>,
    sr: f32,
    mix_c: f32,
}

const BYPASS_FADE_MS: f32 = 10.0;

impl Chain {
    pub fn build(cfg: &ChainConfig, sr: f32) -> Chain {
        Chain {
            slots: cfg
                .slots
                .iter()
                .map(|s| SlotProc {
                    id: s.id,
                    cfg: s.effect.clone(),
                    bypass: s.bypass,
                    mix: if s.bypass { 0.0 } else { 1.0 },
                    proc: Proc::build(&s.effect, sr),
                })
                .collect(),
            sr,
            mix_c: time_coeff(BYPASS_FADE_MS / 4.0, sr),
        }
    }

    /// Take over processor state from the chain this one replaces. Realtime-safe:
    /// state moves by swapping, so nothing is allocated or freed here.
    pub fn adopt_state(&mut self, old: &mut Chain) {
        let sr = self.sr;
        for new in &mut self.slots {
            if let Some(o) = old.slots.iter_mut().find(|o| o.id == new.id) {
                let can = match (&o.proc, &new.proc) {
                    (Proc::Convolver(a), Proc::Convolver(b)) => a.same_kernel(b),
                    _ => o.proc.compatible(&new.cfg, sr),
                };
                if can {
                    std::mem::swap(&mut new.proc, &mut o.proc);
                    new.proc.set_params(&new.cfg, sr);
                }
                new.mix = o.mix;
            }
        }
    }

    pub fn process(&mut self, buf: &mut [f32], meters: &Meters) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let target = if slot.bypass { 0.0 } else { 1.0 };
            let settled = (slot.mix - target).abs() < 1e-4;
            if settled && slot.bypass {
                slot.mix = 0.0;
                // Keep dynamics/filters warm so un-bypassing doesn't thump.
                for &x in buf.iter() {
                    slot.proc.process(x);
                }
            } else if settled {
                slot.mix = 1.0;
                for x in buf.iter_mut() {
                    *x = slot.proc.process(*x);
                }
            } else {
                for x in buf.iter_mut() {
                    slot.mix = slot.mix * self.mix_c + target * (1.0 - self.mix_c);
                    let wet = slot.proc.process(*x);
                    *x += (wet - *x) * slot.mix;
                }
            }
            if i < MAX_SLOTS {
                let gr = if slot.bypass { 0.0 } else { slot.proc.reduction_db() };
                meters.gr[i].raise(gr);
                match &slot.proc {
                    Proc::Gate(g) => meters.det[i].raise(g.detector_level()),
                    Proc::Autogain(a) => meters.aux[i].store(if slot.bypass { 0.0 } else { a.gain_db() }),
                    _ => {}
                }
            }
        }
        let (peak, rms) = block_levels(buf);
        meters.out_peak.raise(peak);
        meters.out_rms.raise(rms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_round_trips_through_json() {
        let cfg = ChainConfig::sm57_voice();
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: ChainConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert!(json.contains("\"type\": \"compressor\""));
    }

    #[test]
    fn adopting_state_keeps_processing_continuous() {
        let sr = 48_000.0;
        let meters = Meters::default();
        let cfg = ChainConfig::sm57_voice();
        let mut a = Chain::build(&cfg, sr);
        let tone = |n: usize| 0.2 * (2.0 * std::f32::consts::PI * 300.0 * n as f32 / sr).sin();
        let mut buf: Vec<f32> = (0..4800).map(tone).collect();
        a.process(&mut buf, &meters);
        let last = buf[4799];

        // Same config rebuilt + adopted: output continues smoothly (no reset transient).
        let mut b = Chain::build(&cfg, sr);
        b.adopt_state(&mut a);
        let mut next: Vec<f32> = (4800..4801).map(tone).collect();
        b.process(&mut next, &meters);
        assert!((next[0] - last).abs() < 0.05, "jump {} -> {}", last, next[0]);
    }

    #[test]
    fn eq_response_sums_bands() {
        let cfg = ChainConfig::sm57_voice();
        let eq = &cfg.slots[1].effect;
        let at_5800 = eq.response_db(5800.0, 48_000.0).unwrap();
        // -4 dB peak plus neighbouring bands' skirts.
        assert!(at_5800 < -3.0 && at_5800 > -6.0, "{at_5800}");
    }
}
