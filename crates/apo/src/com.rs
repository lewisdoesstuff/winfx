//! Hand-written COM object for the APO, with aggregation support.
//!
//! Layout: one heap `Obj` holding a vtable pointer per interface. The first
//! (`inner`) is the non-delegating IUnknown that owns the reference count; the
//! others forward QueryInterface/AddRef/Release to the controlling unknown,
//! which is audiodg's outer object when aggregated, or `inner` otherwise.

use crate::{CLSID_MFX, CLSID_SFX, Core, clsid_name, log};
use std::ffi::c_void;
use std::mem::offset_of;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_POINTER, HANDLE, S_FALSE, S_OK,
};
use windows::Win32::Media::Audio::Apo::*;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, Ref, implement};

#[repr(C)]
struct Obj {
    inner: *const IUnknown_Vtbl,
    apo: *const IAudioProcessingObject_Vtbl,
    rt: *const IAudioProcessingObjectRT_Vtbl,
    cfg: *const IAudioProcessingObjectConfiguration_Vtbl,
    sfx: *const IAudioSystemEffects2_Vtbl,
    refs: AtomicU32,
    /// Controlling unknown. Not reference-counted (COM aggregation rule).
    outer: *mut c_void,
    core: Core,
}

const OFF_APO: usize = offset_of!(Obj, apo);
const OFF_RT: usize = offset_of!(Obj, rt);
const OFF_CFG: usize = offset_of!(Obj, cfg);
const OFF_SFX: usize = offset_of!(Obj, sfx);

unsafe fn obj<'a, const OFF: usize>(this: *mut c_void) -> &'a Obj {
    unsafe { &*((this as *const u8).sub(OFF) as *const Obj) }
}

unsafe fn unknown_vtbl<'a>(p: *mut c_void) -> &'a IUnknown_Vtbl {
    unsafe { &**(p as *const *const IUnknown_Vtbl) }
}

fn hr(r: windows_core::Result<()>) -> HRESULT {
    match r {
        Ok(()) => S_OK,
        Err(e) => e.code(),
    }
}

// ---------- non-delegating IUnknown ----------

unsafe extern "system" fn inner_qi(this: *mut c_void, iid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() || iid.is_null() {
        return E_POINTER;
    }
    unsafe {
        *ppv = std::ptr::null_mut();
        let o = &*(this as *const Obj);
        let iid = &*iid;
        let p: *const c_void = if *iid == IUnknown::IID {
            &o.inner as *const _ as _
        } else if *iid == IAudioProcessingObject::IID {
            &o.apo as *const _ as _
        } else if *iid == IAudioProcessingObjectRT::IID {
            &o.rt as *const _ as _
        } else if *iid == IAudioProcessingObjectConfiguration::IID {
            &o.cfg as *const _ as _
        } else if *iid == IAudioSystemEffects::IID || *iid == IAudioSystemEffects2::IID {
            &o.sfx as *const _ as _
        } else {
            log::line(&format!("[{}] QueryInterface {iid:?}: not supported", clsid_name(&o.core_clsid())));
            return E_NOINTERFACE;
        };
        // AddRef through the interface we hand out (delegates to the outer object).
        (unknown_vtbl(p as *mut c_void).AddRef)(p as *mut c_void);
        *ppv = p as *mut c_void;
    }
    S_OK
}

unsafe extern "system" fn inner_addref(this: *mut c_void) -> u32 {
    unsafe { (*(this as *const Obj)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn inner_release(this: *mut c_void) -> u32 {
    unsafe {
        let n = (*(this as *const Obj)).refs.fetch_sub(1, Ordering::Release) - 1;
        if n == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this as *mut Obj));
        }
        n
    }
}

static INNER_VTBL: IUnknown_Vtbl =
    IUnknown_Vtbl { QueryInterface: inner_qi, AddRef: inner_addref, Release: inner_release };

// ---------- delegating IUnknown (for every interface but `inner`) ----------

