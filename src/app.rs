//! The main window.

use crate::api::{self, mws, nexus};
use crate::config::{ModDb, Settings};
use crate::game::{self, LoaderStatus, Store};
use crate::model::*;
use crate::modpack;
use crate::theme::{self, *};
use crate::worker::{Msg, TaskOutcome, Worker};
use eframe::egui::{self, Align, Align2, Color32, Layout, RichText, Rounding, Sense, Stroke};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const BEARDLIB_MWS_ID: u64 = 14924;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Browse,
    Installed,
    Downloads,
    Settings,
}

#[derive(Default)]
struct Browse {
    query: String,
    sort: usize,
    tags: BTreeSet<u64>,
    category: Option<u64>,
    category_name: Option<String>,
    mods: Vec<RemoteMod>,
    has_more: bool,
    loading: bool,
    error: Option<String>,
    page: u32,
    gen: u64,
    loaded_once: bool,
}

struct Detail {
    source: Source,
    id: u64,
    info: RemoteMod,
    full_loaded: bool,
    managers_disabled: bool,
    files: Option<Result<Vec<RemoteFile>, String>>,
    deps: Option<Vec<Dependency>>,
}

#[derive(Debug, Clone, PartialEq)]
enum TaskState {
    Running,
    Done(String),
    /// A modpack zip was written to this path.
    Saved(PathBuf, u64),
    Browser(String),
    Failed(String),
}

struct Task {
    id: u64,
    name: String,
    progress: f32,
    status: String,
    state: TaskState,
    /// Which remote mod this task installs (to avoid queueing a dependency twice).
    target: Option<(Source, u64)>,
    /// Page to open for manual steps.
    link: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum ToastKind {
    Info,
    Good,
    Warn,
    Bad,
}

struct Toast {
    text: String,
    kind: ToastKind,
    until: Instant,
}

/// "Share modpack" window.
#[derive(Default)]
struct ShareDialog {
    name: String,
    include_disabled: bool,
}

/// "Import modpack" window.
#[derive(Default)]
struct ImportDialog {
    text: String,
    /// `text` the current `pack` was decoded from.
    parsed_for: String,
    pack: Option<Result<modpack::Pack, String>>,
    /// Bumped on every re-parse so late name lookups for an old code are ignored.
    gen: u64,
    /// Name lookups still running for `pack`.
    pending: usize,
    /// Set when a modpack zip was opened instead of a code.
    zip: Option<(PathBuf, modpack::ZipManifest)>,
    /// Zip mode: overwrite mods the user already has with the pack's copies.
    replace_existing: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PackStatus {
    Installed,
    Downloading,
    HiddenNsfw,
    Missing,
}

// Short-lived UI messages; boxing the big variants would only add noise.
#[allow(clippy::large_enum_variant)]
enum Action {
    OpenDetail(RemoteMod),
    Install(InstallRequest),
    ToggleSelect(RemoteMod),
    Search { reset: bool },
    SetEnabled(String, bool),
    Uninstall(String),
    AddTag(String, String),
    RemoveTag(String, String),
    InstallSelected,
}

pub struct App {
    settings: Settings,
    db: ModDb,
    game_dir: Option<PathBuf>,
    store: Store,
    loaders: LoaderStatus,
    worker: Worker,
    rx: Receiver<Msg>,

    view: View,
    source: Source,
    mws: Browse,
    nexus: Browse,
    mws_tags: Vec<TagChip>,
    mws_categories: Vec<mws::Category>,
    nexus_categories: BTreeSet<String>,
    selected: Vec<RemoteMod>,
    detail: Option<Detail>,
    last_query_edit: Option<Instant>,

    tasks: Vec<Task>,
    next_task: u64,
    toasts: Vec<Toast>,

    inst_query: String,
    inst_tag: Option<String>,
    inst_kind: Option<ModKind>,
    tag_inputs: HashMap<String, String>,
    adding_tag_for: Option<String>,
    confirm_uninstall: Option<String>,
    confirm_show_nsfw: bool,
    share: Option<ShareDialog>,
    import: Option<ImportDialog>,
    checking_updates: bool,
    game_running: bool,

    api_key_input: String,
    show_key: bool,
    nexus_user: Option<Result<(String, bool), String>>,
    nxm_registered: bool,
    pending_nxm: Vec<String>,
    setup_dismissed: bool,
    /// Dev hook: PD2MM_SCREENSHOT=<png path> saves a frame after startup.
    shot: Option<(String, Instant, bool)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, worker: Worker, rx: Receiver<Msg>, startup_nxm: Option<String>, startup_file: Option<PathBuf>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let _ = crate::worker::EGUI_CTX.set(cc.egui_ctx.clone());

        let mut settings = Settings::load();
        if let Some(scale) = settings.ui_scale {
            cc.egui_ctx.set_zoom_factor(scale);
        }
        let game_dir = std::env::var_os("PD2MM_GAME_DIR")
            .map(PathBuf::from)
            .filter(|p| game::is_game_dir(p))
            .or_else(|| settings.game_dir.clone().filter(|p| game::is_game_dir(p)))
            .or_else(|| game::detect().map(|(p, _)| p));
        if game_dir.is_some() && settings.game_dir != game_dir {
            settings.game_dir = game_dir.clone();
            settings.save();
        }
        let api_key_input = settings.nexus_api_key.clone();
        let mut app = Self {
            store: game_dir.as_deref().map(game::store_of).unwrap_or(Store::Manual),
            loaders: game_dir.as_deref().map(game::loader_status).unwrap_or_default(),
            settings,
            db: ModDb::load(),
            game_dir,
            worker,
            rx,
            view: View::Browse,
            source: Source::ModWorkshop,
            mws: Browse { sort: 0, ..Default::default() },
            nexus: Browse { sort: 0, ..Default::default() },
            mws_tags: vec![],
            mws_categories: vec![],
            nexus_categories: BTreeSet::new(),
            selected: vec![],
            detail: None,
            last_query_edit: None,
            tasks: vec![],
            next_task: 1,
            toasts: vec![],
            inst_query: String::new(),
            inst_tag: None,
            inst_kind: None,
            tag_inputs: HashMap::new(),
            adding_tag_for: None,
            confirm_uninstall: None,
            confirm_show_nsfw: false,
            share: None,
            import: None,
            checking_updates: false,
            game_running: false,
            api_key_input,
            show_key: false,
            nexus_user: None,
            nxm_registered: game::nxm_registered_to_us(),
            pending_nxm: startup_nxm.into_iter().collect(),
            setup_dismissed: false,
            shot: std::env::var("PD2MM_SCREENSHOT").ok().map(|p| (p, Instant::now(), false)),
        };
        // Test hook: PD2MM_AUTOINSTALL=mws:<id> or nexus:<id> installs that mod on startup.
        if let Ok(spec) = std::env::var("PD2MM_AUTOINSTALL") {
            if let Some((src, id)) = spec.split_once(':') {
                if let Ok(id) = id.parse::<u64>() {
                    let source = if src == "nexus" { Source::Nexus } else { Source::ModWorkshop };
                    let page_url = if source == Source::Nexus {
                        format!("https://www.nexusmods.com/payday2/mods/{id}")
                    } else {
                        format!("https://modworkshop.net/mod/{id}")
                    };
                    let info = RemoteMod::stub(source, id, format!("{src} {id}"), page_url);
                    app.quick_install(&info);
                }
            }
        }
        match std::env::var("PD2MM_VIEW").as_deref() {
            Ok("installed") => app.view = View::Installed,
            Ok("settings") => app.view = View::Settings,
            Ok("downloads") => app.view = View::Downloads,
            Ok("share") => {
                app.view = View::Installed;
                app.share = Some(ShareDialog { name: "Test pack".into(), ..Default::default() });
            }
            Ok("import") => {
                app.view = View::Installed;
                app.open_import(std::env::var("PD2MM_IMPORT").unwrap_or_default());
            }
            Ok("nexus") => {
                app.source = Source::Nexus;
                app.search(Source::Nexus, true);
            }
            _ => {}
        }
        app.rescan();
        if let Some(p) = startup_file {
            app.open_file(p);
        }
        app.worker.watch_game();
        app.worker.load_mws_meta();
        app.search(Source::ModWorkshop, true);
        if !app.settings.nexus_api_key.is_empty() {
            app.worker.validate_nexus(app.settings.nexus_api_key.clone());
        }
        app
    }

    // ---------------------------------------------------------------- helpers

    fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        let secs = if matches!(kind, ToastKind::Bad | ToastKind::Warn) { 8 } else { 4 };
        self.toasts.push(Toast { text: text.into(), kind, until: Instant::now() + Duration::from_secs(secs) });
    }

    fn rescan(&mut self) {
        if let Some(g) = self.game_dir.clone() {
            if !self.game_running {
                let moved = game::repair_misplaced(&g, &mut self.db);
                if !moved.is_empty() {
                    self.toast(
                        format!("Fixed {} mod(s) that were in the wrong folder: {}", moved.len(), moved.join(", ")),
                        ToastKind::Good,
                    );
                }
            }
            game::scan(&g, &mut self.db);
            self.db.save();
            self.loaders = game::loader_status(&g);
        }
    }

    fn browse(&mut self, s: Source) -> &mut Browse {
        if s == Source::Nexus { &mut self.nexus } else { &mut self.mws }
    }

    fn search(&mut self, source: Source, reset: bool) {
        let hide = self.settings.hide_nsfw;
        let b = self.browse(source);
        if reset {
            b.page = 1;
            b.mods.clear();
            b.has_more = false;
        } else {
            b.page += 1;
        }
        b.gen += 1;
        b.loading = true;
        b.error = None;
        b.loaded_once = true;
        let gen = b.gen;
        match source {
            Source::Nexus => {
                let q = nexus::Query {
                    text: b.query.clone(),
                    sort: nexus::SORTS[b.sort].0,
                    category: b.category_name.clone(),
                    hide_adult: hide,
                    offset: (b.page - 1) * 30,
                };
                self.worker.search_nexus(q, gen);
            }
            _ => {
                let q = mws::Query {
                    text: b.query.clone(),
                    sort: mws::SORTS[b.sort].0,
                    tags: b.tags.iter().copied().collect(),
                    category: b.category,
                    page: b.page,
                };
                self.worker.search_mws(q, gen);
            }
        }
    }

    fn installed_ids(&self) -> HashSet<(Source, u64)> {
        self.db.mods.values().filter_map(|m| m.source_id.map(|id| (m.source, id))).collect()
    }

    fn start_install(&mut self, req: InstallRequest) {
        if self.game_dir.is_some() && self.game_running {
            self.toast("PAYDAY 2 is running. Mods take effect next time you start it, and some files may be locked until you close it.", ToastKind::Warn);
        }
        if self.enqueue_install(req) {
            self.toast("Download started", ToastKind::Info);
        }
    }

    /// Queue an install without any toasts. Returns false if no game folder is set.
    fn enqueue_install(&mut self, req: InstallRequest) -> bool {
        let Some(game) = self.game_dir.clone() else {
            self.toast("Set your PAYDAY 2 folder first (Settings).", ToastKind::Warn);
            self.view = View::Settings;
            return false;
        };
        let id = self.next_task;
        self.next_task += 1;
        self.tasks.push(Task {
            id,
            name: req.display_name.clone(),
            progress: 0.0,
            status: "Queued".into(),
            state: TaskState::Running,
            target: req.mod_info.as_ref().map(|m| (m.source, m.id)),
            link: req.mod_info.as_ref().map(|m| m.page_url.clone()).filter(|u| !u.is_empty()),
        });
        self.worker.install(id, req, game, self.settings.nexus_api_key.clone());
        true
    }

