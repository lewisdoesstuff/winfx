//! A small app log for bug reports: %LOCALAPPDATA%\winfx\winfx.log, rotated
//! to winfx.log.1 at 1 MB. Engine state changes, errors and panics.

use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 1 << 20;

pub fn path() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("winfx").join("winfx.log")
}

pub fn line(msg: &str) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {msg}\n",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Log panics (then let the default hook print them too).
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        line(&format!("panic: {info}"));
        default(info);
    }));
}