unsafe extern "system" fn d_qi<const OFF: usize>(
    this: *mut c_void,
    iid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let o = obj::<OFF>(this);
        (unknown_vtbl(o.outer).QueryInterface)(o.outer, iid, ppv)
    }
}
unsafe extern "system" fn d_addref<const OFF: usize>(this: *mut c_void) -> u32 {
    unsafe {
        let o = obj::<OFF>(this);
        (unknown_vtbl(o.outer).AddRef)(o.outer)
    }
}
unsafe extern "system" fn d_release<const OFF: usize>(this: *mut c_void) -> u32 {
    unsafe {
        let o = obj::<OFF>(this);
        (unknown_vtbl(o.outer).Release)(o.outer)
    }
}
const fn delegating<const OFF: usize>() -> IUnknown_Vtbl {
    IUnknown_Vtbl { QueryInterface: d_qi::<OFF>, AddRef: d_addref::<OFF>, Release: d_release::<OFF> }
}

// ---------- IAudioProcessingObject ----------

unsafe extern "system" fn apo_reset(this: *mut c_void) -> HRESULT {
    hr(unsafe { obj::<OFF_APO>(this) }.core.reset())
}
unsafe extern "system" fn apo_get_latency(_this: *mut c_void, t: *mut i64) -> HRESULT {
    if t.is_null() {
        return E_POINTER;
    }
    unsafe { *t = 0 };
    S_OK
}
unsafe extern "system" fn apo_get_registration_properties(
    this: *mut c_void,
    pp: *mut *mut APO_REG_PROPERTIES,
) -> HRESULT {
    if pp.is_null() {
        return E_POINTER;
    }
    match unsafe { obj::<OFF_APO>(this) }.core.registration_properties() {
        Ok(p) => {
            unsafe { *pp = p };
            S_OK
        }
        Err(e) => e.code(),
    }
}
unsafe extern "system" fn apo_initialize(this: *mut c_void, size: u32, data: *const u8) -> HRESULT {
    hr(unsafe { obj::<OFF_APO>(this) }.core.initialize(size, data))
}
unsafe fn format_query(this: *mut c_void, which: &str, requested: *mut c_void, pp: *mut *mut c_void) -> HRESULT {
    if pp.is_null() {
        return E_POINTER;
    }
    unsafe {
        *pp = std::ptr::null_mut();
        let req = IAudioMediaType::from_raw_borrowed(&requested);
        match obj::<OFF_APO>(this).core.format_supported(which, req) {
            Ok(t) => {
                *pp = t.into_raw();
                S_OK
            }
            Err(e) => e.code(),
        }
    }
}
unsafe extern "system" fn apo_is_input_format_supported(
    this: *mut c_void,
    _opp: *mut c_void,
    req: *mut c_void,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { format_query(this, "Input", req, pp) }
}
unsafe extern "system" fn apo_is_output_format_supported(
    this: *mut c_void,
    _opp: *mut c_void,
    req: *mut c_void,
    pp: *mut *mut c_void,
) -> HRESULT {
    unsafe { format_query(this, "Output", req, pp) }
}
unsafe extern "system" fn apo_get_input_channel_count(this: *mut c_void, n: *mut u32) -> HRESULT {
    if n.is_null() {
        return E_POINTER;
    }
    unsafe { *n = obj::<OFF_APO>(this).core.input_channel_count() };
    S_OK
}

static APO_VTBL: IAudioProcessingObject_Vtbl = IAudioProcessingObject_Vtbl {
    base__: delegating::<OFF_APO>(),
    Reset: apo_reset,
    GetLatency: apo_get_latency,
    GetRegistrationProperties: apo_get_registration_properties,
    Initialize: apo_initialize,
    IsInputFormatSupported: apo_is_input_format_supported,
    IsOutputFormatSupported: apo_is_output_format_supported,
    GetInputChannelCount: apo_get_input_channel_count,
};

// ---------- IAudioProcessingObjectRT ----------

unsafe extern "system" fn rt_process(
    this: *mut c_void,
    nin: u32,
    ppin: *const *const APO_CONNECTION_PROPERTY,
    nout: u32,
    ppout: *mut *mut APO_CONNECTION_PROPERTY,
) {
    if nin < 1 || nout < 1 || ppin.is_null() || ppout.is_null() {
        return;
    }
    unsafe {
        let (inp, out) = (&**ppin, &mut **ppout);
        obj::<OFF_RT>(this).core.process(inp, out);
    }
}
unsafe extern "system" fn rt_calc_input_frames(_this: *mut c_void, n: u32) -> u32 {
    n
}
unsafe extern "system" fn rt_calc_output_frames(_this: *mut c_void, n: u32) -> u32 {
    n
}

