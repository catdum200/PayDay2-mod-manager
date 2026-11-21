//! Background jobs. Every network / disk-heavy operation runs on its own thread
//! and reports back to the UI through a channel.

use crate::api::{mws, nexus};
use crate::config;
use crate::installer::{self, NOT_AN_ARCHIVE};
use crate::model::{Dependency, FileKind, InstallRequest, InstalledMod, RemoteFile, RemoteMod, Source, TagChip};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

pub static EGUI_CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

#[allow(clippy::large_enum_variant)] // short-lived channel messages
pub enum Msg {
    Page { source: Source, gen: u64, append: bool, result: Result<(Vec<RemoteMod>, bool), String> },
    MwsTags(Vec<TagChip>),
    MwsCategories(Vec<mws::Category>),
    Detail { source: Source, id: u64, result: Result<(RemoteMod, bool), String> },
    Files { source: Source, id: u64, result: Result<Vec<RemoteFile>, String> },
    TaskProgress { id: u64, progress: f32, status: String },
    TaskDone { id: u64, outcome: Result<TaskOutcome, String> },
    Nxm(String),
    Focus,
    /// A file opened with the app (double-clicked .pd2pack, or "Open with").
    OpenFile(PathBuf),
    NexusUser(Result<(String, bool), String>),
    GameRunning(bool),
    /// Requirements of a mod shown in the detail panel.
    Deps { source: Source, id: u64, deps: Vec<Dependency> },
    /// Requirements discovered while installing (the UI queues missing ones).
    TaskDeps { parent: String, deps: Vec<Dependency> },
    UpdateInfo { key: String, version: Option<String> },
    UpdateCheckDone,
    /// Name of a mod from an imported modpack code (None if the lookup failed).
    PackModInfo { gen: u64, source: Source, id: u64, name: Option<String>, nsfw: bool },
}

pub enum TaskOutcome {
    Installed { mods: Vec<InstalledMod>, warnings: Vec<String> },
    OpenedBrowser(String),
    /// A modpack zip was written.
    Exported { path: PathBuf, bytes: u64 },
}

#[derive(Clone)]
pub struct Worker {
    tx: Sender<Msg>,
}

fn repaint() {
    if let Some(ctx) = EGUI_CTX.get() {
        ctx.request_repaint();
    }
}

impl Worker {
    pub fn new(tx: Sender<Msg>) -> Self {
        Self { tx }
    }

