//! Direct mode plumbing: find, check and attach the WinFX APO (the effects
//! plug-in that runs inside the Windows audio engine) on capture endpoints.
//!
//! Unelevated: everything here except `setup`/`remove`, which need admin
//! (COM registration, the DLL under Program Files, restarting the audio
//! service). Windows lets normal users write an endpoint's FxProperties, but
//! the audio service caches them, so attaching to an endpoint it has already
//! used only takes effect after a restart.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_SUCCESS, PROPERTYKEY};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_READ, KEY_SET_VALUE, REG_SZ, RRF_RT_ANY, RegCloseKey, RegEnumKeyExW,
    RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{GUID, PCWSTR};

pub const CLSID_SFX: GUID = GUID::from_u128(0x77069dd4_922e_4a92_8b31_ea606fedce4b);
pub const CLSID_MFX: GUID = GUID::from_u128(0xb1392a38_ebf1_43a8_b2bb_0ec0e7e87d37);

const CAPTURE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture";
const SFX_VALUE: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5";
const DISABLE_SYSFX: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e), pid: 5 };

pub fn clsid_string(g: &GUID) -> String {
    format!("{{{g:?}}}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The `{guid}` part of an MMDevice id like `{0.0.1.00000000}.{guid}`.
pub fn endpoint_guid(device_id: &str) -> Option<&str> {
    let g = device_id.rsplit('.').next()?;
    (g.len() == 38 && g.starts_with('{') && g.ends_with('}')).then_some(g)
}

fn open(path: &str, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<HKEY> {
    let mut k = HKEY::default();
    let p = wide(path);
    unsafe { (RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(p.as_ptr()), None, access, &mut k) == ERROR_SUCCESS).then_some(k) }
}

/// Read a value as a string (REG_SZ, or REG_BINARY holding a serialized
/// VT_LPWSTR/VT_BSTR PROPVARIANT, as IPolicyConfig writes), or a DWORD as text.
fn get(path: &str, name: &str) -> Option<String> {
    let (p, n) = (wide(path), wide(name));
    let mut buf = vec![0u8; 4096];
    let mut len = buf.len() as u32;
    let mut ty = windows::Win32::System::Registry::REG_VALUE_TYPE::default();
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(p.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_ANY,
            Some(&mut ty),
            Some(buf.as_mut_ptr() as _),
            Some(&mut len),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    buf.truncate(len as usize);
    let utf16 = |b: &[u8]| {
        let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        String::from_utf16_lossy(&w)
    };
    match ty.0 {
        1 | 2 => Some(utf16(&buf)),                                           // REG_SZ / REG_EXPAND_SZ
        4 if buf.len() >= 4 => Some(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]).to_string()), // REG_DWORD
        3 if buf.len() > 8 => Some(utf16(&buf[8..])),                          // serialized PROPVARIANT
        _ => None,
    }
}

fn subkeys(path: &str) -> Vec<String> {
    let Some(k) = open(path, KEY_READ) else { return vec![] };
    let mut out = vec![];
    let mut i = 0;
    loop {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let r = unsafe { RegEnumKeyExW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if r != ERROR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..len as usize]));
        i += 1;
    }
    unsafe {
        let _ = RegCloseKey(k);
    }
    out
}

/// The APO's COM class is registered (setup has run).
pub fn installed() -> bool {
    get(&format!(r"SOFTWARE\Classes\CLSID\{}\InprocServer32", clsid_string(&CLSID_SFX)), "").is_some_and(|p| Path::new(&p).exists())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    /// `{guid}`
    pub guid: String,
    /// "Mic | Line | Instrument 1"
    pub name: String,
    /// "Audient EVO4"
    pub device: String,
    /// 1 = active; other values: disabled / not present / unplugged.
    pub state: u32,
    pub attached: bool,
}

