//! Windows integration: start-at-login (HKCU Run key), single instance, tray icon.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};
use windows::core::{w, PCWSTR};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("WinFX");

/// Returns false if another instance already holds the mutex.
/// The handle is leaked on purpose: it must live as long as the process.
pub fn claim_single_instance(name: &str) -> bool {
    let wide: Vec<u16> = format!("Local\\{name}").encode_utf16().chain(Some(0)).collect();
    let h: windows::core::Result<HANDLE> = unsafe { CreateMutexW(None, true, PCWSTR(wide.as_ptr())) };
    h.is_ok() && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS
}

pub fn autostart_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size))
            .is_ok()
    }
}

pub fn set_autostart(on: bool) -> bool {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            let cmd = format!("\"{}\" --hidden", exe.display());
            let wide: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            )
            .is_ok()
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE).is_ok()
        }
    }
}

const SHOW_EVENT: PCWSTR = w!("Local\\WinFX-show");

/// Auto-reset event a second launch sets to ask this instance to show its window.
pub struct ShowEvent(HANDLE);

impl ShowEvent {
    pub fn take(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

pub fn create_show_event() -> Option<ShowEvent> {
    unsafe { CreateEventW(None, false, false, SHOW_EVENT).ok().map(ShowEvent) }
}

pub fn signal_show_existing() {
    unsafe {
        if let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, SHOW_EVENT) {
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
        }
    }
}

pub enum TrayAction {
    Show,
    ToggleBypass,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    show: MenuId,
    bypass: MenuId,
    quit: MenuId,
}

impl Tray {
    pub fn new() -> anyhow::Result<Tray> {
        let show = MenuItem::new("Show WinFX", true, None);
        let bypass = MenuItem::new("Bypass all effects", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &bypass, &PredefinedMenuItem::separator(), &quit])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("WinFX")
            .with_icon(make_icon())
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Tray { _icon: icon, show: show.id().clone(), bypass: bypass.id().clone(), quit: quit.id().clone() })
    }

    /// Drain pending tray/menu events (call periodically on the UI thread).
    pub fn poll(&self) -> Vec<TrayAction> {
        let mut out = vec![];
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                out.push(TrayAction::Show);
            }
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.show {
                out.push(TrayAction::Show);
            } else if ev.id == self.bypass {
                out.push(TrayAction::ToggleBypass);
            } else if ev.id == self.quit {
                out.push(TrayAction::Quit);
            }
        }
        out
    }
}

/// 32×32 icon drawn in code: a rounded mic capsule on a teal disc.
fn make_icon() -> Icon {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5 - 16.0, y as f32 + 0.5 - 16.0);
            let disc = 15.5 - (fx * fx + fy * fy).sqrt();
            // capsule: |x| ≤ 4, y in [-9, 3] with round ends
            let cy = fy.clamp(-6.0, 0.0);
            let cap = 4.5 - (fx * fx + (fy - cy) * (fy - cy)).sqrt();
            let stand = (fx.abs() <= 1.0 && (5.0..=9.0).contains(&fy)) || (fx.abs() <= 5.0 && (9.0..=10.5).contains(&fy));
            let i = (y * N + x) * 4;
            if disc > 0.0 {
                let a = (disc.min(1.0) * 255.0) as u8;
                let white = cap > 0.0 || stand;
                let (r, g, b) = if white { (245, 250, 250) } else { (20, 150, 140) };
                rgba[i..i + 4].copy_from_slice(&[r, g, b, a]);
            }
        }
    }
    Icon::from_rgba(rgba, N as u32, N as u32).expect("valid icon")
}

// ---------- direct mode setup (elevated) ----------

/// Where the elevated helper writes its outcome for the unelevated app.
fn admin_result_path() -> std::path::PathBuf {
    engine::apo::data_dir().join("admin-result.txt")
}

/// `winfx.exe --apo-setup {endpoint-guid}...` / `--apo-remove`, run elevated by
/// `run_elevated`. Returns the exit code, or None for a normal launch.
pub fn run_admin_command() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--apo-setup") => {
            let dll = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("winfx_apo.dll")));
            match dll {
                Some(dll) if dll.exists() => engine::apo::setup(&dll, &args[1..]),
                _ => Err("winfx_apo.dll is missing next to winfx.exe".into()),
            }
        }
        Some("--apo-remove") => engine::apo::remove(),
        _ => return None,
    };
    let _ = engine::apo::ensure_data_dir();
    let text = match &result {
        Ok(m) => format!("ok\n{m}"),
        Err(e) => format!("error\n{e}"),
    };
    let _ = std::fs::write(admin_result_path(), text);
    Some(if result.is_ok() { 0 } else { 1 })
}

/// Run this exe elevated with `args` (one UAC prompt), wait, and return what it
/// reported. Blocks: call from a background task.
pub fn run_elevated(args: &str) -> Result<String, String> {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_w: Vec<u16> = exe.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let args_w: Vec<u16> = args.encode_utf16().chain(Some(0)).collect();
    let _ = std::fs::remove_file(admin_result_path());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            "Setup was cancelled.".into()
        } else {
            format!("Couldn't start setup: {}", e.message())
        });
    }
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    let text = std::fs::read_to_string(admin_result_path()).unwrap_or_default();
    let (status, msg) = text.split_once('\n').unwrap_or(("", ""));
    match (code, status) {
        (0, "ok") => Ok(msg.to_string()),
        _ if !msg.is_empty() => Err(msg.to_string()),
        _ => Err(format!("Setup failed (exit code {code}).")),
    }
}
