//! Locating the PAYDAY 2 install and managing mod folders on disk.

use crate::config::ModDb;
use crate::model::{InstalledMod, ModKind, Source};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

pub const EPIC_APP_NAME: &str = "7d6b8784bab94d6cac5fea5ab7bb07a4";
const BLT_SKIP: &[&str] = &["base", "logs", "saves", "downloads"];
pub const DISABLED_DIR: &str = "PD2MM_disabled";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Epic,
    Steam,
    Manual,
}

impl Store {
    pub fn label(self) -> &'static str {
        match self {
            Store::Epic => "Epic Games",
            Store::Steam => "Steam",
            Store::Manual => "Custom folder",
        }
    }
}

pub fn is_game_dir(p: &Path) -> bool {
    p.join("PAYDAY2.exe").is_file() || p.join("payday2_win32_release.exe").is_file()
}

/// Find PAYDAY 2 via Epic launcher manifests, then Steam.
pub fn detect() -> Option<(PathBuf, Store)> {
    if let Some(p) = detect_epic() {
        return Some((p, Store::Epic));
    }
    if let Some(p) = detect_steam() {
        return Some((p, Store::Steam));
    }
    None
}

pub fn store_of(p: &Path) -> Store {
    if p.join(".egstore").exists() {
        Store::Epic
    } else if p.to_string_lossy().to_lowercase().contains("steamapps") {
        Store::Steam
    } else {
        Store::Manual
    }
}

