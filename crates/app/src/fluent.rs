//! Windows 11 Fluent look: colour tokens (light/dark, system accent), the
//! gpui-component theme overrides that make its widgets match, and a few
//! Fluent-style building blocks.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, px, rgb, rgba, App, Div, ElementId, Global, Hsla, InteractiveElement as _,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window,
    WindowAppearance,
};
use gpui_component::{Theme, ThemeMode};

pub const FONT: &str = "Segoe UI Variable Text";
pub const FONT_DISPLAY: &str = "Segoe UI Variable Display";
pub const ICON_FONT: &str = "Segoe Fluent Icons";

/// Segoe Fluent Icons code points.
pub mod g {
    pub const ADD: char = '\u{E710}';
    pub const CANCEL: char = '\u{E711}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const CHEVRON_UP: char = '\u{E70E}';
    pub const SAVE: char = '\u{E74E}';
    pub const MIC: char = '\u{E720}';
    pub const HEADPHONE: char = '\u{E7F6}';
    pub const INFO: char = '\u{E946}';
    pub const COMPLETED: char = '\u{E930}';
    pub const WARNING: char = '\u{E7BA}';
    pub const ERROR: char = '\u{EA39}';
    pub const SETTINGS: char = '\u{E713}';
    pub const MIN: char = '\u{E921}';
    pub const MAX: char = '\u{E922}';
    pub const RESTORE: char = '\u{E923}';
    pub const CLOSE: char = '\u{E8BB}';
}

#[derive(Clone)]
pub struct Palette {
    pub dark: bool,
    pub mica_fallback: Hsla,
    pub card: Hsla,
    pub stroke: Hsla,
    pub divider: Hsla,
    pub control: Hsla,
    pub control_hover: Hsla,
    pub control_pressed: Hsla,
    pub control_stroke: Hsla,
    pub flyout: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub text3: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    pub on_accent: Hsla,
    pub accent_text: Hsla,
    pub accent_fill: Hsla,
    pub icon_bg: Hsla,
    pub subtle_hover: Hsla,
    pub subtle_pressed: Hsla,
    pub track: Hsla,
    pub meter_track: Hsla,
    pub success: Hsla,
    pub success_bg: Hsla,
    pub caution: Hsla,
    pub caution_bg: Hsla,
    pub critical: Hsla,
    pub critical_bg: Hsla,
    pub info_bg: Hsla,
    pub graph_bg: Hsla,
    pub grid: Hsla,
    pub zero: Hsla,
    pub toggle_off: Hsla,
    pub close_hover: Hsla,
}

fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// `hex` as 0xRRGGBB with alpha 0..1.
fn ca(hex: u32, a: f32) -> Hsla {
    rgba((hex << 8) | (a.clamp(0.0, 1.0) * 255.0).round() as u32).into()
}

/// Windows' accent palette (light3, light2, light1, base, dark1, dark2, dark3, …).
fn system_accent() -> [u32; 8] {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut buf = [0u8; 32];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    }
    .is_ok();
    if !ok || len < 32 {
        // Windows' default blue.
        return [0x99EBFF, 0x4CC2FF, 0x0091F8, 0x0078D4, 0x0067C0, 0x003E92, 0x001A68, 0xF7630C];
    }
    std::array::from_fn(|i| u32::from_be_bytes([0, buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]]))
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        let acc = system_accent();
        if dark {
            let accent = acc[1];
            Palette {
                dark,
                mica_fallback: c(0x202020),
                card: ca(0xFFFFFF, 0.051),
                stroke: ca(0xFFFFFF, 0.06),
                divider: ca(0xFFFFFF, 0.08),
                control: ca(0xFFFFFF, 0.061),
                control_hover: ca(0xFFFFFF, 0.084),
                control_pressed: ca(0xFFFFFF, 0.033),
                control_stroke: ca(0xFFFFFF, 0.07),
                flyout: c(0x2C2C2C),
                text: c(0xFFFFFF),
                text2: ca(0xFFFFFF, 0.786),
                text3: ca(0xFFFFFF, 0.6),
                accent: c(accent),
                accent_hover: ca(accent, 0.9),
                accent_pressed: ca(accent, 0.8),
                on_accent: c(0x000000),
                accent_text: c(acc[0]),
                accent_fill: ca(accent, 0.14),
                icon_bg: ca(accent, 0.12),
                subtle_hover: ca(0xFFFFFF, 0.061),
                subtle_pressed: ca(0xFFFFFF, 0.042),
                track: ca(0xFFFFFF, 0.545),
                meter_track: ca(0xFFFFFF, 0.09),
                success: c(0x6CCB5F),
                success_bg: c(0x393D1B),
                caution: c(0xFCE100),
                caution_bg: c(0x433519),
                critical: c(0xFF99A4),
                critical_bg: c(0x442726),
                info_bg: ca(0xFFFFFF, 0.051),
                graph_bg: ca(0x000000, 0.3),
                grid: ca(0xFFFFFF, 0.06),
                zero: ca(0xFFFFFF, 0.25),
                toggle_off: ca(0xFFFFFF, 0.786),
                close_hover: c(0xC42B1C),
            }
        } else {
            let accent = acc[4];
            Palette {
                dark,
                mica_fallback: c(0xF3F3F3),
                card: ca(0xFFFFFF, 0.7),
                stroke: ca(0x000000, 0.0578),
                divider: ca(0x000000, 0.08),
                control: ca(0xFFFFFF, 0.7),
                control_hover: ca(0xF9F9F9, 0.5),
                control_pressed: ca(0xF9F9F9, 0.3),
                control_stroke: ca(0x000000, 0.09),
                flyout: c(0xF9F9F9),
                text: ca(0x000000, 0.896),
                text2: ca(0x000000, 0.606),
                text3: ca(0x000000, 0.56),
                accent: c(accent),
                accent_hover: ca(accent, 0.9),
                accent_pressed: ca(accent, 0.8),
                on_accent: c(0xFFFFFF),
                accent_text: c(acc[5]),
                accent_fill: ca(accent, 0.1),
                icon_bg: ca(accent, 0.08),
                subtle_hover: ca(0x000000, 0.037),
                subtle_pressed: ca(0x000000, 0.024),
                track: ca(0x000000, 0.446),
                meter_track: ca(0x000000, 0.07),
                success: c(0x0F7B0F),
                success_bg: c(0xDFF6DD),
                caution: c(0x9D5D00),
                caution_bg: c(0xFFF4CE),
                critical: c(0xC42B1C),
                critical_bg: c(0xFDE7E9),
                info_bg: ca(0xF6F6F6, 0.5),
                graph_bg: ca(0xFFFFFF, 0.8),
                grid: ca(0x000000, 0.05),
                zero: ca(0x000000, 0.2),
                toggle_off: ca(0x000000, 0.606),
                close_hover: c(0xC42B1C),
            }
        }
    }
}

