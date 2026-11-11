//! Main window, Windows 11 Fluent style.

use crate::fluent::{self, fl, g, Severity};
use crate::params::{self, ParamKey, ParamSpec};
use crate::state::{is_cable, AppState};
use crate::system;
use engine::chain::{EffectConfig, EffectSlot};
use engine::dsp::lin_to_db;
use engine::meters::MAX_SLOTS;
use engine::{ChainConfig, DeviceSel, RunState};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    anchored, canvas, deferred, div, fill, point, px, relative, size, svg, Anchor, AnyElement, App,
    AppContext as _, Bounds, ClickEvent, Context, Entity, ExternalPaths, InteractiveElement as _, IntoElement,
    PathPromptOptions,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder,
    Pixels, Render, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, WindowControlArea,
};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_component::IndexPath;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

type NameSelect = SelectState<Vec<SharedString>>;

const EQ_H: f32 = 200.0;
const EQ_RANGE: f32 = 12.0;

pub struct MainView {
    state: Entity<AppState>,
    input_sel: Entity<NameSelect>,
    output_sel: Entity<NameSelect>,
    monitor_sel: Entity<NameSelect>,
    preset_sel: Entity<NameSelect>,
    monitor_vol: Entity<SliderState>,
    preset_name: Entity<InputState>,
    cards: Vec<Card>,
    eq: EqUi,
    levels: Levels,
    status: engine::Status,
    autostart: bool,
    add_open: bool,
    save_as_open: bool,
    /// Result of the last import: (severity, title, details).
    import_notice: Option<(Severity, String, String)>,
    popover_closed_at: Instant,
    device_snapshot: (Vec<String>, Vec<String>),
    last_device_poll: Instant,
    /// Spectrum behind the EQ curve.
    analyzer: crate::spectrum::Analyzer,
    /// A/B: hear the original. Level-match from running loudness of both.
    ab_original: bool,
    ab_in_ms: f32,
    ab_out_ms: f32,
    /// Gate auto-calibration in progress / last result (slot id, message, ok).
    calib: Option<Calib>,
    calib_result: Option<(u32, String, bool)>,
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
}

/// Gate calibration: listen to the gate's detector during silence, then speech.
struct Calib {
    slot: u32,
    talking: bool,
    until: Instant,
    quiet: Vec<f32>,
    talk: Vec<f32>,
}

const CALIB_QUIET: Duration = Duration::from_secs(4);
const CALIB_TALK: Duration = Duration::from_secs(5);

fn percentile(v: &mut [f32], q: f32) -> f32 {
    if v.is_empty() {
        return -90.0;
    }
    v.sort_by(f32::total_cmp);
    v[((v.len() - 1) as f32 * q).round() as usize]
}

struct Card {
    id: u32,
    expanded: bool,
    params: Vec<ParamCtl>,
}

struct ParamCtl {
    spec: ParamSpec,
    slider: Entity<SliderState>,
    _sub: Subscription,
}

/// EQ editor state: selected band, drag in progress, the graph's on-screen bounds.
struct EqUi {
    slot: Option<u32>,
    band: usize,
    drag: Option<usize>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Selected band's Frequency / Gain / Q / Slope sliders.
    ctls: Vec<ParamCtl>,
    gain_ctl: Option<ParamCtl>,
    type_sel: Option<Entity<NameSelect>>,
    _type_sub: Option<Subscription>,
}

/// Display-side meter state (dB), with peak hold/decay.
struct Levels {
    in_peak: f32,
    in_rms: f32,
    out_peak: f32,
    out_rms: f32,
    gr: [f32; MAX_SLOTS],
    /// Signed live values (auto-gain's gain).
    aux: [f32; MAX_SLOTS],
    last: Instant,
}

impl Default for Levels {
    fn default() -> Self {
        Levels { in_peak: -90.0, in_rms: -90.0, out_peak: -90.0, out_rms: -90.0, gr: [0.0; MAX_SLOTS], aux: [0.0; MAX_SLOTS], last: Instant::now() }
    }
}

#[derive(Clone)]
struct DragSlot {
    id: u32,
    title: SharedString,
}

impl Render for DragSlot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx);
        div()
            .px_3()
            .py_1()
            .rounded(px(4.))
            .bg(p.accent)
            .text_color(p.on_accent)
            .font_family(fluent::FONT)
            .child(self.title.clone())
    }
}

fn names(list: &[engine::devices::DeviceInfo]) -> Vec<SharedString> {
    list.iter().map(|d| SharedString::from(d.name.clone())).collect()
}

fn selected_row(list: &[engine::devices::DeviceInfo], sel: &DeviceSel) -> Option<IndexPath> {
    match sel {
        DeviceSel::Device { id, name } => list
            .iter()
            .position(|d| &d.id == id)
            .or_else(|| list.iter().position(|d| &d.name == name)),
        DeviceSel::Default => list.iter().position(|d| d.name.starts_with("CABLE Input")),
    }
    .map(IndexPath::new)
}

/// Monitor's "Default" means the Windows default playback device.
fn monitor_row(list: &[engine::devices::DeviceInfo], sel: &DeviceSel) -> Option<IndexPath> {
    match sel {
        DeviceSel::Default => {
            let def = engine::devices::default_device(engine::devices::Dir::Output, engine::devices::Role::Console)?;
            list.iter().position(|d| d.id == def.id).map(IndexPath::new)
        }
        other => selected_row(list, other),
    }
}

fn eq_x(f: f32) -> f32 {
    ((f / 20.0).log10() / 3.0).clamp(0.0, 1.0)
}

fn eq_y(db: f32, h: f32) -> f32 {
    h / 2.0 - db.clamp(-EQ_RANGE, EQ_RANGE) * (h / 2.0) / EQ_RANGE
}