    pub fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        repaint();
    }

    fn spawn(&self, f: impl FnOnce(&Worker) + Send + 'static) {
        let w = self.clone();
        std::thread::spawn(move || f(&w));
    }

    pub fn search_mws(&self, q: mws::Query, gen: u64) {
        self.spawn(move |w| {
            let append = q.page > 1;
            let result = mws::search(&q).map_err(|e| e.to_string());
            w.send(Msg::Page { source: Source::ModWorkshop, gen, append, result });
        });
    }

    pub fn search_nexus(&self, q: nexus::Query, gen: u64) {
        self.spawn(move |w| {
            let append = q.offset > 0;
            let result = nexus::search(&q).map_err(|e| e.to_string());
            w.send(Msg::Page { source: Source::Nexus, gen, append, result });
        });
    }

    pub fn load_mws_meta(&self) {
        self.spawn(|w| {
            if let Ok(t) = mws::tags() {
                w.send(Msg::MwsTags(t));
            }
            if let Ok(c) = mws::categories() {
                w.send(Msg::MwsCategories(c));
            }
        });
    }

    pub fn load_detail(&self, source: Source, id: u64) {
        self.spawn(move |w| {
            let result = match source {
                Source::ModWorkshop => mws::detail(id).map_err(|e| e.to_string()),
                Source::Nexus => nexus::detail(id).map(|m| (m, false)).map_err(|e| e.to_string()),
                Source::Local => Err("n/a".into()),
            };
            w.send(Msg::Detail { source, id, result });
        });
        self.spawn(move |w| {
            let result = match source {
                Source::ModWorkshop => mws::files(id).map_err(|e| e.to_string()),
                Source::Nexus => nexus::files(id).map_err(|e| e.to_string()),
                Source::Local => Err("n/a".into()),
            };
            w.send(Msg::Files { source, id, result });
        });
        self.spawn(move |w| {
            let deps = fetch_deps(source, id);
            w.send(Msg::Deps { source, id, deps });
        });
    }

    /// Poll every few seconds for a running PAYDAY2.exe.
    pub fn watch_game(&self) {
        self.spawn(|w| {
            let mut last = None;
            loop {
                let now = crate::game::game_running();
                if last != Some(now) {
                    w.send(Msg::GameRunning(now));
                    last = Some(now);
                }
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        });
    }

    pub fn validate_nexus(&self, key: String) {
        self.spawn(move |w| {
            let r = nexus::validate(&key).map(|u| (u.name, u.premium)).map_err(|e| e.to_string());
            w.send(Msg::NexusUser(r));
        });
    }

    pub fn check_updates(&self, mods: Vec<(String, Source, u64, String)>) {
        self.spawn(move |w| {
            for (key, source, id, current) in mods {
                let remote = match source {
                    Source::ModWorkshop => mws::version(id).ok(),
                    Source::Nexus => nexus::version(id).ok(),
                    Source::Local => None,
                };
                let version = remote.filter(|r| !r.trim().is_empty() && !same_version(r, &current));
                w.send(Msg::UpdateInfo { key, version });
            }
            w.send(Msg::UpdateCheckDone);
        });
    }

    /// Look up names for the mods in an imported modpack code (codes only store ids).
    /// A few threads share the list so big packs don't take ages or flood the sites.
    pub fn lookup_pack_names(&self, gen: u64, mods: Vec<(Source, u64)>) {
        let queue = std::sync::Arc::new(std::sync::Mutex::new(mods));
        for _ in 0..4 {
            let queue = queue.clone();
            self.spawn(move |w| {
                // Pop under the lock, then release it before the network call.
                let next = || queue.lock().ok().and_then(|mut q| q.pop());
                while let Some((source, id)) = next() {
                    let info = match source {
                        Source::ModWorkshop => mws::detail(id).map(|(m, _)| m).ok(),
                        Source::Nexus => nexus::detail(id).ok(),
                        Source::Local => None,
                    };
                    let nsfw = info.as_ref().map(|m| m.nsfw).unwrap_or(false);
                    let name = info.map(|m| m.name).filter(|n| !n.trim().is_empty());
                    w.send(Msg::PackModInfo { gen, source, id, name, nsfw });
                }
            });
        }
    }

    /// Pack installed mods (with their files) into a modpack zip.
    pub fn export_pack(&self, id: u64, game_dir: PathBuf, mods: Vec<InstalledMod>, name: String, dest: PathBuf) {
        self.spawn(move |w| {
            let progress = |p: f32, s: &str| w.send(Msg::TaskProgress { id, progress: p, status: s.to_string() });
            let outcome = crate::modpack::export_zip(&game_dir, &mods, &name, &dest, &progress)
                .map(|bytes| TaskOutcome::Exported { path: dest.clone(), bytes })
                .map_err(|e| e.to_string());
            w.send(Msg::TaskDone { id, outcome });
        });
    }

    /// Install a modpack zip, leaving out the mods whose keys are in `skip`.
    pub fn install_pack_zip(&self, id: u64, archive: PathBuf, game_dir: PathBuf, skip: std::collections::HashSet<String>) {
        self.spawn(move |w| {
            let progress = |p: f32, s: &str| w.send(Msg::TaskProgress { id, progress: p, status: s.to_string() });
            let outcome = installer::install_pack_zip(&game_dir, &archive, &skip, &progress)
                .map(|r| TaskOutcome::Installed { mods: r.mods, warnings: r.warnings })
                .map_err(|e| e.to_string());
            w.send(Msg::TaskDone { id, outcome });
        });
    }

    pub fn install(&self, id: u64, req: InstallRequest, game_dir: PathBuf, api_key: String) {
        self.spawn(move |w| {
            let progress = |p: f32, s: &str| {
                w.send(Msg::TaskProgress { id, progress: p, status: s.to_string() });
            };
            let parent = req.display_name.clone();
            let on_deps = |deps: Vec<Dependency>| {
                if !deps.is_empty() {
                    w.send(Msg::TaskDeps { parent: parent.clone(), deps });
                }
            };
            let outcome = run_install(&req, &game_dir, &api_key, &progress, &on_deps).map_err(|e| e.to_string());
            w.send(Msg::TaskDone { id, outcome });
        });
    }
}

fn open_browser(url: &str, why: &str) -> anyhow::Result<TaskOutcome> {
    if !url.is_empty() {
        let _ = open::that(url);
    }
    Ok(TaskOutcome::OpenedBrowser(why.to_string()))
}

