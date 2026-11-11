//! WinFX audio processing object (APO): runs the effects inside the Windows
//! audio engine (audiodg.exe), on the real capture device.
//!
//! Spike stage: pass-through plus a quiet 1 kHz tone, so a plain recording
//! shows whether Windows loaded us. Non-real-time calls are logged to
//! C:\ProgramData\WinFX-dev\apo-logs\apo.log.

#![allow(non_snake_case)]

mod log;

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER, S_FALSE};
use windows::Win32::Media::Audio::Apo::*;
use windows::Win32::System::Com::{CoTaskMemAlloc, IClassFactory, IClassFactory_Impl};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref, implement};

pub const CLSID_SFX: GUID = GUID::from_u128(0x77069dd4_922e_4a92_8b31_ea606fedce4b);
pub const CLSID_MFX: GUID = GUID::from_u128(0xb1392a38_ebf1_43a8_b2bb_0ec0e7e87d37);
/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

fn clsid_name(c: &GUID) -> &'static str {
    if *c == CLSID_SFX {
        "SFX"
    } else if *c == CLSID_MFX {
        "MFX"
    } else {
        "?"
    }
}

/// State touched by the real-time thread. LockForProcess/UnlockForProcess never
/// run concurrently with APOProcess, so APOProcess's try_lock never contends.
#[derive(Default)]
struct Rt {
    locked: bool,
    channels: usize,
    rate: f32,
    phase: f32,
}

#[implement(IAudioProcessingObject, IAudioProcessingObjectRT, IAudioProcessingObjectConfiguration, IAudioSystemEffects, IAudioSystemEffects2)]
struct Apo {
    clsid: GUID,
    rt: Mutex<Rt>,
    frames: AtomicU64,
    calls: AtomicU64,
}

impl Apo {
    fn new(clsid: GUID) -> Self {
        Apo { clsid, rt: Mutex::new(Rt::default()), frames: AtomicU64::new(0), calls: AtomicU64::new(0) }
    }
    fn tag(&self) -> &'static str {
        clsid_name(&self.clsid)
    }
}

fn float_format(t: &IAudioMediaType) -> windows_core::Result<UNCOMPRESSEDAUDIOFORMAT> {
    let mut f = UNCOMPRESSEDAUDIOFORMAT::default();
    unsafe { t.GetUncompressedAudioFormat(&mut f)? };
    Ok(f)
}

fn describe(f: &UNCOMPRESSEDAUDIOFORMAT) -> String {
    format!(
        "{} Hz, {} ch, {}-byte {} (mask {:#x})",
        f.fFramesPerSecond,
        f.dwSamplesPerFrame,
        f.dwBytesPerSampleContainer,
        if f.guidFormatType == SUBTYPE_FLOAT { "float" } else { "non-float" },
        f.dwChannelMask
    )
}

fn check_format(tag: &str, which: &str, requested: Ref<IAudioMediaType>) -> windows_core::Result<IAudioMediaType> {
    let Some(req) = requested.as_ref() else { return Err(E_POINTER.into()) };
    let f = float_format(req)?;
    let ok = f.guidFormatType == SUBTYPE_FLOAT && f.dwBytesPerSampleContainer == 4 && f.dwSamplesPerFrame > 0;
    log::line(&format!("[{tag}] Is{which}FormatSupported {} -> {}", describe(&f), if ok { "yes" } else { "no" }));
    if ok { Ok(req.clone()) } else { Err(APOERR_FORMAT_NOT_SUPPORTED.into()) }
}

impl IAudioProcessingObject_Impl for Apo_Impl {
    fn Reset(&self) -> windows_core::Result<()> {
        if let Ok(mut rt) = self.rt.lock() {
            rt.phase = 0.0;
        }
        Ok(())
    }

    fn GetLatency(&self) -> windows_core::Result<i64> {
        Ok(0)
    }