impl MainView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = state.read(cx);
        let (inputs, outputs) = (s.inputs.clone(), s.outputs.clone());
        let input_row = selected_row(&inputs, &s.config.engine.input);
        let output_row = selected_row(&outputs, &s.config.engine.output);
        let presets: Vec<SharedString> = s.presets.iter().map(|p| p.clone().into()).collect();
        let preset_row = s
            .config
            .preset
            .as_ref()
            .and_then(|p| s.presets.iter().position(|x| x == p))
            .map(IndexPath::new);
        let device_snapshot = (
            inputs.iter().map(|d| d.id.clone()).collect(),
            outputs.iter().map(|d| d.id.clone()).collect(),
        );
        let mon_outputs = s.monitor_outputs();
        let mon_row = monitor_row(&mon_outputs, &s.config.engine.monitor.device);
        let mon_db = s.config.engine.monitor.volume_db;

        let input_sel = cx.new(|cx| SelectState::new(names(&inputs), input_row, window, cx));
        let output_sel = cx.new(|cx| SelectState::new(names(&outputs), output_row, window, cx));
        let monitor_sel = cx.new(|cx| SelectState::new(names(&mon_outputs), mon_row, window, cx));
        let preset_sel = cx.new(|cx| SelectState::new(presets, preset_row, window, cx));
        let monitor_vol =
            cx.new(|_| SliderState::new().min(-40.0).max(6.0).step(0.5).default_value(mon_db));
        let preset_name = cx.new(|cx| InputState::new(window, cx).placeholder("Preset name"));

        let subs = vec![
            window.observe_window_appearance(|window, cx| fluent::sync(window, cx)),
            cx.subscribe_in(&input_sel, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                this.state.update(cx, |s, _| {
                    if let Some(d) = s.inputs.iter().find(|d| d.name == name.as_ref()).cloned() {
                        s.set_engine_config(|c| c.input = DeviceSel::device(&d));
                    }
                });
                cx.notify();
            }),
            cx.subscribe_in(&output_sel, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                this.state.update(cx, |s, _| {
                    if let Some(d) = s.outputs.iter().find(|d| d.name == name.as_ref()).cloned() {
                        s.set_engine_config(|c| c.output = DeviceSel::device(&d));
                    }
                });
                cx.notify();
            }),
            cx.subscribe_in(&monitor_sel, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                this.state.update(cx, |s, _| {
                    if let Some(d) = s.monitor_outputs().into_iter().find(|d| d.name == name.as_ref()) {
                        s.set_monitor(|m| m.device = DeviceSel::device(&d));
                    }
                });
                cx.notify();
            }),
            cx.subscribe_in(&preset_sel, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                let name = name.to_string();
                if this.state.update(cx, |s, _| s.load_preset(&name)) {
                    this.rebuild_cards(window, cx);
                }
            }),
            cx.subscribe_in(&monitor_vol, window, |this, _, ev: &SliderEvent, _, cx| {
                if let SliderEvent::Change(v) = ev {
                    let db = v.start();
                    this.state.update(cx, |s, _| s.set_monitor_volume(db));
                    cx.notify();
                }
            }),
        ];

        let ticker = cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(33)).await;
            if this.update_in(cx, |this, window, cx| this.tick(window, cx)).is_err() {
                break;
            }
        });

        let mut view = MainView {
            state,
            input_sel,
            output_sel,
            monitor_sel,
            preset_sel,
            monitor_vol,
            preset_name,
            cards: vec![],
            eq: EqUi {
                slot: None,
                band: 0,
                drag: None,
                bounds: Rc::new(Cell::new(Bounds::default())),
                ctls: vec![],
                gain_ctl: None,
                type_sel: None,
                _type_sub: None,
            },
            levels: Levels::default(),
            status: engine::Status::default(),
            autostart: system::autostart_enabled(),
            add_open: false,
            save_as_open: false,
            import_notice: None,
            popover_closed_at: Instant::now(),
            device_snapshot,
            last_device_poll: Instant::now(),
            analyzer: crate::spectrum::Analyzer::default(),
            ab_original: false,
            ab_in_ms: 0.0,
            ab_out_ms: 0.0,
            calib: None,
            calib_result: None,
            _ticker: ticker,
            _subs: subs,
        };
        view.rebuild_cards(window, cx);
        // A preset file passed on the command line ("Open with WinFX").
        if let Some(path) = view.state.update(cx, |s, _| s.pending_import.take()) {
            view.import_path(&path, window, cx);
        }
        view
    }

    fn edit_chain(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut ChainConfig)) {
        self.state.update(cx, |s, _| {
            let mut c = s.chain().clone();
            f(&mut c);
            s.set_chain(c);
        });
        cx.notify();
    }

    fn rebuild_cards(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chain = self.state.read(cx).chain().clone();
        let old: Vec<(u32, bool)> = self.cards.iter().map(|c| (c.id, c.expanded)).collect();
        let first_build = old.is_empty();
        self.eq.slot = chain.slots.iter().find(|s| matches!(s.effect, EffectConfig::Eq(_))).map(|s| s.id);
        self.cards = chain
            .slots
            .iter()
            .map(|slot| {
                let is_eq = matches!(slot.effect, EffectConfig::Eq(_));
                let expanded = old.iter().find(|(id, _)| *id == slot.id).map_or(
                    std::env::var_os("WINFX_EXPAND_ALL").is_some()
                        || (first_build && is_eq),
                    |(_, e)| *e,
                );
                // EQ bands are edited through the graph + selected-band sliders.
                let params = if is_eq {
                    vec![]
                } else {
                    params::specs(&slot.effect)
                        .into_iter()
                        .map(|spec| self.param_ctl(slot, spec, window, cx))
                        .collect()
                };
                Card { id: slot.id, expanded, params }
            })
            .collect();
        self.select_band(self.eq.band, window, cx);
        cx.notify();
    }

    fn param_ctl(&self, slot: &EffectSlot, spec: ParamSpec, window: &mut Window, cx: &mut Context<Self>) -> ParamCtl {
        let v = params::get(&slot.effect, spec.key);
        let slider = cx.new(|_| {
            let s = SliderState::new().min(spec.min).max(spec.max).step(spec.step).default_value(v);
            if spec.log { s.scale(SliderScale::Logarithmic) } else { s }
        });
        let (id, key) = (slot.id, spec.key);
        let sub = cx.subscribe_in(&slider, window, move |this, _, ev: &SliderEvent, _, cx| {
            if let SliderEvent::Change(v) = ev {
                let v = v.start();
                this.edit_chain(cx, |c| {
                    if let Some(s) = c.slots.iter_mut().find(|s| s.id == id) {
                        params::set(&mut s.effect, key, v);
                    }
                });
            }
        });
        ParamCtl { spec, slider, _sub: sub }
    }

    // ---------- EQ editor ----------

    fn eq_slot(&self, cx: &App) -> Option<EffectSlot> {
        let id = self.eq.slot?;
        self.state.read(cx).chain().slots.iter().find(|s| s.id == id).cloned()
    }

    fn select_band(&mut self, band: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(slot) = self.eq_slot(cx) else {
            self.eq.ctls.clear();
            return;
        };
        let EffectConfig::Eq(eq) = &slot.effect else { return };
        let band = band.min(eq.bands.len().saturating_sub(1));
        self.eq.band = band;
        let specs = params::specs(&slot.effect);
        self.eq.ctls = specs
            .iter()
            .filter(|s| {
                matches!(s.key, ParamKey::BandFreq(i) | ParamKey::BandGain(i) | ParamKey::BandQ(i) | ParamKey::BandSlope(i) if i == band)
            })
            .cloned()
            .map(|mut spec| {
                spec.label = match spec.key {
                    ParamKey::BandFreq(_) => "Frequency".into(),
                    ParamKey::BandGain(_) => "Gain".into(),
                    ParamKey::BandSlope(_) => "Slope".into(),
                    _ => "Q".into(),
                };
                self.param_ctl(&slot, spec, window, cx)
            })
            .collect();
        self.eq.gain_ctl = specs
            .into_iter()
            .find(|s| s.key == ParamKey::EqGain)
            .map(|spec| self.param_ctl(&slot, spec, window, cx));

        // Band type menu.
        let kinds: Vec<SharedString> = params::BAND_KINDS.iter().map(|(_, n)| SharedString::from(*n)).collect();
        let current = eq.bands.get(band).map(|b| b.kind);
        let row = params::BAND_KINDS.iter().position(|(k, _)| Some(*k) == current).map(IndexPath::new);
        let sel = cx.new(|cx| SelectState::new(kinds, row, window, cx));
        let id = slot.id;
        let sub = cx.subscribe_in(&sel, window, move |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
            let SelectEvent::Confirm(Some(name)) = ev else { return };
            let Some((kind, _)) = params::BAND_KINDS.iter().find(|(_, n)| *n == name.as_ref()) else { return };
            let band = this.eq.band;
            this.edit_chain(cx, |c| {
                if let Some(EffectSlot { effect: EffectConfig::Eq(eq), .. }) = c.slots.iter_mut().find(|s| s.id == id) {
                    if let Some(b) = eq.bands.get_mut(band) {
                        b.kind = *kind;
                    }
                }
            });
            // The gain slider appears/disappears with the type.
            this.select_band(band, window, cx);
        });
        self.eq.type_sel = Some(sel);
        self.eq._type_sub = Some(sub);
        cx.notify();
    }

    fn eq_handles(&self, cx: &App) -> Vec<(f32, f32, bool)> {
        let Some(EffectSlot { effect: EffectConfig::Eq(eq), .. }) = self.eq_slot(cx) else { return vec![] };
        eq.bands
            .iter()
            .map(|b| (b.freq_hz, if b.kind.has_gain() { b.gain_db } else { 0.0 }, b.enabled))
            .collect()
    }

    fn eq_edit_band(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut engine::chain::EqParams, usize)) {
        let (Some(id), band) = (self.eq.slot, self.eq.band) else { return };
        self.edit_chain(cx, |c| {
            if let Some(EffectSlot { effect: EffectConfig::Eq(eq), .. }) = c.slots.iter_mut().find(|s| s.id == id) {
                f(eq, band);
            }
        });
    }

    fn eq_add_band(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut new_ix = 0;
        self.eq_edit_band(cx, |eq, _| {
            eq.bands.push(engine::chain::EqBand {
                enabled: true,
                kind: engine::dsp::FilterKind::Peak,
                freq_hz: 1000.0,
                gain_db: 0.0,
                q: 1.0,
                slope: 1,
            });
            new_ix = eq.bands.len() - 1;
        });
        self.select_band(new_ix, window, cx);
    }

    fn eq_remove_band(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.eq_edit_band(cx, |eq, band| {
            if eq.bands.len() > 1 && band < eq.bands.len() {
                eq.bands.remove(band);
            }
        });
        self.select_band(self.eq.band.saturating_sub(1), window, cx);
    }

    // ---------- import ----------

    fn pick_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update_in(cx, |this, window, cx| this.import_path(&path, window, cx));
                }
            }
        })
        .detach();
    }

    fn import_path(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let result = self.state.update(cx, |s, _| s.import_file(path));
        self.import_notice = Some(match result {
            Ok(outcome) => {
                let sev = if outcome.report.skipped.is_empty() && outcome.report.approximated.is_empty() {
                    Severity::Success
                } else {
                    Severity::Info
                };
                (sev, outcome.title.clone(), outcome.summary())
            }
            Err(e) => (Severity::Error, format!("Couldn’t import “{file}”."), e),
        });
        // The presets list and selection may have changed.
        let (presets, current) = {
            let s = self.state.read(cx);
            (s.presets.clone(), s.config.preset.clone())
        };
        let row = current.and_then(|c| presets.iter().position(|p| *p == c)).map(IndexPath::new);
        let items: Vec<SharedString> = presets.into_iter().map(Into::into).collect();
        self.preset_sel.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_index(row, window, cx);
        });
        self.eq.band = 0;
        self.rebuild_cards(window, cx);
        if let Some(id) = self.eq.slot {
            if let Some(c) = self.cards.iter_mut().find(|c| c.id == id) {
                c.expanded = true;
            }
        }
    }

    fn eq_mouse_down(&mut self, pos: gpui::Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let b = self.eq.bounds.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        if w <= 0.0 {
            return;
        }
        let (mx, my) = (f32::from(pos.x - b.origin.x), f32::from(pos.y - b.origin.y));
        let hit = self
            .eq_handles(cx)
            .iter()
            .enumerate()
            .map(|(i, (f, g, _))| {
                let (hx, hy) = (eq_x(*f) * w, eq_y(*g, h));
                (i, ((hx - mx).powi(2) + (hy - my).powi(2)).sqrt())
            })
            .filter(|(_, d)| *d < 16.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        if let Some(i) = hit {
            if i != self.eq.band {
                self.select_band(i, window, cx);
            }
            self.eq.drag = Some(i);
        }
    }

    fn eq_mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.eq.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.eq.drag = None;
            return;
        }
        let b = self.eq.bounds.get();
        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
        let fx = (f32::from(ev.position.x - b.origin.x) / w).clamp(0.0, 1.0);
        let freq = (20.0 * 1000f32.powf(fx)).clamp(20.0, 20_000.0).round();
        let gain = ((h / 2.0 - f32::from(ev.position.y - b.origin.y)) * EQ_RANGE / (h / 2.0)).clamp(-18.0, 18.0);
        let gain = (gain * 10.0).round() / 10.0;
        let Some(id) = self.eq.slot else { return };
        self.edit_chain(cx, |c| {
            if let Some(s) = c.slots.iter_mut().find(|s| s.id == id) {
                params::set(&mut s.effect, ParamKey::BandFreq(i), freq);
                params::set(&mut s.effect, ParamKey::BandGain(i), gain);
            }
        });
        for ctl in &self.eq.ctls {
            let v = match ctl.spec.key {
                ParamKey::BandFreq(_) => freq,
                ParamKey::BandGain(_) => gain,
                _ => continue,
            };
            ctl.slider.update(cx, |s, cx| s.set_value(v, window, cx));
        }
    }

    /// Mouse wheel over the graph adjusts the selected band's Q.
    fn eq_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.eq.slot else { return };
        let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
        if dy == 0.0 {
            return;
        }
        let band = self.eq.band;
        let mut new_q = 0.0;
        self.edit_chain(cx, |c| {
            if let Some(s) = c.slots.iter_mut().find(|s| s.id == id) {
                let q = params::get(&s.effect, ParamKey::BandQ(band));
                new_q = ((q * 1.1f32.powf(dy / 20.0)) * 100.0).round().clamp(10.0, 1000.0) / 100.0;
                params::set(&mut s.effect, ParamKey::BandQ(band), new_q);
            }
        });
        for ctl in &self.eq.ctls {
            if matches!(ctl.spec.key, ParamKey::BandQ(_)) {
                ctl.slider.update(cx, |s, cx| s.set_value(new_q, window, cx));
            }
        }
    }

    // ---------- chain edits ----------

    fn move_slot(&mut self, from_id: u32, to_id: u32, cx: &mut Context<Self>) {
        if from_id == to_id {
            return;
        }
        self.edit_chain(cx, |c| {
            let Some(from) = c.slots.iter().position(|s| s.id == from_id) else { return };
            let slot = c.slots.remove(from);
            let to = c.slots.iter().position(|s| s.id == to_id).unwrap_or(c.slots.len());
            let to = if from <= to { to + 1 } else { to };
            c.slots.insert(to.min(c.slots.len()), slot);
        });
        let order: Vec<u32> = self.state.read(cx).chain().slots.iter().map(|s| s.id).collect();
        self.cards.sort_by_key(|c| order.iter().position(|id| *id == c.id));
    }

    fn add_effect(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(effect) = params::new_effect(kind) else { return };
        let mut new_id = 0;
        self.edit_chain(cx, |c| {
            new_id = c.next_id();
            c.slots.push(EffectSlot { id: new_id, bypass: false, effect });
        });
        self.rebuild_cards(window, cx);
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == new_id) {
            card.expanded = true;
        }
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (status, meters) = {
            let s = self.state.read(cx);
            (s.engine.status(), s.engine.meters().clone())
        };
        let now = Instant::now();
        let dt = now.duration_since(self.levels.last).as_secs_f32();
        self.levels.last = now;
        let fall = 24.0 * dt;
        let db = |x: f32| lin_to_db(x).max(-90.0);
        let l = &mut self.levels;
        l.in_peak = db(meters.in_peak.take()).max(l.in_peak - fall);
        l.out_peak = db(meters.out_peak.take()).max(l.out_peak - fall);
        let smooth = |old: f32, new: f32| if new > old { new } else { old + (new - old) * (dt * 8.0).min(1.0) };
        l.in_rms = smooth(l.in_rms, db(meters.in_rms.take()));
        l.out_rms = smooth(l.out_rms, db(meters.out_rms.take()));
        for (i, a) in l.aux.iter_mut().enumerate() {
            *a = meters.aux[i].load();
        }
        for (i, gr) in l.gr.iter_mut().enumerate() {
            let v = meters.gr[i].take();
            *gr = if v > *gr { v } else { *gr + (v - *gr) * (dt * 6.0).min(1.0) };
        }
        self.status = status;

        // Spectrum: only while an EQ (or filter) card is open.
        let eq_open = self.cards.iter().any(|c| c.expanded && Some(c.id) == self.eq.slot);
        let tap = self.state.read(cx).engine.tap().clone();
        if eq_open {
            self.analyzer.update(&tap, dt);
        }

        // A/B level match: running mean-square of input and output while auditioning.
        if tap.is_auditioning() && !self.ab_original {
            let a = (dt * 0.5).min(1.0);
            let (i, o) = (engine::dsp::db_to_lin(l.in_rms), engine::dsp::db_to_lin(l.out_rms));
            self.ab_in_ms += (i * i - self.ab_in_ms) * a;
            self.ab_out_ms += (o * o - self.ab_out_ms) * a;
        }
        if self.ab_original {
            let gain = if self.ab_in_ms > 1e-10 { (self.ab_out_ms / self.ab_in_ms).sqrt().clamp(0.1, 10.0) } else { 1.0 };
            tap.set_original(true, gain);
        }

        // Gate calibration.
        if let Some(c) = &mut self.calib {
            let chain = self.state.read(cx).chain().clone();
            match chain.slots.iter().position(|s| s.id == c.slot) {
                None => self.calib = None,
                Some(ix) => {
                    let v = lin_to_db(meters.det[ix].take()).max(-90.0);
                    if c.talking { c.talk.push(v) } else { c.quiet.push(v) }
                    if Instant::now() >= c.until {
                        if !c.talking {
                            c.talking = true;
                            c.until = Instant::now() + CALIB_TALK;
                        } else {
                            let c = self.calib.take().unwrap();
                            self.finish_calibration(c, window, cx);
                        }
                    }
                }
            }
        }

        self.state.update(cx, |s, _| s.save_if_dirty());
        if self.last_device_poll.elapsed() > Duration::from_secs(3) {
            self.last_device_poll = Instant::now();
            self.poll_devices(window, cx);
        }
        cx.notify();
    }

    fn poll_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (inputs, outputs, mons, ein, eout, emon) = self.state.update(cx, |s, _| {
            s.refresh_devices();
            (
                s.inputs.clone(),
                s.outputs.clone(),
                s.monitor_outputs(),
                s.config.engine.input.clone(),
                s.config.engine.output.clone(),
                s.config.engine.monitor.device.clone(),
            )
        });
        let snap = (
            inputs.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
            outputs.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
        );
        if snap == self.device_snapshot {
            return;
        }
        self.device_snapshot = snap;
        self.input_sel.update(cx, |s, cx| {
            s.set_items(names(&inputs), window, cx);
            s.set_selected_index(selected_row(&inputs, &ein), window, cx);
        });
        self.output_sel.update(cx, |s, cx| {
            s.set_items(names(&outputs), window, cx);
            s.set_selected_index(selected_row(&outputs, &eout), window, cx);
        });
        self.monitor_sel.update(cx, |s, cx| {
            s.set_items(names(&mons), window, cx);
            s.set_selected_index(monitor_row(&mons, &emon), window, cx);
        });
    }

    fn save_preset_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.preset_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let (presets, current) = self.state.update(cx, |s, _| {
            s.save_preset(&name);
            (s.presets.clone(), s.config.preset.clone())
        });
        let row = current.and_then(|c| presets.iter().position(|p| *p == c)).map(IndexPath::new);
        let items: Vec<SharedString> = presets.into_iter().map(Into::into).collect();
        self.preset_sel.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_index(row, window, cx);
        });
        self.preset_name.update(cx, |s, cx| s.set_value("", window, cx));
        self.save_as_open = false;
        cx.notify();
    }

    fn overwrite_preset(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| {
            if let Some(name) = s.config.preset.clone() {
                s.save_preset(&name);
            }
        });
    }

    // ---------- audition ----------

    fn toggle_record(&mut self, cx: &mut Context<Self>) {
        let (engine_tap, auditioning) = {
            let s = self.state.read(cx);
            (s.engine.tap().clone(), s.engine.tap().is_auditioning())
        };
        if engine_tap.is_recording() {
            engine_tap.stop_recording();
        } else {
            if auditioning {
                self.set_audition(false, cx);
            }
            engine_tap.start_recording();
        }
        cx.notify();
    }

    fn set_audition(&mut self, on: bool, cx: &mut Context<Self>) {
        if !on {
            self.set_ab(false, cx);
        }
        self.ab_in_ms = 0.0;
        self.ab_out_ms = 0.0;
        self.state.read(cx).engine.set_audition(on);
        cx.notify();
    }

    fn set_ab(&mut self, original: bool, cx: &mut Context<Self>) {
        self.ab_original = original;
        if !original {
            self.state.read(cx).engine.tap().set_original(false, 1.0);
        }
        cx.notify();
    }

    fn save_recording(&mut self, cx: &mut Context<Self>) {
        let samples = self.state.read(cx).engine.tap().export_recording();
        let dir = crate::state::recordings_dir();
        let path = dir.join(format!("recording {}.wav", crate::state::local_timestamp()));
        let result = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|_| engine::wav::write_mono(&path, &samples, engine::SAMPLE_RATE));
        self.import_notice = Some(match result {
            Ok(()) => (Severity::Success, "Recording saved.".into(), path.display().to_string()),
            Err(e) => (Severity::Error, "Couldn’t save the recording.".into(), e),
        });
        cx.notify();
    }

    fn load_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Load into Audition".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                this.set_audition(false, cx);
                let max = engine::tap::REC_SECONDS as f32;
                this.import_notice = Some(match engine::wav::read_mono(&path, engine::SAMPLE_RATE as f32, max) {
                    Ok((samples, _)) => {
                        let secs = this.state.read(cx).engine.tap().load_recording(&samples);
                        (Severity::Success, format!("Loaded {secs:.1} s into Audition."), "Press Play to hear it through the chain.".into())
                    }
                    Err(e) => (Severity::Error, "Couldn’t load that file.".into(), e),
                });
                cx.notify();
            });
        })
        .detach();
    }

    // ---------- gate calibration ----------

    fn start_calibration(&mut self, slot: u32, cx: &mut Context<Self>) {
        self.calib_result = None;
        self.calib = Some(Calib { slot, talking: false, until: Instant::now() + CALIB_QUIET, quiet: vec![], talk: vec![] });
        cx.notify();
    }

    fn finish_calibration(&mut self, mut c: Calib, window: &mut Window, cx: &mut Context<Self>) {
        let noise = percentile(&mut c.quiet, 0.95);
        // Speech: the louder half of what was heard while talking (skips pauses).
        let voice = percentile(&mut c.talk, 0.75);
        let threshold = ((noise + 6.0).clamp(-80.0, -20.0) * 2.0).round() / 2.0;
        let gap = voice - threshold;
        let id = c.slot;
        self.edit_chain(cx, |chain| {
            if let Some(EffectSlot { effect: EffectConfig::Gate(g), .. }) = chain.slots.iter_mut().find(|s| s.id == id) {
                g.threshold_db = threshold;
            }
        });
        if let Some(card) = self.cards.iter().find(|c| c.id == id) {
            for ctl in &card.params {
                if ctl.spec.key == ParamKey::Threshold {
                    ctl.slider.update(cx, |s, cx| s.set_value(threshold, window, cx));
                }
            }
        }
        let f = |v: f32| format!("{v:.0} dB").replace('-', "−");
        let (msg, ok) = if gap >= 10.0 {
            (format!("Room noise {} · your voice {} → threshold set to {}.", f(noise), f(voice), f(threshold)), true)
        } else {
            (
                format!(
                    "Threshold set to {}, but your voice ({}) is only {:.0} dB above the room noise ({}). Raise the EVO's input gain or get closer to the mic, then calibrate again.",
                    f(threshold), f(voice), gap.max(0.0), f(noise)
                ),
                false,
            )
        };
        self.calib_result = Some((id, msg, ok));
        cx.notify();
    }

    fn close_popovers(&mut self, cx: &mut Context<Self>) {
        if self.add_open || self.save_as_open {
            self.add_open = false;
            self.save_as_open = false;
            self.popover_closed_at = Instant::now();
            cx.notify();
        }
    }

    /// A click on the button that opened a popover first closes it via
    /// mouse-down-outside; don't immediately reopen it on the click.
    fn just_closed(&self) -> bool {
        self.popover_closed_at.elapsed() < Duration::from_millis(250)
    }
}