fn run_install(
    req: &InstallRequest,
    game_dir: &std::path::Path,
    api_key: &str,
    progress: installer::Progress,
    on_deps: &dyn Fn(Vec<Dependency>),
) -> anyhow::Result<TaskOutcome> {
    let dl_path = config::temp_dir().join(format!("dl_{}", installer::now_nanos()));
    let result = run_install_inner(req, game_dir, api_key, progress, on_deps, &dl_path);
    // Never leave partial or finished downloads behind.
    let _ = std::fs::remove_file(&dl_path);
    result
}

const WEBSITE_LINK: &str = "The mod's download link points to a website, so it was opened in your browser. Drag the file into the manager once it's downloaded.";

fn run_install_inner(
    req: &InstallRequest,
    game_dir: &std::path::Path,
    api_key: &str,
    progress: installer::Progress,
    on_deps: &dyn Fn(Vec<Dependency>),
    dl_path: &std::path::Path,
) -> anyhow::Result<TaskOutcome> {
    // Download `url` and install it; a web page instead of an archive opens in the browser.
    let fetch_install = |url: &str, browser_url: &str, meta: Option<&RemoteMod>, source: Source, ver: &str, site_ver: &str| -> anyhow::Result<TaskOutcome> {
        let r = installer::download(url, dl_path, progress).and_then(|p| {
            installer::install_archive(game_dir, &p, meta, source, ver, &req.display_name, progress)
        });
        match r {
            Ok(r) => Ok(installed(r, site_ver)),
            Err(e) if e.to_string() == NOT_AN_ARCHIVE => open_browser(browser_url, WEBSITE_LINK),
            Err(e) => Err(e),
        }
    };

    // 1. Local archive.
    if let Some(p) = &req.local_path {
        return match installer::install_archive(game_dir, p, req.mod_info.as_ref(), req.source, "", &req.display_name, progress) {
            Ok(r) => Ok(installed(r, "")),
            Err(e) if e.to_string() == NOT_AN_ARCHIVE => Err(anyhow::anyhow!("That file isn't a .zip, .7z or .rar archive.")),
            Err(e) => Err(e),
        };
    }

    // 2. nxm:// link from the browser.
    if let Some(link) = &req.nxm {
        let l = nexus::parse_nxm(link)?;
        progress(0.02, "Resolving Nexus download…");
        on_deps(nexus::requirements(l.mod_id).unwrap_or_default());
        let meta = nexus::detail(l.mod_id).ok();
        let version = nexus::files(l.mod_id)
            .ok()
            .and_then(|fs| fs.into_iter().find(|f| f.id == l.file_id))
            .map(|f| f.version)
            .unwrap_or_default();
        let url = nexus::download_link(api_key, l.mod_id, l.file_id, Some((&l.key, &l.expires))).map_err(|e| {
            if e.to_string() == "NEEDS_KEY" {
                anyhow::anyhow!("Add your Nexus API key in Settings to install from Nexus links.")
            } else {
                e
            }
        })?;
        let site_ver = meta.as_ref().map(|m| m.version.clone()).unwrap_or_default();
        let page = meta.as_ref().map(|m| m.page_url.clone()).unwrap_or_default();
        return fetch_install(&url, &page, meta.as_ref(), Source::Nexus, &version, &site_ver);
    }

    let meta = req.mod_info.clone().ok_or_else(|| anyhow::anyhow!("Nothing to install"))?;

    // 3. ModWorkshop: respect the author's "no mod managers" setting and find the main download.
    let mut site_version = meta.version.clone();
    let mut primary_download = None;
    if meta.source == Source::ModWorkshop {
        progress(0.01, "Checking mod…");
        if let Ok(p) = mws::primary(meta.id) {
            if p.managers_disabled {
                return open_browser(
                    &meta.page_url,
                    "The author asks that this mod be downloaded from the website, so its page was opened in your browser. Drag the file in once it's downloaded.",
                );
            }
            if !p.version.is_empty() {
                site_version = p.version;
            }
            primary_download = p.download;
            on_deps(p.deps);
        }
    }

    if meta.source == Source::Nexus {
        on_deps(nexus::requirements(meta.id).unwrap_or_default());
    }

    // 4. Pick a file if the user didn't choose one.
    let file = match &req.file {
        Some(f) => f.clone(),
        None => {
            progress(0.02, "Finding files…");
            let files = match meta.source {
                Source::ModWorkshop => mws::files(meta.id)?,
                Source::Nexus => nexus::files(meta.id)?,
                Source::Local => vec![],
            };
            let main = primary_download.as_ref().and_then(|(kind, id)| {
                files.iter().find(|f| {
                    f.id == *id
                        && match f.kind {
                            FileKind::Direct(_) => kind == "file",
                            FileKind::Link(_) => kind == "link",
                            FileKind::Nexus { .. } => false,
                        }
                })
            });
            match main.or(files.first()) {
                Some(f) => f.clone(),
                None => return open_browser(&meta.page_url, "This mod has no downloadable files listed, so its page was opened in your browser."),
            }
        }
    };

    match &file.kind {
        FileKind::Direct(url) => fetch_install(url, &meta.page_url, Some(&meta), meta.source, &file.version, &site_version),
        FileKind::Link(url) => {
            let direct = guess_direct_link(url);
            match fetch_install(&direct, url, Some(&meta), meta.source, &file.version, &site_version) {
                // Link hosts that refuse direct downloads: let the user grab it in the browser.
                Err(e) if is_download_error(&e) => open_browser(url, WEBSITE_LINK),
                r => r,
            }
        }
        FileKind::Nexus { mod_id, file_id } => {
            progress(0.02, "Requesting Nexus download link…");
            match nexus::download_link(api_key, *mod_id, *file_id, None) {
                Ok(url) => fetch_install(&url, &meta.page_url, Some(&meta), Source::Nexus, &file.version, &site_version),
                Err(e) => {
                    let msg = e.to_string();
                    let page = format!(
                        "https://www.nexusmods.com/{}/mods/{}?tab=files&file_id={}&nmm=1",
                        nexus::DOMAIN, mod_id, file_id
                    );
                    if msg == "NEEDS_KEY" {
                        open_browser(&page, "Nexus needs you to log in, so the file page was opened. Add your API key in Settings and register the 'Mod Manager Download' handler for one-click installs, or drag the downloaded file into the manager.")
                    } else if msg == "NEEDS_PREMIUM" {
                        open_browser(&page, "Free Nexus accounts must click 'Slow download' on the website. The page is open; with the nxm handler registered (Settings) it will install automatically.")
                    } else {
                        Err(e)
                    }
                }
            }
        }
    }
}

