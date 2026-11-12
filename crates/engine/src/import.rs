//! Importers for other apps' presets:
//!
//! - **EasyEffects** preset JSON (`~/.config/easyeffects/input/*.json` or
//!   `~/.local/share/easyeffects/input/*.json`): the whole chain. Plugins we have
//!   an equivalent for are mapped; the rest are listed in the report.
//! - **Equalizer APO / AutoEQ** parametric text (`Preamp:` + `Filter n: ON PK Fc … Hz Gain … dB Q …`):
//!   becomes an EQ.
//!
//! EasyEffects' EQ is LSP's parametric EQ. Its "APO (DR)" band mode is the RBJ
//! cookbook biquad we use, so those bands import exactly; LSP's other band modes
//! (RLC/BWC/LRX) are approximated by the RBJ filter of the same type, frequency,
//! gain and Q.

use crate::chain::{ChainConfig, EffectConfig, EffectSlot, EqBand, EqParams, FilterParams, MAX_SLOPE};
use crate::dsp::{
    AutogainParams, AutotuneParams, BassEnhancerParams, CompressorParams, CrusherParams, DeesserMode, DeesserParams,
    ExciterParams, ExpanderParams, FilterKind, GainParams, GateParams, HarmonizerParams, LimiterParams,
    MaximizerParams, PitchParams, ReverbParams, Scale,
};
use serde_json::Value;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportReport {
    /// What was imported, e.g. "Equalizer (12 bands)".
    pub mapped: Vec<String>,
    /// Imported, but not an exact match.
    pub approximated: Vec<String>,
    /// Plugins/settings with no equivalent here.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Imported {
    /// A whole chain (EasyEffects preset).
    Chain(ChainConfig, ImportReport),
    /// An EQ only (APO / AutoEQ file).
    Eq(EqParams, ImportReport),
}

/// Detect the format from the content and import it.
pub fn import(text: &str) -> Result<Imported, String> {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    // A WinFX preset (e.g. shared from another machine) loads as-is.
    if t.starts_with('{')
        && let Ok(chain) = serde_json::from_str::<ChainConfig>(t)
    {
        let report = ImportReport {
            mapped: vec![format!("{} effects (WinFX preset)", chain.slots.len())],
            ..Default::default()
        };
        return Ok(Imported::Chain(chain, report));
    }
    if t.starts_with('{') {
        let (c, r) = from_easyeffects(t)?;
        Ok(Imported::Chain(c, r))
    } else {
        let (e, r) = from_apo(t)?;
        Ok(Imported::Eq(e, r))
    }
}

// ---------------------------------------------------------------- EasyEffects

