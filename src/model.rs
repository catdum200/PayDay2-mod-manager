//! Shared data types used across the app.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    ModWorkshop,
    Nexus,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagChip {
    pub id: Option<u64>,
    pub name: String,
    /// RGB colour if the site provides one.
    pub color: Option<[u8; 3]>,
}

/// A mod listed on a remote site.
#[derive(Debug, Clone)]
pub struct RemoteMod {
    pub source: Source,
    pub id: u64,
    pub name: String,
    pub author: String,
    pub summary: String,
    pub description: String,
    pub thumb: Option<String>,
    pub image: Option<String>,
    pub tags: Vec<TagChip>,
    pub category: String,
    pub downloads: u64,
    pub likes: u64,
    pub version: String,
    pub updated: String,
    pub nsfw: bool,
    pub page_url: String,
}

impl RemoteMod {
    /// Minimal entry for a mod known only by site and id (dependencies, share codes,
    /// the BeardLib button). The installer fetches everything else it needs.
    pub fn stub(source: Source, id: u64, name: impl Into<String>, page_url: impl Into<String>) -> Self {
        RemoteMod {
            source,
            id,
            name: name.into(),
            author: String::new(),
            summary: String::new(),
            description: String::new(),
            thumb: None,
            image: None,
            tags: vec![],
            category: String::new(),
            downloads: 0,
            likes: 0,
            version: String::new(),
            updated: String::new(),
            nsfw: false,
            page_url: page_url.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum FileKind {
    /// Direct HTTP download.
    Direct(String),
    /// External link (may or may not be a direct archive).
    Link(String),
    /// Nexus file — needs API key / nxm to resolve.
    Nexus { mod_id: u64, file_id: u64 },
}

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub id: u64,
    pub name: String,
    pub version: String,
    pub size: u64,
    pub label: String,
    pub kind: FileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum ModKind {
    /// SuperBLT Lua mod in `mods/`.
    Blt,
    /// Asset override / BeardLib mod in `assets/mod_overrides/`.
    Override,
    /// BeardLib custom heist in `Maps/`.
    Map,
    /// Files copied into the game root (e.g. SuperBLT itself).
    Root,
}

impl ModKind {
    pub fn label(self) -> &'static str {
        match self {
            ModKind::Blt => "BLT",
            ModKind::Override => "Override",
            ModKind::Map => "Map",
            ModKind::Root => "Game root",
        }
    }
    pub fn dir_name(self) -> &'static str {
        match self {
            ModKind::Blt => "mods",
            ModKind::Override => "assets/mod_overrides",
            ModKind::Map => "Maps",
            ModKind::Root => "",
        }
    }
    pub fn key_prefix(self) -> &'static str {
        match self {
            ModKind::Blt => "mods",
            ModKind::Override => "overrides",
            ModKind::Map => "maps",
            ModKind::Root => "root",
        }
    }
}

/// A mod present in the game folder (tracked by the manager or found on disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledMod {
    /// Unique key: `<kind prefix>/<folder>`.
    pub key: String,
    pub folder: String,
    pub kind: ModKind,
    pub name: String,
    pub version: String,
    pub author: String,
    pub source: Source,
    pub source_id: Option<u64>,
    pub page_url: Option<String>,
    #[serde(default)]
    pub site_tags: Vec<String>,
    #[serde(default)]
    pub user_tags: Vec<String>,
    pub enabled: bool,
    #[serde(default)]
    pub nsfw: bool,
    #[serde(default)]
    pub installed_at: u64,
    /// For `Root` mods: relative paths of files we copied.
    #[serde(default)]
    pub root_files: Vec<String>,
    /// Mod version as listed on the website at install time (used for update checks).
    #[serde(default)]
    pub source_version: String,
    #[serde(skip)]
    pub update_available: Option<String>,
}

impl InstalledMod {
    pub fn make_key(kind: ModKind, folder: &str) -> String {
        format!("{}/{}", kind.key_prefix(), folder.to_lowercase())
    }
}

/// A mod that another mod needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// Where we can install it from (Local = unknown / off-site only).
    pub source: Source,
    pub id: Option<u64>,
    pub name: String,
    pub url: Option<String>,
    pub optional: bool,
}

/// What the user picked to install.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub source: Source,
    pub mod_info: Option<RemoteMod>,
    pub file: Option<RemoteFile>,
    /// Local archive path (drag & drop / file picker / nxm-downloaded).
    pub local_path: Option<std::path::PathBuf>,
    /// nxm:// link to resolve.
    pub nxm: Option<String>,
    pub display_name: String,
}
