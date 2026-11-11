//! WinFX audio processing object (APO): runs the effects inside the Windows
//! audio engine (audiodg.exe), on the real capture device.
//!
//! audiodg creates APOs through COM aggregation, which windows-rs's
//! `#[implement]` can't do, so the APO object is laid out by hand in `com.rs`.
//! This file holds the behaviour; `com.rs` holds the vtables.
//!
//! The effects chain comes from the WinFX app through shared memory (see
//! `host.rs` and `engine::shared`); until one arrives, or while bypassed, audio
//! passes through untouched. Non-real-time calls are logged to
//! C:\ProgramData\WinFX\apo.log.

#![allow(non_snake_case)]

mod com;
mod host;
mod log;

use engine::Chain;
use engine::meters::{Meters, block_levels};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use windows::Win32::Foundation::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Media::Audio::Apo::*;
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows_core::{GUID, Interface};

pub use engine::apo::{CLSID_MFX, CLSID_SFX};
/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

pub(crate) fn clsid_name(c: &GUID) -> &'static str {
    if *c == CLSID_SFX {
        "SFX"
    } else if *c == CLSID_MFX {
        "MFX"
    } else {
        "?"
    }
}

type Result<T> = windows_core::Result<T>;

/// State touched by the real-time thread. LockForProcess/UnlockForProcess never
/// run concurrently with APOProcess, so APOProcess's try_lock never contends.
#[derive(Default)]
struct Rt {
    locked: bool,
    channels: usize,
    rate: f32,
    inst: Option<Arc<host::Inst>>,
    chain: Option<Box<Chain>>,
    /// Mono working buffer, sized at LockForProcess.
    scratch: Vec<f32>,
    /// Used when the shared block isn't mapped.
    own_meters: Option<Box<Meters>>,
}

/// Counters the real-time thread bumps; logged at UnlockForProcess.
#[derive(Default)]
struct Stats {
    calls: AtomicU64,
    frames: AtomicU64,
    silent_calls: AtomicU64,
    /// f32 bits of the largest input sample seen.
    in_peak: AtomicU32,
    /// Processing time per call, nanoseconds.
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}

pub(crate) struct Core {
    clsid: GUID,
    rt: Mutex<Rt>,
    stats: Stats,
}

fn float_format(t: &IAudioMediaType) -> Result<UNCOMPRESSEDAUDIOFORMAT> {
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

fn wide(dst: &mut [u16], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().chain(std::iter::once(0))) {
        *d = c;
    }
}

impl Core {
    pub(crate) fn new(clsid: GUID) -> Self {
        Core { clsid, rt: Mutex::new(Rt::default()), stats: Stats::default() }
    }

