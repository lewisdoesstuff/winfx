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
    /// De-esser split/wide.
    Mode,
    Target,
    Silence,
    MaxBoost,
    MaxCut,
    Speed,
    Amount,
    Drive,
    Floor,
    Mix,
    Semitones,
    Cents,
    Window,
    Voice1Semi,
    Voice1Db,
    Voice2Semi,
    Voice2Db,
    DryDb,
    Key,
    ScaleKind,
    Room,
    Damping,
    Predelay,
    Time,
    Feedback,
    HighCut,
    LowCut,
    Bits,
    Downsample,
    BandFreq(usize),
    BandGain(usize),
    BandQ(usize),
    BandSlope(usize),
    /// Filter card slope.
    Slope,
    /// EQ output gain.
    EqGain,
}

#[derive(Clone, Copy, Debug)]
pub enum Unit {
    Hz,
    Db,
    /// A dB amount with no sign (hysteresis, range, knee).
    DbAmount,
    Ms,
    Ratio,
    /// Cascaded stages, shown as dB/oct.
    Slope,
    /// Loudness, LUFS.
    Lufs,
    Seconds,
    /// 0 = split band, 1 = whole signal.
    DeessMode,
    Percent,
    Semitones,
    Note,
    ScaleName,
    Times,
    Cents,
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
            p(ParamKey::Slope, "Slope", Unit::Slope, 1.0, 4.0, 1.0, false),
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
                v.push(p(BandSlope(i), &format!("{name} slope"), Unit::Slope, 1.0, 4.0, 1.0, false));
                v
            })
            .chain([p(EqGain, "Output gain", Db, -24.0, 24.0, 0.1, false)])
            .collect(),
        EffectConfig::Gate(_) => vec![
            p(Threshold, "Threshold", Db, -80.0, 0.0, 0.5, false),
            p(Hysteresis, "Hysteresis", DbAmount, 0.0, 20.0, 0.5, false),
            p(Attack, "Attack", Ms, 0.1, 50.0, 0.1, false),
            p(Hold, "Hold", Ms, 0.0, 500.0, 1.0, false),
            p(Release, "Release", Ms, 5.0, 1000.0, 1.0, false),
            p(Range, "Range", DbAmount, 0.0, 80.0, 0.5, false),
            p(ScHpf, "Sidechain HPF", Hz, 20.0, 500.0, 1.0, true),
        ],
        EffectConfig::Compressor(_) => vec![
            p(Threshold, "Threshold", Db, -60.0, 0.0, 0.5, false),
            p(ParamKey::Ratio, "Ratio", Unit::Ratio, 1.0, 20.0, 0.1, false),
            p(Attack, "Attack", Ms, 0.1, 100.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 1000.0, 1.0, false),
            p(Knee, "Knee", DbAmount, 0.0, 24.0, 0.5, false),
            p(Rms, "RMS window", Ms, 0.1, 100.0, 0.1, false),
            p(Makeup, "Make-up", Db, 0.0, 24.0, 0.1, false),
        ],
        EffectConfig::Gain(_) => vec![p(Gain, "Gain", Db, -24.0, 24.0, 0.1, false)],
        EffectConfig::Limiter(_) => vec![
            p(Ceiling, "Ceiling", Db, -12.0, 0.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 500.0, 1.0, false),
            p(Lookahead, "Lookahead", Ms, 0.5, 10.0, 0.1, false),
        ],
        EffectConfig::Deesser(_) => vec![
            p(Freq, "Frequency", Hz, 2000.0, 16_000.0, 10.0, true),
            p(Threshold, "Threshold", Db, -60.0, 0.0, 0.5, false),
            p(ParamKey::Ratio, "Ratio", Unit::Ratio, 1.0, 20.0, 0.1, false),
            p(Range, "Range", DbAmount, 0.0, 24.0, 0.5, false),
            p(Release, "Release", Ms, 5.0, 300.0, 1.0, false),
            p(Mode, "Mode", DeessMode, 0.0, 1.0, 1.0, false),
        ],
        EffectConfig::Expander(_) => vec![
            p(Threshold, "Threshold", Db, -80.0, 0.0, 0.5, false),
            p(ParamKey::Ratio, "Ratio", Unit::Ratio, 1.0, 10.0, 0.1, false),
            p(Range, "Range", DbAmount, 0.0, 60.0, 0.5, false),
            p(Attack, "Attack", Ms, 0.1, 100.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 1000.0, 1.0, false),
            p(Knee, "Knee", DbAmount, 0.0, 24.0, 0.5, false),
        ],
        EffectConfig::Autogain(_) => vec![
            p(Target, "Target", Lufs, -36.0, -6.0, 0.5, false),
            p(Silence, "Silence below", Lufs, -70.0, -20.0, 0.5, false),
            p(MaxBoost, "Max boost", DbAmount, 0.0, 24.0, 0.5, false),
            p(MaxCut, "Max cut", DbAmount, 0.0, 24.0, 0.5, false),
            p(Speed, "Speed", Seconds, 0.2, 10.0, 0.1, false),
        ],
        EffectConfig::Exciter(_) => vec![
            p(Amount, "Amount", Db, -40.0, 6.0, 0.5, false),
            p(Drive, "Drive", None, 0.5, 10.0, 0.1, false),
            p(Freq, "From", Hz, 1000.0, 12_000.0, 10.0, true),
            p(Ceiling, "Up to", Hz, 6000.0, 20_000.0, 10.0, true),
        ],
        EffectConfig::BassEnhancer(_) => vec![
            p(Amount, "Amount", Db, -40.0, 6.0, 0.5, false),
            p(Drive, "Drive", None, 0.5, 10.0, 0.1, false),
            p(Freq, "Below", Hz, 40.0, 300.0, 1.0, true),
            p(Floor, "Floor", Hz, 10.0, 120.0, 1.0, true),
        ],
        EffectConfig::Convolver(_) => vec![
            p(Mix, "Mix", Percent, 0.0, 1.0, 0.01, false),
            p(Gain, "Gain", Db, -24.0, 24.0, 0.1, false),
        ],
        EffectConfig::Pitch(_) => vec![
            p(ParamKey::Semitones, "Semitones", Unit::Semitones, -12.0, 12.0, 1.0, false),
            p(ParamKey::Cents, "Fine", Unit::Cents, -100.0, 100.0, 1.0, false),
            p(Mix, "Mix", Percent, 0.0, 1.0, 0.01, false),
            p(Window, "Grain", Ms, 20.0, 80.0, 1.0, false),
        ],
        EffectConfig::Harmonizer(_) => vec![
            p(Voice1Semi, "Voice 1", Unit::Semitones, -12.0, 12.0, 1.0, false),
            p(Voice1Db, "Voice 1 level", Db, -40.0, 6.0, 0.5, false),
            p(Voice2Semi, "Voice 2", Unit::Semitones, -12.0, 12.0, 1.0, false),
            p(Voice2Db, "Voice 2 level", Db, -40.0, 6.0, 0.5, false),
            p(DryDb, "Your voice", Db, -40.0, 6.0, 0.5, false),
        ],
        EffectConfig::Autotune(_) => vec![
            p(Key, "Key", Note, 0.0, 11.0, 1.0, false),
            p(ScaleKind, "Scale", ScaleName, 0.0, 2.0, 1.0, false),
            p(Speed, "Retune speed", Ms, 0.0, 200.0, 1.0, false),
            p(Mix, "Amount", Percent, 0.0, 1.0, 0.01, false),
        ],
        EffectConfig::Reverb(_) => vec![
            p(Room, "Room size", Percent, 0.0, 1.0, 0.01, false),
            p(Damping, "Damping", Percent, 0.0, 1.0, 0.01, false),
            p(Predelay, "Pre-delay", Ms, 0.0, 200.0, 1.0, false),
            p(Mix, "Amount", Percent, 0.0, 1.0, 0.01, false),
        ],
        EffectConfig::Delay(_) => vec![
            p(Time, "Time", Ms, 10.0, 2000.0, 1.0, true),
            p(Feedback, "Feedback", Percent, 0.0, 0.95, 0.01, false),
            p(Mix, "Amount", Percent, 0.0, 1.0, 0.01, false),
            p(HighCut, "High cut", Hz, 1000.0, 20_000.0, 10.0, true),
            p(LowCut, "Low cut", Hz, 20.0, 1000.0, 1.0, true),
        ],
        EffectConfig::Crusher(_) => vec![
            p(Bits, "Bits", None, 1.0, 16.0, 0.1, false),
            p(Downsample, "Downsample", Times, 1.0, 64.0, 1.0, false),
            p(Mix, "Mix", Percent, 0.0, 1.0, 0.01, false),
            p(Gain, "Gain", Db, -24.0, 12.0, 0.1, false),
        ],
        EffectConfig::Maximizer(_) => vec![
            p(Threshold, "Threshold", Db, -24.0, 0.0, 0.1, false),
            p(Ceiling, "Ceiling", Db, -6.0, 0.0, 0.1, false),
            p(Release, "Release", Ms, 5.0, 500.0, 1.0, false),
        ],
    }
}