    fn GetRegistrationProperties(&self) -> windows_core::Result<*mut APO_REG_PROPERTIES> {
        unsafe {
            let p = CoTaskMemAlloc(size_of::<APO_REG_PROPERTIES>()) as *mut APO_REG_PROPERTIES;
            if p.is_null() {
                return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            let mut r: APO_REG_PROPERTIES = std::mem::zeroed();
            r.clsid = self.clsid;
            r.Flags = APO_FLAG_DEFAULT;
            wide(&mut r.szFriendlyName, &format!("WinFX capture effects ({})", self.tag()));
            wide(&mut r.szCopyrightInfo, "WinFX");
            r.u32MajorVersion = 1;
            r.u32MinorVersion = 0;
            r.u32MinInputConnections = 1;
            r.u32MaxInputConnections = 1;
            r.u32MinOutputConnections = 1;
            r.u32MaxOutputConnections = 1;
            r.u32MaxInstances = u32::MAX;
            r.u32NumAPOInterfaces = 1;
            r.iidAPOInterfaceList = [IAudioProcessingObject::IID];
            p.write(r);
            Ok(p)
        }
    }

    fn Initialize(&self, cbdatasize: u32, pbydata: *const u8) -> windows_core::Result<()> {
        let mut msg = format!("[{}] Initialize {} bytes", self.tag(), cbdatasize);
        if !pbydata.is_null() && cbdatasize as usize >= size_of::<APOInitSystemEffects2>() {
            let init = unsafe { &*(pbydata as *const APOInitSystemEffects2) };
            msg += &format!(
                " (SystemEffects2: mode {:?}, discovery-only {})",
                init.AudioProcessingMode,
                init.InitializeForDiscoveryOnly.as_bool()
            );
        } else if cbdatasize as usize >= size_of::<APOInitSystemEffects>() {
            msg += " (SystemEffects)";
        }
        log::line(&msg);
        Ok(())
    }

    fn IsInputFormatSupported(
        &self,
        _opposite: Ref<IAudioMediaType>,
        requested: Ref<IAudioMediaType>,
    ) -> windows_core::Result<IAudioMediaType> {
        check_format(self.tag(), "Input", requested)
    }

    fn IsOutputFormatSupported(
        &self,
        _opposite: Ref<IAudioMediaType>,
        requested: Ref<IAudioMediaType>,
    ) -> windows_core::Result<IAudioMediaType> {
        check_format(self.tag(), "Output", requested)
    }

    fn GetInputChannelCount(&self) -> windows_core::Result<u32> {
        Ok(self.rt.lock().map(|r| r.channels as u32).unwrap_or(0))
    }
}

impl IAudioProcessingObjectConfiguration_Impl for Apo_Impl {
    fn LockForProcess(
        &self,
        nin: u32,
        ppin: *const *const APO_CONNECTION_DESCRIPTOR,
        nout: u32,
        _ppout: *const *const APO_CONNECTION_DESCRIPTOR,
    ) -> windows_core::Result<()> {
        if nin != 1 || nout != 1 || ppin.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let desc = unsafe { &**ppin };
        let Some(t) = desc.pFormat.as_ref() else { return Err(E_POINTER.into()) };
        let f = float_format(t)?;
        log::line(&format!("[{}] LockForProcess {} max {} frames", self.tag(), describe(&f), desc.u32MaxFrameCount));
        let mut rt = self.rt.lock().map_err(|_| windows_core::Error::from(E_INVALIDARG))?;
        rt.locked = true;
        rt.channels = f.dwSamplesPerFrame as usize;
        rt.rate = f.fFramesPerSecond;
        rt.phase = 0.0;
        self.frames.store(0, Relaxed);
        self.calls.store(0, Relaxed);
        Ok(())
    }

