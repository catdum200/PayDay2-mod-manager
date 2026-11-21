//! Settings and the installed-mod database, stored as JSON in %APPDATA%\PD2ModManager.

use crate::model::InstalledMod;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    // PD2MM_DATA_DIR lets automated tests run without touching the real settings.
    let dir = match std::env::var_os("PD2MM_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("PD2ModManager"),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("PD2ModManager");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Files edited by Notepad/PowerShell often start with a byte-order mark that JSON parsers reject.
fn strip_bom(s: &str) -> &str {
    s.trim_start_matches('\u{feff}')
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub game_dir: Option<PathBuf>,
    /// NSFW mods are hidden unless the user explicitly turns this off.
    #[serde(default = "default_true")]
    pub hide_nsfw: bool,
    #[serde(default)]
    pub nexus_api_key: String,
    #[serde(default)]
    pub ui_scale: Option<f32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { game_dir: None, hide_nsfw: true, nexus_api_key: String::new(), ui_scale: None }
    }
}

impl Settings {
    fn path() -> PathBuf {
        data_dir().join("settings.json")
    }
    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(strip_bom(&s)).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), s);
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModDb {
    pub mods: BTreeMap<String, InstalledMod>,
}

impl ModDb {
    fn path() -> PathBuf {
        data_dir().join("installed.json")
    }
    pub fn load() -> Self {
        let Ok(raw) = std::fs::read_to_string(Self::path()) else { return Self::default() };
        match serde_json::from_str(strip_bom(&raw)) {
            Ok(db) => db,
            Err(_) => {
                // Keep the unreadable file instead of silently overwriting it.
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = std::fs::copy(Self::path(), data_dir().join(format!("installed.broken-{stamp}.json")));
                Self::default()
            }
        }
    }
    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let tmp = Self::path().with_extension("json.tmp");
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, Self::path());
            }
        }
    }
}