pub fn band_name(b: &EqBand) -> &'static str {
    if b.kind == FilterKind::Peak { "Peak" } else { kind_name(b.kind) }
}

fn has_gain(k: FilterKind) -> bool {
    k.has_gain()
}

/// Band types offered in the EQ editor, in menu order.
pub const BAND_KINDS: &[(FilterKind, &str)] = &[
    (FilterKind::Peak, "Bell"),
    (FilterKind::LowShelf, "Low shelf"),
    (FilterKind::HighShelf, "High shelf"),
    (FilterKind::HighPass, "High-pass"),
    (FilterKind::LowPass, "Low-pass"),
    (FilterKind::Notch, "Notch"),
    (FilterKind::BandPass, "Band-pass"),
    (FilterKind::AllPass, "All-pass"),
];

pub fn kind_name(k: FilterKind) -> &'static str {
    BAND_KINDS.iter().find(|(kk, _)| *kk == k).map_or("Bell", |(_, n)| n)
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
        (EffectConfig::Eq(e), BandSlope(i)) => e.bands.get(i).map_or(1.0, |b| b.slope as f32),
        (EffectConfig::Eq(e), EqGain) => e.gain_db,
        (EffectConfig::Filter(f), ParamKey::Slope) => f.slope as f32,
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
        (EffectConfig::Deesser(d), Freq) => d.freq_hz,
        (EffectConfig::Deesser(d), Threshold) => d.threshold_db,
        (EffectConfig::Deesser(d), Ratio) => d.ratio,
        (EffectConfig::Deesser(d), Range) => d.range_db,
        (EffectConfig::Deesser(d), Release) => d.release_ms,
        (EffectConfig::Deesser(d), Mode) => (d.mode == engine::dsp::DeesserMode::Wide) as u8 as f32,
        (EffectConfig::Expander(e), Threshold) => e.threshold_db,
        (EffectConfig::Expander(e), Ratio) => e.ratio,
        (EffectConfig::Expander(e), Range) => e.range_db,
        (EffectConfig::Expander(e), Attack) => e.attack_ms,
        (EffectConfig::Expander(e), Release) => e.release_ms,
        (EffectConfig::Expander(e), Knee) => e.knee_db,
        (EffectConfig::Autogain(a), Target) => a.target_lufs,
        (EffectConfig::Autogain(a), Silence) => a.silence_lufs,
        (EffectConfig::Autogain(a), MaxBoost) => a.max_boost_db,
        (EffectConfig::Autogain(a), MaxCut) => a.max_cut_db,
        (EffectConfig::Autogain(a), Speed) => a.speed_s,
        (EffectConfig::Exciter(e), Amount) => e.amount_db,
        (EffectConfig::Exciter(e), Drive) => e.drive,
        (EffectConfig::Exciter(e), Freq) => e.freq_hz,
        (EffectConfig::Exciter(e), Ceiling) => e.ceiling_hz,
        (EffectConfig::BassEnhancer(b), Amount) => b.amount_db,
        (EffectConfig::BassEnhancer(b), Drive) => b.drive,
        (EffectConfig::BassEnhancer(b), Freq) => b.freq_hz,
        (EffectConfig::BassEnhancer(b), Floor) => b.floor_hz,
        (EffectConfig::Convolver(c), Mix) => c.mix,
        (EffectConfig::Convolver(c), Gain) => c.gain_db,
        (EffectConfig::Pitch(x), Semitones) => x.semitones,
        (EffectConfig::Pitch(x), Cents) => x.cents,
        (EffectConfig::Pitch(x), Mix) => x.mix,
        (EffectConfig::Pitch(x), Window) => x.window_ms,
        (EffectConfig::Harmonizer(x), Voice1Semi) => x.voice1_semitones,
        (EffectConfig::Harmonizer(x), Voice1Db) => x.voice1_db,
        (EffectConfig::Harmonizer(x), Voice2Semi) => x.voice2_semitones,
        (EffectConfig::Harmonizer(x), Voice2Db) => x.voice2_db,
        (EffectConfig::Harmonizer(x), DryDb) => x.dry_db,
        (EffectConfig::Autotune(x), Key) => x.key as f32,
        (EffectConfig::Autotune(x), ScaleKind) => match x.scale {
            engine::dsp::Scale::Chromatic => 0.0,
            engine::dsp::Scale::Major => 1.0,
            engine::dsp::Scale::Minor => 2.0,
        },
        (EffectConfig::Autotune(x), Speed) => x.speed_ms,
        (EffectConfig::Autotune(x), Mix) => x.mix,
        (EffectConfig::Reverb(x), Room) => x.room_size,
        (EffectConfig::Reverb(x), Damping) => x.damping,
        (EffectConfig::Reverb(x), Predelay) => x.predelay_ms,
        (EffectConfig::Reverb(x), Mix) => x.mix,
        (EffectConfig::Delay(x), Time) => x.time_ms,
        (EffectConfig::Delay(x), Feedback) => x.feedback,
        (EffectConfig::Delay(x), Mix) => x.mix,
        (EffectConfig::Delay(x), HighCut) => x.high_cut_hz,
        (EffectConfig::Delay(x), LowCut) => x.low_cut_hz,
        (EffectConfig::Crusher(x), Bits) => x.bits,
        (EffectConfig::Crusher(x), Downsample) => x.downsample,
        (EffectConfig::Crusher(x), Mix) => x.mix,
        (EffectConfig::Crusher(x), Gain) => x.gain_db,
        (EffectConfig::Maximizer(m), Threshold) => m.threshold_db,
        (EffectConfig::Maximizer(m), Ceiling) => m.ceiling_db,
        (EffectConfig::Maximizer(m), Release) => m.release_ms,
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
        (EffectConfig::Eq(e), BandSlope(i)) => e.bands.get_mut(i).into_iter().for_each(|b| b.slope = v.round() as u8),
        (EffectConfig::Eq(e), EqGain) => e.gain_db = v,
        (EffectConfig::Filter(f), ParamKey::Slope) => f.slope = v.round() as u8,
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
        (EffectConfig::Deesser(d), Freq) => d.freq_hz = v,
        (EffectConfig::Deesser(d), Threshold) => d.threshold_db = v,
        (EffectConfig::Deesser(d), Ratio) => d.ratio = v,
        (EffectConfig::Deesser(d), Range) => d.range_db = v,
        (EffectConfig::Deesser(d), Release) => d.release_ms = v,
        (EffectConfig::Deesser(d), Mode) => {
            d.mode = if v >= 0.5 { engine::dsp::DeesserMode::Wide } else { engine::dsp::DeesserMode::Split }
        }
        (EffectConfig::Expander(e), Threshold) => e.threshold_db = v,
        (EffectConfig::Expander(e), Ratio) => e.ratio = v,
        (EffectConfig::Expander(e), Range) => e.range_db = v,
        (EffectConfig::Expander(e), Attack) => e.attack_ms = v,
        (EffectConfig::Expander(e), Release) => e.release_ms = v,
        (EffectConfig::Expander(e), Knee) => e.knee_db = v,
        (EffectConfig::Autogain(a), Target) => a.target_lufs = v,
        (EffectConfig::Autogain(a), Silence) => a.silence_lufs = v,
        (EffectConfig::Autogain(a), MaxBoost) => a.max_boost_db = v,
        (EffectConfig::Autogain(a), MaxCut) => a.max_cut_db = v,
        (EffectConfig::Autogain(a), Speed) => a.speed_s = v,
        (EffectConfig::Exciter(e), Amount) => e.amount_db = v,
        (EffectConfig::Exciter(e), Drive) => e.drive = v,
        (EffectConfig::Exciter(e), Freq) => e.freq_hz = v,
        (EffectConfig::Exciter(e), Ceiling) => e.ceiling_hz = v,
        (EffectConfig::BassEnhancer(b), Amount) => b.amount_db = v,
        (EffectConfig::BassEnhancer(b), Drive) => b.drive = v,
        (EffectConfig::BassEnhancer(b), Freq) => b.freq_hz = v,
        (EffectConfig::BassEnhancer(b), Floor) => b.floor_hz = v,
        (EffectConfig::Convolver(c), Mix) => c.mix = v,
        (EffectConfig::Convolver(c), Gain) => c.gain_db = v,
        (EffectConfig::Pitch(x), Semitones) => x.semitones = v.round(),
        (EffectConfig::Pitch(x), Cents) => x.cents = v,
        (EffectConfig::Pitch(x), Mix) => x.mix = v,
        (EffectConfig::Pitch(x), Window) => x.window_ms = v,
        (EffectConfig::Harmonizer(x), Voice1Semi) => x.voice1_semitones = v.round(),
        (EffectConfig::Harmonizer(x), Voice1Db) => x.voice1_db = v,
        (EffectConfig::Harmonizer(x), Voice2Semi) => x.voice2_semitones = v.round(),
        (EffectConfig::Harmonizer(x), Voice2Db) => x.voice2_db = v,
        (EffectConfig::Harmonizer(x), DryDb) => x.dry_db = v,
        (EffectConfig::Autotune(x), Key) => x.key = v.round().clamp(0.0, 11.0) as u8,
        (EffectConfig::Autotune(x), ScaleKind) => {
            x.scale = match v.round() as i32 {
                1 => engine::dsp::Scale::Major,
                2 => engine::dsp::Scale::Minor,
                _ => engine::dsp::Scale::Chromatic,
            }
        }
        (EffectConfig::Autotune(x), Speed) => x.speed_ms = v,
        (EffectConfig::Autotune(x), Mix) => x.mix = v,
        (EffectConfig::Reverb(x), Room) => x.room_size = v,
        (EffectConfig::Reverb(x), Damping) => x.damping = v,
        (EffectConfig::Reverb(x), Predelay) => x.predelay_ms = v,
        (EffectConfig::Reverb(x), Mix) => x.mix = v,
        (EffectConfig::Delay(x), Time) => x.time_ms = v,
        (EffectConfig::Delay(x), Feedback) => x.feedback = v,
        (EffectConfig::Delay(x), Mix) => x.mix = v,
        (EffectConfig::Delay(x), HighCut) => x.high_cut_hz = v,
        (EffectConfig::Delay(x), LowCut) => x.low_cut_hz = v,
        (EffectConfig::Crusher(x), Bits) => x.bits = v,
        (EffectConfig::Crusher(x), Downsample) => x.downsample = v.round(),
        (EffectConfig::Crusher(x), Mix) => x.mix = v,
        (EffectConfig::Crusher(x), Gain) => x.gain_db = v,
        (EffectConfig::Maximizer(m), Threshold) => m.threshold_db = v,
        (EffectConfig::Maximizer(m), Ceiling) => m.ceiling_db = v,
        (EffectConfig::Maximizer(m), Release) => m.release_ms = v,
        _ => {}
    }
}

