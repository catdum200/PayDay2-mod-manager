//! Downloading, extracting and placing mods into the right PAYDAY 2 folders.

use crate::api;
use crate::game;
use crate::model::{InstalledMod, ModKind, RemoteMod, Source};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub type Progress<'a> = &'a dyn Fn(f32, &str);

/// Error message returned when a "download" is actually a web page.
pub const NOT_AN_ARCHIVE: &str = "NOT_AN_ARCHIVE";

pub fn download(url: &str, dest: &Path, progress: Progress) -> Result<PathBuf> {
    progress(0.0, "Connecting…");
    let mut resp = api::client().get(url).send().with_context(|| format!("downloading {url}"))?;
    if !resp.status().is_success() {
        bail!("Download failed: HTTP {}", resp.status());
    }
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if ctype.starts_with("text/html") {
        bail!(NOT_AN_ARCHIVE);
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut last_report = std::time::Instant::now();
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        if last_report.elapsed().as_millis() > 100 {
            last_report = std::time::Instant::now();
            let mb = done as f64 / 1_048_576.0;
            if total > 0 {
                progress(
                    done as f32 / total as f32 * 0.8,
                    &format!("Downloading {:.1} / {:.1} MB", mb, total as f64 / 1_048_576.0),
                );
            } else {
                progress(0.4, &format!("Downloading {:.1} MB", mb));
            }
        }
    }
    file.flush()?;
    Ok(dest.to_path_buf())
}

#[derive(Debug, PartialEq)]
enum ArchiveType {
    Zip,
    SevenZ,
    Rar,
}

fn sniff(path: &Path) -> Result<ArchiveType> {
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 8];
    let n = f.read(&mut head)?;
    let h = &head[..n];
    if h.starts_with(b"PK\x03\x04") || h.starts_with(b"PK\x05\x06") {
        return Ok(ArchiveType::Zip);
    }
    if h.starts_with(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C]) {
        return Ok(ArchiveType::SevenZ);
    }
    if h.starts_with(b"Rar!\x1A\x07") {
        return Ok(ArchiveType::Rar);
    }
    let text = String::from_utf8_lossy(h).to_lowercase();
    if text.trim_start().starts_with('<') {
        bail!(NOT_AN_ARCHIVE);
    }
    bail!("Unsupported file type (expected .zip, .7z or .rar)")
}

fn safe_relative(p: &Path) -> bool {
    p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

pub fn extract(archive: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    match sniff(archive)? {
        ArchiveType::Zip => {
            let f = std::fs::File::open(archive)?;
            let mut z = zip::ZipArchive::new(f)?;
            for i in 0..z.len() {
                let mut entry = z.by_index(i)?;
                let Some(rel) = entry.enclosed_name() else { continue };
                let target = out.join(rel);
                if entry.is_dir() {
                    std::fs::create_dir_all(&target)?;
                } else {
                    if let Some(p) = target.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    let mut o = std::fs::File::create(&target)?;
                    std::io::copy(&mut entry, &mut o)?;
                }
            }
        }
        ArchiveType::SevenZ => {
            let base = out.to_path_buf();
            sevenz_rust::decompress_file_with_extract_fn(archive, out, |entry, reader, dest| {
                let ok = dest.strip_prefix(&base).map(safe_relative).unwrap_or(false);
                if !ok {
                    return Ok(true);
                }
                sevenz_rust::default_entry_extract_fn(entry, reader, dest)
            })
            .map_err(|e| anyhow!("7z extraction failed: {e}"))?;
        }
        ArchiveType::Rar => {
            let path_str = archive.to_string_lossy().to_string();
            let mut arc = unrar::Archive::new(&path_str)
                .open_for_processing()
                .map_err(|e| anyhow!("RAR open failed: {e}"))?;
            while let Some(header) = arc.read_header().map_err(|e| anyhow!("RAR read failed: {e}"))? {
                let ok = safe_relative(&header.entry().filename);
                arc = if header.entry().is_file() && ok {
                    header.extract_with_base(out).map_err(|e| anyhow!("RAR extract failed: {e}"))?
                } else {
                    header.skip().map_err(|e| anyhow!("RAR skip failed: {e}"))?
                };
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Placement {
    pub src: PathBuf,
    pub kind: ModKind,
    pub folder: String,
}

const ASSET_DIRS: &[&str] = &[
    "units", "guis", "soundbanks", "movies", "fonts", "anims", "effects", "environments",
    "settings", "strings", "physic_effects", "shaders", "textures", "core", "packages",
];

pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "Mod".into() } else { s }
}

fn children(dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = vec![];
    let mut files = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name == "__MACOSX" || name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else {
                files.push(p);
            }
        }
    }
    (dirs, files)
}

fn has_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let (_, files) = children(dir);
    files.into_iter().find(|f| {
        f.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(name)).unwrap_or(false)
    })
}

fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn find_loader_dll(dir: &Path, depth: u32) -> Option<PathBuf> {
    if has_file(dir, "WSOCK32.dll").is_some() || has_file(dir, "IPHLPAPI.dll").is_some() {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    children(dir).0.into_iter().find_map(|d| find_loader_dll(&d, depth - 1))
}

/// Work out where each piece of an extracted archive belongs.
#[cfg(test)]
pub fn plan(root: &Path, fallback_name: &str) -> Vec<Placement> {
    plan_checked(root, fallback_name).0
}

/// Like `plan`, but also says whether the location was a guess (no mod.txt,
/// supermod.xml, main.xml or known asset folders were found).
pub fn plan_checked(root: &Path, fallback_name: &str) -> (Vec<Placement>, bool) {
    let fallback = sanitize(fallback_name);
    if let Some(dir) = find_loader_dll(root, 3) {
        return (vec![Placement { src: dir, kind: ModKind::Root, folder: fallback }], false);
    }
    let mut out = vec![];
    visit(root, root, 0, &fallback, &mut out);
    let guessed = out.is_empty();
    if guessed {
        let (dirs, files) = children(root);
        if dirs.len() == 1 && files.is_empty() {
            out.push(Placement { src: dirs[0].clone(), kind: ModKind::Override, folder: dir_name(&dirs[0]) });
        } else {
            out.push(Placement { src: root.to_path_buf(), kind: ModKind::Override, folder: fallback });
        }
    }
    (out, guessed)
}

fn visit(dir: &Path, root: &Path, depth: u32, fallback: &str, out: &mut Vec<Placement>) {
    if depth > 6 {
        return;
    }
    let is_root = dir == root;
    let folder = if is_root { fallback.to_string() } else { dir_name(dir) };
    let lname = dir_name(dir).to_lowercase();
    let (subdirs, _) = children(dir);

    // SuperBLT mods are defined by mod.txt (JSON) or supermod.xml (XML); both live in mods/.
    if has_file(dir, "mod.txt").is_some() || has_file(dir, "supermod.xml").is_some() {
        out.push(Placement { src: dir.to_path_buf(), kind: ModKind::Blt, folder });
        return;
    }
    // Battle Announcers voice pack: installed as its own mod_overrides folder and
    // wrapped into the layout Battle Announcers scans for (see game::wrap_announcer_pack).
    if game::is_bare_announcer_pack(dir) || has_file(dir, game::ANNOUNCER_CARD).is_some() {
        out.push(Placement { src: dir.to_path_buf(), kind: ModKind::Override, folder });
        return;
    }
    if !is_root {
        let container = match lname.as_str() {
            "mods" => Some(ModKind::Blt),
            "mod_overrides" => Some(ModKind::Override),
            "maps" => Some(ModKind::Map),
            _ => None,
        };
        if let Some(kind) = container {
            for d in subdirs {
                let name = dir_name(&d);
                if kind == ModKind::Blt && ["base", "saves", "logs", "downloads"].contains(&name.to_lowercase().as_str()) {
                    continue;
                }
                out.push(Placement { src: d, kind, folder: name });
            }
            return;
        }
    }
    if let Some(main) = has_file(dir, "main.xml") {
        let text = std::fs::read_to_string(&main).unwrap_or_default().to_lowercase();
        let kind = if text.contains("<level") || text.contains("<narrative") || text.contains("<instance") {
            ModKind::Map
        } else {
            ModKind::Override
        };
        out.push(Placement { src: dir.to_path_buf(), kind, folder });
        return;
    }
    let looks_like_assets = has_file(dir, "add.xml").is_some()
        || subdirs.iter().any(|d| ASSET_DIRS.contains(&dir_name(d).to_lowercase().as_str()));
    if looks_like_assets {
        out.push(Placement { src: dir.to_path_buf(), kind: ModKind::Override, folder });
        return;
    }
    for d in subdirs {
        visit(&d, root, depth + 1, fallback, out);
    }
}

pub struct InstallResult {
    pub mods: Vec<InstalledMod>,
    pub warnings: Vec<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Extract `archive` and copy it into the game.
pub fn install_archive(
    game_dir: &Path,
    archive: &Path,
    meta: Option<&RemoteMod>,
    source: Source,
    file_version: &str,
    fallback_name: &str,
    progress: Progress,
) -> Result<InstallResult> {
    let work = crate::config::temp_dir().join(format!("extract_{}", now_nanos()));
    // Remove the extraction folder however this function exits.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(work.clone());
    progress(0.82, "Extracting…");
    extract(archive, &work)?;
    // A modpack zip made by this app: put every mod back where it came from.
    if let Some(manifest) = std::fs::read_to_string(work.join(crate::modpack::MANIFEST)).ok().and_then(|r| crate::modpack::parse_manifest(&r)) {
        return install_pack_dir(game_dir, &work, &manifest, &Default::default(), progress);
    }
    progress(0.9, "Installing…");
    let (placements, guessed) = plan_checked(&work, fallback_name);
    let mut mods = vec![];
    let mut warnings = vec![];
    if guessed {
        warnings.push(format!(
            "Couldn't tell where '{}' belongs, so it was put in assets/mod_overrides. Check the mod's install instructions; it may need to go inside another mod's folder.",
            meta.map(|m| m.name.as_str()).unwrap_or(fallback_name)
        ));
    }
    let loaders = game::loader_status(game_dir);

    for p in &placements {
        let mut installed = InstalledMod {
            key: InstalledMod::make_key(p.kind, &p.folder),
            folder: p.folder.clone(),
            kind: p.kind,
            name: meta.map(|m| m.name.clone()).unwrap_or_else(|| p.folder.clone()),
            version: if !file_version.is_empty() {
                file_version.to_string()
            } else {
                meta.map(|m| m.version.clone()).unwrap_or_default()
            },
            author: meta.map(|m| m.author.clone()).unwrap_or_default(),
            source,
            source_id: meta.map(|m| m.id),
            page_url: meta.map(|m| m.page_url.clone()),
            site_tags: meta.map(|m| m.tags.iter().map(|t| t.name.clone()).collect()).unwrap_or_default(),
            user_tags: vec![],
            enabled: true,
            nsfw: meta.map(|m| m.nsfw).unwrap_or(false),
            installed_at: now(),
            root_files: vec![],
            source_version: meta.map(|m| m.version.clone()).unwrap_or_default(),
            update_available: None,
        };
        match p.kind {
            ModKind::Root => {
                for entry in walkdir::WalkDir::new(&p.src) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel = entry.path().strip_prefix(&p.src)?;
                    let target = game_dir.join(rel);
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(entry.path(), &target).map_err(|e| perm_hint(e, &target))?;
                    installed.root_files.push(rel.to_string_lossy().to_string());
                }
                installed.key = format!("root/{}", p.folder.to_lowercase());
            }
            kind => {
                let dest = game::kind_dir(game_dir, kind).join(&p.folder);
                let disabled = game::disabled_dir(game_dir, kind).join(&p.folder);
                if disabled.exists() {
                    let _ = std::fs::remove_dir_all(&disabled);
                }
                if dest.exists() {
                    std::fs::remove_dir_all(&dest).map_err(|e| perm_hint(e, &dest))?;
                }
                game::copy_dir(&p.src, &dest).map_err(|e| anyhow!("{e}. {}", PERM_HINT))?;
                if kind == ModKind::Override && game::is_bare_announcer_pack(&dest) {
                    game::wrap_announcer_pack(&dest).map_err(|e| anyhow!("{e}. {}", PERM_HINT))?;
                    if !game::kind_dir(game_dir, ModKind::Blt).join("Battle Announcers").is_dir() {
                        warnings.push(format!("'{}' needs the Battle Announcers mod, which isn't installed.", installed.name));
                    }
                }
                if kind == ModKind::Blt {
                    if let Some((n, v, a)) = game::read_mod_txt(&dest) {
                        if meta.is_none() && !n.is_empty() {
                            installed.name = n;
                        }
                        if installed.version.is_empty() {
                            installed.version = v;
                        }
                        if installed.author.is_empty() {
                            installed.author = a;
                        }
                    }
                    if !loaders.superblt {
                        warnings.push(format!("'{}' needs SuperBLT, which isn't installed.", installed.name));
                    }
                }
                if kind != ModKind::Blt && dest.join("main.xml").exists() && !loaders.beardlib {
                    warnings.push(format!("'{}' needs BeardLib, which isn't installed.", installed.name));
                }
                if placements.len() > 1 && meta.is_some() {
                    installed.name = format!("{} ({})", installed.name, p.folder);
                }
            }
        }
        mods.push(installed);
    }
    progress(1.0, "Installed");
    Ok(InstallResult { mods, warnings })
}

/// Install a modpack zip, leaving out the mods whose keys are in `skip`.
pub fn install_pack_zip(
    game_dir: &Path,
    archive: &Path,
    skip: &std::collections::HashSet<String>,
    progress: Progress,
) -> Result<InstallResult> {
    let work = crate::config::temp_dir().join(format!("pack_{}", now_nanos()));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(work.clone());
    progress(0.02, "Extracting…");
    extract(archive, &work)?;
    let raw = std::fs::read_to_string(work.join(crate::modpack::MANIFEST))
        .map_err(|_| anyhow!("That zip isn't a modpack made by PD2 Mod Manager."))?;
    let manifest = crate::modpack::parse_manifest(&raw)
        .ok_or_else(|| anyhow!("This modpack was made by a newer version of PD2 Mod Manager, or it's damaged."))?;
    install_pack_dir(game_dir, &work, &manifest, skip, progress)
}

/// Copy each mod of an extracted modpack (see `modpack` for the layout) into the game.
fn install_pack_dir(
    game_dir: &Path,
    work: &Path,
    manifest: &crate::modpack::ZipManifest,
    skip: &std::collections::HashSet<String>,
    progress: Progress,
) -> Result<InstallResult> {
    let mut mods = vec![];
    let mut warnings = vec![];
    let todo: Vec<&InstalledMod> = manifest.mods.iter().filter(|m| !skip.contains(&m.key)).collect();
    for (i, pm) in todo.iter().enumerate() {
        progress(0.3 + 0.7 * i as f32 / todo.len().max(1) as f32, &format!("Installing {}", pm.name));
        let mut m = (*pm).clone();
        m.installed_at = now();
        m.root_files.clear();
        if m.kind == ModKind::Root {
            for rel in &pm.root_files {
                let rel_path = Path::new(rel);
                let src = work.join("root").join(rel_path);
                if rel.is_empty() || !safe_relative(rel_path) || !src.is_file() {
                    continue;
                }
                let target = game_dir.join(rel_path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&src, &target).map_err(|e| perm_hint(e, &target))?;
                m.root_files.push(rel.clone());
            }
            if m.root_files.is_empty() {
                warnings.push(format!("'{}' had no files in the modpack, so it was skipped.", m.name));
                continue;
            }
        } else {
            let src = work.join(m.kind.dir_name()).join(&m.folder);
            if !src.is_dir() {
                warnings.push(format!("'{}' had no files in the modpack, so it was skipped.", m.name));
                continue;
            }
            let dest = game::kind_dir(game_dir, m.kind).join(&m.folder);
            let disabled = game::disabled_dir(game_dir, m.kind).join(&m.folder);
            if disabled.exists() {
                let _ = std::fs::remove_dir_all(&disabled);
            }
            if dest.exists() {
                std::fs::remove_dir_all(&dest).map_err(|e| perm_hint(e, &dest))?;
            }
            game::copy_dir(&src, &dest).map_err(|e| anyhow!("{e}. {}", PERM_HINT))?;
        }
        mods.push(m);
    }
    let loaders = game::loader_status(game_dir);
    if mods.iter().any(|m| m.kind == ModKind::Blt) && !loaders.superblt {
        warnings.push("Some mods in this pack need SuperBLT, which isn't installed.".into());
    }
    if mods.iter().any(|m| m.kind != ModKind::Blt && game::kind_dir(game_dir, m.kind).join(&m.folder).join("main.xml").exists()) && !loaders.beardlib {
        warnings.push("Some mods in this pack need BeardLib, which isn't installed.".into());
    }
    progress(1.0, "Installed");
    Ok(InstallResult { mods, warnings })
}

const PERM_HINT: &str = "Close PAYDAY 2 if it's running. If the game is in Program Files, try running the manager as administrator";

fn perm_hint(e: std::io::Error, p: &Path) -> anyhow::Error {
    anyhow!("{} ({}). {}", e, p.display(), PERM_HINT)
}

pub fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn plans() {
        let t = std::env::temp_dir().join(format!("pd2mm_test_{}_{}", std::thread::current().name().unwrap_or("t").replace("::", "_"), now_nanos()));
        // BLT mod wrapped in a folder
        let a = t.join("a");
        mk(&a, "CoolHud/mod.txt");
        let p = plan(&a, "x");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind, ModKind::Blt);
        assert_eq!(p[0].folder, "CoolHud");
        // override inside assets/mod_overrides
        let b = t.join("b");
        mk(&b, "assets/mod_overrides/Skin/units/a.texture");
        let p = plan(&b, "x");
        assert_eq!(p[0].kind, ModKind::Override);
        assert_eq!(p[0].folder, "Skin");
        // loose mod.txt at root
        let c = t.join("c");
        mk(&c, "mod.txt");
        let p = plan(&c, "My Mod");
        assert_eq!(p[0].folder, "My Mod");
        // superblt
        let d = t.join("d");
        mk(&d, "WSOCK32.dll");
        mk(&d, "mods/base/mod.txt");
        assert_eq!(plan(&d, "SuperBLT")[0].kind, ModKind::Root);
        // beardlib map
        let e = t.join("e");
        std::fs::create_dir_all(e.join("Heist")).unwrap();
        std::fs::write(e.join("Heist/main.xml"), "<table name=\"H\"><narrative/></table>").unwrap();
        assert_eq!(plan(&e, "x")[0].kind, ModKind::Map);
        // SuperBLT supermod.xml mod (Extra Heist Info layout) must go to mods/ as ONE folder
        let f = t.join("f");
        mk(&f, "EHI/supermod.xml");
        mk(&f, "EHI/assets/guis/textures/a.texture");
        mk(&f, "EHI/lua/units/x.lua");
        let p = plan(&f, "x");
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!((p[0].kind, p[0].folder.as_str()), (ModKind::Blt, "EHI"));
        // supermod.xml with a units folder (Better Assault Indicator style) is still BLT
        let g = t.join("g");
        mk(&g, "Better Assault Indicator/supermod.xml");
        mk(&g, "Better Assault Indicator/units/a.unit");
        assert_eq!(plan(&g, "x")[0].kind, ModKind::Blt);
        // Battle Announcers voice pack (loose files in a folder) goes to mod_overrides
        let h = t.join("h");
        std::fs::create_dir_all(h.join("Anis_SS")).unwrap();
        std::fs::write(
            h.join("Anis_SS/config.txt"),
            "{ \"OnEnemyKilled\": [ { \"sound\": \"Anis_SS/a.ogg\", \"icon\": \"Anis_SS/icon.dds\" } ] }",
        )
        .unwrap();
        mk(&h, "Anis_SS/a.ogg");
        let (p, guessed) = plan_checked(&h, "x");
        assert!(!guessed);
        assert_eq!((p[0].kind, p[0].folder.as_str()), (ModKind::Override, "Anis_SS"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn wraps_announcer_pack() {
        let t = std::env::temp_dir().join(format!("pd2mm_test_{}_{}", std::thread::current().name().unwrap_or("t").replace("::", "_"), now_nanos()));
        // Folder name differs from the prefix the config uses; the config wins.
        let pack = t.join("Anis Pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("config.txt"), "{ \"OnHeistComplete\": [ { \"sound\": \"Anis_SS/v.ogg\" } ] }").unwrap();
        mk(&pack, "v.ogg");
        assert!(game::is_bare_announcer_pack(&pack));
        game::wrap_announcer_pack(&pack).unwrap();
        assert!(pack.join(game::ANNOUNCER_CARD).is_file());
        assert!(pack.join("files/Anis_SS/config.txt").is_file());
        assert!(pack.join("files/Anis_SS/v.ogg").is_file());
        assert!(!game::is_bare_announcer_pack(&pack));
        let _ = std::fs::remove_dir_all(&t);
    }

    fn tracked(kind: ModKind, folder: &str, root_files: &[&str]) -> InstalledMod {
        InstalledMod {
            key: InstalledMod::make_key(kind, folder),
            folder: folder.into(),
            kind,
            name: folder.into(),
            version: "1".into(),
            author: String::new(),
            source: Source::Local,
            source_id: None,
            page_url: None,
            site_tags: vec![],
            user_tags: vec![],
            enabled: true,
            nsfw: false,
            installed_at: 0,
            root_files: root_files.iter().map(|s| s.to_string()).collect(),
            source_version: String::new(),
            update_available: None,
        }
    }

    #[test]
    fn modpack_zip_round_trip() {
        let t = std::env::temp_dir().join(format!("pd2mm_test_zip_{}", now_nanos()));
        let src = t.join("src_game");
        mk(&src, "mods/Cool Mod/mod.txt");
        mk(&src, "mods/Cool Mod/lua/x.lua");
        mk(&src, "assets/mod_overrides/Anis_SS/battle_announcers_identity_card.txt");
        mk(&src, "assets/mod_overrides/Anis_SS/files/Anis_SS/a.ogg");
        mk(&src, "WSOCK32.dll");
        let mods = [
            tracked(ModKind::Blt, "Cool Mod", &[]),
            tracked(ModKind::Override, "Anis_SS", &[]),
            tracked(ModKind::Root, "SuperBLT", &["WSOCK32.dll"]),
        ];
        let zip = t.join("My pack.zip");
        crate::modpack::export_zip(&src, &mods, "My pack", &zip, &|_, _| {}).unwrap();
        let manifest = crate::modpack::read_zip_manifest(&zip).expect("manifest");
        assert_eq!((manifest.name.as_str(), manifest.mods.len()), ("My pack", 3));

        // Install from the confirmation window, skipping one mod the user already has.
        let dst = t.join("dst_game");
        std::fs::create_dir_all(&dst).unwrap();
        let skip = [crate::modpack::pack_key(&mods[1])].into_iter().collect();
        let r = install_pack_zip(&dst, &zip, &skip, &|_, _| {}).unwrap();
        assert_eq!(r.mods.len(), 2);
        assert!(dst.join("mods/Cool Mod/lua/x.lua").is_file());
        assert!(dst.join("WSOCK32.dll").is_file());
        assert!(!dst.join("assets/mod_overrides/Anis_SS").exists());

        // Dropping the zip on the normal installer also recognises it as a pack.
        let dst2 = t.join("dst2_game");
        std::fs::create_dir_all(&dst2).unwrap();
        let r = install_archive(&dst2, &zip, None, Source::Local, "", "x", &|_, _| {}).unwrap();
        assert_eq!(r.mods.len(), 3);
        assert!(dst2.join("assets/mod_overrides/Anis_SS/files/Anis_SS/a.ogg").is_file());
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn modpack_rejects_unsafe_paths() {
        use crate::modpack::safe_folder;
        assert!(safe_folder("Anis_SS") && safe_folder("Cool Mod"));
        for bad in ["", ".", "..", "../evil", "a/b", "a\\b", "C:"] {
            assert!(!safe_folder(bad), "{bad:?}");
        }
        let raw = r#"{"format":1,"name":"x","mods":[{"key":"k","folder":"..","kind":"Blt","name":"n","version":"","author":"","source":"Local","source_id":null,"page_url":null,"enabled":true}]}"#;
        assert!(crate::modpack::parse_manifest(raw).unwrap().mods.is_empty());
        assert!(crate::modpack::parse_manifest(&raw.replace("\"format\":1", "\"format\":99")).is_none());
    }
}