fn endpoint(guid: &str) -> Option<Endpoint> {
    let base = format!(r"{CAPTURE}\{guid}");
    let props = format!(r"{base}\Properties");
    Some(Endpoint {
        guid: guid.to_string(),
        name: get(&props, "{a45c254e-df1c-4efd-8020-67d146a850e0},2")?,
        device: get(&props, "{b3f8fa53-0004-438e-9003-51a46e139bfc},6").unwrap_or_default(),
        state: get(&base, "DeviceState").and_then(|s| s.parse().ok()).unwrap_or(0),
        attached: get(&format!(r"{base}\FxProperties"), SFX_VALUE)
            .is_some_and(|v| v.eq_ignore_ascii_case(&clsid_string(&CLSID_SFX))),
    })
}

/// Every capture endpoint (any state) for the same input as `guid`: same
/// device and endpoint name. Some drivers (Audient's, for one) create a new
/// set of endpoints per sample rate, so this covers the other rates too.
pub fn siblings(guid: &str) -> Vec<Endpoint> {
    let Some(me) = endpoint(guid) else { return vec![] };
    subkeys(CAPTURE)
        .iter()
        .filter_map(|g| endpoint(g))
        .filter(|e| e.name == me.name && e.device == me.device)
        .collect()
}

pub fn attached(guid: &str) -> bool {
    endpoint(guid).is_some_and(|e| e.attached)
}

/// "Audio enhancements" are off for the endpoint, so Windows skips its APOs.
pub fn enhancements_off(guid: &str) -> bool {
    get(&format!(r"{CAPTURE}\{guid}\FxProperties"), "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5").is_some_and(|v| v == "1")
}

/// Turn audio enhancements on or off. Unelevated and applied live: goes
/// through the audio service, as the Sound settings page does.
pub fn set_enhancements(device_id: &str, on: bool) -> windows::core::Result<()> {
    policy::set_fx_u32(device_id, &DISABLE_SYSFX, if on { 0 } else { 1 })
}

/// Point an endpoint's stream-effect slot at the APO, unelevated. Takes effect
/// without a restart only for endpoints the audio service hasn't loaded yet.
pub fn attach_unelevated(guid: &str) -> windows::core::Result<()> {
    let key = format!(r"{CAPTURE}\{guid}\FxProperties");
    let Some(k) = open(&key, KEY_SET_VALUE | KEY_QUERY_VALUE) else {
        return Err(windows::Win32::Foundation::E_ACCESSDENIED.into());
    };
    let v = wide(&clsid_string(&CLSID_SFX));
    let n = wide(SFX_VALUE);
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
    let r = unsafe { RegSetValueExW(k, PCWSTR(n.as_ptr()), None, REG_SZ, Some(bytes)) };
    unsafe {
        let _ = RegCloseKey(k);
    }
    r.ok()
}

/// Where setup puts the DLL.
pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("WinFX")
}

/// Shared data folder (shared memory file, APO log). Must allow LOCAL SERVICE
/// (the audio engine) and the user to read and write; `ensure_data_dir` sets that.
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("WinFX")
}

/// Create the data folder if needed and grant LOCAL SERVICE and Users modify
/// access. Works unelevated: the creator owns the folder and may set its DACL.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    if !dir.join(".acl").exists() {
        std::fs::create_dir_all(&dir)?;
        run_hidden("icacls", &[dir.to_str().unwrap_or_default(), "/grant", "*S-1-5-19:(OI)(CI)M", "*S-1-5-32-545:(OI)(CI)M"])?;
        std::fs::write(dir.join(".acl"), b"")?;
    }
    Ok(dir)
}

fn run_hidden(exe: &str, args: &[&str]) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let st = std::process::Command::new(exe).args(args).creation_flags(CREATE_NO_WINDOW).status()?;
    if st.success() { Ok(()) } else { Err(std::io::Error::other(format!("{exe} failed ({st})"))) }
}