fn detect_epic() -> Option<PathBuf> {
    let program_data = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let dir = Path::new(&program_data).join("Epic\\EpicGamesLauncher\\Data\\Manifests");
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "item").unwrap_or(true) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let app = v["AppName"].as_str().unwrap_or_default();
        let display = v["DisplayName"].as_str().unwrap_or_default().to_lowercase();
        if app == EPIC_APP_NAME || display.contains("payday 2") || display == "payday2" {
            if let Some(loc) = v["InstallLocation"].as_str() {
                let p = PathBuf::from(loc.replace('/', "\\"));
                if is_game_dir(&p) {
                    return Some(p);
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn detect_steam() -> Option<PathBuf> {
    use winreg::enums::*;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let steam: String = hkcu.open_subkey("Software\\Valve\\Steam").ok()?.get_value("SteamPath").ok()?;
    let steam = PathBuf::from(steam.replace('/', "\\"));
    let mut libs = vec![steam.clone()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps\\libraryfolders.vdf")) {
        for line in vdf.lines() {
            let t = line.trim();
            if t.starts_with("\"path\"") {
                if let Some(val) = t.split('"').nth(3) {
                    libs.push(PathBuf::from(val.replace("\\\\", "\\")));
                }
            }
        }
    }
    libs.into_iter()
        .map(|l| l.join("steamapps\\common\\PAYDAY 2"))
        .find(|p| is_game_dir(p))
}

#[cfg(not(windows))]
fn detect_steam() -> Option<PathBuf> {
    None
}

#[derive(Debug, Clone, Default)]
pub struct LoaderStatus {
    pub superblt: bool,
    pub beardlib: bool,
}

pub fn loader_status(game: &Path) -> LoaderStatus {
    let dll = game.join("WSOCK32.dll").is_file() || game.join("IPHLPAPI.dll").is_file();
    LoaderStatus {
        superblt: dll && game.join("mods\\base").is_dir(),
        beardlib: game.join("mods\\BeardLib").is_dir() || game.join("mods\\beardlib").is_dir(),
    }
}

pub fn kind_dir(game: &Path, kind: ModKind) -> PathBuf {
    match kind {
        ModKind::Root => game.to_path_buf(),
        k => game.join(k.dir_name()),
    }
}

pub fn disabled_dir(game: &Path, kind: ModKind) -> PathBuf {
    game.join(DISABLED_DIR).join(kind.key_prefix())
}

pub fn mod_path(game: &Path, m: &InstalledMod) -> PathBuf {
    if m.enabled {
        kind_dir(game, m.kind).join(&m.folder)
    } else {
        disabled_dir(game, m.kind).join(&m.folder)
    }
}

/// Read name/version/author from a BLT mod.txt (lenient JSON).
pub fn read_mod_txt(dir: &Path) -> Option<(String, String, String)> {
    let raw = std::fs::read_to_string(dir.join("mod.txt")).ok()?;
    let raw = raw.trim_start_matches('\u{feff}');
    let v: Option<serde_json::Value> =
        serde_json::from_str(raw).ok().or_else(|| json5::from_str(raw).ok());
    if let Some(v) = v {
        let s = |k: &str| match &v[k] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        return Some((s("name"), s("version"), s("author")));
    }
    // Fallback: crude scan for "name" : "..."
    let find = |key: &str| -> String {
        let pat = format!("\"{key}\"");
        raw.find(&pat)
            .and_then(|i| raw[i + pat.len()..].split('"').nth(1).map(|s| s.to_string()))
            .unwrap_or_default()
    };
    Some((find("name"), find("version"), find("author")))
}

/// Read a mod name from a SuperBLT supermod.xml or BeardLib main.xml: the `name`
/// attribute of the top-level element (`<mod name = "…">`, `<table name="…">`).
pub fn read_main_xml(dir: &Path) -> Option<String> {
    ["supermod.xml", "main.xml"].iter().find_map(|f| {
        let raw = std::fs::read_to_string(dir.join(f)).ok()?;
        root_attr(&raw, "name").map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    })
}

/// Value of `attr` on the first real element of an XML document. Skips the
/// `<?xml?>` prolog, comments and doctypes, and allows spaces around `=`.
fn root_attr(raw: &str, attr: &str) -> Option<String> {
    let mut rest = raw;
    loop {
        rest = &rest[rest.find('<')?..];
        if rest.starts_with("<?") {
            rest = &rest[rest.find("?>")? + 2..];
        } else if rest.starts_with("<!--") {
            rest = &rest[rest.find("-->")? + 3..];
        } else if rest.starts_with("<!") {
            rest = &rest[rest.find('>')? + 1..];
        } else {
            break;
        }
    }
    // Skip "<" and the element name.
    let mut s = rest[1..].trim_start_matches(|c: char| !c.is_whitespace() && c != '>' && c != '/');
    loop {
        s = s.trim_start();
        if s.is_empty() || s.starts_with('>') || s.starts_with("/>") {
            return None;
        }
        let eq = s.find('=')?;
        let key = s[..eq].trim();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = after[1..].find(quote)?;
        if key == attr {
            return Some(after[1..1 + end].to_string());
        }
        s = &after[1 + end + 1..];
    }
}

/// True if `dir` is a SuperBLT mod (it must live in `mods/`, never in mod_overrides).
pub fn is_blt_mod_dir(dir: &Path) -> bool {
    if dir.join("supermod.xml").is_file() {
        return true;
    }
    // mod.txt must be real BLT JSON with a name, not just a readme.
    let Ok(raw) = std::fs::read_to_string(dir.join("mod.txt")) else { return false };
    let raw = raw.trim_start_matches('\u{feff}');
    let v: Option<serde_json::Value> = serde_json::from_str(raw).ok().or_else(|| json5::from_str(raw).ok());
    v.map(|v| v["name"].is_string()).unwrap_or(false)
}

pub const ANNOUNCER_CARD: &str = "battle_announcers_identity_card.txt";
/// Event lists a Battle Announcers config.txt can contain.
const ANNOUNCER_EVENTS: &[&str] = &["OnEnemyKilled", "OnHeistComplete", "OnPlayerDodge", "OnPlayerDamage"];

/// True if `dir` is a Battle Announcers voice pack (a `config.txt` with announcer
/// events) that isn't yet wrapped in the layout Battle Announcers scans for.
pub fn is_bare_announcer_pack(dir: &Path) -> bool {
    if dir.join(ANNOUNCER_CARD).is_file() {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(dir.join("config.txt")) else { return false };
    ANNOUNCER_EVENTS.iter().any(|e| raw.contains(&format!("\"{e}\"")))
}

/// The sub-folder name a pack's config expects, taken from its sound/icon paths
/// (e.g. `"sound": "Anis_SS/x.ogg"` gives `Anis_SS`). Falls back to the folder name.
fn announcer_pack_name(dir: &Path) -> String {
    let fallback = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Pack".into());
    let Ok(raw) = std::fs::read_to_string(dir.join("config.txt")) else { return fallback };
    for key in ["\"sound\"", "\"icon\""] {
        if let Some(i) = raw.find(key) {
            let val = raw[i + key.len()..].split('"').nth(1).unwrap_or("");
            if let Some((prefix, _)) = val.replace('\\', "/").split_once('/') {
                let prefix = prefix.trim();
                if !prefix.is_empty() && !prefix.contains("..") && !prefix.contains(':') {
                    return prefix.to_string();
                }
            }
        }
    }
    fallback
}

/// Battle Announcers only loads `<mods or mod_overrides>/<X>/files/<pack>/config.txt`
/// when `<X>/battle_announcers_identity_card.txt` exists, and resolves sound paths
/// relative to `<X>/files/`. Rearrange a bare pack at `dir` into that layout in place.
pub fn wrap_announcer_pack(dir: &Path) -> Result<()> {
    let inner = announcer_pack_name(dir);
    let tmp = dir.with_file_name(format!(
        "{}.pd2mm_tmp",
        dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    ));
    move_dir(dir, &tmp)?;
    let target = dir.join("files").join(&inner);
    if let Err(e) = move_dir(&tmp, &target) {
        let _ = move_dir(&tmp, dir); // put it back as it was
        return Err(e);
    }
    std::fs::write(dir.join(ANNOUNCER_CARD), "")?;
    Ok(())
}

/// Move SuperBLT mods that ended up in assets/mod_overrides (which makes SuperBLT
/// crash with "failed to load wren system") into mods/, and wrap bare Battle
/// Announcers packs so they get loaded. Returns the names fixed.
pub fn repair_misplaced(game: &Path, db: &mut ModDb) -> Vec<String> {
    let mut moved = vec![];
    for (enabled, base) in [
        (true, kind_dir(game, ModKind::Override)),
        (false, disabled_dir(game, ModKind::Override)),
    ] {
        for folder in list_dirs(&base) {
            let from = base.join(&folder);
            if is_bare_announcer_pack(&from) {
                if wrap_announcer_pack(&from).is_ok() {
                    moved.push(folder);
                }
                continue;
            }
            if !is_blt_mod_dir(&from) {
                continue;
            }
            let to = if enabled { kind_dir(game, ModKind::Blt) } else { disabled_dir(game, ModKind::Blt) }.join(&folder);
            if to.exists() {
                continue; // don't clobber an existing mod; leave it for the user
            }
            if move_dir(&from, &to).is_err() {
                continue;
            }
            let old_key = InstalledMod::make_key(ModKind::Override, &folder);
            let new_key = InstalledMod::make_key(ModKind::Blt, &folder);
            if let Some(mut m) = db.mods.remove(&old_key) {
                m.kind = ModKind::Blt;
                m.key = new_key.clone();
                db.mods.insert(new_key, m);
            }
            moved.push(folder);
        }
    }
    moved
}

fn list_dirs(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
    }
    out
}

/// Reconcile the database with what is actually on disk.
pub fn scan(game: &Path, db: &mut ModDb) {
    let mut seen = std::collections::HashSet::new();
    for kind in [ModKind::Blt, ModKind::Override, ModKind::Map] {
        for (enabled, base) in [(true, kind_dir(game, kind)), (false, disabled_dir(game, kind))] {
            for folder in list_dirs(&base) {
                if kind == ModKind::Blt && enabled && BLT_SKIP.iter().any(|s| s.eq_ignore_ascii_case(&folder)) {
                    continue;
                }
                if folder.starts_with('.') {
                    continue;
                }
                let key = InstalledMod::make_key(kind, &folder);
                if !seen.insert(key.clone()) {
                    continue; // same folder enabled and disabled; keep first (enabled)
                }
                let path = base.join(&folder);
                let (mut name, mut version, mut author) = (String::new(), String::new(), String::new());
                if let Some((n, v, a)) = read_mod_txt(&path) {
                    name = n;
                    version = v;
                    author = a;
                } else if let Some(n) = read_main_xml(&path) {
                    name = n;
                }
                if name.trim().is_empty() {
                    name = folder.clone();
                }
                let entry = db.mods.entry(key.clone()).or_insert_with(|| InstalledMod {
                    key: key.clone(),
                    folder: folder.clone(),
                    kind,
                    name: name.clone(),
                    version: version.clone(),
                    author: author.clone(),
                    source: Source::Local,
                    source_id: None,
                    page_url: None,
                    site_tags: vec![],
                    user_tags: vec![],
                    enabled,
                    nsfw: false,
                    installed_at: 0,
                    root_files: vec![],
                    source_version: String::new(),
                    update_available: None,
                });
                entry.enabled = enabled;
                entry.folder = folder.clone();
                if !version.is_empty() {
                    entry.version = version;
                }
                if entry.source == Source::Local && !name.is_empty() {
                    entry.name = name;
                }
            }
        }
    }
    db.mods.retain(|k, m| {
        if m.kind == ModKind::Root {
            m.root_files.first().map(|f| game.join(f).exists()).unwrap_or(false)
        } else {
            seen.contains(k)
        }
    });
}

pub fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if to.exists() {
        std::fs::remove_dir_all(to).with_context(|| format!("removing {}", to.display()))?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir(from, to)?;
            std::fs::remove_dir_all(from)?;
            Ok(())
        }
    }
}

pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(from)?;
        let target = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(entry.path(), &target)
                .with_context(|| format!("copying to {}", target.display()))?;
        }
    }
    Ok(())
}

