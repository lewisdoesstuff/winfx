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
use crate::dsp::{CompressorParams, FilterKind, GainParams, GateParams, LimiterParams};
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
        report.approximated.push("Equalizer: separate left/right settings — left channel used".into());
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
            "GraphicEQ files aren't supported — use AutoEQ's ParametricEQ.txt instead".into()
        } else {
            "no \"Filter:\" lines found — not an Equalizer APO / AutoEQ file".into()
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
        // Gate, gate makeup (+1 dB makeup −1 dB wet = 0 → omitted), EQ, compressor, limiter.
        assert_eq!(kinds, ["Gate", "Equalizer", "Compressor", "Limiter"]);
        assert_eq!(report.skipped, ["Noise reduction (RNNoise)", "De-esser"]);

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

        assert!(chain.slots[3].bypass, "bypass state carries over");
        let EffectConfig::Limiter(l) = &chain.slots[3].effect else { panic!() };
        assert_eq!(l.ceiling_db, -1.5);

        // Ids are unique and the chain round-trips as a WinFX preset.
        let back: ChainConfig = serde_json::from_str(&serde_json::to_string(&chain).unwrap()).unwrap();
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