pub fn format(unit: Unit, v: f32) -> String {
    match unit {
        Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
        Unit::Hz => format!("{v:.0} Hz"),
        Unit::Db => format!("{v:+.1} dB").replace('-', "−"),
        Unit::DbAmount => format!("{v:.1} dB"),
        Unit::Ms if v < 10.0 => format!("{v:.1} ms"),
        Unit::Ms => format!("{v:.0} ms"),
        Unit::Ratio => format!("{v:.1}:1"),
        Unit::Slope => format!("{} dB/oct", (v.round() as i32) * 12),
        Unit::Lufs => format!("{v:.1} LUFS").replace('-', "−"),
        Unit::Seconds => format!("{v:.1} s"),
        Unit::DeessMode => if v >= 0.5 { "Whole signal".into() } else { "Highs only".into() },
        Unit::Percent => format!("{:.0} %", v * 100.0),
        Unit::Semitones => {
            let s = v.round() as i32;
            match s {
                0 => "0 st".into(),
                12 | -12 => format!("{} octave", if s > 0 { "+1" } else { "−1" }),
                _ => format!("{s:+} st").replace('-', "−"),
            }
        }
        Unit::Note => NOTE_NAMES[(v.round() as usize).min(11)].into(),
        Unit::ScaleName => ["Chromatic", "Major", "Minor"][(v.round() as usize).min(2)].into(),
        Unit::Times => format!("{v:.0}×"),
        Unit::Cents => format!("{v:+.0} ¢").replace('-', "−"),
        Unit::None => format!("{v:.2}"),
    }
}