// ---------- rendering ----------

fn caption_button(id: &'static str, ch: char, area: WindowControlArea, close: bool, cx: &App) -> impl IntoElement {
    let p = fl(cx);
    let (hover_bg, hover_fg) = if close { (p.close_hover, gpui::white()) } else { (p.subtle_hover, p.text) };
    div()
        .id(id)
        .w(px(46.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(p.text)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .window_control_area(area)
        .child(fluent::glyph(ch, 10.))
}

fn slider_row(p: &ParamCtl, value: f32, cx: &App) -> impl IntoElement {
    let pal = fl(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .justify_between()
                .text_xs()
                .line_height(px(16.))
                .child(div().text_color(pal.text2).child(p.spec.label.clone()))
                .child(div().text_color(pal.text).child(params::format(p.spec.unit, value))),
        )
        .child(Slider::new(&p.slider).bg(pal.accent).text_color(pal.accent))
}

fn transfer_curve(c: &engine::dsp::CompressorParams, cx: &App) -> impl IntoElement {
    let p = fl(cx);
    let (bg, grid, line, stroke) = (p.graph_bg, p.grid, p.accent, p.stroke);
    let (thr, ratio, knee) = (c.threshold_db, c.ratio, c.knee_db);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .flex_none()
        .child(
            div().size(px(148.)).rounded(px(4.)).border_1().border_color(stroke).overflow_hidden().child(
                canvas(
                    |_, _, _| {},
                    move |b, (), window, _| {
                        window.paint_quad(fill(b, bg));
                        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                        let at = |x_db: f32, y_db: f32| {
                            point(b.origin.x + px((x_db + 60.0) / 60.0 * w), b.origin.y + px(h - (y_db + 60.0) / 60.0 * h))
                        };
                        let mut diag = PathBuilder::stroke(px(1.));
                        diag.move_to(at(-60.0, -60.0));
                        diag.line_to(at(0.0, 0.0));
                        if let Ok(path) = diag.build() {
                            window.paint_path(path, grid);
                        }
                        let mut pb = PathBuilder::stroke(px(2.));
                        for i in 0..=120 {
                            let x = -60.0 + i as f32 * 0.5;
                            let y = x + engine::dsp::compressor::gain_computer(x, thr, ratio, knee);
                            if i == 0 { pb.move_to(at(x, y)) } else { pb.line_to(at(x, y)) }
                        }
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, line);
                        }
                    },
                )
                .size_full(),
            ),
        )
        .child(fluent::caption("Input → output", cx))
}

