//! Set an endpoint's FX property through IPolicyConfig (what Windows' Sound
//! settings use), so the audio service applies it live without admin.
//!   cargo run -p engine --example policy_fx -- <endpoint-id-guid> <pid> <clsid|u32:N|show> [fmtid]
//! e.g. ... {c950d195-...} 5 {77069DD4-922E-4A92-8B31-EA606FEDCE4B}
//!      ... {c950d195-...} 5 u32:1 {1da5d803-d492-4edd-8c23-e0c0ffee7f0e}   (Disable_SysFx)
#![allow(non_snake_case)]
use std::ffi::c_void;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR};

/// Undocumented, but stable since Windows 7 (IPolicyConfig, Windows 10+ IID).
#[repr(transparent)]
#[derive(Clone)]
struct IPolicyConfig(IUnknown);
unsafe impl Interface for IPolicyConfig {
    type Vtable = IPolicyConfig_Vtbl;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}
#[repr(C)]
struct IPolicyConfig_Vtbl {
    base__: IUnknown_Vtbl,
    GetMixFormat: usize,
    GetDeviceFormat: usize,
    ResetDeviceFormat: usize,
    SetDeviceFormat: usize,
    GetProcessingPeriod: usize,
    SetProcessingPeriod: usize,
    GetShareMode: usize,
    SetShareMode: usize,
    GetPropertyValue:
        unsafe extern "system" fn(*mut c_void, PCWSTR, BOOL, *const PROPERTYKEY, *mut PROPVARIANT) -> HRESULT,
    SetPropertyValue:
        unsafe extern "system" fn(*mut c_void, PCWSTR, BOOL, *const PROPERTYKEY, *const PROPVARIANT) -> HRESULT,
    SetDefaultEndpoint: usize,
    SetEndpointVisibility: usize,
}
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const FX: GUID = GUID::from_u128(0xd04e05a6_594b_4fb6_a80d_01af5eed7d1d);

fn main() -> windows::core::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (ep, pid, val) = (&a[0], a[1].parse::<u32>().unwrap(), &a[2]);
    let id: Vec<u16> = format!("{{0.0.1.00000000}}.{ep}").encode_utf16().chain([0]).collect();
    let fmtid = a.get(3).map(|g| GUID::try_from(g.trim_matches(|c| c == '{' || c == '}')).unwrap()).unwrap_or(FX);
    let key = PROPERTYKEY { fmtid, pid };
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let pc: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let vt = &**(pc.as_raw() as *const *const IPolicyConfig_Vtbl);
        let mut cur = PROPVARIANT::default();
        (vt.GetPropertyValue)(pc.as_raw(), PCWSTR(id.as_ptr()), true.into(), &key, &mut cur).ok()?;
        println!("current: {}", cur.to_string());
        let _ = PropVariantClear(&mut cur);
        if val != "show" {
            let v = match val.strip_prefix("u32:") {
                Some(n) => PROPVARIANT::from(n.parse::<u32>().unwrap()),
                None => PROPVARIANT::from(val.as_str()),
            };
            (vt.SetPropertyValue)(pc.as_raw(), PCWSTR(id.as_ptr()), true.into(), &key, &v).ok()?;
            let mut now = PROPVARIANT::default();
            (vt.GetPropertyValue)(pc.as_raw(), PCWSTR(id.as_ptr()), true.into(), &key, &mut now).ok()?;
            println!("now:     {}", now.to_string());
        }
    }
    Ok(())
}
