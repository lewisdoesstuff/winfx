//! Per-effect parameter tables: ranges, units, get/set, display formatting.

use engine::chain::{EffectConfig, EqBand, FilterParams};
use engine::dsp::FilterKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKey {
    Freq,
    Q,
    Threshold,
    Hysteresis,
    Attack,
    Hold,
    Release,
    Range,
    ScHpf,
    Ratio,
    Knee,
    Rms,
    Makeup,
    Gain,
    Ceiling,
    Lookahead,
    BandFreq(usize),
    BandGain(usize),
    BandQ(usize),
}

#[derive(Clone, Copy, Debug)]
pub enum Unit {
    Hz,
    Db,
    Ms,
    Ratio,
    None,
}

#[derive(Clone, Debug)]
pub struct ParamSpec {
    pub key: ParamKey,
    pub label: String,
    pub unit: Unit,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub log: bool,
}

fn p(key: ParamKey, label: &str, unit: Unit, min: f32, max: f32, step: f32, log: bool) -> ParamSpec {
    ParamSpec { key, label: label.into(), unit, min, max, step, log }
}

pub fn specs(cfg: &EffectConfig) -> Vec<ParamSpec> {
    use ParamKey::*;
    use Unit::*;
    match cfg {
        EffectConfig::Filter(_) => vec![
            p(Freq, "Frequency", Hz, 20.0, 20_000.0, 1.0, true),
            p(Q, "Q", None, 0.3, 4.0, 0.01, false),
        ],
        EffectConfig::Eq(e) => e
            .bands
            .iter()
            .enumerate()
            .flat_map(|(i, b)| {
                let name = format!("{} · {}", i + 1, band_name(b));
                let mut v = vec![p(BandFreq(i), &format!("{name} freq"), Hz, 20.0, 20_000.0, 1.0, true)];
                if has_gain(b.kind) {
                    v.push(p(BandGain(i), &format!("{name} gain"), Db, -18.0, 18.0, 0.1, false));
                }
                v.push(p(BandQ(i), &format!("{name} Q"), None, 0.1, 10.0, 0.01, false));
                v
            })
            .collect(),
        EffectConfig::Gate(_) => vec![
            p(Threshold, "Threshold", Db, -80.0, 0.0, 0.5, false),
            p(Hysteresis, "Hysteresis", Db, 0.0, 20.0, 0.5, false),
            p(Attack, "Attack", Ms, 0.1, 50.0, 0.1, false),
            p(Hold, "Hold", Ms, 0.0, 500.0, 1.0, false),
            p(Release, "Release", Ms, 5.0, 1000.0, 1.0, false),
            p(Range, "Range", Db, 0.0, 80.0, 0.5, false),
            p(ScHpf, "Sidechain HPF", Hz, 20.0, 500.0, 1.0, true),
        ],
        EffectConfig::Compressor(_) => vec![
            p(Threshold, "Threshold", Db, -60.0, 0.0, 0.5, false),
            p(ParamKey::Ratio, "Ratio", Unit::Ratio, 1.0, 20.0, 0.1, false),
            p(Attack, "Attack", Ms, 0.1, 100.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 1000.0, 1.0, false),
            p(Knee, "Knee", Db, 0.0, 24.0, 0.5, false),
            p(Rms, "RMS window", Ms, 0.1, 100.0, 0.1, false),
            p(Makeup, "Make-up", Db, 0.0, 24.0, 0.1, false),
        ],
        EffectConfig::Gain(_) => vec![p(Gain, "Gain", Db, -24.0, 24.0, 0.1, false)],
        EffectConfig::Limiter(_) => vec![
            p(Ceiling, "Ceiling", Db, -12.0, 0.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 500.0, 1.0, false),
            p(Lookahead, "Lookahead", Ms, 0.5, 10.0, 0.1, false),
        ],
    }
}

pub fn band_name(b: &EqBand) -> &'static str {
    match b.kind {
        FilterKind::LowShelf => "Low shelf",
        FilterKind::HighShelf => "High shelf",
        FilterKind::Peak => "Peak",
        FilterKind::HighPass => "High-pass",
        FilterKind::LowPass => "Low-pass",
    }
}

fn has_gain(k: FilterKind) -> bool {
    matches!(k, FilterKind::Peak | FilterKind::LowShelf | FilterKind::HighShelf)
}

pub fn filter_name(f: &FilterParams) -> &'static str {
    match f.kind {
        FilterKind::HighPass => "High-pass",
        FilterKind::LowPass => "Low-pass",
        _ => "Filter",
    }
}

pub fn get(cfg: &EffectConfig, key: ParamKey) -> f32 {
    use ParamKey::*;
    match (cfg, key) {
        (EffectConfig::Filter(f), Freq) => f.freq_hz,
        (EffectConfig::Filter(f), Q) => f.q,
        (EffectConfig::Eq(e), BandFreq(i)) => e.bands.get(i).map_or(0.0, |b| b.freq_hz),
        (EffectConfig::Eq(e), BandGain(i)) => e.bands.get(i).map_or(0.0, |b| b.gain_db),
        (EffectConfig::Eq(e), BandQ(i)) => e.bands.get(i).map_or(0.0, |b| b.q),
        (EffectConfig::Gate(g), Threshold) => g.threshold_db,
        (EffectConfig::Gate(g), Hysteresis) => g.hysteresis_db,
        (EffectConfig::Gate(g), Attack) => g.attack_ms,
        (EffectConfig::Gate(g), Hold) => g.hold_ms,
        (EffectConfig::Gate(g), Release) => g.release_ms,
        (EffectConfig::Gate(g), Range) => g.range_db,
        (EffectConfig::Gate(g), ScHpf) => g.sidechain_hpf_hz,
        (EffectConfig::Compressor(c), Threshold) => c.threshold_db,
        (EffectConfig::Compressor(c), Ratio) => c.ratio,
        (EffectConfig::Compressor(c), Attack) => c.attack_ms,
        (EffectConfig::Compressor(c), Release) => c.release_ms,
        (EffectConfig::Compressor(c), Knee) => c.knee_db,
        (EffectConfig::Compressor(c), Rms) => c.rms_ms,
        (EffectConfig::Compressor(c), Makeup) => c.makeup_db,
        (EffectConfig::Gain(g), Gain) => g.gain_db,
        (EffectConfig::Limiter(l), Ceiling) => l.ceiling_db,
        (EffectConfig::Limiter(l), Release) => l.release_ms,
        (EffectConfig::Limiter(l), Lookahead) => l.lookahead_ms,
        _ => 0.0,
    }
}