impl MainView {
    fn render_audition(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx).clone();
        let (tap, audition_device, audition_error) = {
            let s = self.state.read(cx);
            (s.engine.tap().clone(), self.status.audition_device.clone(), self.status.audition_error.clone())
        };
        let recording = tap.is_recording();
        let playing = tap.is_auditioning();
        let has = tap.has_recording();
        let secs = tap.recorded_seconds();
        let caption = if recording {
            format!("Recording… {secs:.1} s (up to {} s). Talk like you would in a call.", engine::tap::REC_SECONDS)
        } else if playing {
            format!(
                "Looping {secs:.1} s through the chain on {} · the virtual mic is silent meanwhile",
                audition_device.unwrap_or_else(|| "your headphones".into())
            )
        } else if let Some(e) = audition_error {
            format!("Couldn’t play: {e}")
        } else if has {
            format!("{secs:.1} s recorded. Play it, then tweak the chain while it loops.")
        } else {
            "Record a few seconds of your voice, then loop it through the chain while you tune it.".into()
        };

        let segment = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .when(active, |d| d.bg(p.accent).text_color(p.on_accent))
                .when(!active, |d| d.text_color(p.text).hover(|s| s.bg(p.subtle_hover)))
                .child(label)
        };

        fluent::card(cx)
            .px(px(16.))
            .py(px(12.))
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .size(px(36.))
                    .rounded(px(6.))
                    .bg(p.icon_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(p.accent_text)
                    .child(fluent::glyph(g::HEADPHONE, 18.)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(220.))
                    .child("Audition")
                    .child(div().text_xs().line_height(px(16.)).text_color(if recording { p.critical } else { p.text2 }).child(caption)),
            )
            .child(
                fluent::button("rec", cx)
                    .when(recording, |b| b.text_color(p.critical))
                    .child(fluent::glyph(if recording { g::STOP } else { g::RECORD }, 12.))
                    .child(if recording { "Stop" } else if has { "Re-record" } else { "Record" })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_record(cx))),
            )
            .when(has && !recording, |d| {
                d.child(
                    fluent::subtle_button("rec-save", cx)
                        .child(fluent::glyph(g::SAVE, 12.))
                        .child("Save")
                        .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new("Save the raw recording as a WAV file").build(w, cx))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save_recording(cx))),
                )
            })
            .child(
                fluent::subtle_button("rec-load", cx)
                    .child("Load…")
                    .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new("Audition a WAV file instead of a recording").build(w, cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.load_recording(window, cx))),
            )
            .child(
                fluent::button("play", cx)
                    .when(!has || recording, |b| b.opacity(0.5))
                    .child(fluent::glyph(if playing { g::STOP } else { g::PLAY }, 12.))
                    .child(if playing { "Stop" } else { "Play" })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if has && !recording {
                            this.set_audition(!playing, cx);
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(p.control_stroke)
                    .when(!playing, |d| d.opacity(0.5))
                    .child(segment("ab-a", "Processed", !self.ab_original).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_ab(false, cx))))
                    .child(
                        segment("ab-b", "Original", self.ab_original)
                            .tooltip(|w, cx| {
                                gpui_component::tooltip::Tooltip::new("The unprocessed recording, level-matched to the processed sound so loudness doesn’t bias the comparison").build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if playing {
                                    this.set_ab(true, cx);
                                }
                            })),
                    ),
            )
    }

    fn render_ir_picker(&self, slot_id: u32, path: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = fl(cx).clone();
        let (text, color) = if path.is_empty() {
            ("Choose a .wav impulse response — a room, a mic, a speaker cabinet.".to_string(), p.text2)
        } else {
            let name = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            match engine::dsp::convolver::kernel(path, engine::SAMPLE_RATE as f32) {
                Ok(k) => (
                    format!(
                        "{name} · {:.2} s · {} kHz {}",
                        k.seconds,
                        k.source_rate as f32 / 1000.0,
                        if k.channels == 1 { "mono" } else { "stereo, mixed to mono" }
                    ),
                    p.text2,
                ),
                Err(e) => (format!("{name}: {e}"), p.critical),
            }
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .mb(px(8.))
            .child(div().flex_1().min_w_0().text_xs().line_height(px(16.)).text_color(color).child(text))
            .child(
                fluent::button(("choose-ir", slot_id as usize), cx)
                    .child("Choose IR…")
                    .on_click(cx.listener(move |_, _: &ClickEvent, window, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("Use as impulse response".into()),
                        });
                        cx.spawn_in(window, async move |this, cx| {
                            if let Ok(Ok(Some(paths))) = rx.await {
                                if let Some(path) = paths.into_iter().next() {
                                    let _ = this.update(cx, |this, cx| {
                                        this.edit_chain(cx, |c| {
                                            if let Some(EffectSlot { effect: EffectConfig::Convolver(cv), .. }) =
                                                c.slots.iter_mut().find(|s| s.id == slot_id)
                                            {
                                                cv.ir_path = path.to_string_lossy().into_owned();
                                            }
                                        })
                                    });
                                }
                            }
                        })
                        .detach();
                    })),
            )
            .into_any_element()
    }

    fn render_calibration(&self, slot_id: u32, cx: &mut Context<Self>) -> AnyElement {
        let p = fl(cx).clone();
        let busy = self.calib.as_ref().filter(|c| c.slot == slot_id);
        let (text, color) = match (busy, &self.calib_result) {
            (Some(c), _) => {
                let left = c.until.saturating_duration_since(Instant::now()).as_secs_f32().ceil();
                if c.talking {
                    (format!("Now talk normally… {left:.0}"), p.accent_text)
                } else {
                    (format!("Stay quiet — measuring room noise… {left:.0}"), p.accent_text)
                }
            }
            (None, Some((id, msg, ok))) if *id == slot_id => (msg.clone(), if *ok { p.success } else { p.caution }),
            _ => ("Measure your room’s noise and your voice to set the threshold automatically.".to_string(), p.text2),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .mb(px(8.))
            .child(div().flex_1().min_w_0().text_xs().line_height(px(16.)).text_color(color).child(text))
            .child(
                fluent::button(("calibrate", slot_id as usize), cx)
                    .when(busy.is_some(), |b| b.opacity(0.5))
                    .child(if busy.is_some() { "Calibrating…" } else { "Auto-set threshold" })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if this.calib.is_none() {
                            this.start_calibration(slot_id, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    fn render_title_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx);
        let preview = self.state.read(cx).preview;
        div()
            .h(px(48.))
            .flex()
            .flex_row()
            .items_start()
            .flex_none()
            .child(
                div()
                    .id("title-drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pl(px(16.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(5.))
                            .bg(p.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(p.on_accent)
                            .child(fluent::glyph(g::MIC, 12.)),
                    )
                    .child(div().text_xs().text_color(p.text).child(if preview { "WinFX (preview)" } else { "WinFX" })),
            )
            .child(caption_button("cap-min", g::MIN, WindowControlArea::Min, false, cx))
            .child(caption_button(
                "cap-max",
                if window.is_maximized() { g::RESTORE } else { g::MAX },
                WindowControlArea::Max,
                false,
                cx,
            ))
            .child(caption_button("cap-close", g::CLOSE, WindowControlArea::Close, true, cx))
    }

    /// WinFX's own buffering right now: input period + ring target + output period + chain.
    fn latency_estimate_ms(&self, cx: &App) -> Option<f32> {
        let (i, o) = (self.status.input.as_ref()?, self.status.output.as_ref()?);
        let s = self.state.read(cx);
        let target = s.engine.meters().target_frames.load(Relaxed) as f32;
        Some((i.period_frames as f32 + target + o.period_frames as f32) / 48.0 + s.chain().latency_ms())
    }

    fn measured_latency_ms(&self) -> Option<f32> {
        match self.status.latency_test {
            engine::LatencyTest::Done { path_ms, chain_ms, .. } => Some(path_ms + chain_ms),
            _ => None,
        }
    }

    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let st = &self.status;
        let meters = self.state.read(cx).engine.meters().clone();
        let dropouts = meters.underruns.load(Relaxed) + meters.overflows.load(Relaxed);
        match st.state {
            RunState::Running => fluent::info_bar(
                if dropouts > 0 { Severity::Warning } else { Severity::Success },
                "Processing",
                format!(
                    "{} · {}",
                    match (self.measured_latency_ms(), self.latency_estimate_ms(cx)) {
                        (Some(m), _) => format!("{m:.0} ms measured latency"),
                        (None, Some(e)) => format!("~{e:.0} ms in WinFX"),
                        _ => format!("~{:.0} ms added latency", st.latency_ms),
                    },
                    match dropouts {
                        0 => "no dropouts".to_string(),
                        1 => "1 dropout".to_string(),
                        n => format!("{n} dropouts"),
                    }
                ),
                cx,
            )
            .into_any_element(),
            RunState::Starting => fluent::info_bar(Severity::Info, "Starting", "Opening audio devices…".into(), cx).into_any_element(),
            RunState::Retrying => fluent::info_bar(
                Severity::Error,
                "Not processing",
                st.last_error.clone().unwrap_or_else(|| "Retrying…".into()),
                cx,
            )
            .into_any_element(),
            RunState::Stopped => fluent::info_bar(Severity::Info, "Stopped", String::new(), cx).into_any_element(),
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.render_status(cx);
        let p = fl(cx).clone();
        let s = self.state.read(cx);
        let exclusive = s.config.engine.exclusive_input;
        let mon = s.config.engine.monitor.clone();
        let input_caption = match &self.status.input {
            Some(i) if i.exclusive => format!("Exclusive · {:.1} ms period", i.period_frames as f32 / 48.0),
            Some(i) => format!("Shared · {:.1} ms period", i.period_frames as f32 / 48.0),
            None => "Not connected".into(),
        };
        let header = |icon: AnyElement, title: &str, sub: String| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.))
                .child(icon)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(title.to_owned())
                        .child(div().text_xs().line_height(px(16.)).text_color(p.text2).child(sub)),
                )
        };
        let icon_glyph = |ch: char| fluent::glyph(ch, 20.).text_color(p.text).into_any_element();
        let plug = svg().path("winfx/plug.svg").size(px(20.)).flex_none().text_color(p.text).into_any_element();

        let mic_card = fluent::card(cx)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(header(icon_glyph(g::MIC), "Microphone", input_caption))
            .child(Select::new(&self.input_sel).placeholder("Choose microphone"))
            .child(fluent::level_meter("Input level", self.levels.in_peak, self.levels.in_rms, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("Exclusive mode")
                    .child(fluent::switch("exclusive", exclusive, "Lower latency; other apps can't open this mic directly", cx, {
                        let state = self.state.clone();
                        move |on, _, cx| state.update(cx, |s, _| s.set_engine_config(|c| c.exclusive_input = on))
                    })),
            );

        let exclusive_out = s.config.engine.exclusive_output;
        let output_caption = match &self.status.output {
            Some(o) if o.exclusive => format!("Exclusive · {:.1} ms period · pick “CABLE Output” in Discord", o.period_frames as f32 / 48.0),
            Some(o) => format!("Shared · {:.1} ms period · pick “CABLE Output” in Discord", o.period_frames as f32 / 48.0),
            None => "In Discord, pick “CABLE Output”".into(),
        };
        let measuring = self.status.latency_test == engine::LatencyTest::Running;
        let margin_ms = s.engine.meters().margin_frames.load(Relaxed) as f32 / 48.0;
        let (latency_line, latency_detail, latency_detail_color) = match &self.status.latency_test {
            engine::LatencyTest::Done { path_ms, min_ms, max_ms, chain_ms, trials } => (
                format!("Latency: {:.1} ms measured", path_ms + chain_ms),
                format!(
                    "Mic capture → CABLE Output, median of {trials} ({:.1}–{:.1} ms) · buffer margin {margin_ms:.1} ms",
                    min_ms + chain_ms,
                    max_ms + chain_ms
                ),
                p.text2,
            ),
            engine::LatencyTest::Failed(e) => ("Latency: couldn’t measure".to_string(), e.clone(), p.critical),
            _ => (
                match self.latency_estimate_ms(cx) {
                    Some(e) => format!("Latency: ~{e:.0} ms in WinFX"),
                    None => "Latency: —".into(),
                },
                format!("Buffer margin {margin_ms:.1} ms, adapts to dropouts · Measure for the full path"),
                p.text2,
            ),
        };
        let cable_card = fluent::card(cx)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(header(plug, "Virtual microphone", output_caption))
            .child(Select::new(&self.output_sel).placeholder("Choose output"))
            .child(fluent::level_meter("Output level", self.levels.out_peak, self.levels.out_rms, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("Exclusive mode")
                    .child(fluent::switch(
                        "exclusive-out",
                        exclusive_out,
                        "Lowest latency; only WinFX can play into the cable",
                        cx,
                        {
                            let state = self.state.clone();
                            move |on, _, cx| state.update(cx, |s, _| s.set_engine_config(|c| c.exclusive_output = on))
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(latency_line)
                            .child(div().text_xs().line_height(px(16.)).text_color(latency_detail_color).child(latency_detail)),
                    )
                    .child(
                        fluent::button("measure", cx)
                            .when(measuring, |b| b.opacity(0.5))
                            .child(if measuring { "Measuring…" } else { "Measure" })
                            .tooltip(|w, cx| {
                                gpui_component::tooltip::Tooltip::new(
                                    "Sends 5 short clicks through the virtual mic and times them. Anyone listening to CABLE Output will hear them.",
                                )
                                .build(w, cx)
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                if !measuring {
                                    this.state.read(cx).engine.measure_latency();
                                }
                            })),
                    ),
            );

        let mon_caption = match (&self.status.monitor, &self.status.monitor_error) {
            _ if !mon.enabled => "Hear yourself through headphones".to_string(),
            (Some(_), _) => format!("~{:.0} ms to headphones · processed", self.status.monitor_latency_ms),
            (None, Some(e)) => e.clone(),
            _ => "Starting…".into(),
        };
        let monitor_card = fluent::card(cx)
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                header(icon_glyph(g::HEADPHONE), "Monitor", mon_caption).child(fluent::switch(
                    "monitor",
                    mon.enabled,
                    "Hear the processed mic",
                    cx,
                    {
                        let state = self.state.clone();
                        move |on, _, cx| state.update(cx, |s, _| s.set_monitor(|m| m.enabled = on))
                    },
                )),
            )
            .when(mon.enabled, |d| {
                d.child(Select::new(&self.monitor_sel).placeholder("Headphones")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().text_xs().text_color(p.text2).child("Volume"))
                        .child(div().flex_1().child(Slider::new(&self.monitor_vol).bg(p.accent).text_color(p.accent)))
                        .child(
                            div()
                                .w(px(56.))
                                .text_xs()
                                .text_right()
                                .child(params::format(params::Unit::Db, mon.volume_db)),
                        ),
                )
            });

        let app_card = fluent::card(cx).px(px(16.)).child(
            div()
                .min_h(px(64.))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child("Start at login")
                        .child(fluent::caption("Opens in the tray and starts processing", cx)),
                )
                .child(fluent::switch("autostart", self.autostart, "Start WinFX when you sign in", cx, {
                    let view = cx.entity();
                    move |on, _, cx| {
                        view.update(cx, |this, cx| {
                            if system::set_autostart(on) {
                                this.autostart = on;
                            }
                            cx.notify();
                        })
                    }
                })),
        );

        div()
            .id("sidebar")
            .w(px(360.))
            .flex_none()
            .h_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pl(px(24.))
                    .pr(px(12.))
                    .pt(px(4.))
                    .pb(px(24.))
                    .child(status)
                    .child(fluent::section_title("Signal", cx))
                    .child(mic_card)
                    .child(cable_card)
                    .child(monitor_card)
                    .child(fluent::section_title("App", cx))
                    .child(app_card),
            )
            .overflow_y_scrollbar()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx).clone();
        let add_menu = self.add_open.then(|| {
            let items = params::ADDABLE.iter().enumerate().map(|(i, kind)| {
                let kind = *kind;
                let icon = params::new_effect(kind).map(|e| params::icon(&e)).unwrap_or("winfx/eq.svg");
                fluent::subtle_button(("add-item", i), cx)
                    .w_full()
                    .text_color(p.text)
                    .child(svg().path(icon).size(px(16.)).text_color(p.accent_text))
                    .child(kind)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.add_open = false;
                        this.add_effect(kind, window, cx);
                    }))
            });
            popover(cx)
                .w(px(220.))
                .children(items)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popovers(cx)))
        });
        let save_as = self.save_as_open.then(|| {
            popover(cx)
                .w(px(280.))
                .p(px(12.))
                .gap(px(8.))
                .child(div().text_xs().text_color(p.text2).child("Save the current chain as a new preset"))
                .child(Input::new(&self.preset_name))
                .child(
                    div().flex().justify_end().child(
                        fluent::accent_button("save-as-confirm", cx)
                            .child("Save")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.save_preset_as(window, cx))),
                    ),
                )
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_popovers(cx)))
        });

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .min_h(px(40.))
            .mb(px(4.))
            .child(
                div()
                    .flex_1()
                    .font_family(fluent::FONT_DISPLAY)
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Effects"),
            )
            .child(div().w(px(200.)).child(Select::new(&self.preset_sel).placeholder("Preset")))
            .child(
                fluent::button("save", cx)
                    .child(fluent::glyph(g::SAVE, 14.))
                    .child("Save")
                    .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new("Overwrite the selected preset").build(w, cx))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.overwrite_preset(cx))),
            )
            .child(
                fluent::button("import", cx)
                    .child(fluent::glyph(g::IMPORT, 14.))
                    .child("Import…")
                    .tooltip(|w, cx| {
                        gpui_component::tooltip::Tooltip::new(
                            "Load an EasyEffects preset (.json) or an Equalizer APO / AutoEQ file (.txt). You can also drop files on the window.",
                        )
                        .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.pick_import(window, cx))),
            )
            .child(
                div()
                    .relative()
                    .child(fluent::button("save-as", cx).child("Save as…").on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            if !this.just_closed() {
                                this.save_as_open = true;
                                this.add_open = false;
                            }
                            cx.notify();
                        },
                    )))
                    .children(save_as.map(|m| anchor_below(m))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        fluent::accent_button("add-effect", cx)
                            .child(fluent::glyph(g::ADD, 14.))
                            .child("Add effect")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                if !this.just_closed() {
                                    this.add_open = true;
                                    this.save_as_open = false;
                                }
                                cx.notify();
                            })),
                    )
                    .children(add_menu.map(|m| anchor_below(m))),
            )
    }

    fn render_notices(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![];
        if let Some((sev, title, body)) = &self.import_notice {
            out.push(
                fluent::info_bar(*sev, title, body.clone(), cx)
                    .child(
                        fluent::subtle_button("dismiss-import", cx)
                            .w(px(32.))
                            .justify_center()
                            .child(fluent::glyph(g::CANCEL, 10.))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.import_notice = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        let s = self.state.read(cx);
        if let Some(why) = self.status.input.as_ref().and_then(|i| i.exclusive_fallback.as_ref()) {
            out.push(
                fluent::info_bar(
                    Severity::Warning,
                    "Mic is shared.",
                    format!("Exclusive mode failed ({why}), so latency is higher."),
                    cx,
                )
                .into_any_element(),
            );
        }
        if let Some(c) = &s.comms_input {
            let dismissed = s.config.dismissed_comms_warning.as_deref() == Some(c.name.as_str());
            if !is_cable(c) && !dismissed {
                let name = c.name.clone();
                let p = fl(cx).clone();
                out.push(
                    fluent::info_bar(
                        Severity::Info,
                        "Some apps use the communications mic.",
                        format!("Windows’ default communications device is “{}”. Select CABLE Output in those apps, or change it in Sound settings.", c.name),
                        cx,
                    )
                    .child(
                        fluent::subtle_button("open-sound", cx)
                            .text_color(p.accent_text)
                            .child("Sound settings")
                            .on_click(|_, _, cx| cx.open_url("ms-settings:sound")),
                    )
                    .child(
                        fluent::subtle_button("dismiss-comms", cx)
                            .w(px(32.))
                            .justify_center()
                            .child(fluent::glyph(g::CANCEL, 10.))
                            .tooltip(|w, cx| gpui_component::tooltip::Tooltip::new("Dismiss").build(w, cx))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let name = name.clone();
                                this.state.update(cx, |s, _| {
                                    s.config.dismissed_comms_warning = Some(name);
                                    s.save();
                                });
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
                );
            }
        }
        out
    }

    fn render_eq(&self, slot: &EffectSlot, chain: &ChainConfig, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx).clone();
        let EffectConfig::Eq(eq) = &slot.effect else { return div().into_any_element() };
        let sr = engine::SAMPLE_RATE as f32;
        const N: usize = 240;
        let freq = |i: usize| 20.0 * 1000f32.powf(i as f32 / (N - 1) as f32);
        let curve: Vec<f32> = (0..N)
            .map(|i| {
                chain
                    .slots
                    .iter()
                    .filter(|s| !s.bypass)
                    .filter_map(|s| match &s.effect {
                        // Filter shape only: the EQ's output gain has its own slider.
                        EffectConfig::Eq(eq) => Some(eq.bands.iter().map(|b| b.response_db(freq(i), sr)).sum::<f32>()),
                        f @ EffectConfig::Filter(_) => f.response_db(freq(i), sr),
                        _ => None,
                    })
                    .sum::<f32>()
            })
            .collect();
        let (grid, zero, line, fill_c, bg) = (p.grid, p.zero, p.accent, p.accent_fill, p.graph_bg);
        let spectrum = self.analyzer.db.clone();
        let spec_fill = p.text3.opacity(0.14);
        let spec_line = p.text3.opacity(0.35);
        let bounds_cell = self.eq.bounds.clone();
        let handles = self.eq_handles(cx);
        let sel = self.eq.band;

        let graph = div()
            .id("eq-graph")
            .relative()
            .w_full()
            .h(px(EQ_H))
            .rounded(px(4.))
            .border_1()
            .border_color(p.stroke)
            .overflow_hidden()
            .cursor_crosshair()
            .child(
                canvas(
                    move |b, _, _| bounds_cell.set(b),
                    move |b, (), window, _| {
                        window.paint_quad(fill(b, bg));
                        let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
                        for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
                            let x = b.origin.x + px(eq_x(f) * w);
                            window.paint_quad(fill(Bounds { origin: point(x, b.origin.y), size: size(px(1.), b.size.height) }, grid));
                        }
                        for d in [-6.0, 0.0, 6.0] {
                            let y = b.origin.y + px(eq_y(d, h));
                            window.paint_quad(fill(
                                Bounds { origin: point(b.origin.x, y), size: size(b.size.width, px(1.)) },
                                if d == 0.0 { zero } else { grid },
                            ));
                        }
                        // Spectrum: −96…0 dBFS over the full height, behind everything else.
                        let spec_y = |db: f32| b.origin.y + px(h * (db / crate::spectrum::FLOOR_DB).clamp(0.0, 1.0));
                        let col_x = |i: usize| b.origin.x + px(i as f32 / (crate::spectrum::COLUMNS - 1) as f32 * w);
                        let mut sp = PathBuilder::fill();
                        sp.move_to(point(b.origin.x, b.origin.y + b.size.height));
                        for (i, db) in spectrum.iter().enumerate() {
                            sp.line_to(point(col_x(i), spec_y(*db)));
                        }
                        sp.line_to(point(b.origin.x + b.size.width, b.origin.y + b.size.height));
                        sp.close();
                        if let Ok(path) = sp.build() {
                            window.paint_path(path, spec_fill);
                        }
                        let mut sl = PathBuilder::stroke(px(1.));
                        for (i, db) in spectrum.iter().enumerate() {
                            if i == 0 { sl.move_to(point(col_x(i), spec_y(*db))) } else { sl.line_to(point(col_x(i), spec_y(*db))) }
                        }
                        if let Ok(path) = sl.build() {
                            window.paint_path(path, spec_line);
                        }
                        let pt = |i: usize, db: f32| point(b.origin.x + px(i as f32 / (N - 1) as f32 * w), b.origin.y + px(eq_y(db, h)));
                        let mut area = PathBuilder::fill();
                        area.move_to(point(b.origin.x, b.origin.y + px(h / 2.0)));
                        for (i, db) in curve.iter().enumerate() {
                            area.line_to(pt(i, *db));
                        }
                        area.line_to(point(b.origin.x + b.size.width, b.origin.y + px(h / 2.0)));
                        area.close();
                        if let Ok(path) = area.build() {
                            window.paint_path(path, fill_c);
                        }
                        let mut stroke = PathBuilder::stroke(px(2.));
                        for (i, db) in curve.iter().enumerate() {
                            if i == 0 { stroke.move_to(pt(i, *db)) } else { stroke.line_to(pt(i, *db)) }
                        }
                        if let Ok(path) = stroke.build() {
                            window.paint_path(path, line);
                        }
                    },
                )
                .size_full(),
            )
            .children(handles.iter().enumerate().map(|(i, (f, gdb, enabled))| {
                let selected = i == sel;
                let d = if selected { 18.0 } else { 14.0 };
                div()
                    .opacity(if *enabled { 1.0 } else { 0.4 })
                    .absolute()
                    .left(relative(eq_x(*f)))
                    .top(px(eq_y(*gdb, EQ_H) - d / 2.0))
                    .ml(px(-d / 2.0))
                    .size(px(d))
                    .rounded_full()
                    .border_2()
                    .border_color(p.accent)
                    .bg(if selected { p.accent } else { p.flyout })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .line_height(px(10.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(p.on_accent)
                    .when(selected, |d| d.child((i + 1).to_string()))
            }))
            .children([(6.0, "+6"), (0.0, "0 dB"), (-6.0, "−6")].map(|(db, t)| {
                div().absolute().left(px(6.)).top(px(eq_y(db, EQ_H) - 14.)).text_size(px(10.)).line_height(px(12.)).text_color(p.text3).child(t)
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.eq_mouse_down(ev.position, window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| this.eq_mouse_move(ev, window, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                this.eq.drag = None;
                cx.notify();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.eq.drag = None))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| this.eq_scroll(ev, window, cx)));

        let axis = div()
            .relative()
            .h(px(12.))
            .text_size(px(10.))
            .line_height(px(12.))
            .text_color(p.text3)
            .children([(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")].map(|(f, t)| {
                div().absolute().top_0().left(relative(eq_x(f))).ml(px(-8.)).child(t)
            }));

        let cols = eq.bands.len().clamp(1, 8) as u16;
        let tiles = div()
            .grid()
            .grid_cols(cols)
            .gap(px(8.))
            .children(eq.bands.iter().enumerate().map(|(i, b)| {
                let selected = i == sel;
                let row = |k: &str, v: String| {
                    div().flex().justify_between().text_color(p.text2).child(k.to_owned()).child(div().text_color(p.text).child(v))
                };
                let gain = if b.kind.has_gain() { params::format(params::Unit::Db, b.gain_db) } else { "—".to_string() };
                div()
                    .id(("band", i))
                    .min_w_0()
                    .when(!b.enabled, |d| d.opacity(0.55))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if selected { p.accent } else { p.stroke })
                    .bg(if selected { p.accent_fill } else { p.control })
                    .hover(|s| s.border_color(p.accent))
                    .cursor_pointer()
                    .text_xs()
                    .line_height(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .size(px(16.))
                                    .rounded_full()
                                    .bg(if selected { p.accent } else { p.meter_track })
                                    .text_color(if selected { p.on_accent } else { p.text })
                                    .text_size(px(10.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child((i + 1).to_string()),
                            )
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(if b.enabled { params::band_name(b) } else { "Off" }),
                            ),
                    )
                    .child(row("Freq", params::format(params::Unit::Hz, b.freq_hz)))
                    .child(row("Gain", gain))
                    .child(row("Q", format!("{:.2}", b.q)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.select_band(i, window, cx)))
            }));

        let band_on = eq.bands.get(sel).is_some_and(|b| b.enabled);
        let band_bar = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(12.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!("Band {}", sel + 1)))
            .children(self.eq.type_sel.as_ref().map(|s| div().w(px(160.)).child(Select::new(s))))
            .child(fluent::switch("band-on", band_on, if band_on { "Turn this band off" } else { "Turn this band on" }, cx, {
                let view = cx.entity();
                move |on, _, cx| {
                    view.update(cx, |this, cx| {
                        this.eq_edit_band(cx, |eq, band| {
                            if let Some(b) = eq.bands.get_mut(band) {
                                b.enabled = on;
                            }
                        })
                    })
                }
            }))
            .child(div().flex_1())
            .child(
                fluent::button("band-add", cx)
                    .child(fluent::glyph(g::ADD, 12.))
                    .child("Add band")
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.eq_add_band(window, cx))),
            )
            .when(eq.bands.len() > 1, |d| {
                d.child(
                    fluent::subtle_button("band-remove", cx)
                        .text_color(p.critical)
                        .child("Remove band")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.eq_remove_band(window, cx))),
                )
            });

        let editor = div()
            .grid()
            .grid_cols(4)
            .gap_x(px(32.))
            .children(self.eq.ctls.iter().map(|c| slider_row(c, params::get(&slot.effect, c.spec.key), cx)));

        let output = self.eq.gain_ctl.as_ref().map(|c| {
            div()
                .grid()
                .grid_cols(4)
                .gap_x(px(32.))
                .child(slider_row(c, params::get(&slot.effect, c.spec.key), cx))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(graph)
            .child(axis)
            .child(tiles)
            .child(band_bar)
            .child(editor)
            .children(output)
            .child(fluent::caption("Drag the points to change frequency and gain; scroll over the graph to change Q.", cx))
            .into_any_element()
    }

    fn render_card(&self, ix: usize, card: &Card, slot: &EffectSlot, chain: &ChainConfig, cx: &mut Context<Self>) -> AnyElement {
        let p = fl(cx).clone();
        let id = card.id;
        let title: SharedString = params::title(&slot.effect).into();
        let on = !slot.bypass;
        let gr = self.levels.gr.get(ix).copied().unwrap_or(0.0);
        let summary = if on { params::summary(&slot.effect) } else { "Bypassed".into() };
        let dim = if on { 1.0 } else { 0.5 };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .min_h(px(64.))
            .pl(px(4.))
            .pr(px(8.))
            .child(
                div()
                    .id(("grip", id as usize))
                    .w(px(20.))
                    .h(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_grab()
                    .child(svg().path("winfx/grip.svg").w(px(10.)).h(px(16.)).text_color(p.text3))
                    .on_drag(DragSlot { id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone())),
            )
            .child(
                div()
                    .size(px(36.))
                    .rounded(px(6.))
                    .bg(p.icon_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_none()
                    .opacity(dim)
                    .child(svg().path(params::icon(&slot.effect)).size(px(20.)).text_color(p.accent_text)),
            )
            .child(
                div()
                    .id(("title", id as usize))
                    .flex_1()
                    .min_w_0()
                    .h(px(64.))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .cursor_pointer()
                    .child(div().opacity(dim).child(title.clone()))
                    .child(div().text_xs().line_height(px(16.)).text_color(p.text2).child(summary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(c) = this.cards.iter_mut().find(|c| c.id == id) {
                            c.expanded = !c.expanded;
                        }
                        cx.notify();
                    })),
            )
            .when(matches!(slot.effect, EffectConfig::Autotune(_)) && on, |d| {
                let hz = self.levels.aux.get(ix).copied().unwrap_or(0.0);
                d.child(
                    div()
                        .w(px(132.))
                        .flex_none()
                        .text_xs()
                        .text_right()
                        .text_color(p.text2)
                        .child(if hz > 0.0 { format!("hearing {}", params::note_name(hz)) } else { "listening…".into() }),
                )
            })
            .when(matches!(slot.effect, EffectConfig::Autogain(_)) && on, |d| {
                let g = self.levels.aux.get(ix).copied().unwrap_or(0.0);
                d.child(
                    div()
                        .w(px(132.))
                        .flex_none()
                        .text_xs()
                        .text_right()
                        .text_color(p.text2)
                        .child(format!("now {}", params::format(params::Unit::Db, g))),
                )
            })
            .when(params::has_gr(&slot.effect) && on, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .w(px(132.))
                        .flex_none()
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .h(px(4.))
                                .rounded_full()
                                .overflow_hidden()
                                .bg(p.meter_track)
                                .child(div().absolute().top_0().right_0().h_full().w(relative((gr / 24.0).clamp(0.0, 1.0))).bg(p.caution)),
                        )
                        .child(
                            div()
                                .w(px(52.))
                                .text_xs()
                                .text_right()
                                .text_color(p.text2)
                                .child(format!("−{gr:.1} dB")),
                        ),
                )
            })
            .child(div().w(px(24.)).text_xs().text_right().text_color(p.text2).child(if on { "On" } else { "Off" }))
            .child(fluent::switch(("on", id as usize), on, if on { "Bypass" } else { "Enable" }, cx, {
                let view = cx.entity();
                move |on, _, cx| {
                    view.update(cx, |this, cx| {
                        this.edit_chain(cx, |c| {
                            if let Some(s) = c.slots.iter_mut().find(|s| s.id == id) {
                                s.bypass = !on;
                            }
                        })
                    })
                }
            }))
            .child(
                fluent::subtle_button(("expand", id as usize), cx)
                    .w(px(32.))
                    .justify_center()
                    .child(fluent::glyph(if card.expanded { g::CHEVRON_UP } else { g::CHEVRON_DOWN }, 12.))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(c) = this.cards.iter_mut().find(|c| c.id == id) {
                            c.expanded = !c.expanded;
                        }
                        cx.notify();
                    })),
            );

        let body: Option<AnyElement> = card.expanded.then(|| {
            let gate_tools = match &slot.effect {
                EffectConfig::Gate(_) => Some(self.render_calibration(id, cx)),
                EffectConfig::Convolver(c) => Some(self.render_ir_picker(id, &c.ir_path, cx)),
                _ => None,
            };
            let content = match &slot.effect {
                EffectConfig::Eq(_) => self.render_eq(slot, chain, cx).into_any_element(),
                other => div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(24.))
                    .when_some(
                        match other {
                            EffectConfig::Compressor(c) => Some(c.clone()),
                            _ => None,
                        },
                        |d, c| d.child(transfer_curve(&c, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.))
                            .grid()
                            .grid_cols(2)
                            .gap_x(px(32.))
                            .gap_y(px(14.))
                            .children(card.params.iter().map(|c| slider_row(c, params::get(&slot.effect, c.spec.key), cx))),
                    )
                    .into_any_element(),
            };
            div()
                .border_t_1()
                .border_color(p.divider)
                .px(px(20.))
                .pt(px(16.))
                .pb(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .children(gate_tools)
                .child(content)
                .child(
                    div().flex().justify_end().child(
                        fluent::subtle_button(("remove", id as usize), cx)
                            .text_color(p.critical)
                            .child(fluent::glyph(g::CANCEL, 10.))
                            .child("Remove")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.edit_chain(cx, |c| c.slots.retain(|s| s.id != id));
                                this.rebuild_cards(window, cx);
                            })),
                    ),
                )
                .into_any_element()
        });

        div()
            .id(("card", id as usize))
            .bg(p.card)
            .border_1()
            .border_color(p.stroke)
            .rounded(px(8.))
            .overflow_hidden()
            .drag_over::<DragSlot>(move |s, _, _, _| s.border_color(p.accent))
            .on_drop(cx.listener(move |this, d: &DragSlot, _, cx| this.move_slot(d.id, id, cx)))
            .child(header)
            .children(body)
            .into_any_element()
    }
}