fn num(v: &Value, key: &str, default: f32) -> f32 {
    v.get(key).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(default)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// "x1".."x16" → stages.
fn slope(label: &str) -> u8 {
    label.trim_start_matches('x').parse::<u8>().unwrap_or(1).clamp(1, MAX_SLOPE)
}

fn q_or_default(q: f32) -> f32 {
    if q < 0.05 { std::f32::consts::FRAC_1_SQRT_2 } else { q.min(100.0) }
}

/// LSP filter type label → (kind, enabled, exact?).
fn lsp_kind(label: &str) -> Option<(FilterKind, bool, bool)> {
    Some(match label {
        "Off" => (FilterKind::Peak, false, true),
        "Bell" => (FilterKind::Peak, true, true),
        "Hi-pass" | "High-pass" => (FilterKind::HighPass, true, true),
        "Lo-pass" | "Low-pass" => (FilterKind::LowPass, true, true),
        "Hi-shelf" | "High-shelf" => (FilterKind::HighShelf, true, true),
        "Lo-shelf" | "Low-shelf" => (FilterKind::LowShelf, true, true),
        "Notch" => (FilterKind::Notch, true, true),
        "Bandpass" | "Band-pass" => (FilterKind::BandPass, true, true),
        "Allpass" | "All-pass" => (FilterKind::AllPass, true, true),
        // No direct equivalent: closest bell.
        "Resonance" | "Ladder-pass" | "Ladder-rej" | "Ladder-rejection" => (FilterKind::Peak, true, false),
        _ => return None,
    })
}

struct Builder {
    slots: Vec<EffectSlot>,
    report: ImportReport,
}

impl Builder {
    fn push(&mut self, effect: EffectConfig, bypass: bool) {
        let id = self.slots.len() as u32 + 1;
        self.slots.push(EffectSlot { id, bypass, effect });
    }

    fn gain(&mut self, db: f32, bypass: bool, what: &str) {
        if db.abs() >= 0.05 {
            self.push(EffectConfig::Gain(GainParams { gain_db: db }), bypass);
            self.report.mapped.push(format!("{what} ({db:+.1} dB)"));
        }
    }

    /// LSP's dry/wet are dB levels; anything but "wet only" is a parallel mix we don't do.
    fn check_mix(&mut self, p: &Value, name: &str) {
        if p.get("dry").is_some() && num(p, "dry", -80.0) > -60.0 {
            self.report.skipped.push(format!("{name}: parallel dry/wet mix (wet only imported)"));
        }
    }
}

pub fn from_easyeffects(json: &str) -> Result<(ChainConfig, ImportReport), String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("not valid JSON: {e}"))?;
    let mut b = Builder { slots: vec![], report: ImportReport::default() };

    let section = match (root.get("input"), root.get("output")) {
        (Some(s), _) => s,
        (None, Some(s)) => {
            b.report.approximated.push("This is an output (speaker) preset; imported as a mic chain".into());
            s
        }
        _ => return Err("not an EasyEffects preset (no \"input\" or \"output\" section)".into()),
    };
    let order: Vec<&str> = section
        .get("plugins_order")
        .and_then(Value::as_array)
        .ok_or("not an EasyEffects preset (no plugins_order)")?
        .iter()
        .filter_map(Value::as_str)
        .collect();

    for name in order {
        let Some(p) = section.get(name) else { continue };
        let kind = name.split('#').next().unwrap_or(name);
        let bypass = flag(p, "bypass");
        let in_gain = num(p, "input-gain", 0.0);
        let out_gain = num(p, "output-gain", 0.0);
        let pretty = pretty_name(kind);

        match kind {
            "equalizer" => {
                let eq = ee_equalizer(p, &mut b.report);
                b.report.mapped.push(format!("Equalizer ({} bands)", eq.bands.len()));
                b.push(EffectConfig::Eq(EqParams { gain_db: eq.gain_db + in_gain + out_gain, ..eq }), bypass);
            }
            "filter" => {
                b.gain(in_gain, bypass, "Filter input gain");
                ee_filter(p, &mut b, bypass);
                b.gain(out_gain, bypass, "Filter output gain");
            }
            "gate" => {
                b.check_mix(p, "Gate");
                b.gain(in_gain, bypass, "Gate input gain");
                let reduction = num(p, "reduction", -24.0);
                if reduction > 0.0 {
                    b.report.skipped.push("Gate with positive reduction (upward expansion)".into());
                } else {
                    let thr = num(p, "curve-threshold", -24.0);
                    let hyst = if flag(p, "hysteresis") {
                        num(p, "hysteresis-threshold", -12.0).abs()
                    } else {
                        num(p, "curve-zone", -6.0).abs()
                    };
                    let sc_hpf = if text(p, "hpf-mode") == "Off" { 0.0 } else { num(p, "hpf-frequency", 0.0) };
                    b.push(
                        EffectConfig::Gate(GateParams {
                            threshold_db: thr,
                            hysteresis_db: hyst,
                            attack_ms: num(p, "attack", 20.0),
                            hold_ms: 0.0,
                            release_ms: num(p, "release", 100.0),
                            range_db: -reduction,
                            sidechain_hpf_hz: sc_hpf,
                        }),
                        bypass,
                    );
                    b.report.mapped.push("Gate".into());
                }
                let wet = if p.get("wet").is_some() { num(p, "wet", 0.0) } else { 0.0 };
                b.gain(num(p, "makeup", 0.0) + wet + out_gain, bypass, "Gate makeup");
            }
            "compressor" => {
                b.check_mix(p, "Compressor");
                let mode = text(p, "mode");
                if !mode.is_empty() && mode != "Downward" {
                    b.report.skipped.push(format!("Compressor in {mode} mode"));
                    continue;
                }
                b.gain(in_gain, bypass, "Compressor input gain");
                let sc = p.get("sidechain").cloned().unwrap_or(Value::Null);
                let rms_ms = match text(&sc, "mode") {
                    "Peak" => 0.1,
                    _ => num(&sc, "reactivity", 10.0).max(0.1),
                };
                let wet = if p.get("wet").is_some() { num(p, "wet", 0.0) } else { 0.0 };
                b.push(
                    EffectConfig::Compressor(CompressorParams {
                        threshold_db: num(p, "threshold", -12.0),
                        ratio: num(p, "ratio", 4.0).max(1.0),
                        attack_ms: num(p, "attack", 20.0),
                        release_ms: num(p, "release", 100.0),
                        // LSP's knee is the half-width below/above threshold.
                        knee_db: num(p, "knee", -6.0).abs() * 2.0,
                        rms_ms,
                        makeup_db: num(p, "makeup", 0.0) + wet,
                    }),
                    bypass,
                );
                b.report.mapped.push("Compressor".into());
                b.gain(out_gain, bypass, "Compressor output gain");
            }
            "limiter" => {
                b.gain(in_gain, bypass, "Limiter input gain");
                b.push(
                    EffectConfig::Limiter(LimiterParams {
                        ceiling_db: num(p, "threshold", 0.0).min(0.0),
                        release_ms: num(p, "release", 5.0).clamp(1.0, 500.0),
                        lookahead_ms: num(p, "lookahead", 5.0).clamp(0.5, 10.0),
                    }),
                    bypass,
                );
                b.report.mapped.push("Limiter".into());
                b.report.approximated.push("Limiter: LSP's limiter modes and automatic level control".into());
                b.gain(out_gain, bypass, "Limiter output gain");
            }
            "deesser" => {
                b.gain(in_gain, bypass, "De-esser input gain");
                let mode = if text(p, "mode") == "Wide" { DeesserMode::Wide } else { DeesserMode::Split };
                b.push(
                    EffectConfig::Deesser(DeesserParams {
                        freq_hz: num(p, "f1-freq", 6000.0).clamp(1000.0, 16_000.0),
                        threshold_db: num(p, "threshold", -18.0),
                        ratio: num(p, "ratio", 3.0).max(1.0),
                        range_db: 12.0,
                        release_ms: 60.0,
                        mode,
                    }),
                    bypass,
                );
                b.report.mapped.push("De-esser".into());
                b.report.approximated.push("De-esser: LSP's peak sidechain filter and laxity".into());
                b.gain(num(p, "makeup", 0.0) + out_gain, bypass, "De-esser makeup");
            }
            "expander" => {
                b.check_mix(p, "Expander");
                let mode = text(p, "mode");
                if !mode.is_empty() && mode != "Downward" {
                    b.report.skipped.push(format!("Expander in {mode} mode"));
                    continue;
                }
                b.gain(in_gain, bypass, "Expander input gain");
                let sc = p.get("sidechain").cloned().unwrap_or(Value::Null);
                b.push(
                    EffectConfig::Expander(ExpanderParams {
                        threshold_db: num(p, "threshold", -40.0),
                        ratio: num(p, "ratio", 2.0).max(1.0),
                        range_db: 40.0,
                        attack_ms: num(p, "attack", 20.0),
                        release_ms: num(p, "release", 100.0),
                        knee_db: num(p, "knee", -6.0).abs() * 2.0,
                        rms_ms: if text(&sc, "mode") == "Peak" { 0.1 } else { num(&sc, "reactivity", 10.0).max(0.1) },
                    }),
                    bypass,
                );
                b.report.mapped.push("Expander".into());
                b.gain(num(p, "makeup", 0.0) + out_gain, bypass, "Expander makeup");
            }
            "autogain" => {
                b.gain(in_gain, bypass, "Auto-gain input gain");
                b.push(
                    EffectConfig::Autogain(AutogainParams {
                        target_lufs: num(p, "target", -18.0),
                        silence_lufs: num(p, "silence-threshold", -45.0),
                        ..AutogainParams::default()
                    }),
                    bypass,
                );
                b.report.mapped.push("Auto-gain".into());
                b.report.approximated.push("Auto-gain: follows momentary loudness".into());
                b.gain(out_gain, bypass, "Auto-gain output gain");
            }
            "maximizer" => {
                b.gain(in_gain, bypass, "Maximizer input gain");
                b.push(
                    EffectConfig::Maximizer(MaximizerParams {
                        threshold_db: num(p, "threshold", -6.0).min(0.0),
                        ceiling_db: -0.5,
                        release_ms: num(p, "release", 80.0).clamp(1.0, 1000.0),
                    }),
                    bypass,
                );
                b.report.mapped.push("Maximizer".into());
                b.gain(out_gain, bypass, "Maximizer output gain");
            }
            "exciter" => {
                b.gain(in_gain, bypass, "Exciter input gain");
                b.push(
                    EffectConfig::Exciter(ExciterParams {
                        amount_db: num(p, "amount", 0.0).clamp(-60.0, 12.0),
                        drive: num(p, "harmonics", 8.5).clamp(0.5, 10.0),
                        freq_hz: num(p, "scope", 7500.0),
                        ceiling_hz: if flag(p, "ceil-active") { num(p, "ceil", 16_000.0) } else { 20_000.0 },
                    }),
                    bypass,
                );
                b.report.mapped.push("Exciter".into());
                b.report.approximated.push("Exciter: Calf's harmonic blend".into());
                b.gain(out_gain, bypass, "Exciter output gain");
            }
            "bass_enhancer" => {
                b.gain(in_gain, bypass, "Bass enhancer input gain");
                b.push(
                    EffectConfig::BassEnhancer(BassEnhancerParams {
                        amount_db: num(p, "amount", 0.0).clamp(-60.0, 12.0),
                        drive: num(p, "harmonics", 8.5).clamp(0.5, 10.0),
                        freq_hz: num(p, "scope", 100.0),
                        floor_hz: if flag(p, "floor-active") { num(p, "floor", 20.0) } else { 10.0 },
                    }),
                    bypass,
                );
                b.report.mapped.push("Bass enhancer".into());
                b.report.approximated.push("Bass enhancer: Calf's harmonic blend".into());
                b.gain(out_gain, bypass, "Bass enhancer output gain");
            }
            "pitch" => {
                let (wet, dry) =
                    (crate::dsp::db_to_lin(num(p, "wet", 0.0)), crate::dsp::db_to_lin(num(p, "dry", -100.0)));
                b.push(
                    EffectConfig::Pitch(PitchParams {
                        semitones: (num(p, "semitones", 0.0) + 12.0 * num(p, "octaves", 0.0)).clamp(-24.0, 24.0),
                        cents: num(p, "cents", 0.0),
                        mix: (wet / (wet + dry).max(1e-6)).clamp(0.0, 1.0),
                        window_ms: 40.0,
                    }),
                    bypass,
                );
                b.report.mapped.push("Pitch shift".into());
                b.gain(in_gain + out_gain, bypass, "Pitch shift gain");
            }
            "autotune" => {
                const NOTES: [&str; 12] = [
                    "note-c",
                    "note-c-sharp",
                    "note-d",
                    "note-d-sharp",
                    "note-e",
                    "note-f",
                    "note-f-sharp",
                    "note-g",
                    "note-g-sharp",
                    "note-a",
                    "note-a-sharp",
                    "note-b",
                ];
                let on: Vec<bool> = NOTES.iter().map(|k| p.get(*k).and_then(Value::as_bool).unwrap_or(true)).collect();
                const MAJOR: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
                const MINOR: [usize; 7] = [0, 2, 3, 5, 7, 8, 10];
                let fits = |steps: &[usize; 7], key: usize| {
                    (0..12).all(|n| on[n] == steps.iter().any(|s| (s + key) % 12 == n))
                };
                let (key, scale) = if on.iter().all(|x| *x) {
                    (0, Scale::Chromatic)
                } else if let Some(k) = (0..12).find(|&k| fits(&MAJOR, k)) {
                    (k, Scale::Major)
                } else if let Some(k) = (0..12).find(|&k| fits(&MINOR, k)) {
                    (k, Scale::Minor)
                } else {
                    b.report.approximated.push("Autotune: custom note set imported as chromatic".into());
                    (0, Scale::Chromatic)
                };
                b.push(
                    EffectConfig::Autotune(AutotuneParams {
                        key: key as u8,
                        scale,
                        speed_ms: (num(p, "filter", 0.1) * 300.0).clamp(0.0, 200.0),
                        mix: num(p, "correction", 1.0).clamp(0.0, 1.0),
                    }),
                    bypass,
                );
                b.report.mapped.push("Autotune".into());
                b.gain(in_gain + out_gain, bypass, "Autotune gain");
            }
            "harmonizer" => {
                let wet = (num(p, "dry-wet", 63.0) / 127.0).clamp(0.0, 1.0);
                let voice = crate::dsp::lin_to_db(wet.max(1e-4));
                b.push(
                    EffectConfig::Harmonizer(HarmonizerParams {
                        voice1_semitones: num(p, "interval-left", 4.0),
                        voice1_db: voice - 3.0,
                        voice2_semitones: num(p, "interval-right", 7.0),
                        voice2_db: voice - 3.0,
                        dry_db: crate::dsp::lin_to_db((1.0 - wet).max(1e-4)),
                    }),
                    bypass,
                );
                b.report.mapped.push("Harmonizer".into());
                b.report.approximated.push("Harmonizer: chord modes and per-voice gains".into());
                b.gain(in_gain + out_gain, bypass, "Harmonizer gain");
            }
            "reverb" => {
                let decay = num(p, "decay-time", 1.5).clamp(0.4, 15.0);
                let damp = num(p, "hf-damp", 5000.0).clamp(2000.0, 20_000.0);
                b.push(
                    EffectConfig::Reverb(ReverbParams {
                        room_size: ((decay / 0.4).ln() / (15.0f32 / 0.4).ln()).clamp(0.0, 1.0),
                        damping: 1.0 - (damp - 2000.0) / 18_000.0,
                        predelay_ms: num(p, "predelay", 0.0).clamp(0.0, 200.0),
                        mix: crate::dsp::db_to_lin(num(p, "amount", -12.0)).clamp(0.0, 1.0),
                    }),
                    bypass,
                );
                b.report.mapped.push("Reverb".into());
                b.report.approximated.push("Reverb: Calf's room types and bass/treble cuts".into());
                b.gain(in_gain + out_gain, bypass, "Reverb gain");
            }
            "crusher" => {
                b.push(
                    EffectConfig::Crusher(CrusherParams {
                        bits: num(p, "bit-reduction", 4.0).clamp(1.0, 16.0),
                        downsample: num(p, "sample-reduction", 1.0).clamp(1.0, 64.0),
                        mix: 1.0,
                        gain_db: in_gain + out_gain,
                    }),
                    bypass,
                );
                b.report.mapped.push("Crusher".into());
            }
            "delay" => b.report.skipped.push("Delay (a stereo alignment delay, not an echo)".into()),
            _ => b.report.skipped.push(pretty),
        }
    }

    if b.slots.is_empty() {
        return Err(format!(
            "nothing in this preset can be imported (only {} found)",
            if b.report.skipped.is_empty() { "an empty chain".into() } else { b.report.skipped.join(", ") }
        ));
    }
    dedup(&mut b.report.approximated);
    Ok((ChainConfig { slots: b.slots }, b.report))
}

