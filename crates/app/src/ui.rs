//! Main window.

use crate::params::{self, ParamSpec};
use crate::state::{is_cable, AppState};
use crate::system;
use engine::chain::{EffectConfig, EffectSlot};
use engine::dsp::lin_to_db;
use engine::meters::MAX_SLOTS;
use engine::{ChainConfig, DeviceSel, RunState};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    canvas, div, fill, point, px, relative, size, AnyElement, App, AppContext as _, Bounds,
    ClickEvent, Context, Entity, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    PathBuilder, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Task, Window,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_component::switch::Switch;
use gpui_component::{h_flex, v_flex, ActiveTheme as _, IconName, IndexPath, Sizable as _};
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

type NameSelect = SelectState<Vec<SharedString>>;

pub struct MainView {
    state: Entity<AppState>,
    input_sel: Entity<NameSelect>,
    output_sel: Entity<NameSelect>,
    preset_sel: Entity<NameSelect>,
    add_sel: Entity<NameSelect>,
    preset_name: Entity<InputState>,
    cards: Vec<Card>,
    levels: Levels,
    status: engine::Status,
    autostart: bool,
    device_snapshot: (Vec<String>, Vec<String>),
    last_device_poll: Instant,
    _ticker: Task<()>,
    _subs: Vec<Subscription>,
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

/// Display-side meter state (dB), with peak hold/decay.
struct Levels {
    in_peak: f32,
    in_rms: f32,
    out_peak: f32,
    out_rms: f32,
    gr: [f32; MAX_SLOTS],
    last: Instant,
}

impl Default for Levels {
    fn default() -> Self {
        Levels { in_peak: -90.0, in_rms: -90.0, out_peak: -90.0, out_rms: -90.0, gr: [0.0; MAX_SLOTS], last: Instant::now() }
    }
}

const FLOOR_DB: f32 = -60.0;

#[derive(Clone)]
struct DragSlot {
    id: u32,
    title: SharedString,
}

impl Render for DragSlot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
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

        let input_sel = cx.new(|cx| SelectState::new(names(&inputs), input_row, window, cx));
        let output_sel = cx.new(|cx| SelectState::new(names(&outputs), output_row, window, cx));
        let preset_sel = cx.new(|cx| SelectState::new(presets, preset_row, window, cx));
        let add_items: Vec<SharedString> = params::ADDABLE.iter().map(|s| (*s).into()).collect();
        let add_sel = cx.new(|cx| SelectState::new(add_items, None, window, cx));
        let preset_name = cx.new(|cx| InputState::new(window, cx).placeholder("New preset name"));

