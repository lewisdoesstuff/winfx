//! Asset source: our own effect icons, falling back to gpui-component's bundle.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

macro_rules! icon {
    ($body:literal) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

const ICONS: &[(&str, &str)] = &[
    ("winfx/highpass.svg", icon!(r#"<path d="M3 18c4 0 5-11 9-11h9"/>"#)),
    ("winfx/lowpass.svg", icon!(r#"<path d="M3 7h9c4 0 5 11 9 11"/>"#)),
    ("winfx/eq.svg", icon!(r#"<path d="M5 4v16M12 4v16M19 4v16M3 14h4M10 8h4M17 16h4"/>"#)),
    ("winfx/gate.svg", icon!(r#"<path d="M3 16h4V8h10v8h4"/>"#)),
    ("winfx/compressor.svg", icon!(r#"<path d="M4 4v16h16M4 20l8-8 8-4"/>"#)),
    ("winfx/gain.svg", icon!(r#"<path d="M3 18L21 6v12z"/>"#)),
    ("winfx/limiter.svg", icon!(r#"<path d="M3 6h18M3 16c2-6 4-6 6 0s4 6 6 0 3-4 6-4"/>"#)),
    // De-esser: an "s" with a cut line.
    (
        "winfx/deesser.svg",
        icon!(
            r#"<path d="M15 7.5c-.6-1.4-2-2.5-4-2.5-2.2 0-4 1.3-4 3s1.6 2.4 4 3 4 1.4 4 3.2-1.8 3-4 3c-2 0-3.5-1-4-2.6M4 20L20 4"/>"#
        ),
    ),
    // Expander: transfer curve bending down below a knee.
    ("winfx/expander.svg", icon!(r#"<path d="M4 4v16h16M8 20l4-6 8-8"/>"#)),
    // Auto-gain: a level that steers toward a target line.
    ("winfx/autogain.svg", icon!(r#"<path d="M3 12h3l3-6 3 12 3-9 2 3h4M3 4h18"/>"#)),
    // Maximizer: arrow pushing up against a ceiling.
    ("winfx/maximizer.svg", icon!(r#"<path d="M4 4h16M12 20V8M8 12l4-4 4 4"/>"#)),
    // Exciter: sparkle.
    (
        "winfx/exciter.svg",
        icon!(r#"<path d="M12 3v4M12 17v4M3 12h4M17 12h4M6 6l2.5 2.5M15.5 15.5L18 18M6 18l2.5-2.5M15.5 8.5L18 6"/>"#),
    ),
    // Bass enhancer: a low, wide wave.
    ("winfx/bass.svg", icon!(r#"<path d="M2 12c2.5-6 5.5-6 8 0s5.5 6 8 0 2.5-3 4-3"/>"#)),
    // Convolver: an impulse and its decaying response.
    ("winfx/convolver.svg", icon!(r#"<path d="M4 20V4M4 20h16M8 20v-9M11 20v-6M14 20v-4M17 20v-2"/>"#)),
    ("winfx/pitch.svg", icon!(r#"<path d="M4 17c3 0 3-10 6-10s3 10 6 10M17 4v6M14 7l3-3 3 3"/>"#)),
    (
        "winfx/harmonizer.svg",
        icon!(r#"<path d="M9 18V6l10-2v12M9 18a2 2 0 1 1-4 0 2 2 0 0 1 4 0zM19 16a2 2 0 1 1-4 0 2 2 0 0 1 4 0z"/>"#),
    ),
    ("winfx/autotune.svg", icon!(r#"<path d="M3 15c2-6 4-6 6 0s4 6 6 0M15 9h6M15 5h6M15 13h6"/>"#)),
    ("winfx/reverb.svg", icon!(r#"<path d="M4 20V4M8 18V8M12 16v-6M16 15v-4M20 14v-2"/>"#)),
    ("winfx/delay.svg", icon!(r#"<path d="M3 12h2M7 9v6M12 10v4M17 11v2M21 12h0"/>"#)),
    ("winfx/crusher.svg", icon!(r#"<path d="M3 16h3v-4h3V8h3v4h3v4h3v-6h3"/>"#)),
    ("winfx/plug.svg", icon!(r#"<path d="M9 3v5M15 3v5M7 8h10v4a5 5 0 0 1-10 0zM12 17v4"/>"#)),
    (
        "winfx/grip.svg",
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 16" fill="#000"><circle cx="3" cy="3" r="1.2"/><circle cx="7" cy="3" r="1.2"/><circle cx="3" cy="8" r="1.2"/><circle cx="7" cy="8" r="1.2"/><circle cx="3" cy="13" r="1.2"/><circle cx="7" cy="13" r="1.2"/></svg>"##,
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, svg)) = ICONS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(svg.as_bytes())));
        }
        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit_assets::Assets.list(path)
    }
}