fn ee_equalizer(p: &Value, report: &mut ImportReport) -> EqParams {
    let n = p.get("num-bands").and_then(Value::as_u64).unwrap_or(32) as usize;
    let left = p.get("left").cloned().unwrap_or(Value::Null);
    if flag(p, "split-channels") && p.get("right") != p.get("left") {
        report.approximated.push("Equalizer: separate left/right settings (used the left channel)".into());
    }
    let mode = text(p, "mode");
    if !mode.is_empty() && mode != "IIR" {
        report.approximated.push(format!("Equalizer: {mode} (linear-phase) mode imported as IIR"));
    }
    let pitch = num(p, "pitch-left", 0.0);
    let shift = 2f32.powf(pitch / 12.0);

    let mut bands = vec![];
    for i in 0..n {
        let Some(bv) = left.get(format!("band{i}")) else { continue };
        let type_label = text(bv, "type");
        let Some((kind, mut enabled, exact)) = lsp_kind(if type_label.is_empty() { "Bell" } else { type_label }) else {
            report.approximated.push(format!("Equalizer: unknown band type \"{type_label}\" turned off"));
            continue;
        };
        if flag(bv, "mute") {
            enabled = false;
        }
        if !exact {
            report.approximated.push(format!("Equalizer: {type_label} bands as bells"));
        }
        let band_mode = text(bv, "mode");
        if enabled && !band_mode.is_empty() && band_mode != "APO (DR)" && kind != FilterKind::Peak {
            report.approximated.push(format!("Equalizer: {band_mode} filter shapes (closest standard biquad used)"));
        }
        bands.push(EqBand {
            enabled,
            kind,
            freq_hz: (num(bv, "frequency", 1000.0) * shift).clamp(10.0, 24_000.0),
            gain_db: num(bv, "gain", 0.0).clamp(-36.0, 36.0),
            q: q_or_default(num(bv, "q", 0.707)),
            slope: slope(text(bv, "slope")),
        });
    }
    EqParams { bands, gain_db: 0.0 }
}