/// Is this effect a dynamics processor (has a gain-reduction meter)?
pub fn has_gr(cfg: &EffectConfig) -> bool {
    matches!(
        cfg,
        EffectConfig::Gate(_)
            | EffectConfig::Compressor(_)
            | EffectConfig::Limiter(_)
            | EffectConfig::Deesser(_)
            | EffectConfig::Expander(_)
            | EffectConfig::Maximizer(_)
    )
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
        "High-pass" => EffectConfig::Filter(FilterParams { kind: FilterKind::HighPass, freq_hz: 80.0, q: 0.707, slope: 1 }),
        "Low-pass" => EffectConfig::Filter(FilterParams { kind: FilterKind::LowPass, freq_hz: 12_000.0, q: 0.707, slope: 1 }),
        "Equalizer" => EffectConfig::Eq(EqParams {
            bands: vec![
                EqBand { enabled: true, kind: FilterKind::LowShelf, freq_hz: 120.0, gain_db: 0.0, q: 0.707, slope: 1 },
                EqBand { enabled: true, kind: FilterKind::Peak, freq_hz: 500.0, gain_db: 0.0, q: 1.0, slope: 1 },
                EqBand { enabled: true, kind: FilterKind::Peak, freq_hz: 3000.0, gain_db: 0.0, q: 1.0, slope: 1 },
                EqBand { enabled: true, kind: FilterKind::HighShelf, freq_hz: 8000.0, gain_db: 0.0, q: 0.707, slope: 1 },
            ],
            gain_db: 0.0,
        }),
        "Gate" => EffectConfig::Gate(GateParams::default()),
        "Compressor" => EffectConfig::Compressor(CompressorParams::default()),
        "Gain" => EffectConfig::Gain(GainParams::default()),
        "Limiter" => EffectConfig::Limiter(LimiterParams::default()),
        "De-esser" => EffectConfig::Deesser(DeesserParams::default()),
        "Expander" => EffectConfig::Expander(ExpanderParams::default()),
        "Auto-gain" => EffectConfig::Autogain(AutogainParams::default()),
        "Maximizer" => EffectConfig::Maximizer(MaximizerParams::default()),
        "Exciter" => EffectConfig::Exciter(ExciterParams::default()),
        "Bass enhancer" => EffectConfig::BassEnhancer(BassEnhancerParams::default()),
        "Pitch shift" => EffectConfig::Pitch(PitchParams::default()),
        "Harmonizer" => EffectConfig::Harmonizer(HarmonizerParams::default()),
        "Autotune" => EffectConfig::Autotune(AutotuneParams::default()),
        "Reverb" => EffectConfig::Reverb(ReverbParams::default()),
        "Delay" => EffectConfig::Delay(DelayParams::default()),
        "Crusher" => EffectConfig::Crusher(CrusherParams::default()),
        "Convolver" => EffectConfig::Convolver(ConvolverParams { ir_path: String::new(), mix: 1.0, gain_db: 0.0 }),
        _ => return None,
    })
}

