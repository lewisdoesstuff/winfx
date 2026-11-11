//! App-level state that outlives the window: the engine, persisted config,
//! device lists and presets. Lives in an `Entity<AppState>` held by a Global.

use engine::devices::{self, DeviceInfo, Dir, Role};
use engine::{ChainConfig, DeviceSel, Engine, EngineConfig};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub engine: EngineConfig,
    #[serde(default)]
    pub preset: Option<String>,
    /// Last window position/size, restored on open.
    #[serde(default)]
    pub window: Option<SavedWindow>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub maximized: bool,
}

pub struct AppState {
    pub engine: Engine,
    pub config: AppConfig,
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub presets: Vec<String>,
    /// Preview mode: synthetic engine, nothing is written to disk.
    pub preview: bool,
    /// File to import once the window opens (from the command line).
    pub pending_import: Option<PathBuf>,
    dirty_since: Option<Instant>,
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("winfx")
}

fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// "2026-11-11 19-42-05" in local time.
pub fn local_timestamp() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

fn presets_dir() -> PathBuf {
    data_dir().join("presets")
}

pub fn is_cable(d: &DeviceInfo) -> bool {
    d.name.contains("VB-Audio")
}

impl AppState {
    pub fn load(preview: bool) -> Self {
        let saved: Option<AppConfig> = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let first_run = saved.is_none();
        let mut config =
            saved.unwrap_or(AppConfig {
            engine: EngineConfig::default(),
            preset: None,
            window: None,
        });
        let first_run = first_run && !preview;

        let mut s = AppState {
            engine: if preview {
                Engine::start_preview(config.engine.clone())
            } else {
                Engine::start(config.engine.clone())
            },
            config: config.clone(),
            inputs: vec![],
            outputs: vec![],
            presets: vec![],
            preview,
            pending_import: std::env::args_os()
                .skip(1)
                .map(PathBuf::from)
                .find(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("txt"))),
            dirty_since: None,
        };
        s.refresh_devices();
        s.refresh_presets();

