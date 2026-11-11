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
}

pub struct AppState {
    pub engine: Engine,
    pub config: AppConfig,
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
    pub comms_input: Option<DeviceInfo>,
    pub presets: Vec<String>,
    dirty_since: Option<Instant>,
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("winfx")
}

fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

fn presets_dir() -> PathBuf {
    data_dir().join("presets")
}

pub fn is_cable(d: &DeviceInfo) -> bool {
    d.name.contains("VB-Audio")
}

impl AppState {
    pub fn load() -> Self {
        let saved: Option<AppConfig> = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let first_run = saved.is_none();
        let mut config = saved.unwrap_or(AppConfig { engine: EngineConfig::default(), preset: None });

        let mut s = AppState {
            engine: Engine::start(EngineConfig {
                // Start idle-safe: real config applied below once devices are known.
                ..config.engine.clone()
            }),
            config: config.clone(),
            inputs: vec![],
            outputs: vec![],
            comms_input: None,
            presets: vec![],
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
        self.comms_input = devices::default_device(Dir::Input, Role::Communications);
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
        let _ = std::fs::create_dir_all(data_dir());
        if let Ok(json) = serde_json::to_string_pretty(&self.config) {
            let tmp = config_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, config_path());
            }
        }
    }

    pub fn save_preset(&mut self, name: &str) {
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
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' })
        .collect();
    let s = s.trim().to_owned();
    if s.is_empty() { "Preset".into() } else { s }
}
