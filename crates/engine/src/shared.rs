//! State shared between the WinFX app and its APO instances inside the Windows
//! audio engine (audiodg.exe): one fixed-size memory-mapped file.
//!
//! App -> APO: the chain (JSON `ChainConfig`, guarded by a seqlock) and a
//! bypass flag. APO -> app: `Meters`, a heartbeat and the stream format.
//! A file mapping needs no special privileges (unlike `Global\` objects), so
//! the unelevated app and the LOCAL SERVICE audio engine can both open it.

use crate::meters::Meters;
use std::cell::UnsafeCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};

pub const MAGIC: u32 = u32::from_le_bytes(*b"WNFX");
pub const VERSION: u32 = 1;
pub const CFG_CAP: usize = 60 * 1024;

#[repr(C)]
pub struct Shared {
    pub magic: AtomicU32,
    pub version: AtomicU32,
    /// Seqlock over `cfg`/`cfg_len`: odd while the app is writing.
    pub cfg_seq: AtomicU32,
    pub cfg_len: AtomicU32,
    /// 1: the APO passes audio through untouched.
    pub bypass: AtomicU32,
    _pad: u32,
    cfg: UnsafeCell<[u8; CFG_CAP]>,
    /// Frames processed by any APO instance (a heartbeat).
    pub frames: AtomicU64,
    /// Config sequence the APO last applied.
    pub applied_seq: AtomicU32,
    pub rate: AtomicU32,
    pub channels: AtomicU32,
    /// Live APO instances (streams).
    pub instances: AtomicU32,
    pub meters: Meters,
}

// The only non-atomic field is `cfg`, accessed under the seqlock.
unsafe impl Sync for Shared {}

impl Shared {
    pub fn valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == MAGIC && self.version.load(Ordering::Acquire) == VERSION
    }

    /// App side: stamp a fresh (zeroed) block.
    pub fn init(&self) {
        if !self.valid() {
            self.version.store(VERSION, Ordering::Relaxed);
            self.magic.store(MAGIC, Ordering::Release);
        }
    }

    /// App side (single writer). Returns false if the config doesn't fit.
    pub fn write_config(&self, json: &[u8]) -> bool {
        if json.len() > CFG_CAP {
            return false;
        }
        let s = self.cfg_seq.load(Ordering::Relaxed);
        let odd = if s % 2 == 0 { s.wrapping_add(1) } else { s };
        self.cfg_seq.store(odd, Ordering::Relaxed);
        fence(Ordering::Release);
        unsafe { std::ptr::copy_nonoverlapping(json.as_ptr(), (*self.cfg.get()).as_mut_ptr(), json.len()) };
        self.cfg_len.store(json.len() as u32, Ordering::Relaxed);
        self.cfg_seq.store(odd.wrapping_add(1), Ordering::Release);
        true
    }

    /// Reader side (not real-time: allocates). Returns the config if its
    /// sequence differs from `last` and a consistent copy could be taken.
    pub fn read_config(&self, last: u32) -> Option<(u32, Vec<u8>)> {
        let s1 = self.cfg_seq.load(Ordering::Acquire);
        if s1 % 2 == 1 || s1 == last {
            return None;
        }
        let len = (self.cfg_len.load(Ordering::Relaxed) as usize).min(CFG_CAP);
        let mut out = vec![0u8; len];
        let src = self.cfg.get() as *const u8;
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe { std::ptr::read_volatile(src.add(i)) };
        }
        fence(Ordering::Acquire);
        (self.cfg_seq.load(Ordering::Relaxed) == s1).then_some((s1, out))
    }

    pub fn config_seq(&self) -> u32 {
        self.cfg_seq.load(Ordering::Acquire)
    }
}

/// Where the shared block lives. Its folder must let both the user and
/// LOCAL SERVICE (the audio engine) read and write (`apo::ensure_data_dir`).
pub fn default_path() -> PathBuf {
    crate::apo::data_dir().join("shared.bin")
}

/// A mapped view of the shared file. Never moves; unmapped on drop.
pub struct Mapping {
    view: *mut Shared,
    mapping: windows::Win32::Foundation::HANDLE,
    _file: std::fs::File,
}

unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Open (or create) and map the file, growing it to fit `Shared`.
    pub fn open(path: &Path) -> io::Result<Mapping> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE};

        let size = size_of::<Shared>() as u64;
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        if file.metadata()?.len() < size {
            file.set_len(size)?;
        }
        unsafe {
            let h = HANDLE(file.as_raw_handle());
            let mapping = CreateFileMappingW(h, None, PAGE_READWRITE, (size >> 32) as u32, size as u32, None)
                .map_err(|e| io::Error::other(e.to_string()))?;
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, size as usize);
            if view.Value.is_null() {
                let e = io::Error::last_os_error();
                let _ = windows::Win32::Foundation::CloseHandle(mapping);
                return Err(e);
            }
            Ok(Mapping { view: view.Value as *mut Shared, mapping, _file: file })
        }
    }

    pub fn get(&self) -> &Shared {
        unsafe { &*self.view }
    }

    /// For a mapping kept for the life of the process.
    pub fn leak(self) -> &'static Shared {
        let r = unsafe { &*self.view };
        std::mem::forget(self);
        r
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        use windows::Win32::System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile};
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as _ });
            let _ = windows::Win32::Foundation::CloseHandle(self.mapping);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_through_two_mappings() {
        let path = std::env::temp_dir().join(format!("winfx-shared-test-{}.bin", std::process::id()));
        let a = Mapping::open(&path).unwrap();
        let b = Mapping::open(&path).unwrap();
        a.get().init();
        assert!(b.get().valid());
        assert!(b.get().read_config(0).is_none() || b.get().config_seq() == 0);
        assert!(a.get().write_config(br#"{"slots":[]}"#));
        let (seq, cfg) = b.get().read_config(0).unwrap();
        assert_eq!(cfg, br#"{"slots":[]}"#);
        assert!(b.get().read_config(seq).is_none(), "unchanged config is not re-read");
        b.get().meters.in_peak.raise(0.5);
        assert_eq!(a.get().meters.in_peak.take(), 0.5);
        drop((a, b));
        let _ = std::fs::remove_file(path);
    }
}