pub fn set_enabled(game: &Path, m: &mut InstalledMod, enabled: bool) -> Result<()> {
    if m.kind == ModKind::Root {
        return Err(anyhow!("Game-root mods (like SuperBLT) can't be toggled; uninstall instead."));
    }
    if m.enabled == enabled {
        return Ok(());
    }
    let from = mod_path(game, m);
    let to = if enabled { kind_dir(game, m.kind) } else { disabled_dir(game, m.kind) }.join(&m.folder);
    move_dir(&from, &to).map_err(|e| {
        anyhow!("{e}. If the game is running, close it first. If PAYDAY 2 is in Program Files, you may need to run the manager as administrator.")
    })?;
    m.enabled = enabled;
    Ok(())
}

pub fn uninstall(game: &Path, m: &InstalledMod) -> Result<()> {
    if m.kind == ModKind::Root {
        for f in &m.root_files {
            let p = game.join(f);
            if p.is_file() {
                std::fs::remove_file(&p)?;
            }
        }
        return Ok(());
    }
    let p = mod_path(game, m);
    if p.exists() {
        std::fs::remove_dir_all(&p).with_context(|| format!("deleting {}", p.display()))?;
    }
    Ok(())
}

// ---------- nxm:// protocol registration (Windows) ----------

#[cfg(windows)]
pub fn nxm_registered_to_us() -> bool {
    use winreg::enums::*;
    let Ok(exe) = std::env::current_exe() else { return false };
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let Ok(k) = hkcu.open_subkey("Software\\Classes\\nxm\\shell\\open\\command") else { return false };
    let cmd: String = k.get_value("").unwrap_or_default();
    cmd.to_lowercase().contains(&exe.to_string_lossy().to_lowercase())
}