fn ee_filter(p: &Value, b: &mut Builder, bypass: bool) {
    // EasyEffects 6 and older: "mode": "12dB/oct Highpass", "resonance" in dB.
    if p.get("type").is_none() {
        let mode = text(p, "mode");
        let lower = mode.to_ascii_lowercase();
        let kind = if lower.contains("highpass") {
            FilterKind::HighPass
        } else if lower.contains("lowpass") {
            FilterKind::LowPass
        } else {
            b.report.skipped.push(format!("Filter of type \"{mode}\""));
            return;
        };
        let db_per_oct: f32 = lower.split("db").next().and_then(|n| n.trim().parse().ok()).unwrap_or(12.0);
        let stages = ((db_per_oct / 12.0).round() as u8).clamp(1, MAX_SLOPE);
        // −3 dB resonance is a Butterworth (Q 0.707); every dB above raises Q accordingly.
        let q =
            (std::f32::consts::FRAC_1_SQRT_2 * crate::dsp::db_to_lin(num(p, "resonance", -3.0) + 3.0)).clamp(0.3, 10.0);
        let freq = num(p, "frequency", 80.0).clamp(10.0, 24_000.0);
        b.push(EffectConfig::Filter(FilterParams { kind, freq_hz: freq, q, slope: stages }), bypass);
        b.report.mapped.push(format!("Filter ({mode})"));
        return;
    }
    let label = text(p, "type");
    let Some((kind, _, exact)) = lsp_kind(label) else {
        b.report.skipped.push(format!("Filter of type \"{label}\""));
        return;
    };
    let freq = num(p, "frequency", 2000.0).clamp(10.0, 24_000.0);
    let q = q_or_default(num(p, "quality", 0.0));
    let stages = slope(text(p, "slope"));
    if !exact {
        b.report.approximated.push(format!("Filter: {label} as a bell"));
    }
    if matches!(kind, FilterKind::HighPass | FilterKind::LowPass) {
        b.push(EffectConfig::Filter(FilterParams { kind, freq_hz: freq, q, slope: stages }), bypass);
        b.report.mapped.push(format!("Filter ({label})"));
    } else {
        let band = EqBand { enabled: true, kind, freq_hz: freq, gain_db: num(p, "gain", 0.0), q, slope: stages };
        b.push(EffectConfig::Eq(EqParams { bands: vec![band], gain_db: 0.0 }), bypass);
        b.report.mapped.push(format!("Filter ({label}) as a 1-band EQ"));
    }
}