/// Admin only. Install `dll` under Program Files, register it, attach it to
/// every endpoint in `endpoints` (and their sibling endpoints), restart audio.
pub fn setup(dll: &Path, endpoints: &[String]) -> Result<String, String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE as HKLM, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCreateKeyExW};
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let target = dir.join("winfx_apo.dll");
    // The audio engine may have the old DLL loaded: stop it first.
    stop_audio()?;
    let copied = std::fs::copy(dll, &target).map_err(|e| format!("copy DLL: {e}"));
    let mut log = vec![];
    let result = (|| -> Result<(), String> {
        copied?;
        let create = |path: &str| -> Result<HKEY, String> {
            let mut k = HKEY::default();
            let p = wide(path);
            unsafe { RegCreateKeyExW(HKLM, PCWSTR(p.as_ptr()), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut k, None) }
                .ok()
                .map_err(|e| format!("{path}: {e}"))?;
            Ok(k)
        };
        let set_sz = |k: HKEY, name: &str, val: &str| {
            let (n, v) = (wide(name), wide(val));
            let b = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
            unsafe { RegSetValueExW(k, if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) }, None, REG_SZ, Some(b)) }
        };
        let set_dw = |k: HKEY, name: &str, val: u32| {
            let n = wide(name);
            unsafe { RegSetValueExW(k, PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&val.to_le_bytes())) }
        };
        for (clsid, slot) in [(CLSID_SFX, "SFX"), (CLSID_MFX, "MFX")] {
            let c = clsid_string(&clsid);
            let name = format!("WinFX capture effects ({slot})");
            let k = create(&format!(r"SOFTWARE\Classes\CLSID\{c}"))?;
            let _ = set_sz(k, "", &name);
            unsafe { let _ = RegCloseKey(k); }
            let k = create(&format!(r"SOFTWARE\Classes\CLSID\{c}\InprocServer32"))?;
            let _ = set_sz(k, "", target.to_str().unwrap_or_default());
            let _ = set_sz(k, "ThreadingModel", "Both");
            unsafe { let _ = RegCloseKey(k); }
            let k = create(&format!(r"SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{c}"))?;
            let _ = set_sz(k, "FriendlyName", &name);
            let _ = set_sz(k, "Copyright", "WinFX");
            for (n, v) in [
                ("MajorVersion", 1),
                ("MinorVersion", 0),
                ("Flags", 14),
                ("MinInputConnections", 1),
                ("MaxInputConnections", 1),
                ("MinOutputConnections", 1),
                ("MaxOutputConnections", 1),
                ("MaxInstances", u32::MAX),
                ("NumAPOInterfaces", 1),
            ] {
                let _ = set_dw(k, n, v);
            }
            let _ = set_sz(k, "APOInterface0", "{FD7F2B29-24D0-4B5C-B177-592C39F9CA10}");
            unsafe { let _ = RegCloseKey(k); }
        }
        let mut all: Vec<Endpoint> = vec![];
        for e in endpoints {
            for s in siblings(e) {
                if !all.iter().any(|a| a.guid == s.guid) {
                    all.push(s);
                }
            }
        }
        for e in &all {
            backup_fx(&e.guid);
            attach_unelevated(&e.guid).map_err(|err| format!("attach {}: {err}", e.guid))?;
            log.push(format!("{} ({})", e.name, e.guid));
        }
        Ok(())
    })();
    let started = start_audio();
    result?;
    started?;
    let _ = ensure_data_dir();
    Ok(format!("attached to {}", log.join(", ")))
}

/// Admin only. Restore every endpoint the APO is attached to and unregister it.
pub fn remove() -> Result<String, String> {
    let mut n = 0;
    for g in subkeys(CAPTURE) {
        if endpoint(&g).is_some_and(|e| e.attached) {
            restore_fx(&g);
            n += 1;
        }
    }
    stop_audio()?;
    for c in [clsid_string(&CLSID_SFX), clsid_string(&CLSID_MFX)] {
        for k in [format!(r"SOFTWARE\Classes\CLSID\{c}"), format!(r"SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{c}")] {
            let w = wide(&k);
            unsafe {
                let _ = windows::Win32::System::Registry::RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(w.as_ptr()));
            }
        }
    }
    let _ = std::fs::remove_file(install_dir().join("winfx_apo.dll"));
    start_audio()?;
    Ok(format!("restored {n} endpoint(s)"))
}

