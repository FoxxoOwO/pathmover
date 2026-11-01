//! Application state, background task plumbing and the `eframe::App` impl.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::cleaner::{self, CacheTarget, CleanReport};
use crate::config::{Config, MovedEntry};
use crate::relocator::{self, Event as RelocEvent, Level};
use crate::scanner::{self, FolderEntry, ScanProgress, Scope};
use crate::util;
use crate::winapi;

pub const APP_NAME: &str = "PathMover";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Relocate,
    Cleaner,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Dashboard => "Přehled disku",
            Tab::Relocate => "AppData & Symlinky",
            Tab::Cleaner => "Čistič cache",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Good,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
    pub stamp: String,
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub phase: String,
    pub detail: String,
    pub done: u64,
    pub total: u64,
    pub indeterminate: bool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        util::percent(self.done, self.total)
    }

    pub fn percent_text(&self) -> String {
        if self.total == 0 {
            String::new()
        } else {
            format!("{:.0} %", self.fraction() * 100.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Scan,
    CacheScan,
    Clean,
    Move,
    Restore,
}

impl TaskKind {
    pub fn verb(self) -> &'static str {
        match self {
            TaskKind::Scan => "Skenování",
            TaskKind::CacheScan => "Hledání cache",
            TaskKind::Clean => "Čištění",
            TaskKind::Move => "Přesun",
            TaskKind::Restore => "Navracení",
        }
    }
}

pub enum Msg {
    ScanProgress(ScanProgress),
    ScanDone(Vec<FolderEntry>),
    CacheProgress(ScanProgress),
    CacheDone(Vec<CacheTarget>),
    CleanProgress { freed: u64, total: u64, label: String },
    CleanDone(CleanReport),
    MoveEvent(RelocEvent),
    MoveDone(Box<Result<relocator::Moved, String>>),
    RestoreDone { name: String, source: String, result: Result<(), String> },
}

pub struct Task {
    pub kind: TaskKind,
    pub rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
    pub progress: Progress,
    pub started: Instant,
    last_msg: Instant,
}

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub free: u64,
    pub is_system: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    SizeDesc,
    NameAsc,
    StatusFirst,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::SizeDesc => "Největší první",
            SortMode::NameAsc => "Podle názvu",
            SortMode::StatusFirst => "Přesunuté první",
        }
    }
}

/// A move waiting for user confirmation.
#[derive(Debug, Clone)]
pub struct ConfirmMove {
    pub entry: FolderEntry,
    pub target: PathBuf,
    pub keep_backup: bool,
}

/// A restore waiting for user confirmation.
#[derive(Debug, Clone)]
pub struct ConfirmRestore {
    pub entry: MovedEntry,
    pub delete_target: bool,
}

pub struct PathMoverApp {
    pub config: Config,
    pub tab: Tab,
    pub folders: Vec<FolderEntry>,
    pub caches: Vec<CacheTarget>,
    pub clean_report: Option<CleanReport>,
    pub task: Option<Task>,
    pub log: Vec<LogLine>,
    pub disks: Vec<DiskInfo>,
    disks_at: Instant,
    pub is_admin: bool,
    pub search: String,
    pub min_size_mb: f32,
    pub scope_local: bool,
    pub scope_roaming: bool,
    pub only_moved: bool,
    pub sort: SortMode,
    pub target_root_edit: String,
    pub clean_logs: bool,
    pub confirm_move: Option<ConfirmMove>,
    pub confirm_restore: Option<ConfirmRestore>,
    pub show_log: bool,
    pub status: String,
    pub toast: Option<(String, LogKind, Instant)>,
    pub scanned_once: bool,
}

