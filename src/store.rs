//! Everything that is remembered between runs: settings, per-display state, profiles.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Gamma, OutputRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Anchor {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
    Center,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Brightness slider continues below the hardware minimum with software dimming.
    pub combined_brightness: bool,
    /// Talk to monitors over DDC/CI.
    pub ddc_enabled: bool,
    /// ddcutil sleep multiplier: below 1.0 is faster but less tolerant.
    pub ddc_speed: f32,
    /// Percent per step for `penumbra brightness +` and the scroll wheel.
    pub step: u32,
    /// Put the saved DDC values back when a monitor appears.
    pub restore_on_connect: bool,
    /// Put the saved layout (resolution, scale, position) back when a monitor appears.
    pub restore_layout_on_connect: bool,
    /// External monitors follow the built-in panel's brightness.
    pub follow_internal: bool,
    pub panel_anchor: Anchor,
    pub panel_margin: i32,
    /// Virtual screens to create again at start.
    pub virtual_displays: Vec<VirtualDisplay>,
    pub show_welcome: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            combined_brightness: true,
            ddc_enabled: true,
            ddc_speed: 1.0,
            step: 5,
            restore_on_connect: true,
            restore_layout_on_connect: false,
            follow_internal: false,
            panel_anchor: Anchor::TopRight,
            panel_margin: 8,
            virtual_displays: Vec::new(),
            show_welcome: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VirtualDisplay {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub hz: u32,
}

/// What is remembered about one monitor, found again by its identity.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayMemory {
    pub nickname: Option<String>,
    pub hidden: bool,
    /// Brightness slider position, 0.0 - 1.0.
    pub brightness: Option<f64>,
    pub gamma: Gamma,
    /// DDC values by feature code (hex string keys, for readable files).
    pub ddc: BTreeMap<String, u32>,
    pub layout: Option<OutputRequest>,
    pub follow_internal: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub displays: BTreeMap<String, DisplayMemory>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub settings: Settings,
    pub displays: BTreeMap<String, DisplayMemory>,
    pub profiles: BTreeMap<String, Profile>,
}

pub struct Store {
    path: PathBuf,
    pub data: Data,
}

impl Store {
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("PENUMBRA_CONFIG") {
            return PathBuf::from(p);
        }
        directories::ProjectDirs::from("dev", "alecsh", "penumbra")
            .map(|d| d.config_dir().join("config.json"))
            .unwrap_or_else(|| PathBuf::from("penumbra.json"))
    }

    pub fn load(path: PathBuf) -> Store {
        let data = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Store { path, data }
    }

    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(&self.data) {
            if fs::write(&tmp, text).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn memory(&mut self, id: &str) -> &mut DisplayMemory {
        self.data.displays.entry(id.to_string()).or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("penumbra-test-{}", std::process::id()));
        let path = dir.join("c.json");
        let mut s = Store::load(path.clone());
        assert!(s.data.settings.combined_brightness);
        s.memory("a|b|c").brightness = Some(0.4);
        s.data.settings.step = 10;
        s.save();
        let t = Store::load(path);
        assert_eq!(t.data.settings.step, 10);
        assert_eq!(t.data.displays["a|b|c"].brightness, Some(0.4));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn partial_file_still_loads() {
        let d: Data = serde_json::from_str(r#"{"settings":{"step":7}}"#).unwrap();
        assert_eq!(d.settings.step, 7);
        assert!(d.settings.ddc_enabled);
    }
}