static RT_VTBL: IAudioProcessingObjectRT_Vtbl = IAudioProcessingObjectRT_Vtbl {
    base__: delegating::<OFF_RT>(),
    APOProcess: rt_process,
    CalcInputFrames: rt_calc_input_frames,
    CalcOutputFrames: rt_calc_output_frames,
};

// ---------- IAudioProcessingObjectConfiguration ----------

unsafe extern "system" fn cfg_lock(
    this: *mut c_void,
    nin: u32,
    ppin: *const *const APO_CONNECTION_DESCRIPTOR,
    nout: u32,
    _ppout: *const *const APO_CONNECTION_DESCRIPTOR,
) -> HRESULT {
    hr(unsafe { obj::<OFF_CFG>(this) }.core.lock_for_process(nin, ppin, nout))
}
unsafe extern "system" fn cfg_unlock(this: *mut c_void) -> HRESULT {
    hr(unsafe { obj::<OFF_CFG>(this) }.core.unlock_for_process())
}

static CFG_VTBL: IAudioProcessingObjectConfiguration_Vtbl = IAudioProcessingObjectConfiguration_Vtbl {
    base__: delegating::<OFF_CFG>(),
    LockForProcess: cfg_lock,
    UnlockForProcess: cfg_unlock,
};

// ---------- IAudioSystemEffects / IAudioSystemEffects2 ----------

unsafe extern "system" fn sfx_get_effects_list(
    _this: *mut c_void,
    ids: *mut *mut GUID,
    count: *mut u32,
    _event: HANDLE,
) -> HRESULT {
    if ids.is_null() || count.is_null() {
        return E_POINTER;
    }
    unsafe {
        *ids = std::ptr::null_mut();
        *count = 0;
    }
    S_OK
}

static SFX_VTBL: IAudioSystemEffects2_Vtbl = IAudioSystemEffects2_Vtbl {
    base__: IAudioSystemEffects_Vtbl { base__: delegating::<OFF_SFX>() },
    GetEffectsList: sfx_get_effects_list,
};

impl Obj {
    fn core_clsid(&self) -> GUID {
        self.core.clsid
    }
}

/// Create the object. With `outer`, returns the non-delegating IUnknown
/// (`riid` must be IUnknown); without, behaves like a normal QueryInterface.
unsafe fn create(clsid: GUID, outer: Option<*mut c_void>, riid: &GUID, ppv: *mut *mut c_void) -> HRESULT {
    unsafe {
        *ppv = std::ptr::null_mut();
        if outer.is_some() && *riid != IUnknown::IID {
            return CLASS_E_NOAGGREGATION;
        }
        let p = Box::into_raw(Box::new(Obj {
            inner: &INNER_VTBL,
            apo: &APO_VTBL,
            rt: &RT_VTBL,
            cfg: &CFG_VTBL,
            sfx: &SFX_VTBL,
            refs: AtomicU32::new(1),
            outer: std::ptr::null_mut(),
            core: Core::new(clsid),
        }));
        (*p).outer = outer.unwrap_or(p as *mut c_void);
        if outer.is_some() {
            *ppv = p as *mut c_void; // our one reference goes to the outer object
            S_OK
        } else {
            let r = inner_qi(p as *mut c_void, riid, ppv);
            inner_release(p as *mut c_void);
            r
        }
    }
}

// ---------- class factory and DLL exports ----------