pub const ADDABLE: &[&str] = &[
    "High-pass",
    "Low-pass",
    "Equalizer",
    "Gate",
    "Expander",
    "Compressor",
    "De-esser",
    "Exciter",
    "Bass enhancer",
    "Convolver",
    "Auto-gain",
    "Gain",
    "Maximizer",
    "Limiter",
    "Pitch shift",
    "Harmonizer",
    "Autotune",
    "Reverb",
    "Delay",
    "Crusher",
];

/// "A3"-style name of the nearest note to `hz`.
pub fn note_name(hz: f32) -> String {
    if hz <= 0.0 {
        return "—".into();
    }
    let midi = (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32;
    format!("{}{}", NOTE_NAMES[midi.rem_euclid(12) as usize], midi / 12 - 1)
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];

/// One-line card summary, e.g. "−28 dB · 3:1 · 5 / 120 ms".
pub fn summary(cfg: &EffectConfig) -> String {
    let db = |v: f32| format!("{v:.1} dB").replace('-', "−");
    let s = match cfg {
        EffectConfig::Filter(f) => format!("{} · {} · Q {:.2}", format(Unit::Hz, f.freq_hz), format(Unit::Slope, f.slope as f32), f.q),
        EffectConfig::Eq(e) => {
            let on = e.bands.iter().filter(|b| b.enabled).count();
            let g = if e.gain_db.abs() >= 0.05 { format!(" · {} out", format(Unit::Db, e.gain_db)) } else { String::new() };
            format!("{on} band{}{g}", if on == 1 { "" } else { "s" })
        }
        EffectConfig::Gate(g) => format!("{} threshold · {:.0} dB range", db(g.threshold_db), g.range_db),
        EffectConfig::Compressor(c) => format!(
            "{} · {:.1}:1 · {} / {} ms",
            db(c.threshold_db),
            c.ratio,
            trim(c.attack_ms),
            trim(c.release_ms)
        ),
        EffectConfig::Gain(g) => format(Unit::Db, g.gain_db),
        EffectConfig::Limiter(l) => format!("Ceiling {} · {} ms look-ahead", db(l.ceiling_db), trim(l.lookahead_ms)),
        EffectConfig::Deesser(d) => format!(
            "Above {} · {} · {:.1}:1",
            format(Unit::Hz, d.freq_hz),
            db(d.threshold_db),
            d.ratio
        ),
        EffectConfig::Expander(e) => format!("{} · {:.1}:1 · up to {:.0} dB", db(e.threshold_db), e.ratio, e.range_db),
        EffectConfig::Autogain(a) => format!("Target {} · ±{:.0} dB", format(Unit::Lufs, a.target_lufs), a.max_boost_db.max(a.max_cut_db)),
        EffectConfig::Maximizer(m) => format!("+{:.1} dB into {}", -m.threshold_db, db(m.ceiling_db)),
        EffectConfig::Pitch(x) => format!("{} · ~{:.0} ms delay", format(Unit::Semitones, x.semitones), x.window_ms / 2.0),
        EffectConfig::Harmonizer(x) => format!(
            "{} and {} voices",
            format(Unit::Semitones, x.voice1_semitones),
            format(Unit::Semitones, x.voice2_semitones)
        ),
        EffectConfig::Autotune(x) => format!(
            "{} {} · {:.0} ms retune",
            NOTE_NAMES[(x.key as usize).min(11)],
            ["chromatic", "major", "minor"][x.scale as usize],
            x.speed_ms
        ),
        EffectConfig::Reverb(x) => format!("{:.0} % room · {:.0} % wet", x.room_size * 100.0, x.mix * 100.0),
        EffectConfig::Delay(x) => format!("{:.0} ms · {:.0} % feedback", x.time_ms, x.feedback * 100.0),
        EffectConfig::Crusher(x) => format!("{:.1} bits · {:.0}× downsample", x.bits, x.downsample),
        EffectConfig::Exciter(e) => format!("{} from {}", db(e.amount_db), format(Unit::Hz, e.freq_hz)),
        EffectConfig::BassEnhancer(b) => format!("{} below {}", db(b.amount_db), format(Unit::Hz, b.freq_hz)),
        EffectConfig::Convolver(c) => match std::path::Path::new(&c.ir_path).file_name() {
            Some(f) => format!("{} · {:.0} % wet", f.to_string_lossy(), c.mix * 100.0),
            None => "No impulse response chosen".into(),
        },
    };
    s.replace("+-", "−")
}

fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") }
}