pub struct Fluent(pub Palette);
impl Global for Fluent {}

pub fn fl(cx: &App) -> &Palette {
    &cx.global::<Fluent>().0
}

fn is_dark(a: WindowAppearance) -> bool {
    // Debug override for screenshots: WINFX_THEME=light|dark.
    match std::env::var("WINFX_THEME").as_deref() {
        Ok("dark") => return true,
        Ok("light") => return false,
        _ => {}
    }
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

/// Pick light/dark from the system and restyle everything to match.
pub fn sync(window: &Window, cx: &mut App) {
    apply(is_dark(window.appearance()), cx);
}

pub fn sync_app(cx: &mut App) {
    apply(is_dark(cx.window_appearance()), cx);
}

fn apply(dark: bool, cx: &mut App) {
    let p = Palette::new(dark);
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    let t = Theme::global_mut(cx);
    t.font_family = FONT.into();
    t.font_size = px(14.);
    t.radius = px(4.);
    t.radius_lg = px(8.);
    t.shadow = false;
    let k = &mut t.colors;
    k.background = if dark { gpui::hsla(0., 0., 0.17, 1.) } else { gpui::hsla(0., 0., 1., 1.) };
    k.foreground = p.text;
    k.border = p.control_stroke;
    k.input = p.control_stroke;
    k.ring = p.accent;
    k.caret = p.text;
    k.selection = p.accent.opacity(0.3);
    k.primary = p.accent;
    k.primary_hover = p.accent_hover;
    k.primary_active = p.accent_pressed;
    k.primary_foreground = p.on_accent;
    k.secondary = p.control;
    k.secondary_hover = p.control_hover;
    k.secondary_active = p.control_pressed;
    k.secondary_foreground = p.text;
    k.accent = p.subtle_hover;
    k.accent_foreground = p.text;
    k.muted = p.meter_track;
    k.muted_foreground = p.text2;
    k.popover = p.flyout;
    k.popover_foreground = p.text;
    k.list = p.flyout;
    k.list_hover = p.subtle_hover;
    k.list_active = p.accent.opacity(0.12);
    k.list_active_border = p.accent;
    k.slider_bar = p.accent;
    k.slider_thumb = p.accent;
    k.scrollbar = gpui::transparent_black();
    k.scrollbar_thumb = p.text3.opacity(0.5);
    k.scrollbar_thumb_hover = p.text3;
    k.link = p.accent_text;
    k.danger = p.critical;
    k.warning = p.caution;
    k.success = p.success;
    k.title_bar = gpui::transparent_black();
    k.window_border = p.stroke;
    cx.set_global(Fluent(p));
    cx.refresh_windows();
}

// ---------- building blocks ----------

pub fn glyph(ch: char, size: f32) -> Div {
    div()
        .font_family(ICON_FONT)
        .text_size(px(size))
        .line_height(px(size))
        .flex_none()
        .child(ch.to_string())
}

pub fn card(cx: &App) -> Div {
    let p = fl(cx);
    div().bg(p.card).border_1().border_color(p.stroke).rounded(px(8.))
}

pub fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div().text_xs().line_height(px(16.)).text_color(fl(cx).text2).child(text.into())
}