        let subs = vec![
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
            cx.subscribe_in(&preset_sel, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(Some(name)) = ev else { return };
                let name = name.to_string();
                if this.state.update(cx, |s, _| s.load_preset(&name)) {
                    this.rebuild_cards(window, cx);
                }
            }),
            cx.subscribe_in(&add_sel, window, |this, sel, ev: &SelectEvent<Vec<SharedString>>, window, cx| {
                let SelectEvent::Confirm(Some(kind)) = ev else { return };
                if let Some(effect) = params::new_effect(kind) {
                    this.edit_chain(cx, |c| {
                        let id = c.next_id();
                        c.slots.push(EffectSlot { id, bypass: false, effect });
                    });
                    this.rebuild_cards(window, cx);
                    if let Some(card) = this.cards.last_mut() {
                        card.expanded = true;
                    }
                }
                sel.update(cx, |s, cx| s.set_selected_index(None, window, cx));
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
            preset_sel,
            add_sel,
            preset_name,
            cards: vec![],
            levels: Levels::default(),
            status: engine::Status::default(),
            autostart: system::autostart_enabled(),
            device_snapshot,
            last_device_poll: Instant::now(),
            _ticker: ticker,
            _subs: subs,
        };
        view.rebuild_cards(window, cx);
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
        self.cards = chain
            .slots
            .iter()
            .map(|slot| {
                let expanded = old.iter().find(|(id, _)| *id == slot.id).map_or(
                    // Debug aid for screenshots: WINFX_EXPAND_ALL=1 opens every card.
                    std::env::var_os("WINFX_EXPAND_ALL").is_some(),
                    |(_, e)| *e,
                );
                let params = params::specs(&slot.effect)
                    .into_iter()
                    .map(|spec| self.param_ctl(slot, spec, window, cx))
                    .collect();
                Card { id: slot.id, expanded, params }
            })
            .collect();
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

    fn move_slot(&mut self, from_id: u32, to_id: u32, cx: &mut Context<Self>) {
        if from_id == to_id {
            return;
        }
        self.edit_chain(cx, |c| {
            let Some(from) = c.slots.iter().position(|s| s.id == from_id) else { return };
            let slot = c.slots.remove(from);
            let to = c.slots.iter().position(|s| s.id == to_id).unwrap_or(c.slots.len());
            // Dropping onto a card below inserts after it; above inserts before it.
            let to = if from <= to { to + 1 } else { to };
            c.slots.insert(to.min(c.slots.len()), slot);
        });
        let order: Vec<u32> = self.state.read(cx).chain().slots.iter().map(|s| s.id).collect();
        self.cards.sort_by_key(|c| order.iter().position(|id| *id == c.id));
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (status, meters) = {
            let s = self.state.read(cx);
            (s.engine.status(), s.engine.meters().clone())
        };
        let now = Instant::now();
        let dt = now.duration_since(self.levels.last).as_secs_f32();
        self.levels.last = now;
        let fall = 24.0 * dt; // dB/s decay for peaks
        let db = |x: f32| lin_to_db(x).max(-90.0);
        let l = &mut self.levels;
        l.in_peak = db(meters.in_peak.take()).max(l.in_peak - fall);
        l.out_peak = db(meters.out_peak.take()).max(l.out_peak - fall);
        let smooth = |old: f32, new: f32| if new > old { new } else { old + (new - old) * (dt * 8.0).min(1.0) };
        l.in_rms = smooth(l.in_rms, db(meters.in_rms.take()));
        l.out_rms = smooth(l.out_rms, db(meters.out_rms.take()));
        for (i, g) in l.gr.iter_mut().enumerate() {
            let v = meters.gr[i].take();
            *g = if v > *g { v } else { *g + (v - *g) * (dt * 6.0).min(1.0) };
        }
        self.status = status;

        self.state.update(cx, |s, _| s.save_if_dirty());

        if self.last_device_poll.elapsed() > Duration::from_secs(3) {
            self.last_device_poll = Instant::now();
            self.poll_devices(window, cx);
        }
        cx.notify();
    }

    /// Pick up hot-plugged / re-created endpoints.
    fn poll_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (inputs, outputs, ein, eout) = self.state.update(cx, |s, _| {
            s.refresh_devices();
            (s.inputs.clone(), s.outputs.clone(), s.config.engine.input.clone(), s.config.engine.output.clone())
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
    }

    fn save_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    }

    fn overwrite_preset(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| {
            if let Some(name) = s.config.preset.clone() {
                s.save_preset(&name);
            }
        });
    }
}

// ---------- rendering ----------

fn level_color(db: f32, cx: &App) -> Hsla {
    let t = cx.theme();
    if db > -3.0 {
        t.danger
    } else if db > -12.0 {
        t.warning
    } else {
        t.success
    }
}

fn frac(db: f32) -> f32 {
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

fn level_meter(label: &str, peak: f32, rms: f32, cx: &App) -> impl IntoElement {
    let t = cx.theme();
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .text_xs()
                .text_color(t.muted_foreground)
                .child(label.to_owned())
                .child(if peak <= -89.0 { "−∞".to_string() } else { format!("{peak:.1} dB") }),
        )
        .child(
            div()
                .relative()
                .h(px(10.))
                .w_full()
                .rounded_sm()
                .overflow_hidden()
                .bg(t.muted)
                .child(div().absolute().top_0().left_0().h_full().w(relative(frac(rms))).bg(level_color(rms, cx)))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .w(px(2.))
                        .left(relative(frac(peak)))
                        .bg(level_color(peak, cx)),
                ),
        )
}

fn gr_meter(gr: f32, cx: &App) -> impl IntoElement {
    let t = cx.theme();
    h_flex()
        .gap_2()
        .w(px(150.))
        .child(
            div()
                .relative()
                .flex_1()
                .h(px(6.))
                .rounded_sm()
                .overflow_hidden()
                .bg(t.muted)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .h_full()
                        .w(relative((gr / 24.0).clamp(0.0, 1.0)))
                        .bg(t.warning),
                ),
        )
        .child(div().w(px(52.)).text_xs().text_color(t.muted_foreground).child(format!("−{gr:.1} dB")))
}

