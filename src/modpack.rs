//! Modpack share codes.
//!
//! A code is `PD2-` followed by a bit string written 6 bits per character using
//! the URL-safe base64 alphabet (`A–Z a–z 0–9 - _`). The bits are:
//!
//! ```text
//! has name      1 bit
//!   length      6 bits          (only if has name; at most MAX_NAME)
//!   name        8 bits × length (UTF-8)
//! for ModWorkshop, then Nexus:
//!   count + 1   Elias gamma
//!   k           5 bits          (only if count > 0)
//!   gaps        Rice(k) × count (sorted ids stored as id − previous id − 1)
//! padding       0–5 zero bits to finish the last character
//! ```
//!
//! Rice coding with the best `k` for the pack keeps a mod to about 13–15 bits,
//! close to the minimum for ids spread over ModWorkshop's range. Names and NSFW
//! flags aren't stored; the import window looks both up on the mod sites.

use crate::model::{InstalledMod, Source};
use anyhow::{anyhow, bail, Result};

pub const PREFIX: &str = "PD2-";
/// File extension used when a pack is saved to disk instead of copied.
pub const FILE_EXT: &str = "pd2pack";
/// Longest pack name stored in a code (in bytes).
pub const MAX_NAME: usize = 40;

/// Guards against hostile or corrupted codes.
const MAX_MODS: u64 = 1000;
const MAX_ID: u64 = 1 << 40;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const SITES: [Source; 2] = [Source::ModWorkshop, Source::Nexus];

#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub name: String,
    pub mods: Vec<PackMod>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackMod {
    pub source: Source,
    pub id: u64,
    /// Not stored in codes: filled in from the installed mod when sharing, or
    /// looked up on the site when importing (empty until then).
    pub name: String,
    /// Adult content per the site. Not stored in codes; set by the import lookup.
    pub nsfw: bool,
}

impl PackMod {
    pub fn page_url(&self) -> String {
        match self.source {
            Source::Nexus => format!("https://www.nexusmods.com/{}/mods/{}", crate::api::nexus::DOMAIN, self.id),
            _ => format!("https://modworkshop.net/mod/{}", self.id),
        }
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Mod #{}", self.id)
        } else {
            self.name.clone()
        }
    }
}