    /// True if a pack entry is already installed, matching by site id or, for mods
    /// installed from a file (e.g. BeardLib), by name.
    fn pack_mod_installed(&self, m: &modpack::PackMod) -> bool {
        self.dep_installed(&Dependency { source: m.source, id: Some(m.id), name: m.name.clone(), url: None, optional: false })
    }

    fn queued_ids(&self) -> HashSet<(Source, u64)> {
        self.tasks.iter().filter(|t| t.state == TaskState::Running).filter_map(|t| t.target).collect()
    }

    /// Install every mod in an imported pack that isn't installed or downloading yet.
    fn install_pack(&mut self, pack: &modpack::Pack) {
        let queued = self.queued_ids();
        let hide_nsfw = self.settings.hide_nsfw;
        let (mut started, mut nsfw_skipped, mut nexus) = (0, 0, 0);
        for m in &pack.mods {
            let source = m.source;
            if self.pack_mod_installed(m) || queued.contains(&(source, m.id)) {
                continue;
            }
            if hide_nsfw && m.nsfw {
                nsfw_skipped += 1;
                continue;
            }
            let req = InstallRequest {
                source,
                mod_info: Some(RemoteMod::stub(source, m.id, m.display_name(), m.page_url())),
                file: None,
                local_path: None,
                nxm: None,
                display_name: m.display_name(),
            };
            if !self.enqueue_install(req) {
                return;
            }
            started += 1;
            if source == Source::Nexus {
                nexus += 1;
            }
        }
        if started == 0 {
            self.toast("You already have every mod in this pack.", ToastKind::Good);
            return;
        }
        let name = if pack.name.is_empty() { "the modpack".to_string() } else { format!("'{}'", pack.name) };
        self.toast(format!("Installing {started} mod(s) from {name}. Mods they need are added automatically."), ToastKind::Info);
        if self.game_running {
            self.toast("PAYDAY 2 is running. Mods take effect next time you start it.", ToastKind::Warn);
        }
        let premium = matches!(self.nexus_user, Some(Ok((_, true))));
        if nexus > 0 && !premium {
            self.toast(
                format!("{nexus} Nexus mod(s) need the website's download button without a Premium account, so their pages will open in your browser."),
                ToastKind::Warn,
            );
        }
        if nsfw_skipped > 0 {
            self.toast(format!("Skipped {nsfw_skipped} NSFW mod(s) because the content filter is on (Settings)."), ToastKind::Warn);
        }
        self.view = View::Downloads;
    }

    fn open_import(&mut self, text: String) {
        self.import = Some(ImportDialog { text, ..Default::default() });
    }

