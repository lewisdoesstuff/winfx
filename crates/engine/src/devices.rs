//! Endpoint enumeration. Devices are identified by their WASAPI endpoint ID,
//! e.g. `{0.0.1.00000000}.{df79ae04-a1cc-4d1e-bc33-5b7ee59011b1}`.

use crate::Result;
pub use wasapi::Role;
use wasapi::{DeviceEnumerator, DeviceState, Direction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Input,
    Output,
}

impl Dir {
    fn wasapi(self) -> Direction {
        match self {
            Dir::Input => Direction::Capture,
            Dir::Output => Direction::Render,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub dir: Dir,
    pub active: bool,
}

/// Make sure COM is up on this thread. Harmless if it already is (in any mode).
pub(crate) fn ensure_com() {
    let _ = wasapi::initialize_mta();
}

pub fn list(dir: Dir) -> Result<Vec<DeviceInfo>> {
    ensure_com();
    let en = DeviceEnumerator::new()?;
    let mut out = Vec::new();
    for dev in &en.get_device_collection(dir.wasapi())? {
        let dev = dev?;
        out.push(DeviceInfo {
            id: dev.get_id()?,
            name: dev.get_friendlyname().unwrap_or_else(|_| "(unnamed)".into()),
            dir,
            active: matches!(dev.get_state(), Ok(DeviceState::Active)),
        });
    }
    Ok(out)
}

pub fn default_device(dir: Dir, role: Role) -> Option<DeviceInfo> {
    ensure_com();
    let en = DeviceEnumerator::new().ok()?;
    let dev = en.get_default_device_for_role(dir.wasapi(), role).ok()?;
    Some(DeviceInfo { id: dev.get_id().ok()?, name: dev.get_friendlyname().unwrap_or_default(), dir, active: true })
}

/// Case-insensitive match on endpoint ID or name; exact matches win over substrings.
pub fn find(dir: Dir, query: &str) -> Result<Option<DeviceInfo>> {
    let q = query.to_lowercase();
    let devs: Vec<_> = list(dir)?.into_iter().filter(|d| d.active).collect();
    let exact = devs.iter().find(|d| d.id.to_lowercase() == q || d.name.to_lowercase() == q).cloned();
    Ok(exact
        .or_else(|| devs.into_iter().find(|d| d.id.to_lowercase().contains(&q) || d.name.to_lowercase().contains(&q))))
}

/// VB-Audio's playback endpoint ("CABLE Input (VB-Audio Virtual Cable)"),
/// not the 16-channel variant.
pub fn find_cable_input() -> Result<Option<DeviceInfo>> {
    Ok(list(Dir::Output)?.into_iter().find(|d| d.active && d.name.starts_with("CABLE Input")))
}
