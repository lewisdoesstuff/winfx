//! Append-only diagnostic log (C:\ProgramData\WinFX\apo.log). Never call from
//! APOProcess. Each line goes out in one write so concurrent writers don't interleave.

use std::io::Write;

pub fn line(msg: &str) {
    let path = engine::apo::data_dir().join("apo.log");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let pid = std::process::id();
    if cfg!(test) {
        return;
    }
    let line = format!("{}.{:03} pid {pid} {msg}
", now.as_secs(), now.subsec_millis());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}