#[cfg(windows)]
pub fn register_nxm() -> Result<()> {
    use winreg::enums::*;
    let exe = std::env::current_exe()?;
    let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
    let (root, _) = hkcu.create_subkey("Software\\Classes\\nxm")?;
    root.set_value("", &"URL:NXM Protocol")?;
    root.set_value("URL Protocol", &"")?;
    let (cmd, _) = root.create_subkey("shell\\open\\command")?;
    cmd.set_value("", &format!("\"{}\" \"%1\"", exe.display()))?;
    Ok(())
}

#[cfg(not(windows))]
pub fn nxm_registered_to_us() -> bool {
    false
}
#[cfg(not(windows))]
pub fn register_nxm() -> Result<()> {
    Err(anyhow!("Only supported on Windows"))
}

// ---------- launching the game ----------

pub const STEAM_APP_ID: u32 = 218620;

/// Epic launcher URI for this install, built from its manifest.
fn epic_launch_uri(game: &Path) -> Option<String> {
    let program_data = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    let dir = Path::new(&program_data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    let want = game.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let loc = v["InstallLocation"].as_str().unwrap_or_default().replace('/', "\\");
        if loc.trim_end_matches('\\').to_lowercase() != want {
            continue;
        }
        let ns = v["CatalogNamespace"].as_str()?;
        let item = v["CatalogItemId"].as_str()?;
        let app = v["AppName"].as_str()?;
        return Some(format!("com.epicgames.launcher://apps/{ns}%3A{item}%3A{app}?action=launch&silent=true"));
    }
    None
}