/// Combined response of all active filter/EQ stages, log-frequency.
fn eq_curve(chain: &ChainConfig, cx: &App) -> impl IntoElement {
    const N: usize = 240;
    const RANGE: f32 = 18.0;
    let sr = engine::SAMPLE_RATE as f32;
    let freq = |i: usize| 20.0 * 1000f32.powf(i as f32 / (N - 1) as f32);
    let curve: Vec<f32> = (0..N)
        .map(|i| {
            chain
                .slots
                .iter()
                .filter(|s| !s.bypass && matches!(s.effect, EffectConfig::Filter(_) | EffectConfig::Eq(_)))
                .filter_map(|s| s.effect.response_db(freq(i), sr))
                .sum::<f32>()
        })
        .collect();
    let t = cx.theme();
    let grid = t.foreground.opacity(0.08);
    let zero = t.foreground.opacity(0.3);
    let line = gpui::hsla(0.47, 0.65, 0.5, 1.0);
    let bg = t.background;

    canvas(
        |_, _, _| {},
        move |b, (), window, _| {
            window.paint_quad(fill(b, bg));
            let x_of = |f: f32| b.origin.x + b.size.width * ((f / 20.0).log10() / 3.0);
            let y_of = |db: f32| b.origin.y + b.size.height * (0.5 - db / (2.0 * RANGE));
            for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
                let x = x_of(f);
                window.paint_quad(fill(Bounds { origin: point(x, b.origin.y), size: size(px(1.), b.size.height) }, grid));
            }
            for d in [-12.0, -6.0, 0.0, 6.0, 12.0] {
                let y = y_of(d);
                let c = if d == 0.0 { zero } else { grid };
                window.paint_quad(fill(Bounds { origin: point(b.origin.x, y), size: size(b.size.width, px(1.)) }, c));
            }
            let mut pb = PathBuilder::stroke(px(2.));
            for (i, db) in curve.iter().enumerate() {
                let p = point(
                    b.origin.x + b.size.width * (i as f32 / (N - 1) as f32),
                    y_of(db.clamp(-RANGE, RANGE)),
                );
                if i == 0 { pb.move_to(p) } else { pb.line_to(p) }
            }
            if let Ok(path) = pb.build() {
                window.paint_path(path, line);
            }
        },
    )
    .w_full()
    .h(px(150.))
}

/// The combined curve goes on the EQ card, or on the first filter if there's no EQ.
fn shows_curve(chain: &ChainConfig, slot: &EffectSlot) -> bool {
    let first = chain
        .slots
        .iter()
        .find(|s| matches!(s.effect, EffectConfig::Eq(_)))
        .or_else(|| chain.slots.iter().find(|s| matches!(s.effect, EffectConfig::Filter(_))));
    first.is_some_and(|f| f.id == slot.id)
}

