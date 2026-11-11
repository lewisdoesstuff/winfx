#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod params;
mod state;
mod system;
mod ui;

use gpui::{
    px, size, App, AppContext as _, Bounds, Entity, Global, QuitMode, TitlebarOptions,
    WindowBounds, WindowHandle, WindowOptions,
};
use gpui_component::{Root, Theme, ThemeMode};
use state::AppState;
use std::time::Duration;
use system::{Tray, TrayAction};

struct AppGlobal {
    state: Entity<AppState>,
    tray: Option<Tray>,
    window: Option<WindowHandle<Root>>,
}

impl Global for AppGlobal {}

fn main() {
    if !system::claim_single_instance() {
        system::signal_show_existing();
        return;
    }
    let hidden = std::env::args().any(|a| a == "--hidden");

    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);

            let state = cx.new(|_| AppState::load());
            let tray = match Tray::new() {
                Ok(t) => Some(t),
                Err(e) => {
                    eprintln!("tray icon unavailable: {e}");
                    None
                }
            };
            let no_tray = tray.is_none();
            cx.set_global(AppGlobal { state, tray, window: None });

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
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(780.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("WinFX".into()), ..Default::default() }),
        window_min_size: Some(size(px(860.), px(520.))),
        ..Default::default()
    };
    let handle = cx.open_window(opts, |window, cx| {
        let view = cx.new(|cx| ui::MainView::new(state, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    match handle {
        Ok(h) => {
            cx.global_mut::<AppGlobal>().window = Some(h);
            cx.activate(true);
        }
        Err(e) => eprintln!("failed to open window: {e}"),
    }
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
    state.update(cx, |s, _| s.save());
    cx.quit();
}