#[implement(IClassFactory)]
struct Factory {
    clsid: GUID,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        if ppv.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        let iid = unsafe { *riid };
        let outer_raw = outer.as_ref().map(|u| u.as_raw());
        let r = unsafe { create(self.clsid, outer_raw, &iid, ppv) };
        log::line(&format!(
            "[{}] CreateInstance {iid:?}{} -> {r:?}",
            clsid_name(&self.clsid),
            if outer_raw.is_some() { " (aggregated)" } else { "" }
        ));
        r.ok()
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

#[cfg(test)]
mod tests {
    //! Drive the APO the way audiodg does: aggregated under an outer object,
    //! then Initialize -> format checks -> LockForProcess -> APOProcess -> Unlock.
    use super::*;
    use crate::SUBTYPE_FLOAT;
    use windows::Win32::Media::Audio::WAVEFORMATEX;
    use windows_core::implement;

    #[implement(IAudioMediaType)]
    struct FakeType {
        f: UNCOMPRESSEDAUDIOFORMAT,
    }
    impl IAudioMediaType_Impl for FakeType_Impl {
        fn IsCompressedFormat(&self) -> windows_core::Result<BOOL> {
            Ok(false.into())
        }
        fn IsEqual(&self, _o: Ref<IAudioMediaType>) -> windows_core::Result<u32> {
            Ok(0)
        }
        fn GetAudioFormat(&self) -> *mut WAVEFORMATEX {
            std::ptr::null_mut()
        }
        fn GetUncompressedAudioFormat(&self, p: *mut UNCOMPRESSEDAUDIOFORMAT) -> windows_core::Result<()> {
            unsafe { *p = self.f };
            Ok(())
        }
    }