fn eq_axis(cx: &App) -> impl IntoElement {
    let t = cx.theme();
    let x = |f: f32| (f / 20.0).log10() / 3.0;
    let ticks = [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")];
    div()
        .relative()
        .w_full()
        .h(px(14.))
        .text_xs()
        .text_color(t.muted_foreground)
        .children(ticks.into_iter().map(move |(f, label)| {
            div().absolute().top_0().left(relative(x(f))).ml(px(-8.)).child(label)
        }))
        .child(div().absolute().top_0().right_0().child("±18 dB"))
}

impl MainView {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let st = &self.status;
        let (dot, label) = match st.state {
            RunState::Running => (t.success, "Running".to_string()),
            RunState::Starting => (t.muted_foreground, "Starting…".into()),
            RunState::Retrying => (t.danger, "Not running".into()),
            RunState::Stopped => (t.muted_foreground, "Stopped".into()),
        };
        let meters = self.state.read(cx).engine.meters().clone();
        let dropouts = meters.underruns.load(Relaxed) + meters.overflows.load(Relaxed);
        h_flex()
            .px_4()
            .py_2()
            .gap_4()
            .border_b_1()
            .border_color(t.border)
            .child(div().text_lg().font_weight(gpui::FontWeight::SEMIBOLD).child("WinFX"))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().size(px(9.)).rounded_full().bg(dot))
                    .child(label),
            )
            .when(st.state == RunState::Running, |d| {
                d.child(div().text_color(t.muted_foreground).child(format!("~{:.0} ms latency", st.latency_ms)))
                    .child(
                        div()
                            .text_color(if dropouts > 0 { t.warning } else { t.muted_foreground })
                            .child(format!("{dropouts} dropouts")),
                    )
            })
            .child(div().flex_1())
            .child(div().w(px(200.)).child(Select::new(&self.preset_sel).placeholder("Preset").small()))
            .child(
                Button::new("overwrite")
                    .label("Save")
                    .small()
                    .tooltip("Overwrite the selected preset with the current chain")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.overwrite_preset(cx))),
            )
            .child(div().w(px(170.)).child(Input::new(&self.preset_name).small()))
            .child(
                Button::new("save-as")
                    .label("Save as")
                    .small()
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.save_preset(window, cx))),
            )
    }

    fn warnings(&self, cx: &mut Context<Self>) -> Vec<String> {
        let s = self.state.read(cx);
        let mut w = vec![];
        if self.status.state == RunState::Retrying {
            if let Some(e) = &self.status.last_error {
                w.push(format!("Audio isn't running: {e}. Retrying…"));
            }
        }
        if let Some(why) = self.status.input.as_ref().and_then(|i| i.exclusive_fallback.as_ref()) {
            w.push(format!("Mic opened in shared mode (higher latency) because exclusive mode failed: {why}"));
        }
        if let Some(c) = &s.comms_input {
            if !is_cable(c) {
                w.push(format!(
                    "Windows' default communications mic is \"{}\". Apps that follow it get that device, not the processed mic — select CABLE Output in those apps, or make CABLE Output the default communications device in Sound settings.",
                    c.name
                ));
            }
        }
        w
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let s = self.state.read(cx);
        let exclusive = s.config.engine.exclusive_input;
        let info = |si: &Option<engine::StreamInfo>| {
            si.as_ref()
                .map(|i| {
                    format!(
                        "{} ch · {:.1} ms period{}",
                        i.channels,
                        i.period_frames as f32 / 48.0,
                        if i.exclusive { " · exclusive" } else { "" }
                    )
                })
                .unwrap_or_default()
        };
        let section = |title: &str| div().text_xs().text_color(t.muted_foreground).child(title.to_uppercase());
        v_flex()
            .w(px(300.))
            .flex_none()
            .p_4()
            .gap_3()
            .border_r_1()
            .border_color(t.border)
            .child(section("Input (microphone)"))
            .child(Select::new(&self.input_sel).placeholder("Choose mic"))
            .child(div().text_xs().text_color(t.muted_foreground).child(info(&self.status.input)))
            .child(
                Switch::new("exclusive")
                    .checked(exclusive)
                    .label("Exclusive mode (lower latency)")
                    .small()
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        let on = *on;
                        this.state.update(cx, |s, _| s.set_engine_config(|c| c.exclusive_input = on));
                        cx.notify();
                    })),
            )
            .child(level_meter("Input", self.levels.in_peak, self.levels.in_rms, cx))
            .child(div().h(px(8.)))
            .child(section("Output (virtual mic)"))
            .child(Select::new(&self.output_sel).placeholder("Choose output"))
            .child(div().text_xs().text_color(t.muted_foreground).child(info(&self.status.output)))
            .child(level_meter("Output", self.levels.out_peak, self.levels.out_rms, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .child("In Discord and other apps, select \"CABLE Output\" as the microphone."),
            )
            .child(div().flex_1())
            .child(section("App"))
            .child(
                Switch::new("autostart")
                    .checked(self.autostart)
                    .label("Start at login (in tray)")
                    .small()
                    .on_change(cx.listener(|this, on: &bool, _, cx| {
                        if system::set_autostart(*on) {
                            this.autostart = *on;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.muted_foreground)
                    .child("Closing this window keeps processing running in the tray."),
            )
    }

    fn render_card(&self, ix: usize, card: &Card, slot: &EffectSlot, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme();
        let id = card.id;
        let title: SharedString = params::title(&slot.effect).into();
        let gr = self.levels.gr.get(ix).copied().unwrap_or(0.0);
        let chain = self.state.read(cx).chain().clone();

        let header = h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                div()
                    .id(("grip", id as usize))
                    .cursor_grab()
                    .text_color(t.muted_foreground)
                    .child(IconName::EllipsisVertical)
                    .on_drag(DragSlot { id, title: title.clone() }, |d, _, _, cx| cx.new(|_| d.clone())),
            )
            .child(
                Button::new(("expand", id as usize))
                    .icon(if card.expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(c) = this.cards.iter_mut().find(|c| c.id == id) {
                            c.expanded = !c.expanded;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .when(slot.bypass, |d| d.text_color(t.muted_foreground))
                    .child(title.clone()),
            )
            .child(div().flex_1())
            .when(params::has_gr(&slot.effect) && !slot.bypass, |d| d.child(gr_meter(gr, cx)))
            .child(
                Switch::new(("on", id as usize))
                    .checked(!slot.bypass)
                    .small()
                    .tooltip("Enable / bypass")
                    .on_change(cx.listener(move |this, on: &bool, _, cx| {
                        let on = *on;
                        this.edit_chain(cx, |c| {
                            if let Some(s) = c.slots.iter_mut().find(|s| s.id == id) {
                                s.bypass = !on;
                            }
                        });
                    })),
            )
            .child(
                Button::new(("remove", id as usize))
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Remove")
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.edit_chain(cx, |c| c.slots.retain(|s| s.id != id));
                        this.rebuild_cards(window, cx);
                    })),
            );

        let body = card.expanded.then(|| {
            v_flex()
                .px_4()
                .pb_3()
                .gap_2()
                .when(shows_curve(&chain, slot), |d| d.child(eq_curve(&chain, cx)).child(eq_axis(cx)))
                .children(card.params.iter().map(|p| {
                    let v = params::get(&slot.effect, p.spec.key);
                    h_flex()
                        .gap_3()
                        .child(div().w(px(130.)).text_sm().text_color(t.muted_foreground).child(p.spec.label.clone()))
                        .child(div().flex_1().child(Slider::new(&p.slider)))
                        .child(div().w(px(80.)).text_sm().text_right().child(params::format(p.spec.unit, v)))
                }))
        });

        div()
            .id(("card", id as usize))
            .rounded_md()
            .border_1()
            .border_color(t.border)
            .bg(t.secondary)
            .drag_over::<DragSlot>(|s, _, _, cx| s.border_color(cx.theme().primary))
            .on_drop(cx.listener(move |this, d: &DragSlot, _, cx| this.move_slot(d.id, id, cx)))
            .child(header)
            .children(body)
            .into_any_element()
    }
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let chain = self.state.read(cx).chain().clone();
        let warnings = self.warnings(cx);
        let cards: Vec<AnyElement> = self
            .cards
            .iter()
            .enumerate()
            .filter_map(|(ix, card)| {
                let slot = chain.slots.iter().find(|s| s.id == card.id)?;
                Some(self.render_card(ix, card, slot, cx))
            })
            .collect();

        v_flex()
            .size_full()
            .bg(t.background)
            .text_color(t.foreground)
            .text_sm()
            .child(self.render_header(cx))
            .children(warnings.into_iter().map(|w| {
                h_flex()
                    .px_4()
                    .py_2()
                    .gap_2()
                    .bg(t.warning.opacity(0.12))
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().flex_none().text_color(t.warning).child(IconName::TriangleAlert))
                    .child(div().flex_1().min_w_0().whitespace_normal().child(w))
            }))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_start()
                    .child(self.render_sidebar(cx))
                    .child(
                        div().flex_1().h_full().min_w_0().child(
                            v_flex()
                                .id("chain")
                                .size_full()
                                .p_4()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(t.muted_foreground)
                                                .child("EFFECTS CHAIN — drag ⋮ to reorder, top runs first"),
                                        )
                                        .child(
                                            div()
                                                .w(px(180.))
                                                .child(Select::new(&self.add_sel).placeholder("+ Add effect").small()),
                                        ),
                                )
                                .children(cards)
                                .overflow_y_scrollbar(),
                        ),
                    ),
            )
    }
}