/// Icon asset for the effect's card.
pub fn icon(cfg: &EffectConfig) -> &'static str {
    match cfg {
        EffectConfig::Filter(f) if f.kind == FilterKind::LowPass => "winfx/lowpass.svg",
        EffectConfig::Filter(_) => "winfx/highpass.svg",
        EffectConfig::Eq(_) => "winfx/eq.svg",
        EffectConfig::Gate(_) => "winfx/gate.svg",
        EffectConfig::Compressor(_) => "winfx/compressor.svg",
        EffectConfig::Gain(_) => "winfx/gain.svg",
        EffectConfig::Limiter(_) => "winfx/limiter.svg",
        EffectConfig::Deesser(_) => "winfx/deesser.svg",
        EffectConfig::Expander(_) => "winfx/expander.svg",
        EffectConfig::Autogain(_) => "winfx/autogain.svg",
        EffectConfig::Maximizer(_) => "winfx/maximizer.svg",
        EffectConfig::Exciter(_) => "winfx/exciter.svg",
        EffectConfig::BassEnhancer(_) => "winfx/bass.svg",
        EffectConfig::Convolver(_) => "winfx/convolver.svg",
        EffectConfig::Pitch(_) => "winfx/pitch.svg",
        EffectConfig::Harmonizer(_) => "winfx/harmonizer.svg",
        EffectConfig::Autotune(_) => "winfx/autotune.svg",
        EffectConfig::Reverb(_) => "winfx/reverb.svg",
        EffectConfig::Delay(_) => "winfx/delay.svg",
        EffectConfig::Crusher(_) => "winfx/crusher.svg",
    }
}