    /// Open a file passed to the app: a modpack code file or any mod archive.
    fn open_file(&mut self, path: PathBuf) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == modpack::FILE_EXT || ext == "txt" {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.view = View::Installed;
                    self.open_import(text);
                }
                Err(e) => self.toast(format!("Couldn't read {}: {e}", path.display()), ToastKind::Bad),
            }
        } else if ["zip", "7z", "rar"].contains(&ext.as_str()) {
            self.install_local(path);
        } else {
            self.toast(format!("{} isn't a mod archive or modpack", path.display()), ToastKind::Warn);
        }
    }

    fn open_zip_import(&mut self, path: PathBuf, manifest: modpack::ZipManifest) {
        self.view = View::Installed;
        self.import = Some(ImportDialog { zip: Some((path, manifest)), ..Default::default() });
    }

    /// Add a task row (shown under Downloads) and return its id.
    fn new_task(&mut self, name: String, status: &str) -> u64 {
        let id = self.next_task;
        self.next_task += 1;
        self.tasks.push(Task { id, name, progress: 0.0, status: status.into(), state: TaskState::Running, target: None, link: None });
        id
    }

    fn export_pack_zip(&mut self, name: &str, include_disabled: bool) {
        let Some(game) = self.game_dir.clone() else { return };
        let mods: Vec<InstalledMod> = self.db.mods.values().filter(|m| m.enabled || include_disabled).cloned().collect();
        if mods.is_empty() {
            return;
        }
        let base = if name.trim().is_empty() { "PAYDAY 2 modpack" } else { name.trim() };
        let Some(dest) = rfd::FileDialog::new()
            .add_filter("Zip archive", &["zip"])
            .set_file_name(format!("{}.zip", crate::installer::sanitize(base)))
            .set_title("Save modpack zip")
            .save_file()
        else {
            return;
        };
        let id = self.new_task(format!("Modpack zip: {base}"), "Preparing…");
        self.worker.export_pack(id, game, mods, name.to_string(), dest);
        self.toast("Packing your mods into a zip. Progress is under Downloads.", ToastKind::Info);
    }

    /// Compare mod names ignoring case, spaces and punctuation.
    fn norm_name(s: &str) -> String {
        s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
    }

    fn dep_installed(&self, d: &Dependency) -> bool {
        let n = Self::norm_name(&d.name);
        if n.contains("superblt") {
            return self.loaders.superblt;
        }
        if n == "beardlib" || (d.source == Source::ModWorkshop && d.id == Some(BEARDLIB_MWS_ID)) {
            return self.loaders.beardlib;
        }
        self.db.mods.values().any(|m| {
            (d.id.is_some() && m.source == d.source && m.source_id == d.id)
                || (!n.is_empty() && (Self::norm_name(&m.name) == n || Self::norm_name(&m.folder) == n))
        })
    }

    fn dep_queued(&self, d: &Dependency) -> bool {
        self.tasks.iter().any(|t| {
            t.state == TaskState::Running
                && ((t.target.is_some() && t.target == d.id.map(|id| (d.source, id)))
                    || Self::norm_name(&t.name) == Self::norm_name(&d.name))
        })
    }

    /// Install every required dependency that isn't already installed or downloading.
    fn queue_dependencies(&mut self, parent: &str, deps: Vec<Dependency>) {
        let mut optional = vec![];
        for d in deps {
            if self.dep_installed(&d) || self.dep_queued(&d) {
                continue;
            }
            if d.optional {
                optional.push(d.name.clone());
                continue;
            }
            match d.id {
                Some(id) if d.source != Source::Local => {
                    let info = RemoteMod::stub(d.source, id, d.name.clone(), d.url.clone().unwrap_or_default());
                    self.toast(format!("'{parent}' needs '{}', installing it too", d.name), ToastKind::Info);
                    self.start_install(InstallRequest {
                        source: d.source,
                        mod_info: Some(info),
                        file: None,
                        local_path: None,
                        nxm: None,
                        display_name: d.name.clone(),
                    });
                }
                _ => {
                    // Off-site requirement we can't download automatically.
                    let id = self.next_task;
                    self.next_task += 1;
                    let why = format!("Required by '{parent}'. This one isn't on ModWorkshop or Nexus, so install it from its website.");
                    self.tasks.push(Task {
                        id,
                        name: d.name.clone(),
                        progress: 1.0,
                        status: String::new(),
                        state: TaskState::Browser(why),
                        target: None,
                        link: d.url.clone(),
                    });
                    self.toast(format!("'{parent}' needs '{}', which must be installed manually (see Downloads)", d.name), ToastKind::Warn);
                }
            }
        }
        if !optional.is_empty() {
            self.toast(format!("Optional for '{parent}': {}", optional.join(", ")), ToastKind::Info);
        }
    }

    fn launch_game(&mut self) {
        let Some(g) = self.game_dir.clone() else { return };
        let running = self.tasks.iter().filter(|t| t.state == TaskState::Running).count();
        if running > 0 {
            self.toast(format!("{running} download(s) still installing; those mods will load next launch."), ToastKind::Warn);
        }
        match game::launch(&g, self.store) {
            Ok(()) => {
                self.toast(format!("Launching PAYDAY 2 via {}…", self.store.label()), ToastKind::Good);
                self.game_running = true; // shown immediately; the watcher confirms it
            }
            Err(e) => self.toast(format!("Couldn't launch: {e}"), ToastKind::Bad),
        }
    }

    fn quick_install(&mut self, m: &RemoteMod) {
        self.start_install(InstallRequest {
            source: m.source,
            mod_info: Some(m.clone()),
            file: None,
            local_path: None,
            nxm: None,
            display_name: m.name.clone(),
        });
    }

    fn install_local(&mut self, path: PathBuf) {
        // Modpack zips get a confirmation window listing what's inside.
        let is_zip = path.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false);
        if let Some(manifest) = is_zip.then(|| modpack::read_zip_manifest(&path)).flatten() {
            self.open_zip_import(path, manifest);
            return;
        }
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Mod".into());
        self.start_install(InstallRequest {
            source: Source::Local,
            mod_info: None,
            file: None,
            local_path: Some(path),
            nxm: None,
            display_name: name,
        });
    }

    fn set_game_dir(&mut self, p: PathBuf) {
        if game::is_game_dir(&p) {
            self.store = game::store_of(&p);
            self.settings.game_dir = Some(p.clone());
            self.settings.save();
            self.game_dir = Some(p);
            self.db = ModDb::load();
            self.rescan();
            self.toast("PAYDAY 2 folder set", ToastKind::Good);
        } else {
            self.toast("That folder doesn't contain PAYDAY2.exe", ToastKind::Bad);
        }
    }

    fn set_hide_nsfw(&mut self, hide: bool) {
        self.settings.hide_nsfw = hide;
        self.settings.save();
        if self.nexus.loaded_once {
            self.search(Source::Nexus, true);
        }
        if hide {
            self.selected.retain(|m| !m.nsfw);
            if self.detail.as_ref().map(|d| d.info.nsfw).unwrap_or(false) {
                self.detail = None;
            }
        }
    }

    // ---------------------------------------------------------------- messages

    fn handle_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Page { source, gen, append, result } => {
                    let b = self.browse(source);
                    if gen != b.gen {
                        continue;
                    }
                    b.loading = false;
                    match result {
                        Ok((mods, more)) => {
                            if !append {
                                b.mods.clear();
                            }
                            let seen: HashSet<u64> = b.mods.iter().map(|m| m.id).collect();
                            b.mods.extend(mods.into_iter().filter(|m| !seen.contains(&m.id)));
                            b.has_more = more;
                            if source == Source::Nexus {
                                let cats: Vec<String> =
                                    self.nexus.mods.iter().map(|m| m.category.clone()).filter(|c| !c.is_empty()).collect();
                                self.nexus_categories.extend(cats);
                            }
                        }
                        Err(e) => b.error = Some(e),
                    }
                }
                Msg::MwsTags(t) => self.mws_tags = t,
                Msg::MwsCategories(c) => self.mws_categories = c,
                Msg::Detail { source, id, result } => {
                    if let Some(d) = &mut self.detail {
                        if d.source == source && d.id == id {
                            if let Ok((m, disabled)) = result {
                                d.info = m;
                                d.full_loaded = true;
                                d.managers_disabled = disabled;
                            } else {
                                d.full_loaded = true;
                            }
                        }
                    }
                }
                Msg::Files { source, id, result } => {
                    if let Some(d) = &mut self.detail {
                        if d.source == source && d.id == id {
                            d.files = Some(result);
                        }
                    }
                }
                Msg::TaskProgress { id, progress, status } => {
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        t.progress = progress;
                        t.status = status;
                    }
                }
                Msg::TaskDone { id, outcome } => {
                    let name = self.tasks.iter().find(|t| t.id == id).map(|t| t.name.clone()).unwrap_or_default();
                    let state = match outcome {
                        Ok(TaskOutcome::Installed { mods, warnings }) => {
                            let places: Vec<String> =
                                mods.iter().map(|m| format!("{} > {}", m.kind.dir_name(), m.folder)).collect();
                            // Older versions of the same mod installed under a different folder name
                            // would otherwise stay installed next to the new one.
                            let new_keys: HashSet<String> = mods.iter().map(|m| m.key.clone()).collect();
                            let stale: Vec<InstalledMod> = self
                                .db
                                .mods
                                .values()
                                .filter(|o| !new_keys.contains(&o.key))
                                .filter(|o| {
                                    o.source != Source::Local
                                        && mods.iter().any(|m| m.source == o.source && m.source_id.is_some() && m.source_id == o.source_id)
                                })
                                .cloned()
                                .collect();
                            let mut carried_tags: Vec<String> = vec![];
                            for o in &stale {
                                if let Some(g) = &self.game_dir {
                                    if game::uninstall(g, o).is_ok() {
                                        carried_tags.extend(o.user_tags.iter().cloned());
                                        self.db.mods.remove(&o.key);
                                    }
                                }
                            }
                            for mut m in mods {
                                let mut tags = self.db.mods.get(&m.key).map(|o| o.user_tags.clone()).unwrap_or_default();
                                for t in &carried_tags {
                                    if !tags.contains(t) {
                                        tags.push(t.clone());
                                    }
                                }
                                m.user_tags = tags;
                                self.db.mods.insert(m.key.clone(), m);
                            }
                            self.db.save();
                            self.rescan();
                            self.toast(format!("Installed {name}"), ToastKind::Good);
                            for w in warnings {
                                self.toast(w, ToastKind::Warn);
                            }
                            TaskState::Done(places.join(", "))
                        }
                        Ok(TaskOutcome::Exported { path, bytes }) => {
                            self.toast(format!("Modpack saved: {} ({})", path.display(), theme::fmt_size(bytes)), ToastKind::Good);
                            TaskState::Saved(path, bytes)
                        }
                        Ok(TaskOutcome::OpenedBrowser(why)) => {
                            self.toast(why.clone(), ToastKind::Warn);
                            TaskState::Browser(why)
                        }
                        Err(e) => {
                            self.toast(format!("{name}: {e}"), ToastKind::Bad);
                            TaskState::Failed(e)
                        }
                    };
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        t.state = state;
                        t.progress = 1.0;
                    }
                }
                Msg::Nxm(link) => {
                    self.pending_nxm.push(link);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Msg::OpenFile(p) => self.open_file(p),
                Msg::Focus => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Msg::NexusUser(r) => self.nexus_user = Some(r),
                Msg::Deps { source, id, deps } => {
                    if let Some(d) = &mut self.detail {
                        if d.source == source && d.id == id {
                            d.deps = Some(deps);
                        }
                    }
                }
                Msg::TaskDeps { parent, deps } => self.queue_dependencies(&parent, deps),
                Msg::GameRunning(running) => {
                    if self.game_running && !running {
                        // Game closed: pick up anything that changed while it ran.
                        self.rescan();
                    }
                    self.game_running = running;
                }
                Msg::UpdateInfo { key, version } => {
                    if let Some(m) = self.db.mods.get_mut(&key) {
                        m.update_available = version;
                    }
                }
                Msg::PackModInfo { gen, source, id, name, nsfw } => {
                    if let Some(d) = self.import.as_mut().filter(|d| d.gen == gen) {
                        d.pending = d.pending.saturating_sub(1);
                        if let Some(Ok(pack)) = &mut d.pack {
                            for m in pack.mods.iter_mut().filter(|m| m.source == source && m.id == id) {
                                if let Some(n) = &name {
                                    m.name = n.clone();
                                }
                                m.nsfw |= nsfw;
                            }
                        }
                    }
                }
                Msg::UpdateCheckDone => {
                    self.checking_updates = false;
                    let n = self.db.mods.values().filter(|m| m.update_available.is_some()).count();
                    if n == 0 {
                        self.toast("Everything is up to date", ToastKind::Good);
                    } else {
                        self.toast(format!("{n} update(s) available"), ToastKind::Info);
                    }
                }
            }
        }
        for link in std::mem::take(&mut self.pending_nxm) {
            if link.to_lowercase().starts_with("nxm://") {
                self.start_install(InstallRequest {
                    source: Source::Nexus,
                    mod_info: None,
                    file: None,
                    local_path: None,
                    nxm: Some(link.clone()),
                    display_name: format!("Nexus download {}", link.split('/').nth(4).unwrap_or("")),
                });
                self.view = View::Downloads;
            }
        }
    }

    fn apply_actions(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::OpenDetail(m) => {
                    let (source, id) = (m.source, m.id);
                    self.detail = Some(Detail { source, id, info: m, full_loaded: false, managers_disabled: false, files: None, deps: None });
                    self.worker.load_detail(source, id);
                }
                Action::Install(r) => self.start_install(r),
                Action::ToggleSelect(m) => {
                    if let Some(i) = self.selected.iter().position(|s| s.source == m.source && s.id == m.id) {
                        self.selected.remove(i);
                    } else {
                        self.selected.push(m);
                    }
                }
                Action::InstallSelected => {
                    for m in std::mem::take(&mut self.selected) {
                        self.quick_install(&m);
                    }
                }
                Action::Search { reset } => self.search(self.source, reset),
                Action::SetEnabled(key, on) => {
                    if let (Some(g), Some(m)) = (self.game_dir.clone(), self.db.mods.get_mut(&key)) {
                        if let Err(e) = game::set_enabled(&g, m, on) {
                            let e = e.to_string();
                            self.toast(e, ToastKind::Bad);
                        }
                        self.db.save();
                    }
                }
                Action::Uninstall(key) => {
                    if let (Some(g), Some(m)) = (self.game_dir.clone(), self.db.mods.get(&key).cloned()) {
                        match game::uninstall(&g, &m) {
                            Ok(()) => {
                                self.db.mods.remove(&key);
                                self.db.save();
                                self.toast(format!("Removed {}", m.name), ToastKind::Good);
                            }
                            Err(e) => self.toast(format!("Couldn't remove {}: {e}", m.name), ToastKind::Bad),
                        }
                        self.rescan();
                    }
                }
                Action::AddTag(key, tag) => {
                    let tag = tag.trim().to_string();
                    if let Some(m) = self.db.mods.get_mut(&key) {
                        if !tag.is_empty() && !m.user_tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                            m.user_tags.push(tag);
                        }
                    }
                    self.db.save();
                }
                Action::RemoveTag(key, tag) => {
                    if let Some(m) = self.db.mods.get_mut(&key) {
                        m.user_tags.retain(|t| t != &tag);
                    }
                    self.db.save();
                }
            }
        }
    }

    // ---------------------------------------------------------------- top bar

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .exact_height(52.0)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::symmetric(16.0, 0.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // game status pill
                    let (color, text) = match &self.game_dir {
                        Some(_) => (GOOD, format!("PAYDAY 2 · {}", self.store.label())),
                        None => (BAD, "PAYDAY 2 not found".to_string()),
                    };
                    egui::Frame::none().fill(CARD).rounding(Rounding::same(14.0_f32)).inner_margin(egui::Margin::symmetric(10.0, 4.0)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            status_dot(ui, color);
                            ui.label(RichText::new(text).size(13.0));
                        });
                    })
                    .response
                    .on_hover_text(self.game_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Open Settings to choose the folder".into()));

                    if self.game_dir.is_some() {
                        let (c, t) = if self.loaders.superblt { (GOOD, "SuperBLT") } else { (WARN, "SuperBLT missing") };
                        egui::Frame::none().fill(CARD).rounding(Rounding::same(14.0_f32)).inner_margin(egui::Margin::symmetric(10.0, 4.0)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                status_dot(ui, c);
                                ui.label(RichText::new(t).size(13.0));
                            });
                        });
                        let (c, t) = if self.loaders.beardlib { (GOOD, "BeardLib") } else { (MUTED, "No BeardLib") };
                        egui::Frame::none().fill(CARD).rounding(Rounding::same(14.0_f32)).inner_margin(egui::Margin::symmetric(10.0, 4.0)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                status_dot(ui, c);
                                ui.label(RichText::new(t).size(13.0));
                            });
                        });
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut show = !self.settings.hide_nsfw;
                        let before = show;
                        theme::toggle(ui, &mut show).on_hover_text(
                            "NSFW filter. Off by default: adult mods are hidden everywhere until you turn this on.",
                        );
                        let label = if show {
                            RichText::new("🔞 NSFW shown").color(NSFW).strong()
                        } else {
                            RichText::new("🛡 NSFW hidden").color(MUTED)
                        };
                        ui.label(label);
                        if show != before {
                            if show {
                                self.confirm_show_nsfw = true;
                            } else {
                                self.set_hide_nsfw(true);
                            }
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- nav

    fn nav(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .exact_width(212.0)
            .resizable(false)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::same(14.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("PD2").size(26.0).strong().color(ACCENT));
                ui.label(RichText::new("MOD MANAGER").size(13.0).strong().color(MUTED));
                ui.add_space(14.0);

                // Play button
                let w = ui.available_width();
                let (text, fill) = if self.game_running {
                    ("●  PAYDAY 2 is running", Color32::from_rgb(0x2E, 0x35, 0x42))
                } else {
                    ("▶  PLAY PAYDAY 2", GOOD)
                };
                let play = ui.add_enabled(
                    self.game_dir.is_some() && !self.game_running,
                    egui::Button::new(RichText::new(text).size(15.0).strong().color(Color32::WHITE))
                        .fill(fill)
                        .rounding(Rounding::same(8.0_f32))
                        .min_size(egui::vec2(w, 40.0)),
                );
                let play = if self.game_dir.is_none() {
                    play.on_disabled_hover_text("Set your PAYDAY 2 folder in Settings first")
                } else {
                    play.on_hover_text(format!("Launch through {}", self.store.label()))
                };
                if play.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.launch_game();
                }
                ui.add_space(14.0);

                theme::section_title(ui, "Browse");
                let browse_mws = self.view == View::Browse && self.source == Source::ModWorkshop;
                if nav_item(ui, "🔧  ModWorkshop", browse_mws, MWS, None) {
                    self.view = View::Browse;
                    self.source = Source::ModWorkshop;
                }
                let browse_nx = self.view == View::Browse && self.source == Source::Nexus;
                if nav_item(ui, "🌐  Nexus Mods", browse_nx, NEXUS, None) {
                    self.view = View::Browse;
                    self.source = Source::Nexus;
                    if !self.nexus.loaded_once {
                        self.search(Source::Nexus, true);
                    }
                }
                ui.add_space(12.0);
                theme::section_title(ui, "Library");
                let n_inst = self.db.mods.len();
                let n_upd = self.db.mods.values().filter(|m| m.update_available.is_some()).count();
                let badge = if n_upd > 0 { format!("{n_upd}↑") } else { n_inst.to_string() };
                if nav_item(ui, "📦  Installed", self.view == View::Installed, ACCENT, Some(badge)) {
                    self.view = View::Installed;
                    self.rescan();
                }
                let running = self.tasks.iter().filter(|t| t.state == TaskState::Running).count();
                if nav_item(ui, "⬇  Downloads", self.view == View::Downloads, ACCENT, (running > 0).then(|| running.to_string())) {
                    self.view = View::Downloads;
                }
                if nav_item(ui, "⚙  Settings", self.view == View::Settings, ACCENT, None) {
                    self.view = View::Settings;
                }

                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.label(RichText::new("Drop .zip / .7z / .rar files or a .pd2pack anywhere to install").size(11.0).color(MUTED));
                    ui.add_space(4.0);
                    if ghost_button(ui, "📂  Install from file…").clicked() {
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter("Mod archives", &["zip", "7z", "rar"])
                            .set_title("Choose mod archives")
                            .pick_files()
                        {
                            for f in files {
                                self.install_local(f);
                            }
                        }
                    }
                });
            });
    }

    // ---------------------------------------------------------------- browse

    fn browse_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let source = self.source;
        let hide = self.settings.hide_nsfw;
        let installed = self.installed_ids();
        let selected: HashSet<(Source, u64)> = self.selected.iter().map(|m| (m.source, m.id)).collect();

        // --- header + search row
        ui.horizontal(|ui| {
            let (title, color) = if source == Source::Nexus { ("Nexus Mods", NEXUS) } else { ("ModWorkshop", MWS) };
            ui.label(RichText::new(title).size(22.0).strong().color(color));
            ui.label(RichText::new("· PAYDAY 2").size(16.0).color(MUTED));
        });
        ui.add_space(6.0);

        let sorts: &[(&str, &str)] = if source == Source::Nexus { nexus::SORTS } else { mws::SORTS };
        let mut changed = false;
        ui.horizontal(|ui| {
            let b = if source == Source::Nexus { &mut self.nexus } else { &mut self.mws };
            let resp = ui.add(
                egui::TextEdit::singleline(&mut b.query)
                    .hint_text("🔍  Search mods…")
                    .desired_width((ui.available_width() - 420.0).max(200.0))
                    .margin(egui::vec2(10.0, 6.0)),
            );
            if resp.changed() {
                self.last_query_edit = Some(Instant::now());
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.last_query_edit = None;
                changed = true;
            }
            egui::ComboBox::from_id_salt(("sort", source == Source::Nexus))
                .width(170.0)
                .selected_text(sorts[b.sort].1)
                .show_ui(ui, |ui| {
                    for (i, (_, label)) in sorts.iter().enumerate() {
                        if ui.selectable_value(&mut b.sort, i, *label).changed() {
                            changed = true;
                        }
                    }
                });
            // category
            if source == Source::Nexus {
                let current = b.category_name.clone().unwrap_or_else(|| "All categories".into());
                egui::ComboBox::from_id_salt("nx_cat").width(170.0).selected_text(current).show_ui(ui, |ui| {
                    if ui.selectable_label(b.category_name.is_none(), "All categories").clicked() {
                        b.category_name = None;
                        changed = true;
                    }
                    for c in &self.nexus_categories {
                        if ui.selectable_label(b.category_name.as_deref() == Some(c), c).clicked() {
                            b.category_name = Some(c.clone());
                            changed = true;
                        }
                    }
                });
            } else {
                let current = b
                    .category
                    .and_then(|id| self.mws_categories.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "All categories".into());
                egui::ComboBox::from_id_salt("mws_cat").width(170.0).height(400.0).selected_text(current).show_ui(ui, |ui| {
                    if ui.selectable_label(b.category.is_none(), "All categories").clicked() {
                        b.category = None;
                        changed = true;
                    }
                    for c in &self.mws_categories {
                        let label = if c.parent.is_some() { format!("   {}", c.name) } else { c.name.clone() };
                        if ui.selectable_label(b.category == Some(c.id), label).clicked() {
                            b.category = Some(c.id);
                            changed = true;
                        }
                    }
                });
            }
        });

        // live search debounce
        if let Some(t) = self.last_query_edit {
            if t.elapsed() > Duration::from_millis(500) {
                self.last_query_edit = None;
                changed = true;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(120));
            }
        }

        // --- tag chips (ModWorkshop)
        if source == Source::ModWorkshop && !self.mws_tags.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                ui.label(RichText::new("Tags").size(12.0).color(MUTED));
                for t in &self.mws_tags {
                    let Some(id) = t.id else { continue };
                    let on = self.mws.tags.contains(&id);
                    if theme::chip(ui, &t.name, theme::rgb(t.color), on, true).clicked() {
                        if on {
                            self.mws.tags.remove(&id);
                        } else {
                            self.mws.tags.insert(id);
                        }
                        changed = true;
                    }
                }
                if !self.mws.tags.is_empty() && ui.small_button("✖ clear").clicked() {
                    self.mws.tags.clear();
                    changed = true;
                }
            });
        }
        if changed {
            actions.push(Action::Search { reset: true });
        }
        ui.add_space(8.0);

        let b = if source == Source::Nexus { &self.nexus } else { &self.mws };
        let visible: Vec<&RemoteMod> = b.mods.iter().filter(|m| !(hide && m.nsfw)).collect();
        let hidden = b.mods.len() - visible.len();

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} mods", visible.len())).color(MUTED).size(12.5));
            if hidden > 0 {
                ui.label(RichText::new(format!("· {hidden} hidden by NSFW filter")).color(NSFW).size(12.5));
            }
            if b.loading {
                ui.spinner();
            }
        });
        if let Some(e) = &b.error {
            ui.colored_label(BAD, format!("Couldn't load mods: {e}"));
            if ui.button("Retry").clicked() {
                actions.push(Action::Search { reset: true });
            }
        }

        // --- card grid
        let has_more = b.has_more;
        let loading = b.loading;
        egui::ScrollArea::vertical().id_salt(("grid", source == Source::Nexus)).auto_shrink([false, false]).show(ui, |ui| {
            let gap = 14.0;
            let avail = ui.available_width() - 4.0;
            let cols = ((avail + gap) / (250.0 + gap)).floor().max(1.0) as usize;
            let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor();
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for row in visible.chunks(cols) {
                ui.horizontal(|ui| {
                    for m in row {
                        let is_inst = installed.contains(&(m.source, m.id));
                        let is_sel = selected.contains(&(m.source, m.id));
                        mod_card(ui, m, w, is_inst, is_sel, actions);
                    }
                });
            }
            if visible.is_empty() && !loading && b.error.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No mods found").size(18.0).color(MUTED));
                    if hidden > 0 {
                        ui.label(RichText::new("Some results are hidden by the NSFW filter.").color(MUTED));
                    }
                });
            }
            if has_more {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if loading {
                        ui.spinner();
                    } else if ghost_button(ui, "Load more").clicked() {
                        actions.push(Action::Search { reset: false });
                    }
                });
            }
            ui.add_space(20.0);
        });
    }

    fn detail_panel(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(d) = &self.detail else { return };
        let installed = self.installed_ids().contains(&(d.source, d.id));
        let deps_ok: HashSet<String> = d
            .deps
            .iter()
            .flatten()
            .filter(|dep| self.dep_installed(dep))
            .map(|dep| dep.name.clone())
            .collect();
        let mut close = false;
        egui::SidePanel::right("detail")
            .default_width(400.0)
            .width_range(340.0..=560.0)
            .frame(egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::same(16.0)).stroke(Stroke::new(1.0_f32, BORDER)))
            .show(ctx, |ui| {
                let m = &d.info;
                ui.horizontal(|ui| {
                    let (src_c, src_t) = if m.source == Source::Nexus { (NEXUS, "Nexus Mods") } else { (MWS, "ModWorkshop") };
                    theme::chip(ui, src_t, src_c, false, false);
                    if m.nsfw {
                        theme::chip(ui, "NSFW", NSFW, true, false);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new("✖").frame(false)).on_hover_text("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let w = ui.available_width();
                    if let Some(img) = m.image.as_ref().or(m.thumb.as_ref()) {
                        let h = w * 9.0 / 16.0;
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), Sense::hover());
                        ui.painter().rect_filled(rect, Rounding::same(8.0_f32), CARD);
                        ui.put(rect, egui::Image::new(img.as_str()).max_size(rect.size()).rounding(Rounding::same(8.0_f32)));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new(&m.name).size(20.0).strong());
                    ui.label(RichText::new(format!("by {}", m.author)).color(MUTED));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("⬇ {}", fmt_count(m.downloads))).color(MUTED));
                        ui.label(RichText::new(format!("♥ {}", fmt_count(m.likes))).color(MUTED));
                        if !m.version.is_empty() {
                            ui.label(RichText::new(format!("v{}", m.version.trim_start_matches('v'))).color(MUTED));
                        }
                        if !m.updated.is_empty() {
                            ui.label(RichText::new(format!("Updated {}", m.updated)).color(MUTED));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                        if !m.category.is_empty() && m.source == Source::ModWorkshop {
                            theme::chip(ui, &m.category, ACCENT, false, false);
                        }
                        for t in &m.tags {
                            theme::chip(ui, &t.name, theme::rgb(t.color), false, false);
                        }
                    });
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        let label = if installed { "↻  Reinstall" } else { "⬇  Install" };
                        if primary_button(ui, label).clicked() {
                            actions.push(Action::Install(InstallRequest {
                                source: m.source,
                                mod_info: Some(m.clone()),
                                file: None,
                                local_path: None,
                                nxm: None,
                                display_name: m.name.clone(),
                            }));
                        }
                        if ghost_button(ui, "🌐  Open page").clicked() {
                            let _ = open::that(&m.page_url);
                        }
                    });
                    if d.managers_disabled {
                        ui.add_space(4.0);
                        ui.colored_label(WARN, "The author asks that this mod be downloaded from the website. Use 'Open page', then drag the file in.");
                    }

                    ui.add_space(12.0);
                    theme::section_title(ui, "Requirements");
                    match &d.deps {
                        None => {
                            ui.spinner();
                        }
                        Some(deps) if deps.is_empty() => {
                            ui.label(RichText::new("No other mods required.").size(12.5).color(MUTED));
                        }
                        Some(deps) => {
                            for dep in deps {
                                ui.horizontal(|ui| {
                                    let (icon, color, note) = if deps_ok.contains(&dep.name) {
                                        ("\u{2714}", GOOD, "installed")
                                    } else if dep.optional {
                                        ("\u{25CB}", MUTED, "optional")
                                    } else if dep.id.is_some() && dep.source != Source::Local {
                                        ("\u{2B07}", ACCENT, "will be installed automatically")
                                    } else {
                                        ("\u{1F310}", WARN, "install manually")
                                    };
                                    ui.label(RichText::new(icon).color(color));
                                    ui.label(RichText::new(&dep.name).strong());
                                    ui.label(RichText::new(note).size(12.0).color(color));
                                    if let Some(url) = &dep.url {
                                        if ui.small_button("\u{1F310}").on_hover_text(url).clicked() {
                                            let _ = open::that(url);
                                        }
                                    }
                                });
                            }
                        }
                    }

                    ui.add_space(12.0);
                    theme::section_title(ui, "Files");
                    match &d.files {
                        None => {
                            ui.spinner();
                        }
                        Some(Err(e)) => {
                            ui.colored_label(BAD, e);
                        }
                        Some(Ok(files)) if files.is_empty() => {
                            ui.label(RichText::new("No files listed — use the website.").color(MUTED));
                        }
                        Some(Ok(files)) => {
                            for f in files {
                                egui::Frame::none().fill(CARD).rounding(Rounding::same(8.0_f32)).inner_margin(egui::Margin::same(8.0)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.vertical(|ui| {
                                            ui.add(egui::Label::new(RichText::new(&f.name).strong()).wrap_mode(egui::TextWrapMode::Truncate));
                                            let mut meta = vec![];
                                            if !f.label.is_empty() {
                                                meta.push(f.label.clone());
                                            }
                                            if !f.version.is_empty() {
                                                meta.push(format!("v{}", f.version.trim_start_matches('v')));
                                            }
                                            let sz = fmt_size(f.size);
                                            if !sz.is_empty() {
                                                meta.push(sz);
                                            }
                                            if matches!(f.kind, FileKind::Link(_)) {
                                                meta.push("external link".into());
                                            }
                                            ui.label(RichText::new(meta.join(" · ")).size(12.0).color(MUTED));
                                        });
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if theme::colored_button(ui, "Install", ACCENT_DIM).clicked() {
                                                actions.push(Action::Install(InstallRequest {
                                                    source: m.source,
                                                    mod_info: Some(m.clone()),
                                                    file: Some(f.clone()),
                                                    local_path: None,
                                                    nxm: None,
                                                    display_name: format!("{} — {}", m.name, f.name),
                                                }));
                                            }
                                        });
                                    });
                                });
                                ui.add_space(4.0);
                            }
                        }
                    }

                    ui.add_space(12.0);
                    theme::section_title(ui, "Description");
                    let desc = if m.description.is_empty() { m.summary.clone() } else { m.description.clone() };
                    let desc = if m.source == Source::ModWorkshop { api::mws_markup_to_text(&desc) } else { desc };
                    let desc: String = desc.chars().take(8000).collect();
                    if !d.full_loaded && desc.is_empty() {
                        ui.spinner();
                    }
                    ui.label(RichText::new(desc).size(13.5).color(Color32::from_rgb(0xC8, 0xCE, 0xD8)));
                    ui.add_space(20.0);
                });
            });
        if close {
            self.detail = None;
        }
    }

    fn selection_bar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.selected.is_empty() || self.view != View::Browse {
            return;
        }
        egui::TopBottomPanel::bottom("selection")
            .frame(egui::Frame::none().fill(ACCENT_DIM).inner_margin(egui::Margin::symmetric(16.0, 10.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} selected", self.selected.len())).strong().color(Color32::WHITE));
                    let names: Vec<String> = self.selected.iter().take(4).map(|m| m.name.clone()).collect();
                    let more = if self.selected.len() > 4 { format!(" +{}", self.selected.len() - 4) } else { String::new() };
                    ui.add(egui::Label::new(RichText::new(format!("{}{}", names.join(", "), more)).color(Color32::from_rgb(0xB8, 0xCF, 0xEF))).wrap_mode(egui::TextWrapMode::Truncate));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary_button(ui, &format!("⬇  Install {} mods", self.selected.len())).clicked() {
                            actions.push(Action::InstallSelected);
                        }
                        if ghost_button(ui, "Clear").clicked() {
                            self.selected.clear();
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- installed

    fn installed_view(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Installed mods").size(22.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(g) = &self.game_dir {
                    if ghost_button(ui, "🗁  Game folder").clicked() {
                        let _ = open::that(g);
                    }
                }
                if ghost_button(ui, "⟳  Rescan").clicked() {
                    self.rescan();
                }
                if ghost_button(ui, "⬇  Import modpack").on_hover_text("Paste a friend's modpack code to install their mods").clicked() {
                    self.open_import(String::new());
                }
                if ghost_button(ui, "⬆  Share modpack").on_hover_text("Turn your mod list into a code others can paste").clicked() {
                    self.share = Some(ShareDialog::default());
                }
                let can_check = !self.checking_updates;
                if ui.add_enabled(can_check, egui::Button::new(if can_check { "🔃  Check for updates" } else { "Checking…" })).clicked() {
                    let list: Vec<(String, Source, u64, String)> = self
                        .db
                        .mods
                        .values()
                        .filter_map(|m| {
                            let installed_ver = if m.source_version.is_empty() { &m.version } else { &m.source_version };
                            m.source_id.map(|id| (m.key.clone(), m.source, id, installed_ver.clone()))
                        })
                        .filter(|(_, s, _, _)| *s != Source::Local)
                        .collect();
                    if list.is_empty() {
                        self.toast("Only mods installed through the manager can be checked for updates.", ToastKind::Info);
                    } else {
                        self.checking_updates = true;
                        self.worker.check_updates(list);
                    }
                }
            });
        });
        ui.add_space(6.0);

        if self.game_dir.is_none() {
            ui.colored_label(WARN, "Choose your PAYDAY 2 folder in Settings first.");
            return;
        }

        // filters
        let all_tags: BTreeSet<String> = self
            .db
            .mods
            .values()
            .flat_map(|m| m.user_tags.iter().chain(m.site_tags.iter()).cloned())
            .collect();
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.inst_query).hint_text("🔍  Filter installed…").desired_width(260.0).margin(egui::vec2(10.0, 6.0)));
            for (k, label) in [(None, "All"), (Some(ModKind::Blt), "BLT mods"), (Some(ModKind::Override), "Overrides"), (Some(ModKind::Map), "Maps"), (Some(ModKind::Root), "Game root")] {
                if ui.selectable_label(self.inst_kind == k, label).clicked() {
                    self.inst_kind = k;
                }
            }
        });
        if !all_tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                ui.label(RichText::new("Tags").size(12.0).color(MUTED));
                for t in &all_tags {
                    let on = self.inst_tag.as_deref() == Some(t);
                    if theme::chip(ui, t, ACCENT, on, true).clicked() {
                        self.inst_tag = if on { None } else { Some(t.clone()) };
                    }
                }
            });
        }
        ui.add_space(6.0);

        let hide = self.settings.hide_nsfw;
        let q = self.inst_query.to_lowercase();
        let mut list: Vec<InstalledMod> = self
            .db
            .mods
            .values()
            .filter(|m| self.inst_kind.map(|k| k == m.kind).unwrap_or(true))
            .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q) || m.folder.to_lowercase().contains(&q) || m.author.to_lowercase().contains(&q))
            .filter(|m| {
                self.inst_tag
                    .as_ref()
                    .map(|t| m.user_tags.contains(t) || m.site_tags.contains(t))
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        let before = list.len();
        list.retain(|m| !(hide && m.nsfw));
        let hidden = before - list.len();
        list.sort_by(|a, b| {
            b.update_available.is_some().cmp(&a.update_available.is_some()).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let enabled = list.iter().filter(|m| m.enabled).count();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} mods · {} enabled", list.len(), enabled)).color(MUTED).size(12.5));
            if hidden > 0 {
                ui.label(RichText::new(format!("· {hidden} hidden by NSFW filter")).color(NSFW).size(12.5));
            }
        });

        egui::ScrollArea::vertical().id_salt("installed").auto_shrink([false, false]).show(ui, |ui| {
            if list.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No mods here yet").size(18.0).color(MUTED));
                    ui.label(RichText::new("Browse ModWorkshop or Nexus, or drop an archive onto this window.").color(MUTED));
                });
            }
            for m in &list {
                self.installed_row(ui, m, actions);
                ui.add_space(6.0);
            }
            ui.add_space(20.0);
        });
    }

    fn installed_row(&mut self, ui: &mut egui::Ui, m: &InstalledMod, actions: &mut Vec<Action>) {
        let game = self.game_dir.clone().unwrap_or_default();
        egui::Frame::none()
            .fill(if m.enabled { CARD } else { Color32::from_rgb(0x18, 0x1B, 0x21) })
            .rounding(Rounding::same(10.0_f32))
            .stroke(Stroke::new(1.0_f32, if m.update_available.is_some() { WARN.gamma_multiply(0.6) } else { BORDER }))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if m.kind != ModKind::Root {
                        let mut on = m.enabled;
                        if theme::toggle(ui, &mut on).on_hover_text(if on { "Enabled — click to disable" } else { "Disabled — click to enable" }).changed() {
                            actions.push(Action::SetEnabled(m.key.clone(), on));
                        }
                    } else {
                        ui.add_space(36.0);
                    }
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let name_color = if m.enabled { TEXT } else { MUTED };
                            ui.label(RichText::new(&m.name).size(15.0).strong().color(name_color));
                            if !m.version.is_empty() {
                                ui.label(RichText::new(format!("v{}", m.version.trim_start_matches('v'))).size(12.0).color(MUTED));
                            }
                            let kind_color = match m.kind {
                                ModKind::Blt => Color32::from_rgb(0xEA, 0x23, 0x4B),
                                ModKind::Override => Color32::from_rgb(0xA8, 0x71, 0x33),
                                ModKind::Map => Color32::from_rgb(0x3F, 0xB9, 0x7C),
                                ModKind::Root => Color32::from_rgb(0x8B, 0x5C, 0xF6),
                            };
                            theme::chip(ui, m.kind.label(), kind_color, false, false);
                            let (sc, st) = match m.source {
                                Source::ModWorkshop => (MWS, "ModWorkshop"),
                                Source::Nexus => (NEXUS, "Nexus"),
                                Source::Local => (MUTED, "Local"),
                            };
                            theme::chip(ui, st, sc, false, false);
                            if m.nsfw {
                                theme::chip(ui, "NSFW", NSFW, true, false);
                            }
                            if let Some(v) = &m.update_available {
                                theme::chip(ui, &format!("Update > {}", v), WARN, true, false);
                            }
                        });
                        let mut sub = format!("{}/{}", m.kind.dir_name(), m.folder);
                        if !m.enabled {
                            sub.push_str("  (disabled)");
                        }
                        if !m.author.is_empty() {
                            sub = format!("by {} · {}", m.author, sub);
                        }
                        ui.label(RichText::new(sub).size(12.0).color(MUTED));
                        // tags row
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                            for t in &m.user_tags {
                                if theme::chip(ui, &format!("{t}  ✖"), ACCENT, true, true).on_hover_text("Remove tag").clicked() {
                                    actions.push(Action::RemoveTag(m.key.clone(), t.clone()));
                                }
                            }
                            for t in m.site_tags.iter().take(6) {
                                theme::chip(ui, t, Color32::from_rgb(0x5A, 0x63, 0x73), false, false);
                            }
                            if self.adding_tag_for.as_deref() == Some(&m.key) {
                                let input = self.tag_inputs.entry(m.key.clone()).or_default();
                                let r = ui.add(egui::TextEdit::singleline(input).hint_text("tag name").desired_width(110.0));
                                r.request_focus();
                                if r.lost_focus() {
                                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                        actions.push(Action::AddTag(m.key.clone(), input.clone()));
                                    }
                                    input.clear();
                                    self.adding_tag_for = None;
                                }
                            } else if theme::chip(ui, "+ tag", MUTED, false, true).clicked() {
                                self.adding_tag_for = Some(m.key.clone());
                            }
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(RichText::new("🗑").color(BAD))).on_hover_text("Uninstall").clicked() {
                            self.confirm_uninstall = Some(m.key.clone());
                        }
                        if m.kind != ModKind::Root && ui.button("🗁").on_hover_text("Open folder").clicked() {
                            let _ = open::that(game::mod_path(&game, m));
                        }
                        if let Some(url) = &m.page_url {
                            if ui.button("🌐").on_hover_text("Open mod page").clicked() {
                                let _ = open::that(url);
                            }
                        }
                        if m.update_available.is_some() {
                            if let (Some(id), true) = (m.source_id, m.source != Source::Local) {
                                if theme::colored_button(ui, "Update", WARN).clicked() {
                                    let info = RemoteMod {
                                        source: m.source,
                                        id,
                                        name: m.name.clone(),
                                        author: m.author.clone(),
                                        summary: String::new(),
                                        description: String::new(),
                                        thumb: None,
                                        image: None,
                                        tags: m.site_tags.iter().map(|t| TagChip { id: None, name: t.clone(), color: None }).collect(),
                                        category: String::new(),
                                        downloads: 0,
                                        likes: 0,
                                        version: m.update_available.clone().unwrap_or_default(),
                                        updated: String::new(),
                                        nsfw: m.nsfw,
                                        page_url: m.page_url.clone().unwrap_or_default(),
                                    };
                                    actions.push(Action::Install(InstallRequest {
                                        source: m.source,
                                        mod_info: Some(info),
                                        file: None,
                                        local_path: None,
                                        nxm: None,
                                        display_name: m.name.clone(),
                                    }));
                                }
                            }
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- downloads

    fn downloads_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Downloads").size(22.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ghost_button(ui, "Clear finished").clicked() {
                    self.tasks.retain(|t| t.state == TaskState::Running);
                }
            });
        });
        ui.add_space(8.0);
        if self.tasks.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nothing downloading").size(18.0).color(MUTED));
            });
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for t in self.tasks.iter().rev() {
                egui::Frame::none().fill(CARD).rounding(Rounding::same(10.0_f32)).stroke(Stroke::new(1.0_f32, BORDER)).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (icon, c) = match &t.state {
                            TaskState::Running => ("⬇", ACCENT),
                            TaskState::Done(_) | TaskState::Saved(..) => ("✔", GOOD),
                            TaskState::Browser(_) => ("🌐", WARN),
                            TaskState::Failed(_) => ("✖", BAD),
                        };
                        ui.label(RichText::new(icon).size(18.0).color(c));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&t.name).strong());
                            match &t.state {
                                TaskState::Running => {
                                    ui.add(egui::ProgressBar::new(t.progress).text(&t.status).desired_width(ui.available_width()).animate(true));
                                }
                                TaskState::Done(where_) => {
                                    ui.label(RichText::new(format!("Installed: {where_}")).size(12.0).color(MUTED));
                                }
                                TaskState::Saved(path, bytes) => {
                                    ui.label(RichText::new(format!("Saved {} ({})", path.display(), theme::fmt_size(*bytes))).size(12.0).color(MUTED));
                                    if ui.link("Show in folder").clicked() {
                                        if let Some(dir) = path.parent() {
                                            let _ = open::that(dir);
                                        }
                                    }
                                }
                                TaskState::Browser(why) => {
                                    ui.label(RichText::new(why).size(12.0).color(WARN));
                                    if let Some(url) = &t.link {
                                        if ui.link(format!("Open {url}")).clicked() {
                                            let _ = open::that(url);
                                        }
                                    }
                                }
                                TaskState::Failed(e) => {
                                    ui.label(RichText::new(e).size(12.0).color(BAD));
                                }
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        });
    }

    // ---------------------------------------------------------------- settings

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Settings").size(22.0).strong());
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(760.0);
            settings_card(ui, "PAYDAY 2 folder", |ui| {
                match &self.game_dir {
                    Some(p) => {
                        ui.horizontal(|ui| {
                            status_dot(ui, GOOD);
                            ui.label(RichText::new(p.display().to_string()).monospace());
                        });
                        ui.label(RichText::new(format!("Detected: {}", self.store.label())).color(MUTED).size(12.0));
                    }
                    None => {
                        ui.colored_label(WARN, "PAYDAY 2 wasn't found automatically. Pick the folder that contains PAYDAY2.exe.");
                    }
                }
                ui.horizontal(|ui| {
                    if ghost_button(ui, "📂  Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Select your PAYDAY 2 folder").pick_folder() {
                            self.set_game_dir(p);
                        }
                    }
                    if ghost_button(ui, "🔎  Auto-detect").clicked() {
                        match game::detect() {
                            Some((p, _)) => self.set_game_dir(p),
                            None => self.toast("Couldn't find PAYDAY 2 in Epic or Steam", ToastKind::Bad),
                        }
                    }
                });
            });

            settings_card(ui, "Mod loaders", |ui| {
                ui.horizontal(|ui| {
                    status_dot(ui, if self.loaders.superblt { GOOD } else { WARN });
                    ui.label(if self.loaders.superblt { "SuperBLT is installed" } else { "SuperBLT is not installed — most Lua mods need it" });
                    if !self.loaders.superblt && ui.link("Get SuperBLT").clicked() {
                        let _ = open::that("https://superblt.znix.xyz/");
                    }
                });
                ui.horizontal(|ui| {
                    status_dot(ui, if self.loaders.beardlib { GOOD } else { MUTED });
                    ui.label(if self.loaders.beardlib { "BeardLib is installed" } else { "BeardLib is not installed — needed for custom weapons, heists and many overrides" });
                    if !self.loaders.beardlib && primary_button(ui, "Install BeardLib").clicked() {
                        let mut info = RemoteMod::stub(
                            Source::ModWorkshop,
                            BEARDLIB_MWS_ID,
                            "BeardLib",
                            format!("https://modworkshop.net/mod/{BEARDLIB_MWS_ID}"),
                        );
                        info.author = "Simon".into();
                        self.quick_install(&info);
                    }
                });
            });

            settings_card(ui, "Content filter", |ui| {
                let mut show = !self.settings.hide_nsfw;
                ui.horizontal(|ui| {
                    if theme::toggle(ui, &mut show).changed() {
                        if show {
                            self.confirm_show_nsfw = true;
                        } else {
                            self.set_hide_nsfw(true);
                        }
                    }
                    ui.label(if self.settings.hide_nsfw { "NSFW mods are hidden (default)" } else { "NSFW mods are shown" });
                });
                ui.label(RichText::new("Nexus Mods marks adult content itself. ModWorkshop has no NSFW flag, so the manager also hides mods whose name, description or tags contain adult keywords.").size(12.0).color(MUTED));
            });

            settings_card(ui, "Nexus Mods", |ui| {
                ui.label(RichText::new("Browsing Nexus works without an account. To download, paste your personal API key (nexusmods.com > Site preferences > API Keys).").size(12.5).color(MUTED));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.api_key_input).password(!self.show_key).hint_text("Personal API key").desired_width(380.0));
                    if ui.small_button(if self.show_key { "Hide" } else { "Show" }).clicked() {
                        self.show_key = !self.show_key;
                    }
                    if primary_button(ui, "Save").clicked() {
                        self.settings.nexus_api_key = self.api_key_input.trim().to_string();
                        self.settings.save();
                        self.nexus_user = None;
                        if !self.settings.nexus_api_key.is_empty() {
                            self.worker.validate_nexus(self.settings.nexus_api_key.clone());
                        }
                    }
                });
                if ui.link("Get your API key").clicked() {
                    let _ = open::that("https://next.nexusmods.com/settings/api-keys");
                }
                match &self.nexus_user {
                    Some(Ok((name, premium))) => {
                        ui.horizontal(|ui| {
                            status_dot(ui, GOOD);
                            ui.label(format!("Signed in as {name} · {}", if *premium { "Premium — one-click installs" } else { "Free account" }));
                        });
                        if !premium {
                            ui.label(RichText::new("Free accounts: clicking Install opens the Nexus file page; press 'Mod Manager Download' / 'Slow download' there and the manager finishes the install (requires the handler below).").size(12.0).color(MUTED));
                        }
                    }
                    Some(Err(e)) => {
                        ui.colored_label(BAD, e);
                    }
                    None if !self.settings.nexus_api_key.is_empty() => {
                        ui.spinner();
                    }
                    None => {}
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    status_dot(ui, if self.nxm_registered { GOOD } else { MUTED });
                    ui.label(if self.nxm_registered { "This app handles Nexus 'Mod Manager Download' links" } else { "Nexus 'Mod Manager Download' links are not handled by this app" });
                    if !self.nxm_registered && ghost_button(ui, "Register").clicked() {
                        match game::register_nxm() {
                            Ok(()) => {
                                self.nxm_registered = true;
                                self.toast("nxm:// links will now open in PD2 Mod Manager", ToastKind::Good);
                            }
                            Err(e) => self.toast(format!("Couldn't register: {e}"), ToastKind::Bad),
                        }
                    }
                });
                ui.label(RichText::new("This replaces Vortex/other managers as the handler for Nexus download buttons (for all games).").size(11.5).color(MUTED));
            });

            settings_card(ui, "Appearance", |ui| {
                let mut scale = self.settings.ui_scale.unwrap_or(1.0);
                ui.horizontal(|ui| {
                    ui.label("Interface size");
                    if ui.add(egui::Slider::new(&mut scale, 0.8..=1.6).step_by(0.05)).changed() {
                        self.settings.ui_scale = Some(scale);
                        ui.ctx().set_zoom_factor(scale);
                        self.settings.save();
                    }
                });
            });

            settings_card(ui, "About", |ui| {
                ui.label(format!("PD2 Mod Manager v{}", env!("CARGO_PKG_VERSION")));
                ui.horizontal(|ui| {
                    if ui.link("Open data folder").clicked() {
                        let _ = open::that(crate::config::data_dir());
                    }
                    ui.label(RichText::new("· Disabled mods are moved to PAYDAY 2/PD2MM_disabled").size(12.0).color(MUTED));
                });
            });
        });
    }

    // ---------------------------------------------------------------- overlays

    fn overlays(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        // NSFW confirmation
        if self.confirm_show_nsfw {
            let mut open = true;
            egui::Window::new("Show NSFW mods?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Adult-content mods will appear in search results and your library.");
                    ui.label(RichText::new("You can hide them again at any time.").color(MUTED));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::colored_button(ui, "Show NSFW", NSFW).clicked() {
                            self.set_hide_nsfw(false);
                            self.confirm_show_nsfw = false;
                        }
                        if ghost_button(ui, "Keep hidden").clicked() {
                            self.confirm_show_nsfw = false;
                        }
                    });
                });
            if !open {
                self.confirm_show_nsfw = false;
            }
        }
        // uninstall confirmation
        if let Some(key) = self.confirm_uninstall.clone() {
            let name = self.db.mods.get(&key).map(|m| m.name.clone()).unwrap_or_default();
            let mut open = true;
            egui::Window::new("Uninstall mod?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!("Delete '{name}' from your PAYDAY 2 folder?"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::colored_button(ui, "Uninstall", BAD).clicked() {
                            actions.push(Action::Uninstall(key.clone()));
                            self.confirm_uninstall = None;
                        }
                        if ghost_button(ui, "Cancel").clicked() {
                            self.confirm_uninstall = None;
                        }
                    });
                });
            if !open {
                self.confirm_uninstall = None;
            }
        }

        self.share_window(ctx);
        self.import_window(ctx);

        // drag & drop
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(screen, Rounding::ZERO, Color32::from_black_alpha(190));
            painter.rect_stroke(screen.shrink(24.0), Rounding::same(16.0_f32), Stroke::new(2.0_f32, ACCENT));
            painter.text(screen.center(), Align2::CENTER_CENTER, "Drop to install mods or a modpack", egui::FontId::proportional(28.0), Color32::WHITE);
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for p in dropped {
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["zip", "7z", "rar"].contains(&ext.as_str()) {
                self.install_local(p);
            } else if ext == modpack::FILE_EXT {
                match std::fs::read_to_string(&p) {
                    Ok(text) => self.open_import(text),
                    Err(e) => self.toast(format!("Couldn't read {}: {e}", p.display()), ToastKind::Bad),
                }
            } else {
                self.toast(format!("{} isn't a .zip, .7z, .rar or .{} file", p.display(), modpack::FILE_EXT), ToastKind::Warn);
            }
        }

        // toasts
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    ui.set_max_width(380.0);
                    for t in self.toasts.iter().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
                        let c = match t.kind {
                            ToastKind::Info => ACCENT,
                            ToastKind::Good => GOOD,
                            ToastKind::Warn => WARN,
                            ToastKind::Bad => BAD,
                        };
                        egui::Frame::none()
                            .fill(PANEL)
                            .rounding(Rounding::same(8.0_f32))
                            .stroke(Stroke::new(1.0_f32, c))
                            .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    status_dot(ui, c);
                                    ui.add(egui::Label::new(&t.text).wrap());
                                });
                            });
                        ui.add_space(6.0);
                    }
                });
        }
    }

    fn share_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.share.take() else { return };
        let mut open = true;
        let mods: Vec<&InstalledMod> = self.db.mods.values().filter(|m| m.enabled || d.include_disabled).collect();
        let export_count = mods.len();
        let mut export_zip = false;
        let (pack, skipped) = modpack::from_installed(&d.name, mods);
        let code = if pack.mods.is_empty() { String::new() } else { modpack::encode(&pack) };
        let mut close = false;
        egui::Window::new("Share modpack")
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Anyone with PD2 Mod Manager can paste this code under Installed → Import modpack to get the same mods.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Pack name");
                    ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("e.g. Stealth essentials").desired_width(260.0).char_limit(modpack::MAX_NAME));
                });
                ui.checkbox(&mut d.include_disabled, "Include disabled mods");
                ui.add_space(6.0);
                if pack.mods.is_empty() {
                    ui.colored_label(WARN, "None of your mods can be shared yet. Codes only include mods installed from ModWorkshop or Nexus through the manager.");
                } else {
                    ui.label(RichText::new(format!("{} mod(s) in this code", pack.mods.len())).strong());
                    egui::ScrollArea::vertical().id_salt("share_code").max_height(110.0).show(ui, |ui| {
                        let mut shown = code.as_str();
                        ui.add(egui::TextEdit::multiline(&mut shown).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).desired_rows(3));
                    });
                }
                if !skipped.is_empty() {
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(RichText::new(format!("{} mod(s) left out (installed from a file, not a mod site)", skipped.len())).color(MUTED))
                        .id_salt("share_skipped")
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                                for s in &skipped {
                                    ui.label(RichText::new(format!("• {s}")).color(MUTED));
                                }
                            });
                            ui.label(RichText::new("Use Export as zip below to include them.").color(MUTED).size(12.0));
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let ok = !code.is_empty();
                    if ui.add_enabled(ok, egui::Button::new(RichText::new("📋  Copy code").strong()).fill(ACCENT)).clicked() {
                        ctx.copy_text(code.clone());
                        self.toast("Modpack code copied. Paste it anywhere to share it.", ToastKind::Good);
                    }
                    if ui.add_enabled(ok, egui::Button::new("💾  Save as file…")).clicked() {
                        let base = if d.name.trim().is_empty() { "modpack" } else { d.name.as_str() };
                        let file_name = format!("{}.{}", crate::installer::sanitize(base), modpack::FILE_EXT);
                        if let Some(path) = rfd::FileDialog::new().add_filter("PD2 modpack", &[modpack::FILE_EXT]).set_file_name(file_name).save_file() {
                            match std::fs::write(&path, format!("{code}\n")) {
                                Ok(()) => self.toast(format!("Saved {}", path.display()), ToastKind::Good),
                                Err(e) => self.toast(format!("Couldn't save: {e}"), ToastKind::Bad),
                            }
                        }
                    }
                    if ghost_button(ui, "Close").clicked() {
                        close = true;
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(RichText::new("Or send the files").strong());
                ui.label(
                    RichText::new(format!(
                        "Packs all {} mod(s) with their files into one .zip, including mods installed from a file. Your friend drags the zip onto PD2 Mod Manager to install everything. Zips can be large.",
                        export_count
                    ))
                    .color(MUTED)
                    .size(12.5),
                );
                ui.add_space(4.0);
                if ui.add_enabled(export_count > 0, egui::Button::new("📦  Export as zip…")).clicked() {
                    export_zip = true;
                }
            });
        if export_zip {
            self.export_pack_zip(&d.name, d.include_disabled);
        }
        if open && !close {
            self.share = Some(d);
        }
    }

    fn zip_import_window(&mut self, ctx: &egui::Context, mut d: ImportDialog) {
        let Some((path, manifest)) = d.zip.clone() else { return };
        let hide_nsfw = self.settings.hide_nsfw;
        // Mods the user already has: same folder, or the same mod from the same site.
        let existing: HashSet<String> = manifest
            .mods
            .iter()
            .filter(|m| {
                self.db.mods.contains_key(&m.key)
                    || (m.source != Source::Local
                        && m.source_id.is_some()
                        && self.db.mods.values().any(|o| o.source == m.source && o.source_id == m.source_id))
            })
            .map(|m| m.key.clone())
            .collect();
        let skip_for = |m: &InstalledMod, replace: bool| (hide_nsfw && m.nsfw) || (existing.contains(&m.key) && !replace);
        let zip_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut open = true;
        let mut close = false;
        let mut install = false;
        egui::Window::new("Install modpack")
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let title = if manifest.name.is_empty() {
                    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Modpack".into())
                } else {
                    manifest.name.clone()
                };
                ui.label(RichText::new(title).size(17.0).strong());
                ui.label(RichText::new(format!("{} mod(s) · {} zip", manifest.mods.len(), theme::fmt_size(zip_size))).color(MUTED).size(12.5));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt("zip_import_list").max_height(300.0).show(ui, |ui| {
                    for m in &manifest.mods {
                        let (label, color) = if hide_nsfw && m.nsfw {
                            ("Skipped: NSFW", NSFW)
                        } else if existing.contains(&m.key) {
                            if d.replace_existing { ("Will replace", WARN) } else { ("Already have", GOOD) }
                        } else {
                            ("Will install", ACCENT)
                        };
                        ui.horizontal(|ui| {
                            theme::chip(ui, label, color, false, false);
                            ui.label(&m.name);
                            ui.label(RichText::new(m.kind.label()).color(MUTED).size(12.0));
                        });
                    }
                });
                ui.add_space(6.0);
                if !existing.is_empty() {
                    ui.checkbox(&mut d.replace_existing, format!("Replace the {} mod(s) I already have with the pack's copies", existing.len()));
                }
                if hide_nsfw && manifest.mods.iter().any(|m| m.nsfw) {
                    ui.label(RichText::new("NSFW mods are skipped while the content filter is on (Settings).").color(MUTED).size(12.0));
                }
                ui.label(RichText::new("Only install packs from people you trust: mods can run code in your game.").color(MUTED).size(12.0));
                ui.add_space(8.0);
                let todo = manifest.mods.iter().filter(|m| !skip_for(m, d.replace_existing)).count();
                ui.horizontal(|ui| {
                    let text = if todo == 0 { "Nothing to install".to_string() } else { format!("⬇  Install {todo} mod(s)") };
                    if ui.add_enabled(todo > 0, egui::Button::new(RichText::new(text).strong()).fill(ACCENT)).clicked() {
                        install = true;
                    }
                    if ghost_button(ui, "Close").clicked() {
                        close = true;
                    }
                });
            });
        if install {
            let Some(game) = self.game_dir.clone() else {
                self.toast("Set your PAYDAY 2 folder first (Settings).", ToastKind::Warn);
                self.view = View::Settings;
                return;
            };
            let skip: HashSet<String> = manifest.mods.iter().filter(|m| skip_for(m, d.replace_existing)).map(|m| m.key.clone()).collect();
            let name = if manifest.name.is_empty() { "modpack".to_string() } else { manifest.name.clone() };
            let id = self.new_task(format!("Modpack: {name}"), "Queued");
            self.worker.install_pack_zip(id, path, game, skip);
            if self.game_running {
                self.toast("PAYDAY 2 is running. Mods take effect next time you start it.", ToastKind::Warn);
            }
            self.toast(format!("Installing '{name}'. Progress is under Downloads."), ToastKind::Info);
            self.view = View::Downloads;
            return;
        }
        if open && !close {
            self.import = Some(d);
        }
    }

    fn import_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.import.take() else { return };
        if d.zip.is_some() {
            return self.zip_import_window(ctx, d);
        }
        if d.text != d.parsed_for {
            d.parsed_for = d.text.clone();
            d.pack = if d.text.trim().is_empty() { None } else { Some(modpack::decode(&d.text).map_err(|e| e.to_string())) };
            d.gen += 1;
            d.pending = 0;
            if let Some(Ok(pack)) = &d.pack {
                d.pending = pack.mods.len();
                self.worker.lookup_pack_names(d.gen, pack.mods.iter().map(|m| (m.source, m.id)).collect());
            }
        }
        // Precompute per-mod "installed" so the window closure doesn't borrow self.
        let have: HashSet<(Source, u64)> = match &d.pack {
            Some(Ok(pack)) => pack.mods.iter().filter(|m| self.pack_mod_installed(m)).map(|m| (m.source, m.id)).collect(),
            _ => HashSet::new(),
        };
        let queued = self.queued_ids();
        let hide_nsfw = self.settings.hide_nsfw;
        let mut open = true;
        let mut close = false;
        let mut install: Option<modpack::Pack> = None;
        let pending = d.pending;
        egui::Window::new("Import modpack")
            .collapsible(false)
            .resizable(false)
            .default_width(540.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Paste a modpack code below, or open a .pd2pack file or modpack .zip.");
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt("import_code").max_height(90.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut d.text)
                            .hint_text("PD2-…")
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(3),
                    );
                });
                ui.horizontal(|ui| {
                    if ghost_button(ui, "📂  Open file…").clicked() {
                        if let Some(path) = rfd::FileDialog::new().add_filter("PD2 modpack", &[modpack::FILE_EXT, "zip", "txt"]).pick_file() {
                            let is_zip = path.extension().map(|e| e.eq_ignore_ascii_case("zip")).unwrap_or(false);
                            if is_zip {
                                match modpack::read_zip_manifest(&path) {
                                    Some(manifest) => d.zip = Some((path, manifest)),
                                    None => self.toast("That zip isn't a modpack made by PD2 Mod Manager. Use Install from file for normal mod archives.", ToastKind::Warn),
                                }
                            } else {
                                match std::fs::read_to_string(&path) {
                                    Ok(t) => d.text = t,
                                    Err(e) => self.toast(format!("Couldn't read {}: {e}", path.display()), ToastKind::Bad),
                                }
                            }
                        }
                    }
                    if !d.text.is_empty() && ghost_button(ui, "Clear").clicked() {
                        d.text.clear();
                    }
                });
                ui.add_space(6.0);
                match &d.pack {
                    None => {}
                    Some(Err(e)) => {
                        ui.colored_label(BAD, e);
                    }
                    Some(Ok(pack)) => {
                        let status = |m: &modpack::PackMod| -> PackStatus {
                            let key = (m.source, m.id);
                            if have.contains(&key) {
                                PackStatus::Installed
                            } else if queued.contains(&key) {
                                PackStatus::Downloading
                            } else if hide_nsfw && m.nsfw {
                                PackStatus::HiddenNsfw
                            } else {
                                PackStatus::Missing
                            }
                        };
                        let count = |s: PackStatus| pack.mods.iter().filter(|m| status(m) == s).count();
                        let (installed, missing) = (count(PackStatus::Installed), count(PackStatus::Missing));
                        let title = if pack.name.is_empty() { "Unnamed modpack" } else { pack.name.as_str() };
                        ui.label(RichText::new(title).size(17.0).strong());
                        ui.label(
                            RichText::new(format!("{} mod(s) · {installed} already installed · {missing} to install", pack.mods.len()))
                                .color(MUTED)
                                .size(12.5),
                        );
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("import_list").max_height(260.0).show(ui, |ui| {
                            for m in &pack.mods {
                                let (label, color) = match status(m) {
                                    PackStatus::Installed => ("Installed", GOOD),
                                    PackStatus::Downloading => ("Downloading", ACCENT),
                                    PackStatus::HiddenNsfw => ("Skipped: NSFW", NSFW),
                                    PackStatus::Missing => ("Will install", WARN),
                                };
                                ui.horizontal(|ui| {
                                    theme::chip(ui, label, color, false, false);
                                    if m.name.is_empty() && pending > 0 {
                                        ui.label(RichText::new(format!("Mod #{} · looking up name…", m.id)).color(MUTED));
                                    } else {
                                        ui.label(m.display_name());
                                    }
                                    let site = if m.source == Source::Nexus { "Nexus" } else { "ModWorkshop" };
                                    ui.label(RichText::new(site).color(MUTED).size(12.0));
                                    if ui.small_button("🌐").on_hover_text("Open mod page").clicked() {
                                        let _ = open::that(m.page_url());
                                    }
                                });
                            }
                        });
                        if count(PackStatus::HiddenNsfw) > 0 {
                            ui.label(RichText::new("NSFW mods are skipped while the content filter is on (Settings).").color(MUTED).size(12.0));
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            // Wait for names: they're used for folder names and to spot mods installed by hand.
                            let text = if pending > 0 {
                                format!("Looking up mods… ({pending} left)")
                            } else if missing == 0 {
                                "Nothing to install".to_string()
                            } else {
                                format!("⬇  Install {missing} mod(s)")
                            };
                            if pending > 0 {
                                ui.ctx().request_repaint_after(Duration::from_millis(250));
                            }
                            if ui.add_enabled(missing > 0 && pending == 0, egui::Button::new(RichText::new(text).strong()).fill(ACCENT)).clicked() {
                                install = Some(pack.clone());
                            }
                            if ghost_button(ui, "Close").clicked() {
                                close = true;
                            }
                        });
                    }
                }
            });
        if let Some(pack) = install {
            self.install_pack(&pack);
            return; // close the window once the downloads are queued
        }
        if open && !close {
            self.import = Some(d);
        }
    }

    fn setup_screen(&mut self, ui: &mut egui::Ui) {
        ui.add_space(80.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Welcome to PD2 Mod Manager").size(26.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new("We couldn't find PAYDAY 2 automatically. Point us at the folder that contains PAYDAY2.exe").color(MUTED));
            ui.label(RichText::new("(Epic default: C:\\Program Files\\Epic Games\\PAYDAY2)").color(MUTED).size(12.0));
            ui.add_space(16.0);
            if primary_button(ui, "📂  Choose PAYDAY 2 folder").clicked() {
                if let Some(p) = rfd::FileDialog::new().set_title("Select your PAYDAY 2 folder").pick_folder() {
                    self.set_game_dir(p);
                }
            }
            ui.add_space(6.0);
            if ui.link("Browse mods anyway").clicked() {
                self.view = View::Browse;
                self.setup_dismissed = true;
            }
        });
    }
}

