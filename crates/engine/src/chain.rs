//! Effect chain: serializable config (presets) and the realtime processor built from it.
//!
//! The control thread builds a new [`Chain`] whenever the config changes and
//! hands it to the audio thread, which calls [`Chain::adopt_state`] so filters,
//! envelopes and delay lines carry over (no clicks), then sends the old chain
//! back to be dropped off the audio thread.

use crate::dsp::{
    Biquad, Coeffs, Compressor, CompressorParams, FilterKind, Gain, GainParams, Gate, GateParams,
    Limiter, LimiterParams, time_coeff,
};
use crate::meters::{block_levels, Meters, MAX_SLOTS};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterParams {
    pub kind: FilterKind,
    pub freq_hz: f32,
    pub q: f32,
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
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EqParams {
    pub bands: Vec<EqBand>,
}

impl EqBand {
    pub fn coeffs(&self, sr: f32) -> Coeffs {
        if self.enabled {
            Coeffs::design(self.kind, self.freq_hz, self.q, self.gain_db, sr)
        } else {
            Coeffs::IDENTITY
        }
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
        }
    }

    /// Magnitude response in dB at `freq` for filter-type effects (for the EQ curve).
    pub fn response_db(&self, freq: f32, sr: f32) -> Option<f32> {
        match self {
            EffectConfig::Filter(p) => {
                Some(Coeffs::design(p.kind, p.freq_hz, p.q, 0.0, sr).magnitude_db(freq, sr))
            }
            EffectConfig::Eq(p) => {
                Some(p.bands.iter().map(|b| b.coeffs(sr).magnitude_db(freq, sr)).sum())
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
        let band = |kind, freq_hz, gain_db, q| EqBand { enabled: true, kind, freq_hz, gain_db, q };
        let slots = vec![
            EffectConfig::Filter(FilterParams { kind: HighPass, freq_hz: 80.0, q: 0.707 }),
            EffectConfig::Eq(EqParams {
                bands: vec![
                    band(LowShelf, 160.0, 3.0, 0.707),
                    band(Peak, 350.0, -1.5, 1.2),
                    band(Peak, 2200.0, 1.0, 0.9),
                    band(Peak, 5800.0, -4.0, 1.5),
                    band(HighShelf, 9000.0, -2.0, 0.707),
                ],
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
    Filter(Biquad),
    Eq(Vec<Biquad>),
    Gate(Gate),
    Compressor(Compressor),
    Gain(Gain),
    Limiter(Limiter),
}

impl Proc {
    fn build(cfg: &EffectConfig, sr: f32) -> Proc {
        match cfg {
            EffectConfig::Filter(p) => {
                Proc::Filter(Biquad::new(Coeffs::design(p.kind, p.freq_hz, p.q, 0.0, sr)))
            }
            EffectConfig::Eq(p) => {
                Proc::Eq(p.bands.iter().map(|b| Biquad::new(b.coeffs(sr))).collect())
            }
            EffectConfig::Gate(p) => Proc::Gate(Gate::new(p, sr)),
            EffectConfig::Compressor(p) => Proc::Compressor(Compressor::new(p, sr)),
            EffectConfig::Gain(p) => Proc::Gain(Gain::new(p, sr)),
            EffectConfig::Limiter(p) => Proc::Limiter(Limiter::new(p, sr)),
        }
    }

    /// Can this processor take `cfg` in place, without reallocating?
    fn compatible(&self, cfg: &EffectConfig, sr: f32) -> bool {
        match (self, cfg) {
            (Proc::Filter(_), EffectConfig::Filter(_))
            | (Proc::Gate(_), EffectConfig::Gate(_))
            | (Proc::Compressor(_), EffectConfig::Compressor(_))
            | (Proc::Gain(_), EffectConfig::Gain(_)) => true,
            (Proc::Eq(f), EffectConfig::Eq(p)) => f.len() == p.bands.len(),
            (Proc::Limiter(l), EffectConfig::Limiter(p)) => l.compatible(p, sr),
            _ => false,
        }
    }

    fn set_params(&mut self, cfg: &EffectConfig, sr: f32) {
        match (self, cfg) {
            (Proc::Filter(f), EffectConfig::Filter(p)) => {
                f.set_coeffs(Coeffs::design(p.kind, p.freq_hz, p.q, 0.0, sr))
            }
            (Proc::Eq(fs), EffectConfig::Eq(p)) => {
                for (f, b) in fs.iter_mut().zip(&p.bands) {
                    f.set_coeffs(b.coeffs(sr));
                }
            }
            (Proc::Gate(g), EffectConfig::Gate(p)) => g.set_params(p, sr),
            (Proc::Compressor(c), EffectConfig::Compressor(p)) => c.set_params(p, sr),
            (Proc::Gain(g), EffectConfig::Gain(p)) => g.set_params(p, sr),
            (Proc::Limiter(l), EffectConfig::Limiter(p)) => l.set_params(p, sr),
            _ => {}
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        match self {
            Proc::Filter(f) => f.process(x),
            Proc::Eq(fs) => fs.iter_mut().fold(x, |x, f| f.process(x)),
            Proc::Gate(g) => g.process(x),
            Proc::Compressor(c) => c.process(x),
            Proc::Gain(g) => g.process(x),
            Proc::Limiter(l) => l.process(x),
        }
    }

    fn reduction_db(&self) -> f32 {
        match self {
            Proc::Gate(g) => g.reduction_db(),
            Proc::Compressor(c) => c.reduction_db(),
            Proc::Limiter(l) => l.reduction_db(),
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
                if o.proc.compatible(&new.cfg, sr) {
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