fn pretty_name(kind: &str) -> String {
    match kind {
        "rnnoise" => "Noise reduction (RNNoise)".into(),
        "deepfilternet" => "Noise reduction (DeepFilterNet)".into(),
        "speex" => "Speex noise suppression".into(),
        "echo_canceller" => "Echo canceller".into(),
        "deesser" => "De-esser".into(),
        "autogain" => "Autogain".into(),
        "maximizer" => "Maximizer".into(),
        "multiband_compressor" => "Multiband compressor".into(),
        "multiband_gate" => "Multiband gate".into(),
        "bass_enhancer" => "Bass enhancer".into(),
        "stereo_tools" => "Stereo tools".into(),
        other => {
            let mut s = other.replace('_', " ");
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            s
        }
    }
}

fn dedup(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

// ------------------------------------------------------- Equalizer APO / AutoEQ

/// Find `key` (case-insensitive word) and parse the number after it.
fn after(tokens: &[String], key: &str) -> Option<f32> {
    let i = tokens.iter().position(|t| t.eq_ignore_ascii_case(key))?;
    tokens.get(i + 1)?.replace(',', "").parse::<f32>().ok()
}

pub fn from_apo(text: &str) -> Result<(EqParams, ImportReport), String> {
    let mut report = ImportReport::default();
    let mut bands = vec![];
    let mut preamp = 0.0f32;
    let mut saw_graphic = false;
    let mut saw_channel = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("preamp:") {
            let v: String = rest.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
            preamp += v.parse::<f32>().unwrap_or(0.0);
            continue;
        }
        if lower.starts_with("graphiceq:") {
            saw_graphic = true;
            continue;
        }
        if lower.starts_with("channel:") {
            saw_channel = true;
            continue;
        }
        if !lower.starts_with("filter") {
            continue;
        }
        let Some((_, spec)) = line.split_once(':') else { continue };
        let tokens: Vec<String> = spec.split_whitespace().map(str::to_owned).collect();
        let Some(state) = tokens.first() else { continue };
        let enabled = state.eq_ignore_ascii_case("on");
        let Some(code) = tokens.get(1).map(|t| t.to_ascii_uppercase()) else { continue };
        let freq = after(&tokens, "fc").unwrap_or(1000.0);
        let gain = after(&tokens, "gain").unwrap_or(0.0);
        let q = after(&tokens, "q").or_else(|| {
            // "BW Oct n" → Q
            let i = tokens.iter().position(|t| t.eq_ignore_ascii_case("bw"))?;
            let n = tokens.get(i + 2)?.parse::<f32>().ok()?;
            let p = 2f32.powf(n);
            Some(p.sqrt() / (p - 1.0))
        });
        let (kind, q) = match code.as_str() {
            "PK" | "PEQ" | "MODAL" => (FilterKind::Peak, q.unwrap_or(1.0)),
            "LP" => (FilterKind::LowPass, std::f32::consts::FRAC_1_SQRT_2),
            "HP" => (FilterKind::HighPass, std::f32::consts::FRAC_1_SQRT_2),
            "LPQ" => (FilterKind::LowPass, q.unwrap_or(0.707)),
            "HPQ" => (FilterKind::HighPass, q.unwrap_or(0.707)),
            "LS" | "LSC" => (FilterKind::LowShelf, q.unwrap_or(0.707)),
            "HS" | "HSC" => (FilterKind::HighShelf, q.unwrap_or(0.707)),
            "NO" => (FilterKind::Notch, q.unwrap_or(30.0)),
            "BP" => (FilterKind::BandPass, q.unwrap_or(0.707)),
            "AP" => (FilterKind::AllPass, q.unwrap_or(0.707)),
            other => {
                report.skipped.push(format!("APO filter type {other}"));
                continue;
            }
        };
        if matches!(tokens.get(2).map(|t| t.to_ascii_uppercase()).as_deref(), Some("6DB" | "12DB")) {
            report.approximated.push(format!("{code} with a fixed dB/oct slope"));
        }
        bands.push(EqBand {
            enabled,
            kind,
            freq_hz: freq.clamp(10.0, 24_000.0),
            gain_db: gain.clamp(-36.0, 36.0),
            q: q_or_default(q),
            slope: 1,
        });
    }

    if saw_channel {
        report.approximated.push("Per-channel sections were merged into one EQ".into());
    }
    if bands.is_empty() {
        return Err(if saw_graphic {
            "GraphicEQ files aren't supported. Use AutoEQ's ParametricEQ.txt instead".into()
        } else {
            "no \"Filter:\" lines found. This isn't an Equalizer APO or AutoEQ file".into()
        });
    }
    report.mapped.push(format!("Equalizer ({} bands)", bands.len()));
    if preamp != 0.0 {
        report.mapped.push(format!("Preamp ({preamp:+.1} dB)"));
    }
    dedup(&mut report.approximated);
    Ok((EqParams { bands, gain_db: preamp }, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EE: &str = r#"{
      "input": {
        "blocklist": [],
        "plugins_order": ["rnnoise#0", "gate#0", "equalizer#0", "compressor#0", "deesser#0", "limiter#0"],
        "rnnoise#0": { "bypass": false },
        "gate#0": {
          "attack": 5.0, "bypass": false, "curve-threshold": -50.0, "curve-zone": -2.0,
          "dry": -80.01, "hpf-frequency": 120.0, "hpf-mode": "12 dB/oct", "hysteresis": true,
          "hysteresis-threshold": -3.0, "input-gain": 0.0, "makeup": 1.0, "output-gain": 0.0,
          "reduction": -12.0, "release": 250.0, "wet": -1.0
        },
        "equalizer#0": {
          "bypass": false, "input-gain": -2.0, "output-gain": 0.0, "mode": "IIR", "num-bands": 3,
          "split-channels": false, "pitch-left": 0.0,
          "left": {
            "band0": { "frequency": 80.0, "gain": 0.0, "mode": "RLC (BT)", "mute": false, "q": 0.7, "slope": "x2", "type": "Hi-pass" },
            "band1": { "frequency": 220.0, "gain": -2.0, "mode": "APO (DR)", "mute": false, "q": 1.2, "slope": "x1", "type": "Bell" },
            "band2": { "frequency": 5000.0, "gain": 3.0, "mode": "APO (DR)", "mute": true, "q": 0.7, "slope": "x1", "type": "Hi-shelf" },
            "band3": { "frequency": 9000.0, "gain": 9.0, "mode": "APO (DR)", "mute": false, "q": 0.7, "slope": "x1", "type": "Bell" }
          },
          "right": {}
        },
        "compressor#0": {
          "attack": 15.0, "bypass": false, "dry": -80.01, "input-gain": 0.0, "knee": -6.0, "makeup": 3.0,
          "mode": "Downward", "output-gain": 0.0, "ratio": 3.0, "release": 200.0, "threshold": -18.0,
          "sidechain": { "mode": "RMS", "reactivity": 10.0 }, "wet": 0.0
        },
        "deesser#0": { "bypass": false },
        "limiter#0": { "bypass": true, "input-gain": 0.0, "lookahead": 2.0, "output-gain": 0.0, "release": 5.0, "threshold": -1.5 }
      }
    }"#;

    #[test]
    fn easyeffects_chain_maps_supported_plugins_in_order() {
        let (chain, report) = from_easyeffects(EE).unwrap();
        let kinds: Vec<&str> = chain.slots.iter().map(|s| s.effect.name()).collect();
        // Gate, gate makeup (+1 dB makeup −1 dB wet = 0 → omitted), EQ, compressor, de-esser, limiter.
        assert_eq!(kinds, ["Gate", "Equalizer", "Compressor", "De-esser", "Limiter"]);
        assert_eq!(report.skipped, ["Noise reduction (RNNoise)"]);

        let EffectConfig::Gate(g) = &chain.slots[0].effect else { panic!() };
        assert_eq!((g.threshold_db, g.hysteresis_db, g.range_db, g.sidechain_hpf_hz), (-50.0, 3.0, 12.0, 120.0));

        let EffectConfig::Eq(eq) = &chain.slots[1].effect else { panic!() };
        assert_eq!(eq.bands.len(), 3, "num-bands limits which bands are read");
        assert_eq!(eq.gain_db, -2.0);
        assert_eq!((eq.bands[0].kind, eq.bands[0].slope), (FilterKind::HighPass, 2));
        assert!(!eq.bands[2].enabled, "muted band is disabled");
        assert!(report.approximated.iter().any(|a| a.contains("RLC (BT)")));

        let EffectConfig::Compressor(c) = &chain.slots[2].effect else { panic!() };
        assert_eq!((c.threshold_db, c.ratio, c.knee_db, c.makeup_db, c.rms_ms), (-18.0, 3.0, 12.0, 3.0, 10.0));

        assert!(chain.slots[4].bypass, "bypass state carries over");
        let EffectConfig::Limiter(l) = &chain.slots[4].effect else { panic!() };
        assert_eq!(l.ceiling_db, -1.5);

        // Ids are unique and the chain round-trips as a WinFX preset.
        let back: ChainConfig = serde_json::from_str(&serde_json::to_string(&chain).unwrap()).unwrap();
        assert_eq!(back, chain);
    }

    #[test]
    fn winfx_presets_import_as_is() {
        let chain = ChainConfig::sm57_voice();
        let Imported::Chain(back, _) = import(&serde_json::to_string_pretty(&chain).unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(back, chain);
    }

    #[test]
    fn rejects_non_presets() {
        assert!(from_easyeffects("{\"foo\": 1}").is_err());
        assert!(import("{ broken").is_err());
        assert!(import("hello world").is_err());
    }

    #[test]
    fn apo_autoeq_parametric() {
        let txt = "# AutoEQ\nPreamp: -6.2 dB\n\
            Filter 1: ON LSC Fc 105 Hz Gain 5.6 dB Q 0.70\n\
            Filter 2: ON PK Fc 2,000 Hz Gain -3.4 dB Q 1.41\n\
            Filter 3: OFF PK Fc 4000 Hz Gain 2.0 dB Q 2.00\n\
            Filter 4: ON HSC Fc 10000 Hz Gain -2.1 dB Q 0.70\n\
            Filter 5: ON HP Fc 30 Hz\n\
            Filter 6: ON PK Fc 1000 Hz Gain 1 dB BW Oct 1.0\n";
        let Imported::Eq(eq, report) = import(txt).unwrap() else { panic!() };
        assert_eq!(eq.gain_db, -6.2);
        assert_eq!(eq.bands.len(), 6);
        assert_eq!((eq.bands[0].kind, eq.bands[0].gain_db), (FilterKind::LowShelf, 5.6));
        assert_eq!(eq.bands[1].freq_hz, 2000.0);
        assert!(!eq.bands[2].enabled);
        assert_eq!(eq.bands[4].kind, FilterKind::HighPass);
        assert!((eq.bands[5].q - 1.414).abs() < 0.01, "1 octave ≈ Q 1.41");
        assert!(report.mapped.iter().any(|m| m.contains("Preamp")));
    }

    #[test]
    fn graphic_eq_explains_itself() {
        let err = from_apo("GraphicEQ: 20 -1.2; 21 -1.3").unwrap_err();
        assert!(err.contains("ParametricEQ"));
    }
}