    /// Minimal controlling unknown, like audiodg's wrapper.
    #[repr(C)]
    struct Outer {
        vtbl: *const IUnknown_Vtbl,
        refs: AtomicU32,
        inner: *mut c_void,
    }
    unsafe extern "system" fn o_qi(this: *mut c_void, iid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
        unsafe {
            if *iid == IUnknown::IID {
                o_addref(this);
                *ppv = this;
                return S_OK;
            }
            let o = &*(this as *const Outer);
            (unknown_vtbl(o.inner).QueryInterface)(o.inner, iid, ppv)
        }
    }
    unsafe extern "system" fn o_addref(this: *mut c_void) -> u32 {
        unsafe { (*(this as *const Outer)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
    }
    unsafe extern "system" fn o_release(this: *mut c_void) -> u32 {
        unsafe { (*(this as *const Outer)).refs.fetch_sub(1, Ordering::Relaxed) - 1 }
    }
    static OUTER_VTBL: IUnknown_Vtbl = IUnknown_Vtbl { QueryInterface: o_qi, AddRef: o_addref, Release: o_release };

    fn format(ch: u32) -> IAudioMediaType {
        FakeType {
            f: UNCOMPRESSEDAUDIOFORMAT {
                guidFormatType: SUBTYPE_FLOAT,
                dwSamplesPerFrame: ch,
                dwBytesPerSampleContainer: 4,
                dwValidBitsPerSample: 32,
                fFramesPerSecond: 48000.0,
                dwChannelMask: 0,
            },
        }
        .into()
    }

    #[test]
    fn aggregated_lifecycle() {
        unsafe {
            let mut outer = Box::new(Outer { vtbl: &OUTER_VTBL, refs: AtomicU32::new(1), inner: std::ptr::null_mut() });
            let outer_ptr = &mut *outer as *mut Outer as *mut c_void;
            let mut inner = std::ptr::null_mut();
            assert_eq!(create(CLSID_SFX, Some(outer_ptr), &IUnknown::IID, &mut inner), S_OK);
            outer.inner = inner;
            // Aggregated objects must refuse anything but IUnknown at creation.
            let mut bad = std::ptr::null_mut();
            assert_eq!(
                create(CLSID_SFX, Some(outer_ptr), &IAudioProcessingObject::IID, &mut bad),
                CLASS_E_NOAGGREGATION
            );

            // QI through the outer for each interface; refs land on the outer.
            let before = outer.refs.load(Ordering::Relaxed);
            let mut p = std::ptr::null_mut();
            assert_eq!(o_qi(outer_ptr, &IAudioProcessingObject::IID, &mut p), S_OK);
            assert_eq!(outer.refs.load(Ordering::Relaxed), before + 1, "AddRef must go to the outer object");
            let apo = IAudioProcessingObject::from_raw(p);
            let rt: IAudioProcessingObjectRT = apo.cast().unwrap();
            let cfg: IAudioProcessingObjectConfiguration = apo.cast().unwrap();
            let sfx: IAudioSystemEffects2 = apo.cast().unwrap();
            let _: IAudioSystemEffects = sfx.cast().unwrap();
            // QI back to IUnknown from an interface must give the outer (identity rule).
            let unk: IUnknown = rt.cast().unwrap();
            assert_eq!(unk.as_raw(), outer_ptr);
            drop(unk);

            let props = apo.GetRegistrationProperties().unwrap();
            assert_eq!((*props).clsid, CLSID_SFX);
            windows::Win32::System::Com::CoTaskMemFree(Some(props as _));

            apo.Initialize(&[]).unwrap();
            let fmt = format(2);
            let ok = apo.IsInputFormatSupported(None, &fmt).unwrap();
            assert_eq!(ok.as_raw(), fmt.as_raw());

            let mut in_desc = APO_CONNECTION_DESCRIPTOR {
                Type: APO_CONNECTION_BUFFER_TYPE_ALLOCATED,
                pBuffer: 0,
                u32MaxFrameCount: 480,
                pFormat: std::mem::ManuallyDrop::new(Some(fmt.clone())),
                u32Signature: 0,
            };
            let mut out_desc = APO_CONNECTION_DESCRIPTOR {
                Type: APO_CONNECTION_BUFFER_TYPE_ALLOCATED,
                pBuffer: 0,
                u32MaxFrameCount: 480,
                pFormat: std::mem::ManuallyDrop::new(Some(fmt.clone())),
                u32Signature: 0,
            };
            let pin = [&mut in_desc as *mut _ as *const APO_CONNECTION_DESCRIPTOR];
            let pout = [&mut out_desc as *mut _ as *const APO_CONNECTION_DESCRIPTOR];
            cfg.LockForProcess(&pin, &pout).unwrap();
            assert_eq!(apo.GetInputChannelCount().unwrap(), 2);

            let input = vec![0.25f32; 480 * 2];
            let mut output = vec![0f32; 480 * 2];
            let mut ip = APO_CONNECTION_PROPERTY {
                pBuffer: input.as_ptr() as usize,
                u32ValidFrameCount: 480,
                u32BufferFlags: BUFFER_VALID,
                u32Signature: 0,
            };
            let mut op = APO_CONNECTION_PROPERTY {
                pBuffer: output.as_mut_ptr() as usize,
                u32ValidFrameCount: 0,
                u32BufferFlags: BUFFER_INVALID,
                u32Signature: 0,
            };
            let pip = [&mut ip as *mut APO_CONNECTION_PROPERTY as *const APO_CONNECTION_PROPERTY];
            let mut pop = [&mut op as *mut APO_CONNECTION_PROPERTY];
            rt.APOProcess(1, pip.as_ptr(), 1, pop.as_mut_ptr());
            assert_eq!(op.u32ValidFrameCount, 480);
            assert_eq!(op.u32BufferFlags, BUFFER_VALID);
            // No chain has arrived yet: bit-exact pass-through.
            assert!(output.iter().all(|&v| v == 0.25));
            cfg.UnlockForProcess().unwrap();

            std::mem::ManuallyDrop::drop(&mut in_desc.pFormat);
            std::mem::ManuallyDrop::drop(&mut out_desc.pFormat);
            drop((apo, rt, cfg, sfx, ok, fmt));
            assert_eq!(outer.refs.load(Ordering::Relaxed), 1, "every interface reference released back to the outer");
            // The outer owns the inner's single reference; releasing it frees the object.
            assert_eq!(inner_release(inner), 0);
        }
    }

    #[test]
    fn plain_creation() {
        unsafe {
            let mut p = std::ptr::null_mut();
            assert_eq!(create(CLSID_MFX, None, &IAudioProcessingObjectRT::IID, &mut p), S_OK);
            let rt = IAudioProcessingObjectRT::from_raw(p);
            let apo: IAudioProcessingObject = rt.cast().unwrap();
            assert_eq!(apo.GetLatency().unwrap(), 0);
        }
    }
}