pub fn section_title(text: &'static str, cx: &App) -> Div {
    div()
        .mt(px(16.))
        .mb(px(4.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(fl(cx).text)
        .child(text)
}

/// Fluent toggle switch (40×20).
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    label: &str,
    cx: &App,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let p = fl(cx);
    let (bg, border, knob) =
        if on { (p.accent, p.accent, p.on_accent) } else { (gpui::transparent_black(), p.toggle_off, p.toggle_off) };
    let hover = if on { p.accent_hover } else { p.subtle_hover };
    div()
        .id(id)
        .flex_none()
        .relative()
        .w(px(40.))
        .h(px(20.))
        .rounded_full()
        .border_1()
        .border_color(border)
        .bg(bg)
        .hover(move |s| s.bg(hover))
        .cursor_pointer()
        .tooltip({
            let label: SharedString = label.to_owned().into();
            move |w, cx| gpui_component::tooltip::Tooltip::new(label.clone()).build(w, cx)
        })
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(if on { 23. } else { 3. }))
                .size(px(12.))
                .rounded_full()
                .bg(knob),
        )
        .on_click(move |_, w, cx| on_toggle(!on, w, cx))
}

fn button_base(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px(px(12.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .rounded(px(4.))
        .flex_none()
        .cursor_pointer()
}

/// Neutral Fluent button.
pub fn button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let p = fl(cx);
    let (h, a) = (p.control_hover, p.control_pressed);
    button_base(id)
        .bg(p.control)
        .border_1()
        .border_color(p.control_stroke)
        .text_color(p.text)
        .hover(move |s| s.bg(h))
        .active(move |s| s.bg(a))
}

/// Accent (primary) Fluent button.
pub fn accent_button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let p = fl(cx);
    let (h, a) = (p.accent_hover, p.accent_pressed);
    button_base(id)
        .bg(p.accent)
        .border_1()
        .border_color(p.accent)
        .text_color(p.on_accent)
        .hover(move |s| s.bg(h))
        .active(move |s| s.bg(a))
}

/// Transparent button that highlights on hover.
pub fn subtle_button(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let p = fl(cx);
    let (h, a) = (p.subtle_hover, p.subtle_pressed);
    button_base(id).px(px(8.)).text_color(p.text2).hover(move |s| s.bg(h)).active(move |s| s.bg(a))
}

pub const METER_FLOOR: f32 = -60.0;

fn meter_frac(db: f32) -> f32 {
    ((db - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
}

/// Horizontal level meter with a dB scale.
pub fn level_meter(label: &str, peak: f32, rms: f32, cx: &App) -> Div {
    let p = fl(cx);
    let peak_color = if peak > -3.0 { p.critical } else if peak > -9.0 { p.caution } else { p.accent };
    let ticks = [(-60.0, "−60"), (-40.0, "−40"), (-20.0, "−20"), (-6.0, "−6"), (0.0, "0")];
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .line_height(px(16.))
                .text_color(p.text2)
                .child(label.to_owned())
                .child(if peak <= -89.0 { "−∞".to_owned() } else { format!("{:.1} dB", peak).replace('-', "−") }),
        )
        .child(
            div()
                .relative()
                .h(px(6.))
                .rounded_full()
                .overflow_hidden()
                .bg(p.meter_track)
                .child(div().absolute().top_0().left_0().h_full().rounded_full().w(gpui::relative(meter_frac(rms))).bg(p.accent))
                .child(div().absolute().top_0().h_full().w(px(2.)).left(gpui::relative(meter_frac(peak))).bg(peak_color)),
        )
        .child(
            div()
                .relative()
                .h(px(12.))
                .text_size(px(10.))
                .line_height(px(12.))
                .text_color(p.text3)
                .children(ticks.into_iter().map(|(db, t)| {
                    let x = meter_frac(db);
                    div()
                        .absolute()
                        .top_0()
                        .when(x < 0.99, |d| d.left(gpui::relative(x)))
                        .when(x >= 0.99, |d| d.right_0())
                        .child(t)
                })),
        )
}

#[derive(Clone, Copy)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

pub fn info_bar(sev: Severity, title: &str, message: String, cx: &App) -> Div {
    let p = fl(cx);
    let (bg, fg, icon) = match sev {
        Severity::Info => (p.info_bg, p.accent_text, g::INFO),
        Severity::Success => (p.success_bg, p.success, g::COMPLETED),
        Severity::Warning => (p.caution_bg, p.caution, g::WARNING),
        Severity::Error => (p.critical_bg, p.critical, g::ERROR),
    };
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.))
        .pl(px(16.))
        .pr(px(8.))
        .py(px(12.))
        .rounded(px(8.))
        .bg(bg)
        .border_1()
        .border_color(p.stroke)
        .child(glyph(icon, 16.).mt(px(2.)).text_color(fg))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x(px(6.))
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_owned()))
                        .child(div().text_color(p.text).child(message)),
                ),
        )
}