fn is_download_error(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<reqwest::Error>()) || e.to_string().starts_with("Download failed")
}

fn installed(r: installer::InstallResult, site_version: &str) -> TaskOutcome {
    let mut mods = r.mods;
    if !site_version.trim().is_empty() {
        for m in &mut mods {
            m.source_version = site_version.trim().to_string();
        }
    }
    TaskOutcome::Installed { mods, warnings: r.warnings }
}

/// Compare versions loosely: "v1.2.0" == "1.2.0" == "1.2.0 ".
pub fn same_version(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_start_matches(['v', 'V']).trim().to_lowercase();
    norm(a) == norm(b)
}

pub fn fetch_deps(source: Source, id: u64) -> Vec<Dependency> {
    match source {
        Source::ModWorkshop => mws::primary(id).map(|p| p.deps).unwrap_or_default(),
        Source::Nexus => nexus::requirements(id).unwrap_or_default(),
        Source::Local => vec![],
    }
}

/// Turn common share links into direct downloads.
fn guess_direct_link(url: &str) -> String {
    let u = url.trim();
    if u.contains("github.com") && u.contains("/blob/") {
        return u.replace("/blob/", "/raw/");
    }
    if u.contains("dropbox.com") {
        return u.replace("dl=0", "dl=1");
    }
    if let Some(id) = u.split("drive.google.com/file/d/").nth(1).and_then(|r| r.split('/').next()) {
        return format!("https://drive.google.com/uc?export=download&id={id}");
    }
    u.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(same_version("v1.2.0", "1.2.0 "));
        assert!(same_version("V2", "v2"));
        assert!(!same_version("1.2", "1.3"));
    }
    #[test]
    fn nxm() {
        let l = nexus::parse_nxm("nxm://payday2/mods/359/files/657?key=abc&expires=123&user_id=9").unwrap();
        assert_eq!((l.mod_id, l.file_id, l.key.as_str(), l.expires.as_str()), (359, 657, "abc", "123"));
        assert!(nexus::parse_nxm("nxm://skyrim/mods/1/files/2").is_err());
    }
}