fn popover(cx: &App) -> gpui::Div {
    let p = fl(cx);
    div()
        .flex()
        .flex_col()
        .p(px(4.))
        .bg(p.flyout)
        .border_1()
        .border_color(p.control_stroke)
        .rounded(px(8.))
        .shadow_lg()
        .font_family(fluent::FONT)
        .text_size(px(14.))
        .text_color(p.text)
}

fn anchor_below(menu: gpui::Div) -> impl IntoElement {
    div()
        .absolute()
        .top(px(36.))
        .right_0()
        .child(deferred(anchored().anchor(Anchor::TopRight).snap_to_window_with_margin(px(8.)).child(menu.occlude())).with_priority(1))
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = fl(cx).clone();
        let chain = self.state.read(cx).chain().clone();
        let title_bar = self.render_title_bar(window, cx).into_any_element();
        let sidebar = self.render_sidebar(cx).into_any_element();
        let header = self.render_header(cx).into_any_element();
        let notices = self.render_notices(cx);
        let audition = self.render_audition(cx).into_any_element();
        let cards: Vec<AnyElement> = self
            .cards
            .iter()
            .enumerate()
            .filter_map(|(ix, card)| {
                let slot = chain.slots.iter().find(|s| s.id == card.id)?;
                Some(self.render_card(ix, card, slot, &chain, cx))
            })
            .collect();
        let empty = cards.is_empty();

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .font_family(fluent::FONT)
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(p.text)
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(p.accent_fill))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                if let Some(path) = paths.paths().first() {
                    this.import_path(&path.clone(), window, cx);
                }
            }))
            .child(title_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(sidebar)
                    .child(
                        div().flex_1().min_w_0().h_full().child(
                            div()
                                .id("chain")
                                .size_full()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.))
                                        .pl(px(12.))
                                        .pr(px(24.))
                                        .pt(px(4.))
                                        .pb(px(24.))
                                        .child(header)
                                        .children(notices)
                                        .child(audition)
                                        .children(cards)
                                        .when(empty, |d| {
                                            d.child(
                                                fluent::card(cx)
                                                    .p(px(24.))
                                                    .text_color(p.text2)
                                                    .child("No effects yet. Use Add effect to build a chain."),
                                            )
                                        }),
                                )
                                .overflow_y_scrollbar(),
                        ),
                    ),
            )
    }
}