impl PathMoverApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::new_with_ctx(&cc.egui_ctx)
    }

    /// Constructor that only needs an egui context. Used by `new` and by the
    /// headless layout tests, which build the UI without a real window.
    pub fn new_with_ctx(ctx: &egui::Context) -> Self {
        crate::ui::theme::install(ctx);

        let (config, warning) = Config::load();
        let target_root_edit = config.target_root.clone();
        let clean_logs = config.clean_logs;

        let mut app = Self {
            config,
            tab: Tab::Dashboard,
            folders: Vec::new(),
            caches: Vec::new(),
            clean_report: None,
            task: None,
            log: Vec::new(),
            disks: Vec::new(),
            disks_at: Instant::now() - Duration::from_secs(60),
            is_admin: winapi::is_elevated(),
            search: String::new(),
            min_size_mb: 0.0,
            scope_local: true,
            scope_roaming: true,
            only_moved: false,
            sort: SortMode::SizeDesc,
            target_root_edit,
            clean_logs,
            confirm_move: None,
            confirm_restore: None,
            show_log: false,
            status: "Připraven".to_string(),
            toast: None,
            scanned_once: false,
        };

        if let Some(warning) = warning {
            app.log_push(LogKind::Warn, warning);
        }
        app.refresh_disks();
        app.log_push(
            LogKind::Info,
            format!(
                "PathMover {APP_VERSION} spuštěn{}",
                if app.is_admin {
                    " (zvýšená oprávnění)"
                } else {
                    " (bez admin práv)"
                }
            ),
        );
        app.log_push(
            LogKind::Info,
            format!(
                "Cílový adresář: {} | zaznamenaných přesunů: {}",
                app.config.target_root,
                app.config.entries.len()
            ),
        );
        app.verify_config_entries();
        app
    }

    // ---------------------------------------------------------------- log --

    pub fn log_push(&mut self, kind: LogKind, text: impl Into<String>) {
        let stamp = clock_now();
        self.log.push(LogLine {
            kind,
            text: text.into(),
            stamp,
        });
        if self.log.len() > 500 {
            let overflow = self.log.len() - 500;
            self.log.drain(0..overflow);
        }
    }

    fn toast(&mut self, kind: LogKind, text: impl Into<String>) {
        self.toast = Some((text.into(), kind, Instant::now() + Duration::from_secs(6)));
    }

    /// Check that every recorded move still has a working junction.
    fn verify_config_entries(&mut self) {
        let mut broken: Vec<String> = Vec::new();
        let mut healthy = 0usize;
        for entry in &self.config.entries {
            let source = entry.source_path();
            match winapi::junction_target(&source) {
                Some(target) if util::paths_eq(&target, &entry.target_path()) => healthy += 1,
                _ => broken.push(entry.name.clone()),
            }
        }
        if healthy > 0 {
            self.log_push(
                LogKind::Good,
                format!("{healthy} junction odkazů je aktivních"),
            );
        }
        if !broken.is_empty() {
            self.log_push(
                LogKind::Warn,
                format!(
                    "{} záznamů v config.toml nemá platný odkaz: {}",
                    broken.len(),
                    broken.join(", ")
                ),
            );
        }
    }

    // -------------------------------------------------------------- disks --

    pub fn refresh_disks(&mut self) {
        let mut disks: Vec<DiskInfo> = Vec::new();
        if let Ok(list) = sysinfo::Disks::new_with_refreshed_list() {
            for disk in &list {
                let mount = disk.mount_point().to_string_lossy().to_string();
                let name = disk.name().to_string_lossy().to_string();
                let label = if name.trim().is_empty() {
                    mount.trim_end_matches(['\\', '/']).to_string()
                } else {
                    format!("{} ({})", mount.trim_end_matches(['\\', '/']), name)
                };
                let is_system = mount.to_ascii_uppercase().starts_with("C:");
                disks.push(DiskInfo {
                    name: label,
                    mount,
                    total: disk.total_space(),
                    free: disk.available_space(),
                    is_system,
                });
            }
        }
        disks.sort_by(|a, b| b.is_system.cmp(&a.is_system).then_with(|| a.mount.cmp(&b.mount)));
        self.disks = disks;
        self.disks_at = Instant::now();
    }

    pub fn system_disk(&self) -> Option<&DiskInfo> {
        self.disks.iter().find(|d| d.is_system)
    }

    // -------------------------------------------------------------- tasks --

    pub fn busy(&self) -> bool {
        self.task.is_some()
    }

    pub fn cancel_task(&mut self) {
        if let Some(task) = &self.task {
            task.cancel.store(true, Ordering::Relaxed);
            let kind = task.kind;
            self.log_push(LogKind::Warn, format!("{}: požadavek na zrušení…", kind.verb()));
            self.status = "Ruším…".to_string();
        }
    }

    fn begin_task(&mut self, kind: TaskKind, phase: &str, rx: Receiver<Msg>, cancel: Arc<AtomicBool>) {
        self.task = Some(Task {
            kind,
            rx,
            cancel,
            progress: Progress {
                phase: phase.to_string(),
                ..Default::default()
            },
            started: Instant::now(),
            last_msg: Instant::now(),
        });
        self.status = phase.to_string();
    }

    pub fn start_scan(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let managed = self.config.entries.clone();
        let ctx_thread = ctx.clone();
        let cancel_thread = cancel.clone();

        std::thread::spawn(move || {
            let tx_progress = tx.clone();
            let ctx_progress = ctx_thread.clone();
            let cb = move |p: ScanProgress| {
                let _ = tx_progress.send(Msg::ScanProgress(p));
                ctx_progress.request_repaint();
            };
            let folders = scanner::scan(&managed, &cb, &cancel_thread);
            let _ = tx.send(Msg::ScanDone(folders));
            ctx_thread.request_repaint();
        });

        self.begin_task(TaskKind::Scan, "Skenuji AppData…", rx, cancel);
        self.log_push(LogKind::Info, "Spuštěno skenování AppData\\Local a Roaming");
    }

    pub fn start_cache_scan(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let include_logs = self.clean_logs;
        let ctx_thread = ctx.clone();
        let cancel_thread = cancel.clone();

        std::thread::spawn(move || {
            let tx_progress = tx.clone();
            let ctx_progress = ctx_thread.clone();
            let cb = move |p: ScanProgress| {
                let _ = tx_progress.send(Msg::CacheProgress(p));
                ctx_progress.request_repaint();
            };
            let caches = cleaner::scan(include_logs, &cb, &cancel_thread);
            let _ = tx.send(Msg::CacheDone(caches));
            ctx_thread.request_repaint();
        });

        self.begin_task(
            TaskKind::CacheScan,
            "Hledám cache složky…",
            rx,
            cancel,
        );
        self.log_push(LogKind::Info, "Spuštěno hledání Electron cache v AppData");
    }

    pub fn start_clean(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let targets: Vec<PathBuf> = self
            .caches
            .iter()
            .filter(|c| c.selected && !c.is_link)
            .map(|c| c.path.clone())
            .collect();
        if targets.is_empty() {
            self.toast(LogKind::Warn, "Není vybrána žádná cache složka");
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx_thread = ctx.clone();
        let cancel_thread = cancel.clone();
        let count = targets.len();

        std::thread::spawn(move || {
            let tx_progress = tx.clone();
            let ctx_progress = ctx_thread.clone();
            let cb = move |freed: u64, index: u64, label: &str| {
                let _ = tx_progress.send(Msg::CleanProgress {
                    freed,
                    total: index,
                    label: label.to_string(),
                });
                ctx_progress.request_repaint();
            };
            let report = cleaner::clean(&targets, &cb, &cancel_thread);
            let _ = tx.send(Msg::CleanDone(report));
            ctx_thread.request_repaint();
        });

        self.begin_task(TaskKind::Clean, "Čistím cache…", rx, cancel);
        self.log_push(
            LogKind::Info,
            format!("Čištění {count} cache složek zahájeno"),
        );
    }

    pub fn start_move(&mut self, ctx: &egui::Context, job: ConfirmMove) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx_thread = ctx.clone();
        let cancel_thread = cancel.clone();
        let source = job.entry.path.clone();
        let scope = job.entry.scope.dir_name().to_string();
        let name = job.entry.name.clone();
        let target_root = PathBuf::from(self.config.target_root.clone());
        let keep_backup = job.keep_backup;

        std::thread::spawn(move || {
            let tx_event = tx.clone();
            let ctx_event = ctx_thread.clone();
            let mut emit = move |event: RelocEvent| {
                let _ = tx_event.send(Msg::MoveEvent(event));
                ctx_event.request_repaint();
            };
            let result = relocator::move_folder(
                &source,
                &scope,
                &target_root,
                keep_backup,
                &mut emit,
                &cancel_thread,
            );
            let _ = tx.send(Msg::MoveDone(Box::new(result)));
            ctx_thread.request_repaint();
        });

        self.begin_task(
            TaskKind::Move,
            &format!("Přesouvám {name}…"),
            rx,
            cancel,
        );
        self.log_push(
            LogKind::Info,
            format!("Přesun {name} → {} zahájen", self.config.target_root),
        );
    }

    pub fn start_restore(&mut self, ctx: &egui::Context, job: ConfirmRestore) {
        if self.busy() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let ctx_thread = ctx.clone();
        let cancel_thread = cancel.clone();
        let entry = job.entry.clone();
        let delete_target = job.delete_target;
        let name = entry.name.clone();
        let source = entry.source.clone();
        let name_thread = name.clone();

        std::thread::spawn(move || {
            let tx_event = tx.clone();
            let ctx_event = ctx_thread.clone();
            let mut emit = move |event: RelocEvent| {
                let _ = tx_event.send(Msg::MoveEvent(event));
                ctx_event.request_repaint();
            };
            let result = relocator::restore(&entry, delete_target, &mut emit, &cancel_thread);
            let _ = tx.send(Msg::RestoreDone {
                name: name_thread,
                source,
                result,
            });
            ctx_thread.request_repaint();
        });

        self.begin_task(
            TaskKind::Restore,
            &format!("Navracím {name} na C:…"),
            rx,
            cancel,
        );
        self.log_push(LogKind::Info, format!("Navracení {name} na disk C: zahájeno"));
    }

    /// Drain all pending messages from the running task.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        let mut drain: Vec<Msg> = Vec::new();
        if let Some(task) = &mut self.task {
            while let Ok(msg) = task.rx.try_recv() {
                task.last_msg = Instant::now();
                drain.push(msg);
            }
            // Watchdog: a worker that died without a message must not lock the UI.
            if task.last_msg.elapsed() > Duration::from_secs(900) {
                let kind = task.kind;
                finished = true;
                self.log_push(
                    LogKind::Error,
                    format!("{}: pracovní vlákno přestalo odpovídat", kind.verb()),
                );
            }
        }

        for msg in drain {
            match msg {
                Msg::ScanProgress(p) => {
                    if let Some(task) = &mut self.task {
                        task.progress.phase = "Skenuji AppData…".to_string();
                        task.progress.detail = if p.current.is_empty() {
                            String::new()
                        } else {
                            format!("{} ({} / {} složek)", p.current, p.folders_done, p.folders_total)
                        };
                        task.progress.done = p.folders_done as u64;
                        task.progress.total = p.folders_total as u64;
                        task.progress.indeterminate = p.folders_total == 0;
                    }
                    self.status = format!(
                        "Naskenováno {} v {} složkách",
                        util::human_size(p.bytes),
                        p.folders_done
                    );
                }
                Msg::ScanDone(folders) => {
                    let total: u64 = folders.iter().filter(|f| f.state.is_local()).map(|f| f.size).sum();
                    let moved = folders.iter().filter(|f| f.state.is_moved()).count();
                    self.log_push(
                        LogKind::Good,
                        format!(
                            "Sken hotov: {} složek, {} na C:, {} už přesunutých",
                            folders.len(),
                            util::human_size(total),
                            moved
                        ),
                    );
                    self.folders = folders;
                    self.scanned_once = true;
                    self.tab = Tab::Relocate;
                    self.status = "Sken dokončen".to_string();
                    finished = true;
                }
                Msg::CacheProgress(p) => {
                    if let Some(task) = &mut self.task {
                        task.progress.detail = p.current.clone();
                        task.progress.done = p.folders_done as u64;
                        task.progress.total = p.folders_total as u64;
                        task.progress.indeterminate = p.folders_total == 0;
                    }
                    self.status = format!(
                        "Nalezeno {} cache ({} složek)",
                        util::human_size(p.bytes),
                        p.folders_done
                    );
                }
                Msg::CacheDone(caches) => {
                    let total: u64 = caches.iter().map(|c| c.size).sum();
                    self.log_push(
                        LogKind::Good,
                        format!(
                            "Nalezeno {} cache v {} složkách",
                            util::human_size(total),
                            caches.len()
                        ),
                    );
                    self.caches = caches;
                    self.clean_report = None;
                    self.tab = Tab::Cleaner;
                    self.status = "Cache naskenována".to_string();
                    finished = true;
                }
                Msg::CleanProgress { freed, total, label } => {
                    if let Some(task) = &mut self.task {
                        task.progress.done = total;
                        task.progress.total = self.caches.iter().filter(|c| c.selected).count() as u64;
                        task.progress.detail = label;
                    }
                    self.status = format!("Uvolněno {}", util::human_size(freed));
                }
                Msg::CleanDone(report) => {
                    let msg = format!(
                        "Čištění hotovo: uvolněno {} ({} souborů{}, {} chyb)",
                        util::human_size(report.freed),
                        report.deleted,
                        if report.cancelled { ", zrušeno" } else { "" },
                        report.errors.len()
                    );
                    self.log_push(
                        if report.success() { LogKind::Good } else { LogKind::Warn },
                        msg,
                    );
                    for err in report.errors.iter().take(5) {
                        self.log_push(LogKind::Warn, err.clone());
                    }
                    self.clean_report = Some(report);
                    self.status = "Čištění dokončeno".to_string();
                    self.start_cache_scan(ctx);
                    finished = true;
                }
                Msg::MoveEvent(event) => self.apply_reloc_event(event),
                Msg::MoveDone(result) => {
                    match *result {
                        Ok(moved) => {
                            for warning in &moved.warnings {
                                self.log_push(LogKind::Warn, warning.clone());
                            }
                            if let Some(backup) = &moved.backup {
                                self.log_push(
                                    LogKind::Info,
                                    format!("Záloha ponechána: {}", backup.display()),
                                );
                            }
                            self.log_push(
                                LogKind::Good,
                                format!(
                                    "{} přesunuto do {}",
                                    moved.entry.name,
                                    moved.entry.target
                                ),
                            );
                            self.config.add_entry(moved.entry);
                            if let Err(err) = self.config.save() {
                                self.log_push(LogKind::Error, err);
                            }
                            self.toast(LogKind::Good, "Přesun dokončen");
                            self.start_scan(ctx);
                        }
                        Err(err) => {
                            self.log_push(LogKind::Error, err.clone());
                            self.toast(LogKind::Error, err);
                        }
                    }
                    finished = true;
                }
                Msg::RestoreDone { name, source, result } => {
                    match result {
                        Ok(()) => {
                            self.config.remove_entry(&source);
                            if let Err(err) = self.config.save() {
                                self.log_push(LogKind::Error, err);
                            }
                            self.log_push(LogKind::Good, format!("{name} vráceno na disk C:"));
                            self.toast(LogKind::Good, "Složka vrácena na C:");
                            self.start_scan(ctx);
                        }
                        Err(err) => {
                            self.log_push(LogKind::Error, err.clone());
                            self.toast(LogKind::Error, err);
                        }
                    }
                    finished = true;
                }
            }
        }

        if finished {
            self.task = None;
            self.status = "Připraven".to_string();
        }
    }

    fn apply_reloc_event(&mut self, event: RelocEvent) {
        match event {
            RelocEvent::Phase(phase) => {
                if let Some(task) = &mut self.task {
                    task.progress.phase = phase.label().to_string();
                    if phase == relocator::Phase::Copy || phase == relocator::Phase::Restore {
                        task.progress.done = 0;
                    }
                }
                self.status = phase.label().to_string();
            }
            RelocEvent::Detail(detail) => {
                if let Some(task) = &mut self.task {
                    task.progress.detail = detail;
                }
            }
            RelocEvent::Progress { done, total } => {
                if let Some(task) = &mut self.task {
                    task.progress.done = done;
                    task.progress.total = total;
                }
            }
            RelocEvent::Log(level, message) => {
                let kind = match level {
                    Level::Info => LogKind::Info,
                    Level::Good => LogKind::Good,
                    Level::Warn => LogKind::Warn,
                    Level::Error => LogKind::Error,
                };
                self.log_push(kind, message);
            }
        }
    }

    // ------------------------------------------------------------- helpers --

    pub fn save_config(&mut self) {
        match self.config.save() {
            Ok(()) => self.log_push(
                LogKind::Good,
                format!("Nastavení uloženo do {}", Config::path().display()),
            ),
            Err(err) => self.log_push(LogKind::Error, err),
        }
    }

    /// Indices of folders matching the current filters, in display order.
    pub fn visible_folders(&self) -> Vec<usize> {
        let needle = self.search.trim().to_lowercase();
        let min_bytes = (self.min_size_mb * 1024.0 * 1024.0) as u64;
        let mut rows: Vec<usize> = self
            .folders
            .iter()
            .enumerate()
            .filter(|(_, f)| {
                let scope_ok = match f.scope {
                    Scope::Local => self.scope_local,
                    Scope::Roaming => self.scope_roaming,
                };
                if !scope_ok {
                    return false;
                }
                if self.only_moved && !f.state.is_moved() {
                    return false;
                }
                if f.size < min_bytes {
                    return false;
                }
                if needle.is_empty() {
                    return true;
                }
                f.name.to_lowercase().contains(&needle)
                    || f.path.to_string_lossy().to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect();

        match self.sort {
            SortMode::SizeDesc => rows.sort_by_key(|i| std::cmp::Reverse(self.folders[*i].size)),
            SortMode::NameAsc => rows.sort_by(|a, b| {
                self.folders[*a]
                    .name
                    .to_lowercase()
                    .cmp(&self.folders[*b].name.to_lowercase())
            }),
            SortMode::StatusFirst => rows.sort_by(|a, b| {
                let moved_a = self.folders[*a].state.is_moved();
                let moved_b = self.folders[*b].state.is_moved();
                moved_b
                    .cmp(&moved_a)
                    .then_with(|| self.folders[*b].size.cmp(&self.folders[*a].size))
            }),
        }
        rows
    }

    pub fn local_bytes_total(&self) -> u64 {
        self.folders
            .iter()
            .filter(|f| f.state.is_local())
            .map(|f| f.size)
            .sum()
    }

    pub fn selected_cache_bytes(&self) -> u64 {
        cleaner::total_selected(&self.caches)
    }

    pub fn selected_cache_count(&self) -> usize {
        self.caches.iter().filter(|c| c.selected).count()
    }

    /// Called once per frame.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.disks_at.elapsed() > Duration::from_secs(10) {
            self.refresh_disks();
        }
        if let Some((_, _, until)) = &self.toast {
            if Instant::now() > *until {
                self.toast = None;
            }
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn clock_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

impl eframe::App for PathMoverApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl PathMoverApp {
    /// Draw one frame into `ui`. Separated from the `eframe::App` impl so the
    /// layout can be driven from tests without an `eframe::Frame`.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.tick(&ctx);
        crate::ui::root(self, ui);
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}