fn backup_path(guid: &str) -> PathBuf {
    data_dir().join("fx-backup").join(format!("{guid}.txt"))
}

/// Remember the endpoint's original stream effect (first time only).
fn backup_fx(guid: &str) {
    let p = backup_path(guid);
    if p.exists() {
        return;
    }
    if let Some(orig) = get(&format!(r"{CAPTURE}\{guid}\FxProperties"), SFX_VALUE) {
        if !orig.eq_ignore_ascii_case(&clsid_string(&CLSID_SFX)) {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(&p, orig);
        }
    }
}

fn restore_fx(guid: &str) {
    let key = format!(r"{CAPTURE}\{guid}\FxProperties");
    let Some(k) = open(&key, KEY_SET_VALUE | KEY_QUERY_VALUE) else { return };
    let n = wide(SFX_VALUE);
    match std::fs::read_to_string(backup_path(guid)) {
        Ok(orig) if orig.starts_with('{') => {
            let v = wide(orig.trim());
            let b = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
            unsafe {
                let _ = RegSetValueExW(k, PCWSTR(n.as_ptr()), None, REG_SZ, Some(b));
            }
        }
        _ => unsafe {
            let _ = windows::Win32::System::Registry::RegDeleteValueW(k, PCWSTR(n.as_ptr()));
        },
    }
    unsafe {
        let _ = RegCloseKey(k);
    }
}

fn stop_audio() -> Result<(), String> {
    run_hidden("net", &["stop", "audiosrv", "/y"]).map_err(|e| format!("stop audio service: {e}"))
}

fn start_audio() -> Result<(), String> {
    run_hidden("net", &["start", "audiosrv"]).map_err(|e| format!("start audio service: {e}"))
}

/// IPolicyConfig: undocumented, but what Windows' own Sound settings use to
/// change endpoint properties through the audio service (live, unelevated).
mod policy {
    #![allow(non_snake_case)]
    use std::ffi::c_void;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR};

    #[repr(transparent)]
    #[derive(Clone)]
    struct IPolicyConfig(IUnknown);
    unsafe impl Interface for IPolicyConfig {
        type Vtable = Vtbl;
        const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    }
    #[repr(C)]
    struct Vtbl {
        base__: IUnknown_Vtbl,
        _get_mix_format: usize,
        _get_device_format: usize,
        _reset_device_format: usize,
        _set_device_format: usize,
        _get_processing_period: usize,
        _set_processing_period: usize,
        _get_share_mode: usize,
        _set_share_mode: usize,
        _get_property_value: usize,
        SetPropertyValue: unsafe extern "system" fn(*mut c_void, PCWSTR, BOOL, *const PROPERTYKEY, *const PROPVARIANT) -> HRESULT,
    }
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    pub fn set_fx_u32(device_id: &str, key: &PROPERTYKEY, v: u32) -> windows::core::Result<()> {
        crate::devices::ensure_com();
        let id: Vec<u16> = device_id.encode_utf16().chain([0]).collect();
        unsafe {
            let pc: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
            let vt = &**(pc.as_raw() as *const *const Vtbl);
            let val = PROPVARIANT::from(v);
            (vt.SetPropertyValue)(pc.as_raw(), PCWSTR(id.as_ptr()), true.into(), key, &val).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_from_device_id() {
        assert_eq!(endpoint_guid("{0.0.1.00000000}.{c950d195-7502-492a-89a4-9d30cbb65fc7}"), Some("{c950d195-7502-492a-89a4-9d30cbb65fc7}"));
        assert_eq!(endpoint_guid("nonsense"), None);
    }

    #[test]
    fn clsid_format_matches_registry_style() {
        assert_eq!(clsid_string(&CLSID_SFX), "{77069DD4-922E-4A92-8B31-EA606FEDCE4B}");
    }
}