impl App {
    fn dev_screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, start, requested)) = &mut self.shot else { return };
        if !*requested && start.elapsed()
                > Duration::from_secs(std::env::var("PD2MM_SHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(8)) {
            *requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(path.as_str());
            }
            std::process::exit(0);
        }
    }
}

fn nav_item(ui: &mut egui::Ui, text: &str, active: bool, color: Color32, badge: Option<String>) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let bg = if active { ACCENT_DIM.gamma_multiply(0.8) } else if resp.hovered() { CARD } else { Color32::TRANSPARENT };
        ui.painter().rect_filled(rect, Rounding::same(7.0_f32), bg);
        if active {
            let bar = egui::Rect::from_min_size(rect.min + egui::vec2(0.0, 7.0), egui::vec2(3.0, rect.height() - 14.0));
            ui.painter().rect_filled(bar, Rounding::same(2.0_f32), color);
        }
        ui.painter().text(
            rect.left_center() + egui::vec2(12.0, 0.0),
            Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(14.5),
            if active { Color32::WHITE } else { TEXT },
        );
        if let Some(b) = badge {
            let galley = ui.painter().layout_no_wrap(b, egui::FontId::proportional(11.5), Color32::WHITE);
            let size = galley.size() + egui::vec2(12.0, 4.0);
            let br = egui::Rect::from_center_size(rect.right_center() - egui::vec2(size.x / 2.0 + 8.0, 0.0), size);
            ui.painter().rect_filled(br, Rounding::same(size.y / 2.0), Color32::from_rgb(0x30, 0x37, 0x44));
            ui.painter().galley(br.center() - galley.size() / 2.0, galley, Color32::WHITE);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn settings_card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(10.0_f32))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(16.0).strong());
            ui.add_space(4.0);
            add(ui);
        });
    ui.add_space(10.0);
}