/// Start PAYDAY 2 through its store (so DLC/online work), falling back to the exe.
pub fn launch(game: &Path, store: Store) -> Result<()> {
    match store {
        Store::Epic => {
            if let Some(uri) = epic_launch_uri(game) {
                return open::that(uri).map_err(|e| anyhow!("Couldn't open the Epic Games launcher: {e}"));
            }
        }
        Store::Steam => {
            return open::that(format!("steam://rungameid/{STEAM_APP_ID}"))
                .map_err(|e| anyhow!("Couldn't open Steam: {e}"));
        }
        Store::Manual => {}
    }
    let exe = ["PAYDAY2.exe", "payday2_win32_release.exe"]
        .iter()
        .map(|e| game.join(e))
        .find(|p| p.is_file())
        .ok_or_else(|| anyhow!("PAYDAY2.exe not found in {}", game.display()))?;
    std::process::Command::new(exe).current_dir(game).spawn()?;
    Ok(())
}

/// True while PAYDAY 2 is running (mods can't be changed safely then).
#[cfg(windows)]
pub fn game_running() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq PAYDAY2.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("payday2.exe"))
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn game_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_root_name() {
        // Extra Heist Info: spaces around "=", and later elements with path-like names.
        let ehi = "<?xml version=\"1.0\"?>\n<!-- c -->\n<mod\n    name = \"Extra Heist Info\"\n    version = \"r455\"\n>\n<texture name=\"guis/textures/pd2_mod_ehi/x\"/>\n</mod>";
        assert_eq!(root_attr(ehi, "name").as_deref(), Some("Extra Heist Info"));
        assert_eq!(root_attr(ehi, "version").as_deref(), Some("r455"));
        // BeardLib main.xml; "editor_name" must not match "name".
        assert_eq!(root_attr("<table editor_name='x' name='My Heist'><level/></table>", "name").as_deref(), Some("My Heist"));
        // Root has no name: don't borrow one from a child element.
        assert_eq!(root_attr("<mod author=\"a\"><x name=\"child\"/></mod>", "name"), None);
        assert_eq!(root_attr("not xml", "name"), None);
    }

    #[test]
    #[ignore]
    fn epic_uri_for_detected_install() {
        let (dir, store) = detect().expect("PAYDAY 2 not detected");
        println!("{} ({:?})", dir.display(), store);
        if store == Store::Epic {
            let uri = epic_launch_uri(&dir).expect("no Epic launch URI");
            println!("{uri}");
            assert!(uri.starts_with("com.epicgames.launcher://apps/") && uri.contains(EPIC_APP_NAME));
        }
    }
}