    fn tag(&self) -> &'static str {
        clsid_name(&self.clsid)
    }

    // ----- IAudioProcessingObject -----

    pub(crate) fn reset(&self) -> Result<()> {
        Ok(())
    }

    pub(crate) fn registration_properties(&self) -> Result<*mut APO_REG_PROPERTIES> {
        unsafe {
            let p = CoTaskMemAlloc(size_of::<APO_REG_PROPERTIES>()) as *mut APO_REG_PROPERTIES;
            if p.is_null() {
                return Err(E_OUTOFMEMORY.into());
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

    pub(crate) fn initialize(&self, size: u32, data: *const u8) -> Result<()> {
        let mut msg = format!("[{}] Initialize {} bytes", self.tag(), size);
        if !data.is_null() && size as usize >= size_of::<APOInitSystemEffects2>() {
            let init = unsafe { &*(data as *const APOInitSystemEffects2) };
            msg += &format!(
                " (SystemEffects2: mode {:?}, discovery-only {})",
                init.AudioProcessingMode,
                init.InitializeForDiscoveryOnly.as_bool()
            );
        } else if size as usize >= size_of::<APOInitSystemEffects>() {
            msg += " (SystemEffects)";
        }
        log::line(&msg);
        Ok(())
    }

    /// Accept any float32 PCM layout; the engine converts around us.
    pub(crate) fn format_supported(&self, which: &str, requested: Option<&IAudioMediaType>) -> Result<IAudioMediaType> {
        let Some(req) = requested else { return Err(E_POINTER.into()) };
        let f = float_format(req)?;
        let ok = f.guidFormatType == SUBTYPE_FLOAT && f.dwBytesPerSampleContainer == 4 && f.dwSamplesPerFrame > 0;
        log::line(&format!("[{}] Is{which}FormatSupported {} -> {}", self.tag(), describe(&f), if ok { "yes" } else { "no" }));
        if ok { Ok(req.clone()) } else { Err(APOERR_FORMAT_NOT_SUPPORTED.into()) }
    }

    pub(crate) fn input_channel_count(&self) -> u32 {
        self.rt.lock().map(|r| r.channels as u32).unwrap_or(0)
    }

    // ----- IAudioProcessingObjectConfiguration -----

    pub(crate) fn lock_for_process(&self, nin: u32, ppin: *const *const APO_CONNECTION_DESCRIPTOR, nout: u32) -> Result<()> {
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
        rt.scratch = vec![0.0; (desc.u32MaxFrameCount as usize).max(4096)];
        rt.own_meters = Some(Box::default());
        rt.chain = None;
        let inst = host::Inst::new(rt.rate);
        host::register(inst.clone());
        rt.inst = Some(inst);
        for c in [&self.stats.calls, &self.stats.frames, &self.stats.silent_calls, &self.stats.total_ns, &self.stats.max_ns] {
            c.store(0, Relaxed);
        }
        self.stats.in_peak.store(0, Relaxed);
        Ok(())
    }

    pub(crate) fn unlock_for_process(&self) -> Result<()> {
        if let Ok(mut rt) = self.rt.lock() {
            rt.locked = false;
            rt.chain = None;
            if let Some(i) = rt.inst.take() {
                i.live.store(false, std::sync::atomic::Ordering::Release);
                i.drain();
            }
        }
        let s = &self.stats;
        let peak = f32::from_bits(s.in_peak.load(Relaxed));
        let calls = s.calls.load(Relaxed).max(1);
        log::line(&format!(
            "[{}] UnlockForProcess: {} calls, {} frames, {} flagged silent, input peak {:.1} dBFS, process avg {:.1} us max {:.1} us",
            self.tag(),
            s.calls.load(Relaxed),
            s.frames.load(Relaxed),
            s.silent_calls.load(Relaxed),
            20.0 * peak.max(1e-9).log10(),
            s.total_ns.load(Relaxed) as f64 / calls as f64 / 1000.0,
            s.max_ns.load(Relaxed) as f64 / 1000.0
        ));
        Ok(())
    }

    // ----- IAudioProcessingObjectRT -----

    pub(crate) fn process(&self, inp: &APO_CONNECTION_PROPERTY, out: &mut APO_CONNECTION_PROPERTY) {
        let n = inp.u32ValidFrameCount as usize;
        let src = inp.pBuffer as *const f32;
        let dst = out.pBuffer as *mut f32;
        let silent = inp.u32BufferFlags == BUFFER_SILENT;
        out.u32ValidFrameCount = n as u32;
        out.u32BufferFlags = BUFFER_VALID;

        let t0 = std::time::Instant::now();
        let Ok(mut guard) = self.rt.try_lock() else { return };
        let rt = &mut *guard;
        let ch = rt.channels;
        if !rt.locked || ch == 0 {
            return;
        }
        // Too big for the scratch buffer (never expected): pass through.
        if n > rt.scratch.len() {
            if !silent && src != dst as *const f32 {
                unsafe { std::ptr::copy_nonoverlapping(src, dst, n * ch) };
            }
            return;
        }

        #[cfg(target_arch = "x86_64")]
        #[allow(deprecated)]
        let csr = unsafe {
            use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
            let c = _mm_getcsr();
            _mm_setcsr(c | 0x8040); // flush denormals to zero
            c
        };

        // Pick up a newly built chain (only once the watcher has emptied `trash`).
        if let Some(inst) = &rt.inst {
            use std::sync::atomic::Ordering::{AcqRel, Acquire, Release};
            if inst.trash.load(Acquire).is_null() {
                let p = inst.pending.swap(std::ptr::null_mut(), AcqRel);
                if !p.is_null() {
                    let mut new = unsafe { Box::from_raw(p) };
                    if let Some(mut old) = rt.chain.take() {
                        new.adopt_state(&mut old);
                        inst.trash.store(Box::into_raw(old), Release);
                    }
                    rt.chain = Some(new);
                }
            }
        }

        let sh = host::shared().filter(|s| s.valid());
        let bypass = sh.is_some_and(|s| s.bypass.load(Relaxed) != 0);
        let Rt { scratch, chain, own_meters, rate, .. } = rt;
        let meters: &Meters = match (sh, own_meters.as_deref()) {
            (Some(s), _) => &s.meters,
            (None, Some(m)) => m,
            (None, None) => return,
        };

        // The configured input channel (or the average of all), as mono.
        let pick = sh.map(|s| s.input_channel.load(Relaxed)).unwrap_or(0);
        let buf = &mut scratch[..n];
        if silent {
            buf.fill(0.0);
        } else if pick != engine::shared::MIX_CHANNELS {
            let c = if (pick as usize) < ch { pick as usize } else { 0 };
            for (i, m) in buf.iter_mut().enumerate() {
                *m = unsafe { *src.add(i * ch + c) };
            }
        } else {
            let k = 1.0 / ch as f32;
            for (i, m) in buf.iter_mut().enumerate() {
                let mut acc = 0.0;
                for c in 0..ch {
                    acc += unsafe { *src.add(i * ch + c) };
                }
                *m = acc * k;
            }
        }
        let (ip, ir) = block_levels(buf);
        meters.in_peak.raise(ip);
        meters.in_rms.raise(ir);
        if ip > f32::from_bits(self.stats.in_peak.load(Relaxed)) {
            self.stats.in_peak.store(ip.to_bits(), Relaxed);
        }

        match (chain.as_deref_mut(), bypass) {
            (Some(chain), false) => {
                chain.process(buf, meters);
                for (i, &v) in buf.iter().enumerate() {
                    for c in 0..ch {
                        unsafe { *dst.add(i * ch + c) = v };
                    }
                }
            }
            _ => {
                // Untouched pass-through.
                if silent {
                    unsafe { std::ptr::write_bytes(dst, 0, n * ch) };
                } else if src != dst as *const f32 {
                    unsafe { std::ptr::copy_nonoverlapping(src, dst, n * ch) };
                }
            }
        }
        let out_view = unsafe { std::slice::from_raw_parts(dst as *const f32, n * ch) };
        let (op, or) = block_levels(out_view);
        meters.out_peak.raise(op);
        meters.out_rms.raise(or);

        let s = &self.stats;
        s.calls.fetch_add(1, Relaxed);
        s.frames.fetch_add(n as u64, Relaxed);
        if silent {
            s.silent_calls.fetch_add(1, Relaxed);
        }
        let ns = t0.elapsed().as_nanos() as u64;
        s.total_ns.fetch_add(ns, Relaxed);
        s.max_ns.fetch_max(ns, Relaxed);
        if let Some(sh) = sh {
            sh.frames.fetch_add(n as u64, Relaxed);
            sh.rate.store(*rate as u32, Relaxed);
            sh.channels.store(ch as u32, Relaxed);
        }

        #[cfg(target_arch = "x86_64")]
        #[allow(deprecated)]
        unsafe {
            std::arch::x86_64::_mm_setcsr(csr)
        };
    }
}
