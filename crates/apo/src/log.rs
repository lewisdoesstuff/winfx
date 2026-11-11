//! Append-only diagnostic log. Never call from APOProcess.

use std::io::Write;

const PATH: &str = r"C:\ProgramData\WinFX-dev\apo-logs\apo.log";

pub fn line(msg: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let pid = std::process::id();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(PATH) {
        let _ = writeln!(f, "{}.{:03} pid {pid} {msg}", now.as_secs(), now.subsec_millis());
    }
}
