#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod fluent;
mod params;
mod spectrum;
mod state;
mod system;
mod ui;

use gpui::{
    point, px, size, App, AppContext as _, Bounds, Entity, Global, QuitMode, TitlebarOptions,
    Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::Root;
use state::AppState;
use std::time::Duration;
use system::{Tray, TrayAction};

struct AppGlobal {
    state: Entity<AppState>,
    preview: bool,
    tray: Option<Tray>,
    window: Option<WindowHandle<Root>>,
}

impl Global for AppGlobal {}

fn main() {
    // Elevated helper runs (direct mode setup): no window, no single instance.
    if let Some(code) = system::run_admin_command() {
        std::process::exit(code);
    }
    // --preview: synthetic engine, no tray, nothing saved, opened off-screen
    // without focus. For developing the UI while the real app holds the mic.
    let preview = std::env::args().any(|a| a == "--preview");
    if !system::claim_single_instance(if preview { "WinFX-preview" } else { "WinFX-single-instance" }) {
        if !preview {
            system::signal_show_existing();
        }
        return;
    }
    let hidden = std::env::args().any(|a| a == "--hidden");

    gpui_platform::application()
        .with_assets(assets::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            fluent::sync_app(cx);

            let state = cx.new(|_| AppState::load(preview));
            let tray = if preview {
                None
            } else {
                match Tray::new() {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("tray icon unavailable: {e}");
                        None
                    }
                }
            };
            let no_tray = tray.is_none();
            cx.set_global(AppGlobal { state, preview, tray, window: None });

            cx.on_window_closed(|cx, id| {
                let g = cx.global_mut::<AppGlobal>();
                if g.window.is_some_and(|h| h.window_id() == id) {
                    g.window = None;
                }
                // Without a tray there'd be no way back in, so quit.
                if cx.global::<AppGlobal>().tray.is_none() {
                    quit(cx);
                }
            })
            .detach();

            if !hidden || no_tray {
                open_main_window(cx);
            }

            let show_event = system::create_show_event();
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let mut actions = cx.update(|cx| {
                    cx.global::<AppGlobal>().tray.as_ref().map(|t| t.poll()).unwrap_or_default()
                });
                if show_event.as_ref().is_some_and(|e| e.take()) {
                    actions.push(TrayAction::Show);
                }
                for a in actions {
                    cx.update(|cx| match a {
                        TrayAction::Show => open_main_window(cx),
                        TrayAction::ToggleBypass => toggle_bypass_all(cx),
                        TrayAction::Quit => quit(cx),
                    });
                }
            })
            .detach();
        });
}

fn open_main_window(cx: &mut App) {
    if let Some(h) = cx.global::<AppGlobal>().window {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let state = cx.global::<AppGlobal>().state.clone();
    let preview = cx.global::<AppGlobal>().preview;
    // Dev aid: WINFX_OFFSCREEN=1 opens the real app off-screen without focus, for screenshots.
    let offscreen = preview || std::env::var_os("WINFX_OFFSCREEN").is_some();
    let saved = state.read(cx).config.window;
    let bounds = if offscreen {
        // Off-screen so screenshots (PrintWindow) don't cover the user's work.
        WindowBounds::Windowed(Bounds { origin: point(px(-4000.), px(80.)), size: size(px(1280.), px(860.)) })
    } else {
        restore_bounds(saved, cx)
    };
    let opts = WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(TitlebarOptions { title: Some("WinFX".into()), appears_transparent: true, ..Default::default() }),
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        window_min_size: Some(size(px(960.), px(600.))),
        focus: !offscreen,
        ..Default::default()
    };
    let handle = cx.open_window(opts, |window, cx| {
        fluent::sync(window, cx);
        let st = state.clone();
        window.on_window_should_close(cx, move |window, cx| {
            remember_bounds(&st, window, cx);
            true
        });
        let view = cx.new(|cx| ui::MainView::new(state, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    match handle {
        Ok(h) => {
            cx.global_mut::<AppGlobal>().window = Some(h);
            if !offscreen {
                cx.activate(true);
            }
        }
        Err(e) => eprintln!("failed to open window: {e}"),
    }
}

/// Saved position if it's still on a connected display, else centred.
fn restore_bounds(saved: Option<state::SavedWindow>, cx: &App) -> WindowBounds {
    let default = Bounds::centered(None, size(px(1180.), px(780.)), cx);
    let Some(s) = saved else { return WindowBounds::Windowed(default) };
    let b = Bounds { origin: point(px(s.x), px(s.y)), size: size(px(s.w.max(860.)), px(s.h.max(520.))) };
    if !cx.displays().iter().any(|d| d.bounds().intersects(&b)) {
        return WindowBounds::Windowed(default);
    }
    if s.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }
}

fn remember_bounds(state: &Entity<AppState>, window: &Window, cx: &mut App) {
    if state.read(cx).preview {
        return;
    }
    let wb = window.window_bounds();
    let b = wb.get_bounds();
    let saved = state::SavedWindow {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        w: f32::from(b.size.width),
        h: f32::from(b.size.height),
        maximized: matches!(wb, WindowBounds::Maximized(_)),
    };
    state.update(cx, |s, _| {
        s.config.window = Some(saved);
        s.save();
    });
}

fn toggle_bypass_all(cx: &mut App) {
    let state = cx.global::<AppGlobal>().state.clone();
    state.update(cx, |s, _| {
        let mut chain = s.chain().clone();
        let any_active = chain.slots.iter().any(|s| !s.bypass);
        for slot in &mut chain.slots {
            slot.bypass = any_active;
        }
        s.set_chain(chain);
        s.save();
    });
    cx.refresh_windows();
}

fn quit(cx: &mut App) {
    let state = cx.global::<AppGlobal>().state.clone();
    if let Some(h) = cx.global::<AppGlobal>().window {
        let st = state.clone();
        let _ = h.update(cx, |_, window, cx| remember_bounds(&st, window, cx));
    }
    state.update(cx, |s, _| s.save());
    cx.quit();
}