fn mod_card(ui: &mut egui::Ui, m: &RemoteMod, w: f32, installed: bool, selected: bool, actions: &mut Vec<Action>) {
    let img_h = (w * 9.0 / 16.0).round();
    let card_h = img_h + 150.0;
    let (rect, bg_resp) = ui.allocate_exact_size(egui::vec2(w, card_h), Sense::hover());
    let hovered = bg_resp.hovered() || ui.rect_contains_pointer(rect);
    let stroke = if selected { Stroke::new(2.0_f32, ACCENT) } else { Stroke::new(1.0_f32, BORDER) };
    ui.painter().rect(rect, Rounding::same(10.0_f32), if hovered { CARD_HOVER } else { CARD }, stroke);

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    let ui = &mut child;
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));

    // thumbnail
    let img_rect = egui::Rect::from_min_size(rect.min, egui::vec2(w, img_h));
    let img_resp = ui.allocate_rect(img_rect, Sense::click());
    ui.painter().rect_filled(img_rect, Rounding { nw: 10.0, ne: 10.0, sw: 0.0, se: 0.0 }, Color32::from_rgb(0x12, 0x15, 0x1A));
    match &m.thumb {
        Some(url) => {
            egui::Image::new(url.as_str())
                .max_size(img_rect.size())
                .rounding(Rounding { nw: 10.0, ne: 10.0, sw: 0.0, se: 0.0 })
                .show_loading_spinner(true)
                .paint_at(ui, center_fit(img_rect, ui, url));
        }
        None => {
            let initial = m.name.chars().next().unwrap_or('?').to_uppercase().to_string();
            ui.painter().text(img_rect.center(), Align2::CENTER_CENTER, initial, egui::FontId::proportional(48.0), MUTED);
        }
    }
    if img_resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::OpenDetail(m.clone()));
    }
    // overlay badges
    let mut bx = img_rect.left() + 8.0;
    let mut badge = |ui: &mut egui::Ui, text: &str, c: Color32| {
        let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
        let size = galley.size() + egui::vec2(10.0, 4.0);
        let r = egui::Rect::from_min_size(egui::pos2(bx, img_rect.top() + 8.0), size);
        ui.painter().rect_filled(r, Rounding::same(4.0_f32), c);
        ui.painter().galley(r.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
        bx += size.x + 4.0;
    };
    if installed {
        badge(ui, "✔ INSTALLED", GOOD);
    }
    if m.nsfw {
        badge(ui, "NSFW", NSFW);
    }
    // select checkbox (top-right)
    let cb_rect = egui::Rect::from_min_size(egui::pos2(img_rect.right() - 30.0, img_rect.top() + 6.0), egui::vec2(24.0, 24.0));
    let cb = ui.allocate_rect(cb_rect, Sense::click());
    ui.painter().rect(
        cb_rect,
        Rounding::same(6.0_f32),
        if selected { ACCENT } else { Color32::from_black_alpha(160) },
        Stroke::new(1.5_f32, if selected { ACCENT } else { Color32::from_white_alpha(140) }),
    );
    if selected {
        ui.painter().text(cb_rect.center(), Align2::CENTER_CENTER, "✔", egui::FontId::proportional(14.0), Color32::WHITE);
    }
    if cb.on_hover_text("Select for batch install").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::ToggleSelect(m.clone()));
    }

    // body
    let body = egui::Rect::from_min_max(egui::pos2(rect.left() + 10.0, img_rect.bottom() + 8.0), rect.max - egui::vec2(10.0, 8.0));
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
    b.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
    let title = b.add(egui::Label::new(RichText::new(&m.name).size(14.5).strong()).wrap_mode(egui::TextWrapMode::Truncate).sense(Sense::click()));
    if title.on_hover_text(&m.name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        actions.push(Action::OpenDetail(m.clone()));
    }
    b.add(
        egui::Label::new(RichText::new(format!("by {}  ·  ⬇ {}  ·  ♥ {}", m.author, fmt_count(m.downloads), fmt_count(m.likes))).size(11.5).color(MUTED))
            .wrap_mode(egui::TextWrapMode::Truncate),
    );
    let summary = if m.summary.is_empty() { api::mws_markup_to_text(&m.description).replace('\n', " ") } else { m.summary.clone() };
    let summary: String = summary.chars().take(140).collect();
    {
        let mut job = egui::text::LayoutJob::single_section(
            summary,
            egui::TextFormat { font_id: egui::FontId::proportional(12.0), color: Color32::from_rgb(0xAE, 0xB6, 0xC4), ..Default::default() },
        );
        job.wrap = egui::text::TextWrapping { max_width: body.width(), max_rows: 2, break_anywhere: false, overflow_character: Some('…') };
        let galley = b.fonts(|f| f.layout_job(job));
        let (r, _) = b.allocate_exact_size(egui::vec2(body.width(), 34.0), Sense::hover());
        b.painter().galley(r.min, galley, Color32::WHITE);
    }
    // tags (one line)
    b.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
        ui.set_clip_rect(body.intersect(ui.clip_rect()));
        if !m.category.is_empty() && m.source == Source::ModWorkshop {
            theme::chip(ui, &m.category, ACCENT, false, false);
        }
        for t in m.tags.iter().take(4) {
            theme::chip(ui, &t.name, theme::rgb(t.color), false, false);
        }
    });
    // actions at the bottom
    let btn_rect = egui::Rect::from_min_max(egui::pos2(body.left(), body.bottom() - 28.0), body.max);
    let mut bb = ui.new_child(egui::UiBuilder::new().max_rect(btn_rect).layout(Layout::left_to_right(Align::Center)));
    let label = if installed { "↻ Reinstall" } else { "⬇ Install" };
    let install = if installed {
        theme::colored_button(&mut bb, label, Color32::from_rgb(0x2E, 0x35, 0x42))
    } else {
        primary_button(&mut bb, label)
    };
    if install.clicked() {
        actions.push(Action::Install(InstallRequest {
            source: m.source,
            mod_info: Some(m.clone()),
            file: None,
            local_path: None,
            nxm: None,
            display_name: m.name.clone(),
        }));
    }
    if ghost_button(&mut bb, "Details").clicked() {
        actions.push(Action::OpenDetail(m.clone()));
    }
}