        if first_run || config.engine.input == DeviceSel::Default {
            if let Some(mic) = s.guess_mic() {
                config.engine.input = DeviceSel::device(&mic);
                s.config = config;
                s.engine.set_config(s.config.engine.clone());
                s.save();
            }
        }
        if first_run {
            let _ = std::fs::create_dir_all(presets_dir());
            if s.presets.is_empty() {
                s.save_preset("SM57 voice");
                s.config.preset = Some("SM57 voice".into());
                s.save();
            }
        }
        s
    }

    /// Default input that isn't the cable or a loopback; prefer the Windows default.
    fn guess_mic(&self) -> Option<DeviceInfo> {
        let ok = |d: &DeviceInfo| d.active && !is_cable(d) && !d.name.starts_with("Loop-back");
        devices::default_device(Dir::Input, Role::Console)
            .filter(ok)
            .or_else(|| self.inputs.iter().find(|d| ok(d)).cloned())
    }

    pub fn refresh_devices(&mut self) {
        self.inputs = devices::list(Dir::Input).unwrap_or_default().into_iter().filter(|d| d.active).collect();
        self.outputs = devices::list(Dir::Output).unwrap_or_default().into_iter().filter(|d| d.active).collect();
    }

    pub fn refresh_presets(&mut self) {
        let mut names: Vec<String> = std::fs::read_dir(presets_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                Some(p.file_stem()?.to_string_lossy().into_owned())
            })
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        self.presets = names;
    }

    pub fn chain(&self) -> &ChainConfig {
        &self.config.engine.chain
    }

    /// Apply an edited chain live and schedule a save.
    pub fn set_chain(&mut self, chain: ChainConfig) {
        self.config.engine.chain = chain;
        self.engine.set_chain(self.config.engine.chain.clone());
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    /// Apply device/stream settings (restarts audio) and save now.
    pub fn set_engine_config(&mut self, f: impl FnOnce(&mut EngineConfig)) {
        f(&mut self.config.engine);
        self.engine.set_config(self.config.engine.clone());
        self.save();
    }

    /// Monitor on/off or device change: restarts only the monitor stream.
    pub fn set_monitor(&mut self, f: impl FnOnce(&mut engine::MonitorConfig)) {
        f(&mut self.config.engine.monitor);
        self.engine.set_monitor(self.config.engine.monitor.clone());
        self.save();
    }

    pub fn set_monitor_volume(&mut self, db: f32) {
        self.config.engine.monitor.volume_db = db;
        self.engine.set_monitor_volume(db);
        self.dirty_since.get_or_insert_with(Instant::now);
    }

    /// Playback devices usable for monitoring (not the cable).
    pub fn monitor_outputs(&self) -> Vec<DeviceInfo> {
        self.outputs.iter().filter(|d| !is_cable(d)).cloned().collect()
    }

    pub fn save_if_dirty(&mut self) {
        if self.dirty_since.is_some_and(|t| t.elapsed() > Duration::from_millis(750)) {
            self.save();
        }
    }

    pub fn save(&mut self) {
        self.dirty_since = None;
        if self.preview {
            return;
        }
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(json) = serde_json::to_string_pretty(&self.config) {
            let tmp = config_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, config_path());
            }
        }
    }

    pub fn save_preset(&mut self, name: &str) {
        if self.preview {
            return;
        }
        let _ = std::fs::create_dir_all(presets_dir());
        if let Ok(json) = serde_json::to_string_pretty(self.chain()) {
            let _ = std::fs::write(presets_dir().join(format!("{}.json", sanitize(name))), json);
        }
        self.config.preset = Some(sanitize(name));
        self.refresh_presets();
        self.save();
    }

    pub fn load_preset(&mut self, name: &str) -> bool {
        let Some(chain) = std::fs::read_to_string(presets_dir().join(format!("{name}.json")))
            .ok()
            .and_then(|s| serde_json::from_str::<ChainConfig>(&s).ok())
        else {
            return false;
        };
        self.config.preset = Some(name.to_owned());
        self.set_chain(chain);
        self.save();
        true
    }

    /// Import an EasyEffects preset (becomes a new preset, then loaded) or an
    /// Equalizer APO / AutoEQ file (replaces the EQ in the current chain).
    pub fn import_file(&mut self, path: &std::path::Path) -> Result<ImportOutcome, String> {
        use engine::import::{import, Imported};
        let text = std::fs::read_to_string(path).map_err(|e| format!("couldn't read the file: {e}"))?;
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into());
        match import(&text)? {
            Imported::Chain(chain, report) => {
                let name = self.unique_preset_name(&sanitize(&stem));
                self.set_chain(chain);
                self.save_preset(&name); // writes it and selects it
                self.config.preset = Some(name.clone());
                self.save();
                Ok(ImportOutcome { title: format!("Imported “{name}”"), report })
            }
            Imported::Eq(eq, report) => {
                let mut chain = self.chain().clone();
                let bands = eq.bands.len();
                match chain.slots.iter_mut().find(|s| matches!(s.effect, engine::EffectConfig::Eq(_))) {
                    Some(slot) => {
                        slot.effect = engine::EffectConfig::Eq(eq);
                        slot.bypass = false;
                    }
                    None => {
                        let id = chain.next_id();
                        // After any leading filters, before dynamics.
                        let at = chain
                            .slots
                            .iter()
                            .position(|s| !matches!(s.effect, engine::EffectConfig::Filter(_)))
                            .unwrap_or(chain.slots.len());
                        chain.slots.insert(at, engine::EffectSlot { id, bypass: false, effect: engine::EffectConfig::Eq(eq) });
                    }
                }
                self.set_chain(chain);
                Ok(ImportOutcome { title: format!("Loaded {bands}-band EQ from “{stem}”"), report })
            }
        }
    }

    fn unique_preset_name(&self, base: &str) -> String {
        if !self.presets.iter().any(|p| p == base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} ({n})")).find(|n| !self.presets.contains(n)).unwrap()
    }
}

pub struct ImportOutcome {
    pub title: String,
    pub report: engine::import::ImportReport,
}

impl ImportOutcome {
    /// One readable paragraph for the notice.
    pub fn summary(&self) -> String {
        let r = &self.report;
        let mut parts = vec![];
        if !r.mapped.is_empty() {
            parts.push(format!("Imported: {}.", r.mapped.join(", ")));
        }
        if !r.approximated.is_empty() {
            parts.push(format!("Approximated: {}.", r.approximated.join("; ")));
        }
        if !r.skipped.is_empty() {
            parts.push(format!("Not supported, skipped: {}.", r.skipped.join(", ")));
        }
        parts.join(" ")
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' })
        .collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "Preset".into() } else { s }
}