/// Build a pack from installed mods. Returns the pack and the names of mods that
/// can't be shared because they weren't installed from ModWorkshop or Nexus.
pub fn from_installed<'a>(name: &str, mods: impl IntoIterator<Item = &'a InstalledMod>) -> (Pack, Vec<String>) {
    let mut out: Vec<PackMod> = vec![];
    let mut skipped = vec![];
    for m in mods {
        match (m.source, m.source_id) {
            (Source::ModWorkshop | Source::Nexus, Some(id)) if id > 0 && id < MAX_ID => {
                // One archive can install several folders; list the site mod once.
                if !out.iter().any(|p| p.source == m.source && p.id == id) {
                    out.push(PackMod { source: m.source, id, name: base_name(&m.name), nsfw: m.nsfw });
                }
            }
            _ => skipped.push(m.name.clone()),
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    skipped.sort_by_key(|s| s.to_lowercase());
    (Pack { name: clean_name(name), mods: out }, skipped)
}

/// Multi-folder installs are named "Mod (Folder)"; show just "Mod".
fn base_name(name: &str) -> String {
    match name.rfind(" (") {
        Some(i) if name.ends_with(')') => name[..i].to_string(),
        _ => name.to_string(),
    }
}

/// Trim, drop control characters and cut to MAX_NAME bytes on a char boundary.
fn clean_name(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().chars().filter(|c| !c.is_control()) {
        if s.len() + c.len_utf8() > MAX_NAME {
            break;
        }
        s.push(c);
    }
    s.trim_end().to_string()
}

// ---------------------------------------------------------------- bit I/O

#[derive(Default)]
struct BitWriter {
    bits: Vec<bool>,
}

impl BitWriter {
    fn put(&mut self, value: u64, n: u32) {
        for i in (0..n).rev() {
            self.bits.push((value >> i) & 1 == 1);
        }
    }

    /// Elias gamma code for v >= 1.
    fn gamma(&mut self, v: u64) {
        let n = 64 - v.leading_zeros(); // significant bits
        self.put(0, n - 1);
        self.put(v, n);
    }

    fn rice(&mut self, v: u64, k: u32) {
        for _ in 0..(v >> k) {
            self.bits.push(true);
        }
        self.bits.push(false);
        self.put(v, k);
    }

    fn finish(mut self) -> String {
        while !self.bits.len().is_multiple_of(6) {
            self.bits.push(false);
        }
        self.bits
            .chunks(6)
            .map(|c| ALPHABET[c.iter().fold(0usize, |a, &b| (a << 1) | b as usize)] as char)
            .collect()
    }
}

struct BitReader {
    bits: Vec<bool>,
    pos: usize,
}

impl BitReader {
    fn new(text: &str) -> Option<Self> {
        let mut bits = Vec::with_capacity(text.len() * 6);
        for c in text.bytes() {
            let v = ALPHABET.iter().position(|&a| a == c)?;
            bits.extend((0..6).rev().map(|i| (v >> i) & 1 == 1));
        }
        Some(Self { bits, pos: 0 })
    }

    fn bit(&mut self) -> Option<bool> {
        let b = *self.bits.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn get(&mut self, n: u32) -> Option<u64> {
        (0..n).try_fold(0u64, |a, _| Some((a << 1) | self.bit()? as u64))
    }

    fn gamma(&mut self) -> Option<u64> {
        let mut zeros = 0;
        while !self.bit()? {
            zeros += 1;
            if zeros > 40 {
                return None;
            }
        }
        Some((1 << zeros) | self.get(zeros)?)
    }

    fn rice(&mut self, k: u32) -> Option<u64> {
        let mut q: u64 = 0;
        while self.bit()? {
            q += 1;
        }
        q.checked_shl(k)?.checked_add(self.get(k)?)
    }

    /// Only the zero padding of the last character may be left over.
    fn at_clean_end(&self) -> bool {
        let rest = &self.bits[self.pos..];
        rest.len() < 6 && rest.iter().all(|b| !b)
    }
}

fn rice_cost(gaps: &[u64], k: u32) -> u64 {
    gaps.iter().map(|g| (g >> k) + 1 + u64::from(k)).sum()
}

// ---------------------------------------------------------------- codes

pub fn encode(pack: &Pack) -> String {
    let mut w = BitWriter::default();
    let name = clean_name(&pack.name);
    w.put(u64::from(!name.is_empty()), 1);
    if !name.is_empty() {
        w.put(name.len() as u64, 6);
        for b in name.bytes() {
            w.put(u64::from(b), 8);
        }
    }
    for source in SITES {
        let mut ids: Vec<u64> = pack.mods.iter().filter(|m| m.source == source && m.id > 0 && m.id < MAX_ID).map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        w.gamma(ids.len() as u64 + 1);
        if ids.is_empty() {
            continue;
        }
        let mut prev = 0;
        let gaps: Vec<u64> = ids
            .iter()
            .map(|&id| {
                let g = id - prev - 1;
                prev = id;
                g
            })
            .collect();
        let k = (0..32).min_by_key(|&k| rice_cost(&gaps, k)).unwrap_or(0);
        w.put(u64::from(k), 5);
        for g in gaps {
            w.rice(g, k);
        }
    }
    format!("{PREFIX}{}", w.finish())
}

/// Find and decode a share code. Accepts the code on its own or inside other text
/// (a chat message, a file); the code ends at the first space or punctuation.
pub fn decode(text: &str) -> Result<Pack> {
    let damaged = || anyhow!("The code is damaged or incomplete. Make sure you copied all of it.");
    let start = text.find(PREFIX).ok_or_else(|| anyhow!("That isn't a modpack code. Codes start with {PREFIX}"))?;
    let body: String = text[start + PREFIX.len()..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let mut r = BitReader::new(&body).ok_or_else(damaged)?;

    let mut name = String::new();
    if r.bit().ok_or_else(damaged)? {
        let len = r.get(6).ok_or_else(damaged)? as usize;
        if len == 0 || len > MAX_NAME {
            return Err(damaged());
        }
        let bytes: Vec<u8> = (0..len).map(|_| r.get(8).map(|b| b as u8)).collect::<Option<_>>().ok_or_else(damaged)?;
        name = clean_name(&String::from_utf8_lossy(&bytes));
    }

    let mut mods = vec![];
    for source in SITES {
        let count = r.gamma().ok_or_else(damaged)? - 1;
        if count > MAX_MODS {
            bail!("That modpack lists more than {MAX_MODS} mods.");
        }
        if count == 0 {
            continue;
        }
        let k = r.get(5).ok_or_else(damaged)? as u32;
        let mut id: u64 = 0;
        for _ in 0..count {
            let gap = r.rice(k).ok_or_else(damaged)?;
            id = id.checked_add(gap).and_then(|v| v.checked_add(1)).filter(|&v| v < MAX_ID).ok_or_else(damaged)?;
            mods.push(PackMod { source, id, name: String::new(), nsfw: false });
        }
    }
    if !r.at_clean_end() {
        return Err(damaged());
    }
    if mods.is_empty() {
        bail!("That modpack doesn't contain any mods.");
    }
    Ok(Pack { name, mods })
}

// ---------------------------------------------------------------- zip packs
//
// A zip pack carries the mods' actual files, so it also works for mods that were
// installed from a file. Layout:
//
//   pd2mm-modpack.json            ZipManifest
//   README.txt                    how to open it
//   mods/<folder>/…               SuperBLT mods
//   assets/mod_overrides/<folder>/…
//   Maps/<folder>/…
//   root/<path>                   files of game-root mods (e.g. SuperBLT's DLL)

pub const MANIFEST: &str = "pd2mm-modpack.json";
const MANIFEST_FORMAT: u32 = 1;
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ZipManifest {
    pub format: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub app_version: String,
    pub mods: Vec<InstalledMod>,
}

/// The database key a pack entry gets once installed (never trusts the stored key).
pub fn pack_key(m: &InstalledMod) -> String {
    match m.kind {
        crate::model::ModKind::Root => format!("root/{}", m.folder.to_lowercase()),
        k => InstalledMod::make_key(k, &m.folder),
    }
}

/// Folder names come from other people's files: allow plain names only.
pub fn safe_folder(folder: &str) -> bool {
    !folder.is_empty() && folder != "." && folder != ".." && crate::installer::sanitize(folder) == folder
}

/// Read the manifest from a zip without extracting it. None if it isn't a pack.
pub fn read_zip_manifest(path: &std::path::Path) -> Option<ZipManifest> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let entry = zip.by_name(MANIFEST).ok()?;
    let mut raw = String::new();
    entry.take(MAX_MANIFEST_BYTES).read_to_string(&mut raw).ok()?;
    parse_manifest(&raw)
}

pub fn parse_manifest(raw: &str) -> Option<ZipManifest> {
    let mut m: ZipManifest = serde_json::from_str(raw.trim_start_matches('\u{feff}')).ok()?;
    if m.format == 0 || m.format > MANIFEST_FORMAT {
        return None;
    }
    m.name = clean_name(&m.name);
    m.mods.retain(|x| safe_folder(&x.folder));
    m.mods.truncate(MAX_MODS as usize);
    for x in &mut m.mods {
        x.key = pack_key(x);
        x.enabled = true;
        x.update_available = None;
    }
    Some(m)
}

/// Files that are already compressed gain nothing from deflate; store them as-is.
fn already_compressed(path: &std::path::Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ["ogg", "bik", "zip", "7z", "rar", "png", "jpg", "jpeg", "webm", "mp4", "bnk", "wem", "movie"].contains(&ext.as_str())
}

/// Write `mods` (with their files) to a zip pack at `dest`. Returns the zip's size.
pub fn export_zip(
    game: &std::path::Path,
    mods: &[InstalledMod],
    name: &str,
    dest: &std::path::Path,
    progress: crate::installer::Progress,
) -> Result<u64> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    // Collect (source file, path in zip) first so progress can be shown by size.
    let mut files: Vec<(std::path::PathBuf, String, u64)> = vec![];
    for m in mods {
        if m.kind == crate::model::ModKind::Root {
            for rel in &m.root_files {
                let src = game.join(rel);
                if let Ok(meta) = std::fs::metadata(&src) {
                    files.push((src, format!("root/{}", rel.replace('\\', "/")), meta.len()));
                }
            }
            continue;
        }
        let base = crate::game::mod_path(game, m);
        if !base.is_dir() {
            bail!("'{}' is missing from your game folder. Rescan and try again.", m.name);
        }
        for entry in walkdir::WalkDir::new(&base) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry.path().strip_prefix(&base)?.to_string_lossy().replace('\\', "/");
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((entry.path().to_path_buf(), format!("{}/{}/{rel}", m.kind.dir_name(), m.folder), len));
        }
    }
    let total: u64 = files.iter().map(|f| f.2).sum::<u64>().max(1);

    let manifest = ZipManifest {
        format: MANIFEST_FORMAT,
        name: clean_name(name),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        mods: mods
            .iter()
            .map(|m| InstalledMod { enabled: true, installed_at: 0, update_available: None, user_tags: vec![], ..m.clone() })
            .collect(),
    };
    let part = dest.with_extension("zip.part");
    let result = (|| -> Result<u64> {
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(&part)?));
        let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(MANIFEST, deflate)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;
        zip.start_file("README.txt", deflate)?;
        zip.write_all(
            format!(
                "PAYDAY 2 modpack{}\r\n\r\nOpen this zip with PD2 Mod Manager: drag it onto the window, or use \"Install from file\".\r\nIt will show the {} mod(s) inside and install them to the right folders.\r\n",
                if manifest.name.is_empty() { String::new() } else { format!(": {}", manifest.name) },
                mods.len()
            )
            .as_bytes(),
        )?;
        let mut done: u64 = 0;
        let mut buf = vec![0u8; 256 * 1024];
        for (src, zpath, len) in &files {
            let method = if already_compressed(src) { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
            let opts = SimpleFileOptions::default().compression_method(method).large_file(*len > u32::MAX as u64);
            zip.start_file(zpath.as_str(), opts)?;
            let mut f = std::fs::File::open(src)?;
            loop {
                let n = std::io::Read::read(&mut f, &mut buf)?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buf[..n])?;
            }
            done += len;
            progress(done as f32 / total as f32 * 0.98, &format!("Packing {:.1} / {:.1} MB", done as f64 / 1_048_576.0, total as f64 / 1_048_576.0));
        }
        zip.finish()?.flush()?;
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        std::fs::rename(&part, dest)?;
        Ok(std::fs::metadata(dest)?.len())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModKind;

    fn installed(name: &str, source: Source, id: Option<u64>) -> InstalledMod {
        InstalledMod {
            key: name.to_lowercase(),
            folder: name.into(),
            kind: ModKind::Blt,
            name: name.into(),
            version: String::new(),
            author: String::new(),
            source,
            source_id: id,
            page_url: None,
            site_tags: vec![],
            user_tags: vec![],
            enabled: true,
            nsfw: false,
            installed_at: 0,
            root_files: vec![],
            source_version: String::new(),
            update_available: None,
        }
    }

    fn ids(p: &Pack) -> Vec<(Source, u64)> {
        let mut v: Vec<_> = p.mods.iter().map(|m| (m.source, m.id)).collect();
        v.sort_by_key(|(s, id)| (*s == Source::Nexus, *id));
        v
    }

    fn mws(ids: &[u64]) -> Pack {
        Pack {
            name: String::new(),
            mods: ids.iter().map(|&id| PackMod { source: Source::ModWorkshop, id, name: String::new(), nsfw: false }).collect(),
        }
    }

    #[test]
    fn round_trip() {
        let mods = [
            installed("Void UI", Source::ModWorkshop, Some(19196)),
            installed("BeardLib", Source::ModWorkshop, Some(14924)),
            installed("Some Nexus Mod", Source::Nexus, Some(42)),
            installed("My Local Edit", Source::Local, None),
            installed("Void UI (Extra)", Source::ModWorkshop, Some(19196)),
            installed("First", Source::ModWorkshop, Some(1)),
        ];
        let (pack, skipped) = from_installed("  Stealth pack ", &mods);
        assert_eq!(pack.name, "Stealth pack");
        assert_eq!(pack.mods.len(), 4, "duplicate site mod listed once: {:?}", pack.mods);
        assert_eq!(skipped, vec!["My Local Edit".to_string()]);
        let code = encode(&pack);
        assert!(code.starts_with(PREFIX));
        let back = decode(&code).unwrap();
        assert_eq!(back.name, "Stealth pack");
        assert_eq!(ids(&back), ids(&pack));
        // Found inside a chat message, with words or punctuation right after it.
        assert_eq!(ids(&decode(&format!("try my pack!\n{code} thanks.")).unwrap()), ids(&pack));
        assert_eq!(ids(&decode(&format!("({code})")).unwrap()), ids(&pack));
    }

    #[test]
    fn short_codes() {
        // The 13 ModWorkshop mods from a real install.
        let real = mws(&[20997, 21431, 23732, 24006, 24293, 25950, 31221, 40198, 51185, 54532, 57823, 59940, 60034]);
        let code = encode(&real);
        assert!(code.len() <= 38, "{code} is {} chars", code.len());
        assert_eq!(ids(&decode(&code).unwrap()), ids(&real));
        // 50 mods spread over the id range.
        let big = mws(&(0..50).map(|i| 1000 + i * 1300).collect::<Vec<_>>());
        let code = encode(&big);
        assert!(code.len() < 130, "{} chars", code.len());
        assert_eq!(ids(&decode(&code).unwrap()), ids(&big));
        // A single mod.
        assert!(encode(&mws(&[59940])).len() <= 12);
    }

    #[test]
    fn extreme_ids_round_trip() {
        for set in [vec![1], vec![1, 2, 3], vec![MAX_ID - 1], vec![5, MAX_ID - 1], (1..=300).collect()] {
            let p = mws(&set);
            assert_eq!(ids(&decode(&encode(&p)).unwrap()), ids(&p), "{set:?}");
        }
    }

    #[test]
    fn long_names_are_cut() {
        let mut p = mws(&[7]);
        p.name = "é".repeat(100);
        let back = decode(&encode(&p)).unwrap();
        assert!(back.name.len() <= MAX_NAME && back.name.starts_with('é'));
    }

    #[test]
    fn rejects_bad_codes() {
        assert!(decode("hello").is_err());
        assert!(decode("PD2-").is_err());
        assert!(decode("PD2-AAAA").is_err()); // all zero bits: runs out mid-number
        assert!(decode("PD2-________").is_err()); // all one bits
        assert!(decode(&encode(&mws(&[]))).is_err()); // well-formed but no mods
        let ok = encode(&mws(&[5, 900, 31000]));
        assert!(decode(&ok).is_ok());
        assert!(decode(&ok[..ok.len() - 1]).is_err(), "truncated code must fail");
        assert!(decode(&format!("{ok}A")).is_err(), "extra character must fail");
    }
}
