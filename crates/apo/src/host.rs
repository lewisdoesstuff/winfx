//! Per-process side of the APO: the shared-memory link to the WinFX app and a
//! watcher thread that turns config changes into ready-built `Chain`s.
//!
//! Real-time hand-off, per APO instance (stream):
//!   watcher: build Chain -> `pending` (swap; drops any unclaimed one)
//!   RT:      if `trash` is empty, take `pending`, adopt the old chain's state,
//!            put the old chain in `trash`
//!   watcher: free whatever is in `trash`
//! so the real-time thread never allocates or frees.

use crate::log;
use engine::shared::{Mapping, Shared, default_path};
use engine::{Chain, ChainConfig};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub(crate) struct Inst {
    pub pending: AtomicPtr<Chain>,
    pub trash: AtomicPtr<Chain>,
    /// Sample rate (f32 bits); fixed once locked.
    pub rate: AtomicU32,
    pub live: AtomicBool,
    /// Config sequence this instance has been given (watcher-only).
    applied: AtomicU32,
}

impl Inst {
    pub fn new(rate: f32) -> Arc<Inst> {
        Arc::new(Inst {
            pending: AtomicPtr::new(std::ptr::null_mut()),
            trash: AtomicPtr::new(std::ptr::null_mut()),
            rate: AtomicU32::new(rate.to_bits()),
            live: AtomicBool::new(true),
            applied: AtomicU32::new(u32::MAX),
        })
    }

    /// Free anything parked for the watcher (non-RT callers only).
    pub fn drain(&self) {
        for slot in [&self.pending, &self.trash] {
            let p = slot.swap(std::ptr::null_mut(), AcqRel);
            if !p.is_null() {
                drop(unsafe { Box::from_raw(p) });
            }
        }
    }
}

static REGISTRY: Mutex<Vec<Arc<Inst>>> = Mutex::new(Vec::new());
static SHARED: AtomicPtr<Shared> = AtomicPtr::new(std::ptr::null_mut());
static WATCHER: OnceLock<()> = OnceLock::new();

/// The shared block, once the watcher has mapped it.
pub(crate) fn shared() -> Option<&'static Shared> {
    let p = SHARED.load(Acquire);
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

pub(crate) fn register(inst: Arc<Inst>) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.push(inst);
    }
    WATCHER.get_or_init(|| {
        let _ = std::thread::Builder::new().name("winfx-apo-watcher".into()).spawn(watch);
    });
}

fn watch() {
    log::line("watcher started");
    let mut last_open_try: Option<Instant> = None;
    let mut seq = 0u32;
    let mut cfg: Option<ChainConfig> = None;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Map the shared file (retry every 2 s until it works).
            if shared().is_none() && last_open_try.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                last_open_try = Some(Instant::now());
                match Mapping::open(&default_path()) {
                    Ok(m) => {
                        let s = m.leak();
                        SHARED.store(s as *const Shared as *mut Shared, Release);
                        log::line(&format!("mapped {}", default_path().display()));
                    }
                    Err(e) => log::line(&format!("can't map {}: {e}", default_path().display())),
                }
            }
            if let Some(sh) = shared() {
                if sh.valid() {
                    if let Some((s, bytes)) = sh.read_config(seq) {
                        seq = s;
                        match serde_json::from_slice::<ChainConfig>(&bytes) {
                            Ok(c) => {
                                log::line(&format!("config {s}: {} effects", c.slots.len()));
                                cfg = Some(c);
                            }
                            Err(e) => log::line(&format!("config {s}: bad JSON ({e})")),
                        }
                    }
                }
            }
            let Ok(mut reg) = REGISTRY.lock() else { return };
            reg.retain(|i| {
                // Collect the old chains RT handed back.
                let t = i.trash.swap(std::ptr::null_mut(), AcqRel);
                if !t.is_null() {
                    drop(unsafe { Box::from_raw(t) });
                }
                if !i.live.load(Acquire) {
                    i.drain();
                    return false;
                }
                if let Some(c) = &cfg {
                    if i.applied.load(Relaxed) != seq {
                        let chain = Box::into_raw(Box::new(Chain::build(c, f32::from_bits(i.rate.load(Relaxed)))));
                        let old = i.pending.swap(chain, AcqRel);
                        if !old.is_null() {
                            drop(unsafe { Box::from_raw(old) });
                        }
                        i.applied.store(seq, Relaxed);
                        if let Some(sh) = shared() {
                            sh.applied_seq.store(seq, Release);
                        }
                    }
                }
                true
            });
            if let Some(sh) = shared() {
                sh.instances.store(reg.len() as u32, Relaxed);
            }
        }));
        if r.is_err() {
            log::line("watcher iteration panicked; continuing");
        }
    }
}