/// Fit a loaded image inside `area` keeping aspect ratio (cover-crop is not available, so letterbox).
fn center_fit(area: egui::Rect, ui: &egui::Ui, url: &str) -> egui::Rect {
    let size = ui
        .ctx()
        .try_load_texture(url, egui::TextureOptions::LINEAR, egui::SizeHint::default())
        .ok()
        .and_then(|t| t.size());
    match size {
        Some(s) if s.x > 0.0 && s.y > 0.0 => {
            let scale = (area.width() / s.x).min(area.height() / s.y);
            egui::Rect::from_center_size(area.center(), s * scale)
        }
        _ => area,
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_messages(ctx);
        self.dev_screenshot(ctx);
        let mut actions = vec![];

        self.top_bar(ctx);
        self.nav(ctx);
        self.selection_bar(ctx, &mut actions);
        if self.view == View::Browse {
            self.detail_panel(ctx, &mut actions);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::same(18.0)))
            .show(ctx, |ui| {
                if self.game_dir.is_none() && !self.setup_dismissed && self.view != View::Settings {
                    self.setup_screen(ui);
                    return;
                }
                match self.view {
                    View::Browse => self.browse_view(ui, &mut actions),
                    View::Installed => self.installed_view(ui, &mut actions),
                    View::Downloads => self.downloads_view(ui),
                    View::Settings => self.settings_view(ui),
                }
            });
        self.overlays(ctx, &mut actions);
        self.apply_actions(actions);
    }
}
