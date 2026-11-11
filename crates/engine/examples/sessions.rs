//! List apps with audio sessions on each endpoint (who's using the cable?).
use wasapi::{DeviceEnumerator, Direction};

fn main() {
    let _ = wasapi::initialize_mta();
    let en = DeviceEnumerator::new().unwrap();
    for dir in [Direction::Capture, Direction::Render] {
        for dev in &en.get_device_collection(dir).unwrap() {
            let dev = dev.unwrap();
            let name = dev.get_friendlyname().unwrap_or_default();
            let Ok(manager) = dev.get_iaudiosessionmanager() else { continue };
            let Ok(sessions) = manager.get_audiosessionenumerator() else { continue };
            let mut lines = vec![];
            for i in 0..sessions.get_count().unwrap_or(0) {
                let Ok(s) = sessions.get_session(i) else { continue };
                let pid = s.get_process_id().unwrap_or(0);
                let state = s.get_state().map(|s| format!("{s:?}")).unwrap_or_default();
                let exe = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).ProcessName")])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                    .unwrap_or_default();
                if pid != 0 {
                    lines.push(format!("    {state:<10} pid {pid:<6} {exe}"));
                }
            }
            if !lines.is_empty() {
                println!("{dir:?}  {name}");
                lines.iter().for_each(|l| println!("{l}"));
            }
        }
    }
}