pub fn set(cfg: &mut EffectConfig, key: ParamKey, v: f32) {
    use ParamKey::*;
    match (cfg, key) {
        (EffectConfig::Filter(f), Freq) => f.freq_hz = v,
        (EffectConfig::Filter(f), Q) => f.q = v,
        (EffectConfig::Eq(e), BandFreq(i)) => e.bands.get_mut(i).into_iter().for_each(|b| b.freq_hz = v),
        (EffectConfig::Eq(e), BandGain(i)) => e.bands.get_mut(i).into_iter().for_each(|b| b.gain_db = v),
        (EffectConfig::Eq(e), BandQ(i)) => e.bands.get_mut(i).into_iter().for_each(|b| b.q = v),
        (EffectConfig::Gate(g), Threshold) => g.threshold_db = v,
        (EffectConfig::Gate(g), Hysteresis) => g.hysteresis_db = v,
        (EffectConfig::Gate(g), Attack) => g.attack_ms = v,
        (EffectConfig::Gate(g), Hold) => g.hold_ms = v,
        (EffectConfig::Gate(g), Release) => g.release_ms = v,
        (EffectConfig::Gate(g), Range) => g.range_db = v,
        (EffectConfig::Gate(g), ScHpf) => g.sidechain_hpf_hz = v,
        (EffectConfig::Compressor(c), Threshold) => c.threshold_db = v,
        (EffectConfig::Compressor(c), Ratio) => c.ratio = v,
        (EffectConfig::Compressor(c), Attack) => c.attack_ms = v,
        (EffectConfig::Compressor(c), Release) => c.release_ms = v,
        (EffectConfig::Compressor(c), Knee) => c.knee_db = v,
        (EffectConfig::Compressor(c), Rms) => c.rms_ms = v,
        (EffectConfig::Compressor(c), Makeup) => c.makeup_db = v,
        (EffectConfig::Gain(g), Gain) => g.gain_db = v,
        (EffectConfig::Limiter(l), Ceiling) => l.ceiling_db = v,
        (EffectConfig::Limiter(l), Release) => l.release_ms = v,
        (EffectConfig::Limiter(l), Lookahead) => l.lookahead_ms = v,
        _ => {}
    }
}

pub fn format(unit: Unit, v: f32) -> String {
    match unit {
        Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        Unit::Hz => format!("{v:.0} Hz"),
        Unit::Db => format!("{v:+.1} dB"),
        Unit::Ms if v < 10.0 => format!("{v:.1} ms"),
        Unit::Ms => format!("{v:.0} ms"),
        Unit::Ratio => format!("{v:.1}:1"),
        Unit::None => format!("{v:.2}"),
    }
}

/// Is this effect a dynamics processor (has a gain-reduction meter)?
pub fn has_gr(cfg: &EffectConfig) -> bool {
    matches!(cfg, EffectConfig::Gate(_) | EffectConfig::Compressor(_) | EffectConfig::Limiter(_))
}

pub fn title(cfg: &EffectConfig) -> String {
    match cfg {
        EffectConfig::Filter(f) => filter_name(f).into(),
        other => other.name().into(),
    }
}

/// Defaults for "Add effect".
pub fn new_effect(kind: &str) -> Option<EffectConfig> {
    use engine::chain::*;
    use engine::dsp::*;
    Some(match kind {
        "High-pass" => EffectConfig::Filter(FilterParams { kind: FilterKind::HighPass, freq_hz: 80.0, q: 0.707 }),
        "Low-pass" => EffectConfig::Filter(FilterParams { kind: FilterKind::LowPass, freq_hz: 12_000.0, q: 0.707 }),
        "Equalizer" => EffectConfig::Eq(EqParams {
            bands: vec![
                EqBand { enabled: true, kind: FilterKind::LowShelf, freq_hz: 120.0, gain_db: 0.0, q: 0.707 },
                EqBand { enabled: true, kind: FilterKind::Peak, freq_hz: 500.0, gain_db: 0.0, q: 1.0 },
                EqBand { enabled: true, kind: FilterKind::Peak, freq_hz: 3000.0, gain_db: 0.0, q: 1.0 },
                EqBand { enabled: true, kind: FilterKind::HighShelf, freq_hz: 8000.0, gain_db: 0.0, q: 0.707 },
            ],
        }),
        "Gate" => EffectConfig::Gate(GateParams::default()),
        "Compressor" => EffectConfig::Compressor(CompressorParams::default()),
        "Gain" => EffectConfig::Gain(GainParams::default()),
        "Limiter" => EffectConfig::Limiter(LimiterParams::default()),
        _ => return None,
    })
}

pub const ADDABLE: &[&str] =
    &["High-pass", "Low-pass", "Equalizer", "Gate", "Compressor", "Gain", "Limiter"];