    fn UnlockForProcess(&self) -> windows_core::Result<()> {
        if let Ok(mut rt) = self.rt.lock() {
            rt.locked = false;
        }
        log::line(&format!(
            "[{}] UnlockForProcess after {} calls, {} frames",
            self.tag(),
            self.calls.load(Relaxed),
            self.frames.load(Relaxed)
        ));
        Ok(())
    }
}

impl IAudioProcessingObjectRT_Impl for Apo_Impl {
    fn APOProcess(
        &self,
        nin: u32,
        ppin: *const *const APO_CONNECTION_PROPERTY,
        nout: u32,
        ppout: *mut *mut APO_CONNECTION_PROPERTY,
    ) {
        if nin < 1 || nout < 1 || ppin.is_null() || ppout.is_null() {
            return;
        }
        let (inp, out) = unsafe { (&**ppin, &mut **ppout) };
        let n = inp.u32ValidFrameCount as usize;
        let Ok(mut rt) = self.rt.try_lock() else { return };
        if !rt.locked || rt.channels == 0 {
            return;
        }
        let ch = rt.channels;
        let src = inp.pBuffer as *const f32;
        let dst = out.pBuffer as *mut f32;
        let silent = inp.u32BufferFlags == BUFFER_SILENT;
        let step = std::f32::consts::TAU * 1000.0 / rt.rate.max(1.0);
        let mut phase = rt.phase;
        for i in 0..n {
            let tone = phase.sin() * 0.0316; // -30 dBFS
            phase += step;
            if phase > std::f32::consts::TAU {
                phase -= std::f32::consts::TAU;
            }
            for c in 0..ch {
                let k = i * ch + c;
                unsafe {
                    let x = if silent { 0.0 } else { *src.add(k) };
                    *dst.add(k) = x + tone;
                }
            }
        }
        rt.phase = phase;
        out.u32ValidFrameCount = n as u32;
        out.u32BufferFlags = BUFFER_VALID;
        self.calls.fetch_add(1, Relaxed);
        self.frames.fetch_add(n as u64, Relaxed);
    }

    fn CalcInputFrames(&self, n: u32) -> u32 {
        n
    }

    fn CalcOutputFrames(&self, n: u32) -> u32 {
        n
    }
}

impl IAudioSystemEffects_Impl for Apo_Impl {}

impl IAudioSystemEffects2_Impl for Apo_Impl {
    fn GetEffectsList(
        &self,
        ppeffectsids: *mut *mut GUID,
        pceffects: *mut u32,
        _event: windows::Win32::Foundation::HANDLE,
    ) -> windows_core::Result<()> {
        if ppeffectsids.is_null() || pceffects.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *ppeffectsids = std::ptr::null_mut();
            *pceffects = 0;
        }
        Ok(())
    }
}

fn wide(dst: &mut [u16], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().chain(std::iter::once(0))) {
        *d = c;
    }
}

// ---------- COM plumbing ----------

#[implement(IClassFactory)]
struct Factory {
    clsid: GUID,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, riid: *const GUID, ppv: *mut *mut c_void) -> windows_core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppv = std::ptr::null_mut() };
        let iid = unsafe { *riid };
        if outer.is_some() {
            log::line(&format!("[{}] CreateInstance with an outer IUnknown (aggregation) for {iid:?}: refused", clsid_name(&self.clsid)));
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let obj: IUnknown = Apo::new(self.clsid).into();
        let hr = unsafe { obj.query(&iid, ppv) };
        log::line(&format!("[{}] CreateInstance {iid:?} -> {hr:?}", clsid_name(&self.clsid)));
        hr.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> windows_core::Result<()> {
        Ok(())
    }
}

#[unsafe(no_mangle)]
extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if rclsid.is_null() || riid.is_null() || ppv.is_null() {
        return E_POINTER;
    }
    unsafe { *ppv = std::ptr::null_mut() };
    let clsid = unsafe { *rclsid };
    if clsid != CLSID_SFX && clsid != CLSID_MFX {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    log::line(&format!("DllGetClassObject {}", clsid_name(&clsid)));
    let f: IClassFactory = Factory { clsid }.into();
    unsafe { f.query(&*riid, ppv) }
}

/// Never unload: simpler, and audiodg keeps us for the stream's lifetime anyway.
#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
