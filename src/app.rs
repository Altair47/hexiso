// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Main window: menus, toolbar, explorer tree, file list, sector viewer, properties, log.

use crate::dialogs::Dialog;
use crate::drives::{self, DriveInfo, DriveKind};
use crate::jobs::{Apply, Job, Progress};
use crate::model::{fmt_time, human, Document, Entry, FileSrc, Kind};
use crate::ops::{self, OpenResult, OutFormat};
use crate::source::SECTOR;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, FontId, Key, RichText, Sense, Stroke, StrokeKind, Ui};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lvl {
    Info,
    Ok,
    Warn,
    Err,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Type,
    Date,
    Lba,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Files,
    Sectors,
}

#[derive(Clone)]
pub struct Row {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub kind: String,
    pub mtime: i64,
    pub lba: Option<u64>,
    pub hidden: bool,
}

pub struct HexApp {
    pub ctx: egui::Context,
    pub doc: Document,
    pub version: u64,
    pub cwd: Vec<String>,
    pub sel: BTreeSet<String>,
    pub anchor: Option<String>,
    pub sort: SortKey,
    pub sort_desc: bool,
    pub logs: Vec<(Lvl, String, String)>,
    pub job: Option<Job>,
    pub dialog: Option<Dialog>,
    pub drives: Vec<DriveInfo>,
    pub drives_at: Option<Instant>,
    drive_slot: Arc<std::sync::Mutex<Option<Vec<DriveInfo>>>>,
    drive_busy: Arc<std::sync::atomic::AtomicBool>,
    drives_seen: bool,
    stats_cache: Option<(u64, (u64, u64, u64, u64))>,
    pub frame_ms: f32,
    pub renderer: String,
    last_title: String,
    debug_repaint: bool,
    bench_start: Option<Instant>,
    pub mounts: Vec<(PathBuf, String)>,
    pub expanded: HashSet<Vec<String>>,
    pub filter: String,
    pub view: View,
    pub hex_lba: u64,
    pub hex_input: String,
    pub hex_cache: Option<(u64, Vec<u8>)>,
    pub shot: Option<PathBuf>,
    pub shot_state: Arc<AtomicU8>,
    pub frame: u64,
    pub show_log: bool,
    pub log_problems_only: bool,
    pub started: bool,
    /// window size in points (updated every frame) and the person's panel choices
    /// (None = decide automatically from the window size)
    pub win_w: f32,
    pub win_h: f32,
    pub tree_pref: Option<bool>,
    pub info_pref: Option<bool>,
    last_cwd: Vec<String>,
    rows_cache: Option<(u64, Vec<String>, SortKey, bool, String, Arc<Vec<Row>>)>,
}

/// Toolbar button size for the current frame (0 full, 1 compact, 2 icons only).
static TOOL_MODE: AtomicU8 = AtomicU8::new(0);

pub fn temp_dir(sub: &str) -> PathBuf {
    let p = std::env::temp_dir().join("hexiso").join(sub);
    let _ = std::fs::create_dir_all(&p);
    p
}

fn now_hms() -> String {
    let t = crate::model::now();
    let (_, _, _, h, m, s) = crate::model::civil(t);
    format!("{:02}:{:02}:{:02}", h, m, s)
}

pub fn es<T>(r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

impl HexApp {
    pub fn new(ctx: &egui::Context, shot: Option<PathBuf>, open: Option<PathBuf>) -> Self {
        let mut a = HexApp {
            ctx: ctx.clone(),
            doc: Document::new_blank(),
            version: 0,
            cwd: vec![],
            sel: BTreeSet::new(),
            anchor: None,
            sort: SortKey::Name,
            sort_desc: false,
            logs: vec![],
            job: None,
            dialog: None,
            drives: vec![],
            drives_at: None,
            drive_slot: Arc::new(std::sync::Mutex::new(None)),
            drive_busy: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            drives_seen: false,
            stats_cache: None,
            frame_ms: 0.0,
            renderer: String::new(),
            last_title: String::new(),
            debug_repaint: std::env::var_os("HEXISO_DEBUG_REPAINT").is_some(),
            bench_start: None,
            mounts: vec![],
            expanded: HashSet::new(),
            filter: String::new(),
            view: View::Files,
            hex_lba: 16,
            hex_input: "16".into(),
            hex_cache: None,
            shot,
            shot_state: Arc::new(AtomicU8::new(0)),
            frame: 0,
            show_log: true,
            log_problems_only: false,
            started: false,
            win_w: 1360.0,
            win_h: 820.0,
            tree_pref: None,
            info_pref: None,
            last_cwd: vec![],
            rows_cache: None,
        };
        a.expanded.insert(vec![]);
        a.log(Lvl::Info, "hexiso engine online :: ISO9660 / Joliet / Rock Ridge / UDF / El Torito / FAT / CSO / HXZ".to_string());
        a.log(Lvl::Info, "drop files or folders anywhere to build a new image, or press Ctrl+O to open one".to_string());
        a.refresh_drives();
        if let Some(p) = open {
            a.open_path(p);
        }
        a
    }

    pub fn log(&mut self, l: Lvl, msg: impl Into<String>) {
        self.logs.push((l, now_hms(), msg.into()));
        if self.logs.len() > 2000 {
            self.logs.drain(0..500);
        }
    }

    pub fn touch(&mut self) {
        self.version += 1;
        self.doc.modified = true;
    }

    pub fn set_doc(&mut self, doc: Document) {
        let keep_cwd = self.cwd.clone();
        self.started = true;
        self.doc = doc;
        self.version += 1;
        self.sel.clear();
        self.hex_cache = None;
        self.expanded.clear();
        self.expanded.insert(vec![]);
        self.cwd = if self.doc.dir_at(&keep_cwd).is_some() && !keep_cwd.is_empty() { keep_cwd } else { vec![] };
    }

    /// Ask for a fresh drive list. Runs on a background thread: querying an optical drive can
    /// take from milliseconds to seconds, which must never stall drawing (or window dragging).
    pub fn refresh_drives(&mut self) {
        self.drives_at = Some(Instant::now());
        if self.drive_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let slot = self.drive_slot.clone();
        let busy = self.drive_busy.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let list = drives::list();
            *slot.lock().unwrap() = Some(list);
            busy.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    fn poll_drives(&mut self) {
        let got = self.drive_slot.lock().unwrap().take();
        if let Some(new) = got {
            self.apply_drives(new);
        }
    }

    fn apply_drives(&mut self, new: Vec<DriveInfo>) {
        if self.drives_seen {
            for d in &new {
                match self.drives.iter().find(|o| o.letter == d.letter) {
                    None => self.log(Lvl::Info, format!("drive {}: appeared ({}{})", d.letter, d.kind.name().to_lowercase(), if d.label.is_empty() { String::new() } else { format!(", {}", d.label) })),
                    Some(o) if !o.ready && d.ready => self.log(Lvl::Info, format!("media inserted in {}: {} {}", d.letter, d.label, human(d.total))),
                    Some(o) if o.ready && !d.ready => self.log(Lvl::Info, format!("media removed from {}:", d.letter)),
                    _ => {}
                }
            }
            for o in &self.drives.clone() {
                if !new.iter().any(|d| d.letter == o.letter) {
                    self.log(Lvl::Info, format!("drive {}: disconnected", o.letter));
                }
            }
        } else {
            let opt = new.iter().filter(|d| d.kind == DriveKind::Optical).count();
            let usb = new.iter().filter(|d| d.kind == DriveKind::Removable).count();
            self.log(Lvl::Info, format!("found {} drive(s): {} optical, {} removable", new.len(), opt, usb));
        }
        self.drives_seen = true;
        self.drives = new;
    }

    /// (files, folders, data bytes, image bytes) — recomputed only when the tree changes.
    pub fn stats(&mut self) -> (u64, u64, u64, u64) {
        if let Some((v, s)) = self.stats_cache
            && v == self.version {
                return s;
            }
        let (f, d) = self.doc.root.count();
        let data = self.doc.root.size();
        let img = if self.doc.modified || self.doc.src.is_none() { crate::writer::estimate(&self.doc) } else { self.doc.src.as_ref().map(|s| s.sector_count() * SECTOR).unwrap_or(0) };
        let s = (f, d.saturating_sub(1), data, img);
        self.stats_cache = Some((self.version, s));
        s
    }

    pub fn run<F>(&mut self, title: &str, f: F)
    where
        F: FnOnce(&Progress) -> Result<Apply, String> + Send + 'static,
    {
        if self.job.is_some() {
            self.log(Lvl::Warn, "another operation is still running");
            return;
        }
        self.log(Lvl::Info, format!("{} ...", title.to_lowercase()));
        self.job = Some(Job::spawn(title, &self.ctx, f));
    }

    fn poll_job(&mut self) {
        let res = self.job.as_ref().and_then(|j| j.take());
        if let Some(r) = res {
            let job = self.job.take().unwrap();
            let secs = job.started.elapsed().as_secs_f32();
            match r {
                Ok(apply) => {
                    apply(self);
                    self.log(Lvl::Ok, format!("{} finished in {:.1}s", job.title.to_lowercase(), secs));
                }
                Err(e) => {
                    if e == "cancelled" {
                        self.log(Lvl::Warn, format!("{} cancelled", job.title.to_lowercase()));
                    } else {
                        self.log(Lvl::Err, format!("{} failed: {}", job.title.to_lowercase(), e));
                        self.dialog = Some(Dialog::Message { title: format!("{} failed", job.title), body: e, error: true });
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ actions

    pub fn confirm_discard(&mut self, then: crate::dialogs::Then) {
        if self.doc.modified {
            self.dialog = Some(Dialog::Discard { then });
        } else {
            self.do_then(then);
        }
    }

    pub fn do_then(&mut self, then: crate::dialogs::Then) {
        use crate::dialogs::Then;
        match then {
            Then::New => {
                self.set_doc(Document::new_blank());
                self.started = true;
                self.log(Lvl::Ok, "started a new, empty image in memory (nothing saved yet)");
                self.log(Lvl::Info, "next: add files with ADD or drag & drop, then press SAVE to write it as an .iso file");
            }
            Then::Open(p) => self.open_path(p),
            Then::OpenDialog => self.open_dialog(),
            Then::Exit => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Then::Drive(l) => self.open_drive(l),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("Disc images", &["iso", "bin", "cue", "img", "nrg", "mdf", "cso", "hxz", "vhd", "ima", "flp", "001", "zip", "udf"])
            .add_filter("All files", &["*"])
            .pick_file()
        {
            self.open_path(p);
        }
    }

    pub fn open_path(&mut self, p: PathBuf) {
        self.open_with_password(p, None);
    }

    pub fn open_with_password(&mut self, p: PathBuf, pw: Option<String>) {
        if !p.exists() {
            self.log(Lvl::Warn, format!("not found, skipped: {}", p.display()));
            return;
        }
        let p2 = p.clone();
        self.run("Opening image", move |_prog| {
            let r = ops::open_path(&p2, pw.as_deref()).map_err(|e| format!("{}\n\n{}", p2.display(), e))?;
            Ok(Box::new(move |app: &mut HexApp| match r {
                OpenResult::NeedPassword => app.dialog = Some(Dialog::Password { path: p2, pw: String::new() }),
                OpenResult::Doc(d) => {
                    let (f, dcount) = d.root.count();
                    app.log(Lvl::Ok, format!("opened {} ({}): {} files, {} folders, names read from {}", p2.display(), d.format, f, dcount.saturating_sub(1), d.fs_name));
                    app.cwd.clear();
                    app.set_doc(*d);
                }
            }) as Apply)
        });
    }

    pub fn open_drive(&mut self, letter: char) {
        self.run("Reading drive", move |_p| {
            #[cfg(windows)]
            {
                let src: crate::source::Src = Arc::new(es(crate::source::DriveSource::open(letter))?);
                let mut d = es(ops::doc_from_source(src, None))?;
                d.format = format!("Drive {}:", letter);
                Ok(Box::new(move |app: &mut HexApp| {
                    app.log(Lvl::Ok, format!("drive {}: :: {} :: label {}", letter, d.fs_name, d.label));
                    app.cwd.clear();
                    app.set_doc(d);
                }) as Apply)
            }
            #[cfg(not(windows))]
            {
                let _ = letter;
                Err("drive access is only available on Windows".to_string())
            }
        });
    }

    pub fn switch_view(&mut self, view: String) {
        let Some(src) = self.doc.src.clone() else { return };
        let path = self.doc.path.clone();
        let pw = self.doc.password.clone();
        self.run("Switching file system", move |_p| {
            let mut d = es(ops::doc_from_source(src, Some(&view)))?;
            d.path = path;
            d.password = pw;
            Ok(Box::new(move |app: &mut HexApp| {
                app.log(Lvl::Ok, format!("now showing names from the {} table (the image itself is unchanged)", d.fs_name));
                app.set_doc(d);
            }) as Apply)
        });
    }

    pub fn savable(p: &Path) -> bool {
        matches!(p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("iso") | Some("cso") | Some("hxz") | Some("zip"))
    }

    pub fn save(&mut self) {
        match self.doc.path.clone() {
            Some(p) if Self::savable(&p) => {
                let fmt = OutFormat::from_path(&p);
                let pw = self.doc.password.clone();
                self.save_to(p, fmt, pw);
            }
            _ => self.save_as(),
        }
    }

    pub fn save_as(&mut self) {
        let name = Path::new(&self.doc.title()).with_extension("iso");
        if let Some(p) = rfd::FileDialog::new()
            .set_file_name(name.to_string_lossy())
            .add_filter("ISO image", &["iso"])
            .add_filter("CSO compressed image", &["cso"])
            .add_filter("HXZ compressed / encrypted image", &["hxz"])
            .add_filter("ZIP archive", &["zip"])
            .add_filter("Floppy image 1.44MB", &["img"])
            .save_file()
        {
            let fmt = OutFormat::from_path(&p);
            if fmt == OutFormat::Hxz {
                self.dialog = Some(Dialog::SavePassword { path: p, pw: String::new(), pw2: String::new() });
            } else {
                self.save_to(p, fmt, None);
            }
        }
    }

    pub fn save_to(&mut self, path: PathBuf, fmt: OutFormat, pw: Option<String>) {
        let doc = self.doc.clone();
        self.run("Saving image", move |prog| {
            let n = es(ops::save_doc(&doc, &path, fmt, pw.as_deref(), prog))?;
            let reopened = ops::open_path(&path, pw.as_deref());
            Ok(Box::new(move |app: &mut HexApp| {
                app.log(Lvl::Ok, format!("wrote {} ({})", path.display(), human(n)));
                match reopened {
                    Ok(OpenResult::Doc(d)) => app.set_doc(*d),
                    _ => {
                        app.doc.modified = false;
                        app.doc.path = Some(path.clone());
                    }
                }
            }) as Apply)
        });
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let mut added = 0;
        let mut replaced = 0;
        for p in paths {
            match Entry::from_disk(&p) {
                Ok(e) => {
                    let cwd = self.cwd.clone();
                    if let Some(dir) = self.doc.dir_at_mut(&cwd).and_then(|d| d.children_mut()) {
                        if let Some(i) = dir.iter().position(|x| x.name.eq_ignore_ascii_case(&e.name)) {
                            dir.remove(i);
                            replaced += 1;
                        }
                        dir.push(e);
                        added += 1;
                    }
                }
                Err(err) => self.log(Lvl::Err, format!("cannot add {}: {}", p.display(), err)),
            }
        }
        if added > 0 {
            self.touch();
            self.started = true;
            self.log(Lvl::Ok, format!("added {} item(s) to /{} of {}{}", added, self.cwd.join("/"), self.doc_name(), if replaced > 0 { format!(" ({} replaced)", replaced) } else { String::new() }));
            self.log(Lvl::Info, "not written yet: press SAVE to store the changes");
        }
    }

    pub fn add_files_dialog(&mut self) {
        if let Some(v) = rfd::FileDialog::new().pick_files() {
            self.add_paths(v);
        }
    }
    pub fn add_folder_dialog(&mut self) {
        if let Some(v) = rfd::FileDialog::new().pick_folder() {
            self.add_paths(vec![v]);
        }
    }

    pub fn delete_selected(&mut self) {
        if self.sel.is_empty() {
            return;
        }
        let names = self.sel.clone();
        let cwd = self.cwd.clone();
        if let Some(dir) = self.doc.dir_at_mut(&cwd).and_then(|d| d.children_mut()) {
            dir.retain(|e| !names.contains(&e.name));
        }
        self.doc.boot.retain(|b| b.tree_path.as_ref().map(|p| !(p.len() == cwd.len() + 1 && p.starts_with(&cwd) && names.contains(&p[cwd.len()]))).unwrap_or(true));
        self.log(Lvl::Ok, format!("deleted {} item(s)", names.len()));
        self.sel.clear();
        self.touch();
    }

    pub fn rename(&mut self, old: &str, new: &str) {
        let new = new.trim();
        if new.is_empty() || new.contains('/') || new.contains('\\') {
            self.log(Lvl::Err, "invalid name");
            return;
        }
        let cwd = self.cwd.clone();
        if let Some(dir) = self.doc.dir_at_mut(&cwd).and_then(|d| d.children_mut()) {
            if dir.iter().any(|e| e.name == new && e.name != old) {
                self.log(Lvl::Err, format!("'{}' already exists here", new));
                return;
            }
            if let Some(e) = dir.iter_mut().find(|e| e.name == old) {
                e.name = new.to_string();
            }
        }
        self.sel.clear();
        self.sel.insert(new.to_string());
        self.log(Lvl::Ok, format!("renamed {} -> {}", old, new));
        self.touch();
    }

    pub fn new_folder(&mut self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let cwd = self.cwd.clone();
        if let Some(dir) = self.doc.dir_at_mut(&cwd).and_then(|d| d.children_mut()) {
            if dir.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
                self.log(Lvl::Err, format!("'{}' already exists", name));
                return;
            }
            dir.push(Entry::dir(&name, vec![]));
        }
        self.touch();
        self.log(Lvl::Ok, format!("folder {} created", name));
    }

    pub fn selected_entries(&self) -> Vec<Entry> {
        let Some(dir) = self.doc.dir_at(&self.cwd).and_then(|d| d.children()) else { return vec![] };
        dir.iter().filter(|e| self.sel.contains(&e.name)).cloned().collect()
    }

    pub fn extract(&mut self, all: bool) {
        let items: Vec<Entry> = if all || self.sel.is_empty() {
            self.doc.dir_at(if all { &[] } else { &self.cwd }).and_then(|d| d.children()).cloned().unwrap_or_default()
        } else {
            self.selected_entries()
        };
        if items.is_empty() {
            self.log(Lvl::Warn, "nothing to extract");
            return;
        }
        let Some(dest) = rfd::FileDialog::new().set_title("Extract to folder").pick_folder() else { return };
        let doc = self.doc.clone();
        self.run("Extracting", move |prog| {
            let (n, failed) = es(ops::extract(&doc, &items, &dest, prog))?;
            Ok(Box::new(move |app: &mut HexApp| {
                app.log(if failed > 0 { Lvl::Warn } else { Lvl::Ok }, format!("extracted {} file(s) to {}{}", n, dest.display(), if failed > 0 { format!(", {} failed", failed) } else { String::new() }));
                drives::open_in_shell(&dest);
            }) as Apply)
        });
    }

    pub fn open_entry(&mut self, e: Entry) {
        let doc = self.doc.clone();
        self.run("Opening file", move |prog| {
            let dir = temp_dir(&format!("open/{}", crate::model::now()));
            es(ops::extract(&doc, std::slice::from_ref(&e), &dir, prog))?;
            let p = dir.join(&e.name);
            Ok(Box::new(move |_app: &mut HexApp| drives::open_in_shell(&p)) as Apply)
        });
    }

    /// Make sure the current document exists as a plain ISO file, then hand the path on.
    pub fn with_iso<F>(&mut self, title: &str, then: F)
    where
        F: FnOnce(&Progress, PathBuf) -> Result<Apply, String> + Send + 'static,
    {
        let doc = self.doc.clone();
        let direct = doc
            .path
            .clone()
            .filter(|p| !doc.modified && matches!(p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("iso")));
        self.run(title, move |prog| {
            let path = match direct {
                Some(p) => p,
                None => {
                    let name = Path::new(&doc.title()).with_extension("iso");
                    let tmp = temp_dir("iso").join(name);
                    prog.say("preparing an ISO copy");
                    match (&doc.src, doc.modified) {
                        (Some(src), false) => es(ops::convert(src, &tmp, OutFormat::Iso, None, prog))?,
                        _ => es(ops::save_doc(&doc, &tmp, OutFormat::Iso, None, prog))?,
                    };
                    tmp
                }
            };
            then(prog, path)
        });
    }

    pub fn has_content(&self) -> bool {
        self.doc.src.is_some() || self.doc.root.children().map(|c| !c.is_empty()).unwrap_or(false)
    }

    /// Is a mounted image path (as Windows reports it) a copy of the open document?
    fn same_image(mounted: &str, doc_path: &Option<PathBuf>, stem: &str) -> bool {
        let m = mounted.to_lowercase();
        if let Some(p) = doc_path
            && m == p.display().to_string().to_lowercase() {
                return true;
            }
        // temporary ISO copies made for mounting this image
        let mp = Path::new(&m);
        let name = mp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parent = mp.parent().map(|x| x.display().to_string()).unwrap_or_default();
        let tmp = [temp_dir("iso"), temp_dir("mount")].iter().map(|t| t.display().to_string().to_lowercase()).collect::<Vec<_>>();
        !stem.is_empty() && tmp.contains(&parent) && name.starts_with(&stem.to_lowercase())
    }

    pub fn mount_current(&mut self) {
        if !self.has_content() {
            self.log(Lvl::Warn, "open or build an image first");
            return;
        }
        let doc_path = self.doc.path.clone();
        let stem = Path::new(&self.doc.title()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let name = self.doc_name();
        self.run("Checking virtual drives", move |_| {
            let mine: Vec<(String, String)> = drives::mounted().into_iter().filter(|(p, _)| Self::same_image(p, &doc_path, &stem)).collect();
            Ok(Box::new(move |app: &mut HexApp| {
                // remember drives Windows already has for this image
                for (p, l) in &mine {
                    if !app.mounts.iter().any(|m| &m.1 == l) {
                        app.mounts.push((PathBuf::from(p), l.clone()));
                    }
                }
                if mine.is_empty() {
                    app.mount_now(vec![], false);
                } else {
                    app.log(Lvl::Warn, format!("{} is already mounted as {}", name, mine.iter().map(|m| format!("{}:", m.1)).collect::<Vec<_>>().join(", ")));
                    app.dialog = Some(Dialog::AlreadyMounted { name, existing: mine });
                }
            }) as Apply)
        });
    }

    /// Mount the open image. `unmount_first`: image paths to eject before mounting (remount).
    /// `extra`: mount another copy next to the existing drive(s).
    pub fn mount_now(&mut self, unmount_first: Vec<String>, extra: bool) {
        let doc = self.doc.clone();
        let ext = doc.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let direct = doc.path.clone().filter(|_| !doc.modified && matches!(ext.as_str(), "iso" | "vhd" | "vhdx"));
        let title = if !unmount_first.is_empty() { "Remounting" } else { "Mounting" };
        self.run(title, move |prog| {
            for p in &unmount_first {
                prog.say(format!("ejecting {}", p));
                es(drives::unmount(Path::new(p)))?;
            }
            let stamp = crate::model::now();
            let stem = Path::new(&doc.title()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
            let path = match direct {
                Some(p) if !extra => p,
                Some(p) => {
                    // Windows mounts each file only once: give it a second name (hard link, or a copy)
                    let e = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "iso".into());
                    let link = temp_dir("mount").join(format!("{}_copy{}.{}", stem, stamp, e));
                    if std::fs::hard_link(&p, &link).is_err() {
                        prog.say("copying the image for a second drive");
                        es(std::fs::copy(&p, &link))?;
                    }
                    link
                }
                None => {
                    let tmp = if extra { temp_dir("mount").join(format!("{}_copy{}.iso", stem, stamp)) } else { temp_dir("iso").join(format!("{}.iso", stem)) };
                    prog.say("preparing an ISO copy to mount");
                    match (&doc.src, doc.modified) {
                        (Some(src), false) => es(ops::convert(src, &tmp, OutFormat::Iso, None, prog))?,
                        _ => es(ops::save_doc(&doc, &tmp, OutFormat::Iso, None, prog))?,
                    };
                    tmp
                }
            };
            prog.say("asking Windows to mount it");
            let l = es(drives::mount(&path))?;
            Ok(Box::new(move |app: &mut HexApp| {
                for p in &unmount_first {
                    app.mounts.retain(|m| m.0.display().to_string().to_lowercase() != p.to_lowercase());
                    app.log(Lvl::Ok, format!("ejected {}", p));
                }
                app.mounted(path, l)
            }) as Apply)
        });
    }

    fn mounted(&mut self, p: PathBuf, l: String) {
        self.log(Lvl::Ok, format!("mounted {} as drive {}:", p.display(), l));
        self.mounts.retain(|m| m.1 != l);
        self.mounts.push((p, l.clone()));
        self.refresh_drives();
        if let Some(c) = l.chars().next().filter(|c| c.is_ascii_alphabetic()) {
            drives::open_in_shell(Path::new(&format!("{}:\\", c)));
        }
    }

    pub fn unmount(&mut self, p: PathBuf) {
        self.run("Unmounting", move |_| {
            es(drives::unmount(&p))?;
            Ok(Box::new(move |app: &mut HexApp| {
                app.mounts.retain(|m| m.0 != p);
                app.log(Lvl::Ok, format!("unmounted {}", p.display()));
                app.refresh_drives();
            }) as Apply)
        });
    }

    pub fn test_image(&mut self) {
        let doc = self.doc.clone();
        self.run("Testing image", move |prog| {
            let (ok, bad) = es(ops::test_image(&doc, prog))?;
            Ok(Box::new(move |app: &mut HexApp| {
                let body = if bad.is_empty() {
                    format!("All {} files read back without errors.", ok)
                } else {
                    format!("{} files OK, {} with errors:\n\n{}", ok, bad.len(), bad.iter().take(50).cloned().collect::<Vec<_>>().join("\n"))
                };
                app.log(if bad.is_empty() { Lvl::Ok } else { Lvl::Warn }, format!("test: {} ok, {} errors", ok, bad.len()));
                app.dialog = Some(Dialog::Message { title: "Test result".into(), body, error: !bad.is_empty() });
            }) as Apply)
        });
    }

    fn handle_drop(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        let is_image = |p: &PathBuf| {
            matches!(p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("iso") | Some("cue") | Some("nrg") | Some("mdf") | Some("cso") | Some("hxz"))
        };
        if dropped.len() == 1 && is_image(&dropped[0]) && self.doc.src.is_none() && !self.doc.modified {
            self.open_path(dropped[0].clone());
        } else {
            self.add_paths(dropped);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.job.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let (ctrl_o, ctrl_s, ctrl_n, ctrl_a, ctrl_e, del, f2, back, enter, f5) = ctx.input(|i| {
            let c = i.modifiers.command;
            (
                c && i.key_pressed(Key::O),
                c && i.key_pressed(Key::S),
                c && i.key_pressed(Key::N),
                c && i.key_pressed(Key::A),
                c && i.key_pressed(Key::E),
                i.key_pressed(Key::Delete),
                i.key_pressed(Key::F2),
                i.key_pressed(Key::Backspace),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::F5),
            )
        });
        if ctrl_o {
            self.confirm_discard(crate::dialogs::Then::OpenDialog);
        }
        if ctrl_s {
            self.save();
        }
        if ctrl_n {
            self.confirm_discard(crate::dialogs::Then::New);
        }
        if ctrl_e {
            self.extract(false);
        }
        if f5 {
            self.refresh_drives();
        }
        if typing {
            return;
        }
        if ctrl_a
            && let Some(d) = self.doc.dir_at(&self.cwd).and_then(|d| d.children()) {
                self.sel = d.iter().map(|e| e.name.clone()).collect();
            }
        if del {
            self.delete_selected();
        }
        if f2
            && let Some(n) = self.sel.iter().next().cloned() {
                self.dialog = Some(Dialog::Rename { old: n.clone(), new: n });
            }
        if back && !self.cwd.is_empty() {
            self.cwd.pop();
            self.sel.clear();
        }
        if enter && self.sel.len() == 1 {
            let n = self.sel.iter().next().cloned().unwrap();
            self.activate(&n);
        }
    }

    fn activate(&mut self, name: &str) {
        let e = self.doc.dir_at(&self.cwd).and_then(|d| d.children()).and_then(|c| c.iter().find(|e| e.name == name)).cloned();
        if let Some(e) = e {
            if e.is_dir() {
                self.cwd.push(e.name.clone());
                self.expanded.insert(self.cwd.clone());
                self.sel.clear();
            } else {
                self.open_entry(e);
            }
        }
    }

    pub fn rows(&mut self) -> Arc<Vec<Row>> {
        if let Some((v, c, s, d, f, rows)) = &self.rows_cache
            && *v == self.version && c == &self.cwd && *s == self.sort && *d == self.sort_desc && f == &self.filter {
                return rows.clone();
            }
        let filt = self.filter.to_lowercase();
        let mut rows: Vec<Row> = self
            .doc
            .dir_at(&self.cwd)
            .and_then(|d| d.children())
            .map(|c| {
                c.iter()
                    .filter(|e| filt.is_empty() || e.name.to_lowercase().contains(&filt))
                    .map(|e| Row { name: e.name.clone(), is_dir: e.is_dir(), size: e.size(), kind: e.type_name(), mtime: e.mtime, lba: e.lba, hidden: e.hidden })
                    .collect()
            })
            .unwrap_or_default();
        let key = self.sort;
        let desc = self.sort_desc;
        rows.sort_by(|a, b| {
            let o = match key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Type => a.kind.cmp(&b.kind),
                SortKey::Date => a.mtime.cmp(&b.mtime),
                SortKey::Lba => a.lba.cmp(&b.lba),
            };
            let o = if desc { o.reverse() } else { o };
            b.is_dir.cmp(&a.is_dir).then(o)
        });
        let rows = Arc::new(rows);
        self.rows_cache = Some((self.version, self.cwd.clone(), self.sort, self.sort_desc, self.filter.clone(), rows.clone()));
        rows
    }

    // ------------------------------------------------------------------ drawing

    fn menu_bar(&mut self, ui: &mut Ui) {
        use crate::dialogs::{DrivePurpose, SumTarget, Then};
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(RichText::new("HEX").color(GREEN).strong());
            ui.label(RichText::new("ISO").color(CYAN).strong());
            ui.add_space(6.0);
            ui.menu_button("File", |ui| {
                if ui.button("New data image          Ctrl+N").clicked() {
                    self.confirm_discard(Then::New);
                    ui.close();
                }
                if ui.button("Make floppy image...").clicked() {
                    self.dialog = Some(Dialog::Floppy { big: false });
                    ui.close();
                }
                ui.separator();
                if ui.button("Open image...           Ctrl+O").clicked() {
                    self.confirm_discard(Then::OpenDialog);
                    ui.close();
                }
                if ui.button("Open CD/DVD/BD/USB drive...").clicked() {
                    self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Open, kinds: vec![DriveKind::Optical, DriveKind::Removable, DriveKind::Fixed] });
                    ui.close();
                }
                ui.separator();
                if ui.button("Save                    Ctrl+S").clicked() {
                    self.save();
                    ui.close();
                }
                if ui.button("Save as...").clicked() {
                    self.save_as();
                    ui.close();
                }
                if ui.button("Close image").clicked() {
                    self.confirm_discard(Then::New);
                    ui.close();
                }
                ui.separator();
                if ui.button("Image properties...").clicked() {
                    self.dialog = Some(Dialog::Properties { label: self.doc.label.clone() });
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    self.confirm_discard(Then::Exit);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.button("Add files...").clicked() {
                    self.add_files_dialog();
                    ui.close();
                }
                if ui.button("Add folder...").clicked() {
                    self.add_folder_dialog();
                    ui.close();
                }
                if ui.button("New folder...").clicked() {
                    self.dialog = Some(Dialog::NewFolder { name: "NEW_FOLDER".into() });
                    ui.close();
                }
                ui.separator();
                if ui.button("Rename            F2").clicked() {
                    if let Some(n) = self.sel.iter().next().cloned() {
                        self.dialog = Some(Dialog::Rename { old: n.clone(), new: n });
                    }
                    ui.close();
                }
                if ui.button("Delete            Del").clicked() {
                    self.delete_selected();
                    ui.close();
                }
                ui.separator();
                if ui.button("Extract selected  Ctrl+E").clicked() {
                    self.extract(false);
                    ui.close();
                }
                if ui.button("Extract everything...").clicked() {
                    self.extract(true);
                    ui.close();
                }
            });
            ui.menu_button("Action", |ui| {
                if ui.button("Boot options...").clicked() {
                    self.dialog = Some(Dialog::Boot);
                    ui.close();
                }
                if ui.button("Change label...").clicked() {
                    self.dialog = Some(Dialog::Properties { label: self.doc.label.clone() });
                    ui.close();
                }
                if ui.button("Search in image...").clicked() {
                    self.dialog = Some(Dialog::Search { query: String::new(), results: vec![] });
                    ui.close();
                }
                if ui.button("View sectors").clicked() {
                    self.view = View::Sectors;
                    ui.close();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("Burn image to disc...").clicked() {
                    self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Burn, kinds: vec![DriveKind::Optical] });
                    ui.close();
                }
                if ui.button("Make image from disc / drive...").clicked() {
                    self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::MakeImage, kinds: vec![DriveKind::Optical, DriveKind::Removable] });
                    ui.close();
                }
                if ui.button("Copy disc...").clicked() {
                    self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Copy, kinds: vec![DriveKind::Optical] });
                    ui.close();
                }
                if ui.button("Rip audio CD to WAV...").clicked() {
                    self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Rip, kinds: vec![DriveKind::Optical] });
                    ui.close();
                }
                if ui.button("Create bootable USB drive...").clicked() {
                    self.dialog = Some(Dialog::Usb { drive: None, format: true, fs: "FAT32".into(), confirm: String::new() });
                    ui.close();
                }
                ui.separator();
                if ui.button("Mount image as virtual drive").clicked() {
                    self.mount_current();
                    ui.close();
                }
                if ui.button("Virtual drives...").clicked() {
                    self.dialog = Some(Dialog::Mounts { system: drives::mounted() });
                    ui.close();
                }
                ui.separator();
                if ui.button("Convert image format...").clicked() {
                    self.dialog = Some(Dialog::convert_default(self, OutFormat::Iso));
                    ui.close();
                }
                if ui.button("Compress image...").clicked() {
                    self.dialog = Some(Dialog::convert_default(self, OutFormat::Cso));
                    ui.close();
                }
                if ui.button("Encrypt image...").clicked() {
                    self.dialog = Some(Dialog::convert_default(self, OutFormat::Hxz));
                    ui.close();
                }
                if ui.button("Split file into volumes...").clicked() {
                    self.dialog = Some(Dialog::Split { file: self.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), mb: "700".into() });
                    ui.close();
                }
                if ui.button("Unpack ZIP archive...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("ZIP", &["zip"]).pick_file() {
                        self.open_path(p);
                    }
                    ui.close();
                }
                ui.separator();
                if ui.button("Test image").clicked() {
                    self.test_image();
                    ui.close();
                }
                if ui.button("Checksums...").clicked() {
                    self.dialog = Some(Dialog::Checksum { target: SumTarget::Image, result: None });
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                if ui.selectable_label(self.view == View::Files, "File list").clicked() {
                    self.view = View::Files;
                    ui.close();
                }
                if ui.selectable_label(self.view == View::Sectors, "Sector viewer").clicked() {
                    self.view = View::Sectors;
                    ui.close();
                }
                ui.separator();
                let mut t = self.tree_visible();
                if ui.checkbox(&mut t, "Folder tree").clicked() {
                    self.tree_pref = Some(t);
                }
                let mut i = self.info_visible();
                if ui.checkbox(&mut i, "Image info panel").clicked() {
                    self.info_pref = Some(i);
                }
                ui.checkbox(&mut self.show_log, "Activity log").on_hover_text("Hidden automatically when the window is very short.");
                if ui
                    .button("Automatic layout")
                    .on_hover_text("Show or hide the side panels automatically depending on the window size.")
                    .clicked()
                {
                    self.tree_pref = None;
                    self.info_pref = None;
                    self.show_log = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Refresh drives    F5").clicked() {
                    self.refresh_drives();
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("About...").clicked() {
                    self.dialog = Some(Dialog::About);
                    ui.close();
                }
            });
        });
    }

    fn tool_btn(ui: &mut Ui, glyph: &str, label: &str, tip: &str, enabled: bool) -> bool {
        // 0 = full tile, 1 = compact tile, 2 = icon only (set by `toolbar` from the window width)
        let mode = TOOL_MODE.load(Ordering::Relaxed);
        let size = match mode {
            0 => egui::vec2(66.0, 48.0),
            1 => egui::vec2(54.0, 40.0),
            _ => egui::vec2(30.0, 28.0),
        };
        let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
        let hov = enabled && resp.hovered();
        let down = enabled && resp.is_pointer_button_down_on();
        let p = ui.painter();
        p.rect_filled(rect, 1.0, if down { Color32::from_rgb(0, 80, 56) } else if hov { Color32::from_rgb(8, 36, 30) } else { PANEL2 });
        p.rect_stroke(rect, 1.0, Stroke::new(1.0, if hov { GREEN } else { LINE }), StrokeKind::Inside);
        let c = if hov { CYAN } else { GREEN_DIM };
        let t = 5.0;
        p.line_segment([rect.left_top(), rect.left_top() + egui::vec2(t, 0.0)], Stroke::new(1.0, c));
        p.line_segment([rect.left_top(), rect.left_top() + egui::vec2(0.0, t)], Stroke::new(1.0, c));
        p.line_segment([rect.right_bottom(), rect.right_bottom() - egui::vec2(t, 0.0)], Stroke::new(1.0, c));
        p.line_segment([rect.right_bottom(), rect.right_bottom() - egui::vec2(0.0, t)], Stroke::new(1.0, c));
        let gc = if !enabled { DIM } else if hov { Color32::WHITE } else { GREEN };
        let lc = if enabled { if hov { CYAN } else { TEXT } } else { DIM };
        match mode {
            0 => {
                p.text(rect.center_top() + egui::vec2(0.0, 17.0), Align2::CENTER_CENTER, glyph, FontId::monospace(16.0), gc);
                p.text(rect.center_bottom() - egui::vec2(0.0, 11.0), Align2::CENTER_CENTER, label, FontId::monospace(10.0), lc);
            }
            1 => {
                p.text(rect.center_top() + egui::vec2(0.0, 14.0), Align2::CENTER_CENTER, glyph, FontId::monospace(14.0), gc);
                p.text(rect.center_bottom() - egui::vec2(0.0, 9.0), Align2::CENTER_CENTER, label, FontId::monospace(8.5), lc);
            }
            _ => {
                p.text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::monospace(11.5), gc);
            }
        }
        let clicked = resp.clicked();
        let (title, body, off) = split_tip(tip);
        resp.on_hover_ui(|ui| {
            ui.set_max_width(320.0);
            ui.label(RichText::new(title).color(GREEN).strong());
            ui.label(RichText::new(body).color(TEXT));
            if !enabled && !off.is_empty() {
                ui.label(RichText::new(format!("unavailable: {}", off)).color(AMBER));
            }
        });
        clicked
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        use crate::dialogs::{DrivePurpose, SumTarget, Then};
        let has = self.has_content();
        let mode = self.toolbar_mode();
        TOOL_MODE.store(mode, Ordering::Relaxed);
        ui.add_space(if mode == 2 { 2.0 } else { 4.0 });
        // wraps onto a second row when even the smallest buttons don't fit
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(if mode == 2 { 2.0 } else { 4.0 }, 3.0);
            let no_img = "open an image or add some files first";
            if Self::tool_btn(ui, "+#", "NEW", "New empty image  (Ctrl+N)|Starts a fresh, empty disc image in memory. Add files with ADD or drag them onto the window, then press SAVE to write it as an .iso file. Nothing is written to disk until you save.|", true) {
                self.confirm_discard(Then::New);
            }
            if Self::tool_btn(ui, "</", "OPEN", "Open an image  (Ctrl+O)|Opens a disc image file (ISO, BIN/CUE, NRG, MDF, IMG, VHD, CSO, HXZ, ZIP) so you can browse, extract or edit its files.|", true) {
                self.confirm_discard(Then::OpenDialog);
            }
            if Self::tool_btn(ui, ">>|", "SAVE", &format!("Save  (Ctrl+S)|{}|{}", match &self.doc.path {
                Some(p) if Self::savable(p) => format!("Writes your changes back to {}.", p.display()),
                _ => "Asks where to save, then writes the image as ISO, CSO, HXZ, ZIP or a floppy IMG.".into(),
            }, no_img), has) {
                self.save();
            }
            ui.add_space(if mode == 2 { 3.0 } else { 6.0 });
            if Self::tool_btn(ui, "+F", "ADD", &format!("Add files|Copies files from your computer into the folder shown in the list (/{}) of this image. You can also drag files and folders onto the window.|", self.cwd.join("/")), true) {
                self.add_files_dialog();
            }
            if Self::tool_btn(ui, "<-", "EXTRACT", &format!("Extract  (Ctrl+E)|Copies the selected items out of the image into a folder you choose. With nothing selected, extracts everything in /{}.|{}", self.cwd.join("/"), no_img), has) {
                self.extract(false);
            }
            if Self::tool_btn(ui, "-X", "DELETE", "Delete  (Del)|Removes the selected items from the image. The image file only changes when you SAVE.|select one or more items in the list first", !self.sel.is_empty()) {
                self.delete_selected();
            }
            if Self::tool_btn(ui, "+/", "FOLDER", &format!("New folder|Creates an empty folder inside /{} of the image.|", self.cwd.join("/")), true) {
                self.dialog = Some(Dialog::NewFolder { name: "NEW_FOLDER".into() });
            }
            ui.add_space(if mode == 2 { 3.0 } else { 6.0 });
            if Self::tool_btn(ui, "@:", "MOUNT", &format!("Mount as a drive|Makes Windows show this image as a DVD drive with its own letter, as if the disc were inserted. Eject it from the right-hand panel.|{}", no_img), has) {
                self.mount_current();
            }
            if Self::tool_btn(ui, "(*)", "BURN", &format!("Burn to disc|Writes this image onto a blank CD, DVD or Blu-ray using Windows' built-in disc burner.|{}", no_img), has) {
                self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Burn, kinds: vec![DriveKind::Optical] });
            }
            if Self::tool_btn(ui, "==", "COPY", "Copy a disc|Reads a disc into a temporary image, then burns it onto a blank disc.|", true) {
                self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Copy, kinds: vec![DriveKind::Optical] });
            }
            if Self::tool_btn(ui, "[o]", "MAKE", "Make image from disc|Reads a CD / DVD / Blu-ray or USB drive and saves it as an image file (ISO, CSO or HXZ).|", true) {
                self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::MakeImage, kinds: vec![DriveKind::Optical, DriveKind::Removable] });
            }
            ui.add_space(if mode == 2 { 3.0 } else { 6.0 });
            if Self::tool_btn(ui, "<=>", "CONVERT", "Convert format|Turns any supported image (BIN/CUE, NRG, MDF, CSO, HXZ...) into a standard ISO, or the other way round.|", true) {
                self.dialog = Some(Dialog::convert_default(self, OutFormat::Iso));
            }
            if Self::tool_btn(ui, "}{", "COMPRESS", "Compress / encrypt|Makes a smaller copy of an image (CSO or HXZ), optionally locked with a password.|", true) {
                self.dialog = Some(Dialog::convert_default(self, OutFormat::Hxz));
            }
            if Self::tool_btn(ui, "[=>", "USB", &format!("Bootable USB stick|Copies the open install image (Windows, Linux...) onto a USB stick so a PC can start from it.|{}", no_img), has) {
                self.dialog = Some(Dialog::Usb { drive: None, format: true, fs: "FAT32".into(), confirm: String::new() });
            }
            if Self::tool_btn(ui, "~~", "RIP", "Rip audio CD|Saves the tracks of a music CD as WAV files.|", true) {
                self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Rip, kinds: vec![DriveKind::Optical] });
            }
            if Self::tool_btn(ui, "#?", "SUM", &format!("Checksums|Computes CRC32, MD5, SHA-1 and SHA-256 fingerprints, to check a download or copy is intact.|{}", no_img), has) {
                self.dialog = Some(Dialog::Checksum { target: SumTarget::Image, result: None });
            }
            if Self::tool_btn(ui, "0x", "SECTORS", "Raw sector viewer|Shows the raw bytes of the image, 2048 bytes (one sector) at a time. For the curious; you don't need it for normal use.|open an existing image or drive first", self.doc.src.is_some()) {
                self.view = if self.view == View::Sectors { View::Files } else { View::Sectors };
            }
        });
        ui.add_space(4.0);
    }

    fn tree_panel(&mut self, ui: &mut Ui) {
        section(ui, "FOLDERS IN IMAGE");
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let label = if self.doc.path.is_none() && self.doc.src.is_none() { "new image".to_string() } else { self.doc_name() };
            let mut nav: Option<Vec<String>> = None;
            let mut toggle: Option<Vec<String>> = None;
            tree_node(ui, &self.doc.root, &label, &mut vec![], 0, &self.cwd, &self.expanded, &mut nav, &mut toggle);
            if let Some(t) = toggle
                && !self.expanded.remove(&t) {
                    self.expanded.insert(t);
                }
            if let Some(n) = nav {
                self.cwd = n.clone();
                self.expanded.insert(n);
                self.sel.clear();
            }
        });
    }

    fn props_panel(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "IMAGE");
            let (files, folders, size, _) = self.stats();
            let dirs = folders + 1;
            let mut pick: Option<String> = None;
            egui::Grid::new("img").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                kvt(ui, "file", &self.doc_name(), "The image you are working on. Its full location is shown above the file list.");
                kvt(ui, "format", &self.doc.format, "What kind of image file this is.");
                let r = ui.label(RichText::new("names from").color(DIM));
                r.on_hover_text("A disc can store the same files under several name tables (file systems). Pick which one to read the names from. It only changes how names are shown here, not the image.");
                let views = self.doc.fs_views.clone();
                if views.len() > 1 {
                    let cur = self.doc.fs_name.clone();
                    egui::ComboBox::from_id_salt("fsview")
                        .selected_text(cur.clone())
                        .show_ui(ui, |ui| {
                            for v in &views {
                                if ui.selectable_label(*v == cur, v.as_str()).on_hover_text(fs_help(v)).clicked() {
                                    pick = Some(v.clone());
                                }
                            }
                        })
                        .response
                        .on_hover_text(format!("{}\n\nThis disc also has: {}. Switch to see their names.", fs_help(&cur), views.iter().filter(|v| **v != cur).cloned().collect::<Vec<_>>().join(", ")));
                } else {
                    let h = fs_help(&self.doc.fs_name);
                    let r = ui.label(RichText::new(&self.doc.fs_name).color(CYAN));
                    if !h.is_empty() {
                        r.on_hover_text(h);
                    } else if self.doc.src.is_none() {
                        r.on_hover_text("New images are written with ISO 9660 + Joliet name tables, which every system can read.");
                    }
                }
                ui.end_row();
                kvt(ui, "disc name", &self.doc.label, "The volume label: the name Windows shows for the drive when the image is mounted or burned. Change it in File > Image properties.");
                kvt(ui, "content", &format!("{} files, {} folders", files, dirs.saturating_sub(1)), "");
                kvt(ui, "data size", &human(size), "Total size of all files inside the image.");
                if let Some(src) = &self.doc.src {
                    let n = src.sector_count();
                    kvt(ui, "image size", &human(n * SECTOR), &format!("Size of the disc image: {} sectors of 2048 bytes.", n));
                }
                kvt(ui, "bootable", &if self.doc.boot.is_empty() { "no".to_string() } else { format!("yes ({})", self.doc.boot.iter().map(|b| if b.platform == 0xEF { "UEFI" } else { "BIOS" }).collect::<Vec<_>>().join(" + ")) }, "Whether a PC can start from this disc. Change it in Action > Boot options.");
                for (k, v) in &self.doc.info {
                    kvt(ui, &k.to_lowercase(), v, info_help(k));
                }
                kvt(ui, "changes", if self.doc.modified { "UNSAVED" } else { "none" }, "Edits stay in memory until you press SAVE.");
            });
            if let Some(p) = pick
                && p != self.doc.fs_name {
                    self.switch_view(p);
                }
            if !self.doc.boot.is_empty() {
                section(ui, "BOOT");
                for b in &self.doc.boot {
                    ui.label(RichText::new(format!("{} {}", if b.platform == 0xEF { "UEFI" } else { "BIOS" }, b.label)).color(AMBER).small());
                }
            }
            section(ui, "SELECTION");
            let sel = self.selected_entries();
            if sel.is_empty() {
                ui.label(RichText::new("nothing selected").color(DIM));
            } else if sel.len() == 1 {
                let e = &sel[0];
                egui::Grid::new("sel").num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
                    kv(ui, "name", &e.name);
                    kv(ui, "type", &e.type_name());
                    kv(ui, "size", &format!("{} ({} bytes)", human(e.size()), e.size()));
                    kv(ui, "modified", &fmt_time(e.mtime));
                    if let Some(l) = e.lba {
                        kv(ui, "lba", &format!("{} (0x{:X})", l, l * SECTOR));
                    }
                    if let Kind::File { src, .. } = &e.kind {
                        let s = match src {
                            FileSrc::Image(x) => format!("image, {} extent(s)", x.len()),
                            FileSrc::Disk(p) => format!("disk: {}", p.display()),
                            FileSrc::Mem(_) => "embedded".into(),
                            FileSrc::Zip(_) => "zip member".into(),
                        };
                        kv(ui, "source", &s);
                    }
                });
                if let Some(l) = e.lba
                    && self.doc.src.is_some() && ui.button("view sectors >").clicked() {
                        self.hex_lba = l;
                        self.hex_input = l.to_string();
                        self.view = View::Sectors;
                    }
            } else {
                let total: u64 = sel.iter().map(|e| e.size()).sum();
                ui.label(format!("{} items, {}", sel.len(), human(total)));
            }
            section(ui, "VIRTUAL DRIVES");
            if self.mounts.is_empty() {
                ui.label(RichText::new("none mounted by hexiso").color(DIM));
            }
            let mut eject: Option<PathBuf> = None;
            for (p, l) in &self.mounts {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}:", l)).color(GREEN).strong());
                    ui.label(RichText::new(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).small());
                    if ui.small_button("eject").clicked() {
                        eject = Some(p.clone());
                    }
                });
            }
            if let Some(p) = eject {
                self.unmount(p);
            }
            section(ui, "DRIVES");
            if ui.small_button("refresh").clicked() {
                self.refresh_drives();
            }
            for d in &self.drives {
                let col = match d.kind {
                    DriveKind::Optical => CYAN,
                    DriveKind::Removable => AMBER,
                    _ => DIM,
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}:", d.letter)).color(col).strong());
                    ui.label(RichText::new(d.kind.name()).color(col).small());
                    let info = if d.ready { format!("{} {} {}", d.fs, if d.label.is_empty() { "-" } else { &d.label }, human(d.total)) } else { "no media".into() };
                    ui.label(RichText::new(info).small());
                });
                if d.ready && d.total > 0 {
                    let used = 1.0 - d.free as f32 / d.total as f32;
                    meter(ui, used, 4.0);
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let w = self.win_w;
        ui.horizontal(|ui| {
            let (files, folders, data, size) = self.stats();
            let summary = if w >= 760.0 { format!("{} files, {} folders | {}", files, folders, human(data)) } else { format!("{} files | {}", files, human(data)) };
            ui.label(RichText::new(summary).color(TEXT)).on_hover_text(format!("Everything inside the image: {} files, {} folders, {}.", files, folders, human(data)));
            ui.separator();
            let cap = self.doc.media.bytes();
            let frac = size as f32 / cap as f32;
            let col = if frac > 1.0 { RED } else if frac > 0.9 { AMBER } else { GREEN };
            if w < 600.0 {
                // tiny window: just the percentage, details on hover
                ui.label(RichText::new(format!("{:.0}% {}", frac * 100.0, self.doc.media.label())).color(col))
                    .on_hover_text(format!("This image would fill {:.0}% of a {} ({} of {}). Change the disc type in a wider window.", frac * 100.0, self.doc.media.label(), human(size), human(cap)));
            } else {
            if w >= 1000.0 {
                ui.label(RichText::new("disc fill").color(DIM));
            }
            let (rect, bar) = ui.allocate_exact_size(egui::vec2((w * 0.18).clamp(60.0, 260.0), 14.0), Sense::hover());
            bar.on_hover_text(format!("How much of a {} this image would use ({} of {}). Pick the disc type you plan to burn to on the right; red means it won't fit.", self.doc.media.label(), human(size), human(cap)));
            let p = ui.painter();
            p.rect_filled(rect, 0.0, BG);
            let mut f = rect;
            f.set_width(rect.width() * frac.min(1.0));
            p.rect_filled(f, 0.0, col.gamma_multiply(0.75));
            for i in 1..10 {
                let x = rect.left() + rect.width() * i as f32 / 10.0;
                p.line_segment([egui::pos2(x, rect.bottom() - 4.0), egui::pos2(x, rect.bottom())], Stroke::new(1.0, LINE));
            }
            p.rect_stroke(rect, 0.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
            ui.label(RichText::new(format!("{:.0}% of", frac * 100.0)).color(col));
            let mut m = self.doc.media;
            egui::ComboBox::from_id_salt("media")
                .selected_text(m.label())
                .width(if w >= 900.0 { 120.0 } else { 100.0 })
                .show_ui(ui, |ui| {
                    for x in crate::model::Media::ALL {
                        ui.selectable_value(&mut m, x, x.label());
                    }
                })
                .response
                .on_hover_text("The disc size used for the fill meter. It doesn't change the image.");
            self.doc.media = m;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fc = if self.frame_ms < 8.0 { GREEN_DIM } else if self.frame_ms < 16.0 { AMBER } else { RED };
                ui.label(RichText::new(format!("{:.1}ms", self.frame_ms)).color(fc).small())
                    .on_hover_text(format!("Time to build one frame of the window (lower is smoother).\nGraphics: {}", self.renderer));
                if self.doc.modified {
                    let t = if w >= 1100.0 { "[UNSAVED CHANGES]" } else { "[*]" };
                    ui.label(RichText::new(t).color(AMBER).strong()).on_hover_text("Unsaved changes: press SAVE (Ctrl+S) to write them.");
                }
                if let Some(j) = &self.job {
                    ui.label(RichText::new(format!("{} {:.0}%", j.title.to_lowercase(), j.prog.fraction() * 100.0)).color(CYAN));
                }
                // fills whatever room is left, shortened with "..." instead of overlapping
                if ui.available_width() > 60.0 {
                    let t = format!("{} :: {}", self.doc_name(), self.doc.format);
                    ui.add(egui::Label::new(RichText::new(&t).color(DIM)).truncate()).on_hover_text(t);
                }
            });
        });
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        let (n_warn, n_err) = self.logs.iter().fold((0, 0), |(w, e), x| match x.0 {
            Lvl::Warn => (w + 1, e),
            Lvl::Err => (w, e + 1),
            _ => (w, e),
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("// ACTIVITY LOG").color(GREEN).strong().small());
            ui.label(RichText::new(format!("{} events", self.logs.len())).color(DIM).small());
            if n_warn > 0 {
                ui.label(RichText::new(format!("{} warnings", n_warn)).color(AMBER).small());
            }
            if n_err > 0 {
                ui.label(RichText::new(format!("{} errors", n_err)).color(RED).small());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("clear").clicked() {
                    self.logs.clear();
                }
                if ui.small_button("copy").clicked() {
                    let text: Vec<String> = self.logs.iter().map(|(l, t, m)| format!("[{}] [{}] {}", t, lvl_tag(*l).0.trim(), m)).collect();
                    ui.ctx().copy_text(text.join("\n"));
                }
                ui.checkbox(&mut self.log_problems_only, RichText::new("problems only").small());
            });
        });
        let font = FontId::monospace(11.5);
        // only the rows on screen are laid out (keeps every frame cheap, also while the window moves)
        let rows: Vec<usize> = (0..self.logs.len()).filter(|&i| !(self.log_problems_only && matches!(self.logs[i].0, Lvl::Info | Lvl::Ok))).collect();
        let live = self.job.is_some() as usize;
        let row_h = 15.0;
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show_rows(ui, row_h, rows.len().max(1) + live, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if rows.is_empty() && range.start == 0 {
                ui.add_sized([ui.available_width(), row_h], egui::Label::new(RichText::new("nothing logged yet").color(DIM).small()));
            }
            for r in range.clone() {
                let Some(&i) = rows.get(r) else { continue };
                let (l, t, m) = &self.logs[i];
                let (tag, c) = lvl_tag(*l);
                let mut job = egui::text::LayoutJob::default();
                let fmt = |col: Color32| egui::TextFormat { font_id: font.clone(), color: col, ..Default::default() };
                job.append(&format!("{}  ", t), 0.0, fmt(GREEN_DIM));
                job.append(&format!("{}  ", tag), 0.0, fmt(c));
                job.append(m, 0.0, fmt(if *l == Lvl::Info { TEXT } else { c }));
                log_row(ui, row_h, job).on_hover_text(m.as_str());
            }
            // live line for the operation that is running right now
            if let (Some(j), true) = (&self.job, range.end > rows.len().max(1)) {
                let st = j.prog.status.lock().unwrap().clone();
                let pct = if j.prog.total() > 0 { format!("{:.0}%", j.prog.fraction() * 100.0) } else { "...".into() };
                let spin = ["|", "/", "-", "\\"][(self.frame as usize / 6) % 4];
                let mut job = egui::text::LayoutJob::default();
                let fmt = |col: Color32| egui::TextFormat { font_id: font.clone(), color: col, ..Default::default() };
                job.append(&format!("{}  ", now_hms()), 0.0, fmt(GREEN_DIM));
                job.append(&format!(" {}    ", spin), 0.0, fmt(CYAN));
                job.append(&format!("{} {}{}", j.title.to_lowercase(), pct, if st.is_empty() { String::new() } else { format!("  > {}", st) }), 0.0, fmt(CYAN));
                log_row(ui, row_h, job);
            }
        });
    }

    // ------------------------------------------------------------------ responsive layout

    /// Folder tree: shown automatically from 820 points wide.
    pub fn tree_visible(&self) -> bool {
        self.tree_pref.unwrap_or(self.win_w >= 820.0)
    }
    /// Image info panel: shown automatically from 1150 points wide.
    pub fn info_visible(&self) -> bool {
        self.info_pref.unwrap_or(self.win_w >= 1150.0)
    }
    /// Activity log: hidden automatically on short windows.
    pub fn log_visible(&self) -> bool {
        self.show_log && self.win_h >= 460.0
    }
    /// Toolbar size: 0 = full tiles, 1 = compact tiles, 2 = icons only.
    pub fn toolbar_mode(&self) -> u8 {
        // 18 buttons: full tiles need ~1250 points, compact ~1090, icons ~600
        if self.win_w >= 1280.0 {
            0
        } else if self.win_w >= 1110.0 {
            1
        } else {
            2
        }
    }

    /// Small toggle buttons for the side panels (shown in the bar above the file list).
    fn panel_toggles(&mut self, ui: &mut Ui) {
        let (t, i) = (self.tree_visible(), self.info_visible());
        if ui
            .selectable_label(t, "[=]")
            .on_hover_text(if t { "Hide the folder tree (more room for the file list)." } else { "Show the folder tree." })
            .clicked()
        {
            self.tree_pref = Some(!t);
        }
        if ui
            .selectable_label(i, "[i]")
            .on_hover_text(if i { "Hide the image info panel." } else { "Show the image info panel (format, names, disc name, boot, drives)." })
            .clicked()
        {
            self.info_pref = Some(!i);
        }
    }

    /// Short human name of what is open.
    pub fn doc_name(&self) -> String {
        match (&self.doc.path, &self.doc.src) {
            (Some(p), _) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            (None, Some(_)) => self.doc.format.clone(),
            (None, None) => "new image (not saved yet)".into(),
        }
    }

    fn doc_header(&mut self, ui: &mut Ui) {
        // short windows get a one-line header (full location on hover) to leave room for the list
        let compact = self.win_h < 520.0;
        let frame = egui::Frame::new().fill(PANEL2).stroke(Stroke::new(1.0, LINE)).inner_margin(if compact { egui::Margin::symmetric(8, 3) } else { egui::Margin::symmetric(10, 6) });
        let w = ui.available_width() - 22.0;
        frame.show(ui, |ui| {
            ui.set_width(w);
            ui.set_max_width(w);
            let has = self.has_content() || self.doc.path.is_some();
            if !self.started && !has {
                if compact {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("NO IMAGE OPEN").color(DIM).strong());
                        ui.add(egui::Label::new(RichText::new("open an image or start a new one").color(DIM).small()).truncate());
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("NO IMAGE OPEN").color(DIM).strong());
                        ui.label(RichText::new("open an image file, read a drive, or start a new image and drop files in").color(DIM).small());
                    });
                }
                return;
            }
            if compact {
                let (kind, kc) = match (&self.doc.path, &self.doc.src) {
                    (Some(_), _) => ("IMAGE", GREEN),
                    (None, Some(_)) => ("DRIVE", CYAN),
                    (None, None) => ("NEW", AMBER),
                };
                let path = self.doc.path.clone();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(kind).color(BG).background_color(kc).strong());
                    if self.doc.modified {
                        ui.label(RichText::new("*").color(AMBER).strong()).on_hover_text("Unsaved changes: press SAVE (Ctrl+S).");
                    }
                    if let Some(p) = &path
                        && ui.small_button("show").on_hover_text("Open Windows Explorer with this file selected.").clicked()
                    {
                        let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
                    }
                    let name = self.doc_name();
                    let tip = match &path {
                        Some(p) => format!("{}\n{}", name, p.display()),
                        None => format!("{}\nOnly in memory: add files, then press SAVE.", name),
                    };
                    ui.add(egui::Label::new(RichText::new(&name).color(Color32::WHITE).strong()).truncate()).on_hover_text(tip);
                });
                return;
            }
            let (kind, kc) = match (&self.doc.path, &self.doc.src) {
                (Some(_), _) => ("OPEN IMAGE", GREEN),
                (None, Some(_)) => ("READING DRIVE", CYAN),
                (None, None) => ("NEW IMAGE", AMBER),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind).color(BG).background_color(kc).strong()).on_hover_text(match kind {
                    "OPEN IMAGE" => "You are browsing / editing an image file that exists on your computer.",
                    "READING DRIVE" => "You are browsing a disc or drive directly. Use MAKE to save it as an image file.",
                    _ => "A new image that only exists in memory. Add files, then press SAVE to create the .iso file.",
                });
                if self.doc.modified {
                    ui.label(RichText::new(" UNSAVED ").color(BG).background_color(AMBER).small()).on_hover_text("You changed the file list. Press SAVE (Ctrl+S) to write the changes to disk.");
                } else if self.doc.path.is_some() {
                    ui.label(RichText::new("saved").color(GREEN_DIM).small()).on_hover_text("The file on disk matches what you see.");
                }
                let name = self.doc_name();
                ui.add(egui::Label::new(RichText::new(&name).color(Color32::WHITE).strong().size(15.0)).truncate()).on_hover_text(name);
            });
            match self.doc.path.clone() {
                Some(p) => {
                    let narrow = w < 560.0;
                    ui.horizontal(|ui| {
                        if !narrow {
                            ui.label(RichText::new("location").color(DIM).small());
                        }
                        if ui.small_button(if narrow { "show" } else { "show in folder" }).on_hover_text("Open Windows Explorer with this file selected.").clicked() {
                            let _ = std::process::Command::new("explorer").arg(format!("/select,{}", p.display())).spawn();
                        }
                        if ui.small_button(if narrow { "copy" } else { "copy path" }).on_hover_text("Copy the full location to the clipboard.").clicked() {
                            ui.ctx().copy_text(p.display().to_string());
                            self.log(Lvl::Info, "path copied to the clipboard");
                        }
                        let full = p.display().to_string();
                        ui.add(egui::Label::new(RichText::new(&full).color(CYAN).small()).truncate()).on_hover_text(full);
                    });
                }
                None if self.doc.src.is_none() => {
                    ui.add(egui::Label::new(RichText::new("Only in memory so far. Add files with ADD or drag & drop, then press SAVE to choose where the .iso file goes.").color(DIM).small()).wrap());
                }
                None => {
                    ui.add(egui::Label::new(RichText::new("Read directly from the drive. Press MAKE to save it as an image file.").color(DIM).small()).wrap());
                }
            }
        });
        ui.add_space(4.0);
    }

    fn central(&mut self, ui: &mut Ui) {
        self.doc_header(ui);
        let cw = ui.available_width();
        let narrow = cw < 620.0;
        // flows onto a second line instead of running off the edge
        ui.horizontal_wrapped(|ui| {
            self.panel_toggles(ui);
            ui.separator();
            if ui.selectable_label(self.view == View::Files, if narrow { "FILES" } else { " FILES " }).on_hover_text("The files and folders inside the image.").clicked() {
                self.view = View::Files;
            }
            if ui.selectable_label(self.view == View::Sectors, if narrow { "HEX" } else { " SECTORS " }).on_hover_text("Raw bytes of the image, sector by sector (advanced).").clicked() {
                self.view = View::Sectors;
            }
            ui.separator();
            if self.view == View::Files {
                if ui.add_enabled(!self.cwd.is_empty(), egui::Button::new("^ up")).on_hover_text("Go to the parent folder (Backspace).").clicked() {
                    self.cwd.pop();
                    self.sel.clear();
                }
                if !narrow {
                    ui.label(RichText::new("inside image:").color(DIM)).on_hover_text("The folder of the image you are looking at. Click a part to jump back to it.");
                }
                if ui.link(RichText::new("/ (top)").color(CYAN)).on_hover_text("Top folder of the image.").clicked() {
                    self.cwd.clear();
                    self.sel.clear();
                }
                let mut goto: Option<usize> = None;
                for (i, part) in self.cwd.iter().enumerate() {
                    ui.label(RichText::new("/").color(DIM));
                    if ui.link(RichText::new(part).color(CYAN)).clicked() {
                        goto = Some(i + 1);
                    }
                }
                if let Some(n) = goto {
                    self.cwd.truncate(n);
                    self.sel.clear();
                }
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter...").desired_width((cw * 0.22).clamp(70.0, 160.0)))
                    .on_hover_text("Show only the items in this folder whose name contains this text. Use Action > Search to look in every folder.");
            }
        });
        ui.add_space(2.0);
        match self.view {
            View::Files => {
                let empty = self.doc.src.is_none() && self.doc.root.children().map(|c| c.is_empty()).unwrap_or(true) && self.doc.path.is_none();
                if empty && self.started {
                    self.new_image_hint(ui);
                } else if empty {
                    self.welcome(ui);
                } else {
                    self.file_table(ui);
                }
            }
            View::Sectors => self.sector_view(ui),
        }
    }

    fn new_image_hint(&mut self, ui: &mut Ui) {
        ui.add_space(if self.win_h < 500.0 { 10.0 } else { 40.0 });
        let w = (ui.available_width() - 8.0).clamp(160.0, 620.0);
        ui.vertical_centered(|ui| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 190.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 2.0, if resp.hovered() { Color32::from_rgb(6, 26, 22) } else { PANEL });
            // dashed border
            let dash = 8.0;
            let st = Stroke::new(1.0, if resp.hovered() { GREEN } else { GREEN_DIM });
            let mut x = rect.left();
            while x < rect.right() {
                let x2 = (x + dash).min(rect.right());
                p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x2, rect.top())], st);
                p.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x2, rect.bottom())], st);
                x += dash * 2.0;
            }
            let mut y = rect.top();
            while y < rect.bottom() {
                let y2 = (y + dash).min(rect.bottom());
                p.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.left(), y2)], st);
                p.line_segment([egui::pos2(rect.right(), y), egui::pos2(rect.right(), y2)], st);
                y += dash * 2.0;
            }
            let c = rect.center();
            let p = p.with_clip_rect(rect.shrink(2.0));
            let small = w < 520.0;
            let (f1, f2, f3) = if small { (13.5, 10.5, 9.5) } else { (16.0, 12.5, 11.0) };
            let lines: [&str; 3] = if small {
                ["1. drag files here (or click)", "2. press SAVE -> .iso file", "3. MOUNT, BURN or USB"]
            } else {
                [
                    "1. drag files or folders here  (or click to pick files)",
                    "2. press SAVE to write them into an .iso file",
                    "3. then MOUNT it, BURN it to a disc or put it on a USB stick",
                ]
            };
            p.text(c - egui::vec2(0.0, 52.0), Align2::CENTER_CENTER, "THIS NEW IMAGE IS EMPTY", FontId::monospace(f1), GREEN);
            for (i, l) in lines.iter().enumerate() {
                p.text(c + egui::vec2(0.0, -18.0 + 24.0 * i as f32), Align2::CENTER_CENTER, *l, FontId::monospace(f2), TEXT);
            }
            p.text(c + egui::vec2(0.0, 64.0), Align2::CENTER_CENTER, if small { "nothing is written until you save" } else { "nothing is written to your disk until you save" }, FontId::monospace(f3), DIM);
            if resp.clicked() {
                self.add_files_dialog();
            }
        });
    }

    fn welcome(&mut self, ui: &mut Ui) {
        use crate::dialogs::{DrivePurpose, Then};
        let logo = [
            "██╗  ██╗███████╗██╗  ██╗██╗███████╗ ██████╗ ",
            "██║  ██║██╔════╝╚██╗██╔╝██║██╔════╝██╔═══██╗",
            "███████║█████╗   ╚███╔╝ ██║███████╗██║   ██║",
            "██╔══██║██╔══╝   ██╔██╗ ██║╚════██║██║   ██║",
            "██║  ██║███████╗██╔╝ ██╗██║███████║╚██████╔╝",
            "╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═╝╚══════╝ ╚═════╝ ",
        ];
        let short = self.win_h < 560.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(if short { 8.0 } else { 24.0 });
            let _ = logo;
            pixel_logo(ui);
            ui.vertical_centered(|ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("disc image toolkit  ::  open / edit / extract / build / burn / mount / convert").color(DIM));
                ui.add_space(if short { 10.0 } else { 22.0 });
            });
            let w = ui.available_width();
            let cols = if w > 900.0 {
                3
            } else if w > 470.0 {
                2
            } else {
                1
            };
            let tiles: [(&str, &str, &str); 6] = [
                ("OPEN IMAGE", "ISO BIN/CUE NRG MDF CSO HXZ IMG VHD ZIP", "open"),
                ("BUILD NEW IMAGE", "drop files & folders here, then SAVE", "new"),
                ("READ A DRIVE", "browse a CD / DVD / BD or USB drive", "drive"),
                ("DISC -> IMAGE", "rip a disc to ISO, CSO or HXZ", "make"),
                ("BOOTABLE USB", "write an install ISO onto a stick", "usb"),
                ("AUDIO CD -> WAV", "rip audio tracks losslessly", "rip"),
            ];
            let gap = if w < 600.0 { 8.0 } else { 12.0 };
            let tw = ((w - 24.0) / cols as f32 - gap).clamp(150.0, 330.0);
            let th = if short || tw < 230.0 { 58.0 } else { 74.0 };
            let total_w = cols as f32 * (tw + gap);
            for row in tiles.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.add_space(((w - total_w) / 2.0).max(0.0));
                    for (title, sub, id) in row {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(tw, th), Sense::click());
                        let resp = resp.on_hover_text(*sub);
                        let hov = resp.hovered();
                        let p = ui.painter().with_clip_rect(rect);
                        p.rect_filled(rect, 2.0, if hov { Color32::from_rgb(6, 30, 26) } else { PANEL2 });
                        p.rect_stroke(rect, 2.0, Stroke::new(1.0, if hov { GREEN } else { LINE }), StrokeKind::Inside);
                        let (ft, fs) = if tw < 230.0 { (13.0, 9.5) } else { (15.0, 11.0) };
                        p.text(rect.left_top() + egui::vec2(12.0, th * 0.3), Align2::LEFT_CENTER, format!("> {}", title), FontId::monospace(ft), if hov { Color32::WHITE } else { GREEN });
                        p.text(rect.left_top() + egui::vec2(12.0, th * 0.68), Align2::LEFT_CENTER, *sub, FontId::monospace(fs), DIM);
                        if resp.clicked() {
                            match *id {
                                "open" => self.confirm_discard(Then::OpenDialog),
                                "new" => self.add_files_dialog(),
                                "drive" => self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Open, kinds: vec![DriveKind::Optical, DriveKind::Removable, DriveKind::Fixed] }),
                                "make" => self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::MakeImage, kinds: vec![DriveKind::Optical, DriveKind::Removable] }),
                                "usb" => self.log(Lvl::Warn, "open the ISO you want on the stick first, then press USB"),
                                _ => self.dialog = Some(Dialog::DrivePick { purpose: DrivePurpose::Rip, kinds: vec![DriveKind::Optical] }),
                            }
                        }
                        ui.add_space(gap);
                    }
                });
                ui.add_space(gap);
            }
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("reads  ISO9660 · Joliet · Rock Ridge · UDF 1.02-2.60 · El Torito · FAT12/16/32 · MBR/GPT").color(GREEN_DIM).small());
                ui.label(RichText::new("writes ISO (+Joliet, BIOS/UEFI boot, >4GB files) · CSO · HXZ (deflate + ChaCha20) · ZIP · FAT12 floppy").color(GREEN_DIM).small());
            });
        });
    }

    fn file_table(&mut self, ui: &mut Ui) {
        let rows = self.rows();
        let row_h = 21.0;
        let full = ui.available_width();
        // columns drop out as the list gets narrower: LBA, then date, then type (name + size stay)
        let base = [if full < 480.0 { 84.0f32 } else { 110.0 }, 120.0, 140.0, 90.0];
        let show = [true, full >= 600.0, full >= 720.0, full >= 840.0];
        let cw: [f32; 4] = std::array::from_fn(|i| if show[i] { base[i] } else { 0.0 });
        let name_w = (full - cw.iter().sum::<f32>() - 16.0).max(100.0);
        let xs = [0.0, name_w, name_w + cw[0], name_w + cw[0] + cw[1], name_w + cw[0] + cw[1] + cw[2]];
        let (hrect, _) = ui.allocate_exact_size(egui::vec2(full, 22.0), Sense::hover());
        ui.painter().rect_filled(hrect, 0.0, PANEL2);
        ui.painter().line_segment([hrect.left_bottom(), hrect.right_bottom()], Stroke::new(1.0, GREEN_DIM));
        let heads = [("NAME", SortKey::Name), ("SIZE", SortKey::Size), ("TYPE", SortKey::Type), ("MODIFIED", SortKey::Date), ("LBA", SortKey::Lba)];
        for (i, (h, k)) in heads.iter().enumerate() {
            let w = if i == 0 { name_w } else { cw[i - 1] };
            if w <= 0.0 {
                continue;
            }
            let r = egui::Rect::from_min_size(hrect.min + egui::vec2(xs[i], 0.0), egui::vec2(w, 22.0));
            let resp = ui.interact(r, ui.scope_id().with(("hdr", i)), Sense::click());
            let resp = resp.on_hover_text(match k {
                SortKey::Name => "File or folder name. Click a column title to sort; click again to reverse.",
                SortKey::Size => "Size of the file (for folders: everything inside).",
                SortKey::Type => "Kind of file, from its extension.",
                SortKey::Date => "Last modified date stored in the image.",
                SortKey::Lba => "Where the file starts inside the image (sector number). 'new' means it was added and is not saved into the image yet.",
            });
            let arrow = if self.sort == *k { if self.sort_desc { " v" } else { " ^" } } else { "" };
            ui.painter().text(r.left_center() + egui::vec2(8.0, 0.0), Align2::LEFT_CENTER, format!("{}{}", h, arrow), FontId::monospace(11.5), if resp.hovered() || self.sort == *k { CYAN } else { GREEN_DIM });
            if resp.clicked() {
                if self.sort == *k {
                    self.sort_desc = !self.sort_desc;
                } else {
                    self.sort = *k;
                    self.sort_desc = false;
                }
            }
        }
        if rows.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(if self.filter.is_empty() { "-- empty folder --  drop files here or press ADD" } else { "-- no match --" }).color(DIM));
            });
            return;
        }
        let mut click: Option<(String, bool, bool)> = None;
        let mut dbl: Option<String> = None;
        let mut ctx_action: Option<(&'static str, String)> = None;
        let sel = self.sel.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let r = &rows[i];
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(full, row_h), Sense::click());
                let selected = sel.contains(&r.name);
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 0.0, SEL);
                    p.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(3.0, row_h)), 0.0, GREEN);
                } else if resp.hovered() {
                    p.rect_filled(rect, 0.0, Color32::from_rgb(10, 28, 24));
                } else if i % 2 == 1 {
                    p.rect_filled(rect, 0.0, ROW_ALT);
                }
                let y = rect.center().y;
                let nc = if r.hidden { DIM } else if r.is_dir { CYAN } else { TEXT };
                let name = if r.is_dir { format!("{}/", r.name) } else { r.name.clone() };
                let max_chars = ((name_w - 30.0) / 7.8) as usize;
                let shown = if name.chars().count() > max_chars && max_chars > 3 { format!("{}~", name.chars().take(max_chars - 1).collect::<String>()) } else { name };
                p.text(egui::pos2(rect.left() + 10.0, y), Align2::LEFT_CENTER, if r.is_dir { "d" } else { "-" }, FontId::monospace(12.0), GREEN_DIM);
                p.text(egui::pos2(rect.left() + 24.0, y), Align2::LEFT_CENTER, shown, FontId::monospace(13.0), nc);
                let f = FontId::monospace(12.0);
                p.text(egui::pos2(rect.left() + xs[1] + cw[0] - 12.0, y), Align2::RIGHT_CENTER, human(r.size), f.clone(), if r.is_dir { DIM } else { GREEN });
                if show[1] {
                    p.text(egui::pos2(rect.left() + xs[2] + 8.0, y), Align2::LEFT_CENTER, &r.kind, f.clone(), DIM);
                }
                if show[2] {
                    p.text(egui::pos2(rect.left() + xs[3] + 8.0, y), Align2::LEFT_CENTER, fmt_time(r.mtime), f.clone(), DIM);
                }
                if show[3] {
                    p.text(egui::pos2(rect.left() + xs[4] + 8.0, y), Align2::LEFT_CENTER, r.lba.map(|l| l.to_string()).unwrap_or_else(|| "new".into()), f, if r.lba.is_some() { GREEN_DIM } else { AMBER });
                }
                if resp.double_clicked() {
                    dbl = Some(r.name.clone());
                } else if resp.clicked() {
                    let m = ui.input(|i| i.modifiers);
                    click = Some((r.name.clone(), m.command, m.shift));
                }
                if resp.secondary_clicked() && !selected {
                    click = Some((r.name.clone(), false, false));
                }
                resp.context_menu(|ui| {
                    for (label, act) in [("Open", "open"), ("Extract to...", "extract"), ("Rename", "rename"), ("Delete", "delete"), ("Checksums", "sum"), ("View sectors", "hex"), ("Use as BIOS boot image", "bootbios"), ("Use as UEFI boot image", "bootefi")] {
                        if ui.button(label).clicked() {
                            ctx_action = Some((act, r.name.clone()));
                            ui.close();
                        }
                    }
                });
            }
        });
        if let Some((name, ctrl, shift)) = click {
            if ctrl {
                if !self.sel.remove(&name) {
                    self.sel.insert(name.clone());
                }
                self.anchor = Some(name);
            } else if shift && self.anchor.is_some() {
                let a = rows.iter().position(|r| Some(&r.name) == self.anchor.as_ref()).unwrap_or(0);
                let b = rows.iter().position(|r| r.name == name).unwrap_or(0);
                let (lo, hi) = (a.min(b), a.max(b));
                self.sel = rows[lo..=hi].iter().map(|r| r.name.clone()).collect();
            } else {
                self.sel.clear();
                self.sel.insert(name.clone());
                self.anchor = Some(name);
            }
        }
        if let Some(n) = dbl {
            self.activate(&n);
        }
        if let Some((act, name)) = ctx_action {
            match act {
                "open" => self.activate(&name),
                "extract" => self.extract(false),
                "rename" => self.dialog = Some(Dialog::Rename { old: name.clone(), new: name }),
                "delete" => self.delete_selected(),
                "sum" => {
                    let mut p = self.cwd.clone();
                    p.push(name);
                    self.dialog = Some(Dialog::Checksum { target: crate::dialogs::SumTarget::File(p), result: None });
                }
                "hex" => {
                    if let Some(l) = rows.iter().find(|r| r.name == name).and_then(|r| r.lba) {
                        self.hex_lba = l;
                        self.hex_input = l.to_string();
                        self.view = View::Sectors;
                    }
                }
                "bootbios" | "bootefi" => {
                    let mut p = self.cwd.clone();
                    p.push(name);
                    self.add_boot_from_tree(p, act == "bootefi");
                }
                _ => {}
            }
        }
    }

    pub fn add_boot_from_tree(&mut self, path: Vec<String>, efi: bool) {
        let Some(e) = self.doc.find(&path).cloned() else { return };
        let Kind::File { size, src } = e.kind else {
            self.log(Lvl::Err, "a boot image must be a file");
            return;
        };
        let platform = if efi { 0xEF } else { 0 };
        self.doc.boot.retain(|b| b.platform != platform);
        let info_table = !efi && e.name.to_lowercase().contains("isolinux");
        self.doc.boot.push(crate::model::BootSpec { platform, emulation: 0, load_sectors: if efi { 0 } else { 4 }, info_table, size, src, tree_path: Some(path.clone()), label: path.join("/") });
        self.doc.boot.sort_by_key(|b| b.platform);
        self.log(Lvl::Ok, format!("{} boot image set to /{}", if efi { "UEFI" } else { "BIOS" }, path.join("/")));
        self.touch();
    }

    fn sector_view(&mut self, ui: &mut Ui) {
        let Some(src) = self.doc.src.clone() else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("-- no image source: open an image or a drive to inspect sectors --").color(DIM)));
            return;
        };
        let n = src.sector_count().max(1);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("LBA").color(DIM));
            let r = ui.add(egui::TextEdit::singleline(&mut self.hex_input).desired_width(110.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("go").clicked() || enter {
                let t = self.hex_input.trim().to_lowercase();
                let v = if let Some(h) = t.strip_prefix("0x") { u64::from_str_radix(h, 16).ok() } else { t.parse().ok() };
                if let Some(v) = v {
                    self.hex_lba = v.min(n - 1);
                }
            }
            for (l, d) in [("|<", i64::MIN), ("<<", -16), ("<", -1), (">", 1), (">>", 16), (">|", i64::MAX)] {
                if ui.button(l).clicked() {
                    self.hex_lba = match d {
                        i64::MIN => 0,
                        i64::MAX => n - 1,
                        _ => (self.hex_lba as i64 + d).clamp(0, n as i64 - 1) as u64,
                    };
                }
            }
            for (l, v) in [("PVD", 16u64), ("UDF anchor", 256)] {
                if ui.small_button(l).clicked() {
                    self.hex_lba = v.min(n - 1);
                }
            }
            ui.label(RichText::new(format!("sector {} / {}   offset 0x{:X}", self.hex_lba, n, self.hex_lba * SECTOR)).color(CYAN));
        });
        if self.hex_cache.as_ref().map(|c| c.0) != Some(self.hex_lba) {
            let mut b = vec![0u8; SECTOR as usize];
            let _ = src.read_sectors(self.hex_lba, &mut b);
            self.hex_cache = Some((self.hex_lba, b));
            self.hex_input = self.hex_lba.to_string();
        }
        let data = self.hex_cache.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        ui.separator();
        let base = self.hex_lba * SECTOR;
        // scrolls sideways too when the window is narrower than a 16-byte hex row
        egui::ScrollArea::both().auto_shrink([false, false]).show_rows(ui, 17.0, 128, |ui, range| {
            for row in range {
                let off = row * 16;
                let chunk = &data[off..off + 16];
                let mut job = egui::text::LayoutJob::default();
                let f = FontId::monospace(13.0);
                let fmt = |c: Color32| egui::TextFormat { font_id: f.clone(), color: c, ..Default::default() };
                job.append(&format!("{:010X}  ", base + off as u64), 0.0, fmt(GREEN_DIM));
                for (i, b) in chunk.iter().enumerate() {
                    let c = if *b == 0 { Color32::from_rgb(40, 60, 54) } else if b.is_ascii_graphic() { GREEN } else { CYAN };
                    job.append(&format!("{:02X}{}", b, if i == 7 { "  " } else { " " }), 0.0, fmt(c));
                }
                job.append(" |", 0.0, fmt(DIM));
                let ascii: String = chunk.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }).collect();
                job.append(&ascii, 0.0, fmt(TEXT));
                job.append("|", 0.0, fmt(DIM));
                ui.label(job);
            }
        });
    }

    fn draw_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let prog = job.prog.clone();
        let title = job.title.clone();
        let el = job.started.elapsed().as_secs_f64();
        let frame = self.frame;
        let win_w = self.win_w;
        egui::Modal::new(egui::Id::unique("hexiso-job")).show(ctx, |ui| {
            ui.set_width((win_w - 60.0).clamp(240.0, 520.0));
            ui.label(RichText::new(format!(">> {}", title.to_uppercase())).color(GREEN).strong());
            ui.add_space(6.0);
            let f = prog.fraction();
            let n = if win_w < 520.0 { 18usize } else { 40 };
            let bar: String = if prog.total() > 0 {
                let fill = ((f * n as f32) as usize).min(n);
                format!("[{}{}] {:5.1}%", "#".repeat(fill), ".".repeat(n - fill), f * 100.0)
            } else {
                let pos = (frame as usize / 4) % n;
                let mut s: Vec<char> = ".".repeat(n).chars().collect();
                for k in 0..4 {
                    s[(pos + k) % n] = '#';
                }
                format!("[{}] working", s.into_iter().collect::<String>())
            };
            ui.label(RichText::new(bar).color(CYAN).font(FontId::monospace(14.0)));
            let done = prog.done();
            let speed = if el > 0.2 { done as f64 / el } else { 0.0 };
            let eta = if speed > 0.0 && prog.total() > done { (prog.total() - done) as f64 / speed } else { 0.0 };
            ui.label(RichText::new(format!("{} / {}   {}/s   elapsed {:.0}s   eta {:.0}s", human(done), human(prog.total()), human(speed as u64), el, eta)).color(DIM).small());
            let st = prog.status.lock().unwrap().clone();
            if !st.is_empty() {
                ui.label(RichText::new(format!("> {}", st)).color(TEXT).small());
            }
            ui.add_space(6.0);
            if ui.button("[ cancel ]").clicked() {
                prog.cancel();
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(path) = self.shot.clone() else { return };
        ctx.request_repaint();
        // the screenshot window sits behind other windows; keep painting even when fully covered
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        let st = self.shot_state.load(Ordering::SeqCst);
        if st == 0 && self.frame >= 120 {
            self.shot_state.store(1, Ordering::SeqCst);
            let flag = self.shot_state.clone();
            let c2 = ctx.clone();
            ctx.request_screenshot(move |img| {
                let w = img.size[0] as u32;
                let h = img.size[1] as u32;
                let mut data = Vec::with_capacity((w * h * 4) as usize);
                for px in &img.pixels {
                    data.extend_from_slice(&px.to_srgba_unmultiplied());
                }
                let ok = (|| -> Result<(), Box<dyn std::error::Error>> {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let f = std::fs::File::create(&path)?;
                    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                    enc.set_color(png::ColorType::Rgba);
                    enc.set_depth(png::BitDepth::Eight);
                    let mut wr = enc.write_header()?;
                    wr.write_image_data(&data)?;
                    Ok(())
                })();
                if let Err(e) = ok {
                    eprintln!("screenshot failed: {}", e);
                }
                flag.store(2, Ordering::SeqCst);
                c2.request_repaint();
            });
        }
        if st == 2 || self.frame > 1200 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for HexApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let cpu = frame.info().cpu_usage;
        if self.renderer.is_empty() {
            let name = frame
                .wgpu_render_state()
                .map(|r| {
                    let i = r.adapter.get_info();
                    format!("GPU: {} ({:?})", i.name, i.backend)
                })
                .unwrap_or_else(|| "GPU: unknown".into());
            self.set_renderer(name);
        }
        self.draw(ui, cpu);
    }
}

impl HexApp {
    pub fn set_renderer(&mut self, name: String) {
        let l = name.to_lowercase();
        let slow = l.contains("basic render") || l.contains("llvmpipe") || l.contains("warp");
        self.log(
            if slow { Lvl::Warn } else { Lvl::Info },
            format!("graphics: {}{}", name, if slow { " - software GPU driver; start with HEXISO_RENDERER=cpu for the faster built-in CPU renderer" } else { "" }),
        );
        self.renderer = name;
    }

    /// One frame of the whole window. `cpu` = seconds the previous frame took, when known.
    pub fn draw(&mut self, ui: &mut Ui, cpu: Option<f32>) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        if let Some(ms) = cpu {
            let ms = ms * 1000.0;
            self.frame_ms = if self.frame_ms == 0.0 { ms } else { self.frame_ms * 0.9 + ms * 0.1 };
        }
        if self.debug_repaint {
            eprintln!("frame {} t={:?} causes: {:?}", self.frame, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100000).unwrap_or(0), ctx.repaint_causes());
        }
        // HEXISO_BENCH=1: redraw continuously for 5 s, print frames per second, exit
        if std::env::var_os("HEXISO_BENCH").is_some() {
            let t = self.bench_start.get_or_insert_with(Instant::now).elapsed().as_secs_f64();
            if t > 5.0 {
                eprintln!("bench: {} frames in {:.1}s = {:.1} fps, egui cpu {:.2} ms/frame, {}", self.frame, t, self.frame as f64 / t, self.frame_ms, self.renderer);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
            // measure real painting even if the window happens to be covered
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
        self.poll_job();
        self.poll_drives();
        self.handle_drop(&ctx);
        self.shortcuts(&ctx);
        // background re-check of drives (never blocks this frame)
        if self.drives_at.map(|t| t.elapsed().as_secs() > 15).unwrap_or(true) && self.job.is_none() {
            self.refresh_drives();
        }
        let title = format!("HEXISO // {}{}", self.doc.title(), if self.doc.modified { " *" } else { "" });
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        let bar = egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(8, 3));
        // responsive layout: measure the window, then decide what fits
        let full = ui.max_rect();
        self.win_w = full.width();
        self.win_h = full.height();
        let (w, h) = (self.win_w, self.win_h);
        egui::Panel::top("menu").frame(bar).show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(8, 2)).stroke(Stroke::new(1.0, LINE))).show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(8, 4)).stroke(Stroke::new(1.0, LINE))).show(ui, |ui| self.status_bar(ui));
        if self.log_visible() {
            let log_h = (h * 0.17).clamp(60.0, 120.0);
            egui::Panel::bottom("log").resizable(true).default_size(log_h).max_size((h * 0.4).max(80.0)).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(8, 4))).show(ui, |ui| self.log_panel(ui));
        }
        if self.tree_visible() {
            egui::Panel::left("tree").resizable(true).default_size((w * 0.18).clamp(150.0, 240.0)).max_size((w * 0.4).max(160.0)).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(8))).show(ui, |ui| self.tree_panel(ui));
        }
        if self.info_visible() {
            egui::Panel::right("props").resizable(true).default_size((w * 0.24).clamp(200.0, 300.0)).max_size((w * 0.45).max(200.0)).frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(8))).show(ui, |ui| self.props_panel(ui));
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(if w < 700.0 { 4 } else { 8 }))).show(ui, |ui| self.central(ui));
        if self.cwd != self.last_cwd {
            let n = self.doc.dir_at(&self.cwd).and_then(|d| d.children()).map(|c| c.len()).unwrap_or(0);
            self.log(Lvl::Info, format!("browsing /{} ({} items)", self.cwd.join("/"), n));
            self.last_cwd = self.cwd.clone();
        }
        self.draw_dialog(&ctx);
        self.draw_job(&ctx);
        self.screenshot_tick(&ctx);
    }
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("// {}", title)).color(GREEN).strong().small());
        let r = ui.available_rect_before_wrap();
        let y = r.center().y;
        ui.painter().line_segment([egui::pos2(r.left(), y), egui::pos2(r.right(), y)], Stroke::new(1.0, LINE));
    });
    ui.add_space(2.0);
}

/// "HEXISO" drawn as pixel blocks (fonts don't line box characters up reliably).
fn pixel_logo(ui: &mut Ui) {
    const L: [[&str; 7]; 6] = [
        ["#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"],
        ["#####", "#....", "#....", "####.", "#....", "#....", "#####"],
        ["#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#"],
        ["###", ".#.", ".#.", ".#.", ".#.", ".#.", "###"],
        [".####", "#....", "#....", ".###.", "....#", "....#", "####."],
        [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."],
    ];
    // 34 cells wide in total (28 letter cells + 5 gaps of 1.2): scale to fit the space
    let cell = ((ui.available_width() - 20.0) / 34.0).clamp(4.0, 9.0).floor();
    let gap = (cell / 6.0).max(1.0);
    let widths: Vec<f32> = L.iter().map(|g| g[0].len() as f32 * cell).collect();
    let total: f32 = widths.iter().sum::<f32>() + (L.len() - 1) as f32 * cell * 1.2;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 7.0 * cell + 8.0), Sense::hover());
    let p = ui.painter();
    let mut x = rect.center().x - total / 2.0;
    let y0 = rect.top() + 2.0;
    for (gi, g) in L.iter().enumerate() {
        for (r, row) in g.iter().enumerate() {
            let t = r as f32 / 6.0;
            let col = Color32::from_rgb(0, (255.0 - 40.0 * t) as u8, (156.0 + 80.0 * t) as u8);
            for (c, ch) in row.chars().enumerate() {
                if ch == '#' {
                    let min = egui::pos2(x + c as f32 * cell, y0 + r as f32 * cell);
                    let shadow = egui::Rect::from_min_size(min + egui::vec2(cell / 3.0, cell / 3.0), egui::vec2(cell - gap, cell - gap));
                    p.rect_filled(shadow, 0.0, Color32::from_rgb(0, 60, 44));
                    p.rect_filled(egui::Rect::from_min_size(min, egui::vec2(cell - gap, cell - gap)), 0.0, col);
                }
            }
        }
        x += widths[gi] + cell * 1.2;
    }
    // scanlines
    let mut y = rect.top();
    while y < rect.bottom() {
        p.line_segment([egui::pos2(rect.center().x - total / 2.0 - 10.0, y), egui::pos2(rect.center().x + total / 2.0 + 10.0, y)], Stroke::new(1.0, Color32::from_black_alpha(70)));
        y += 3.0;
    }
}

/// One fixed-height, left-aligned, single-line log row.
fn log_row(ui: &mut Ui, h: f32, job: egui::text::LayoutJob) -> egui::Response {
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.add(egui::Label::new(job).truncate())).inner
}

/// "title|body|why disabled"
fn split_tip(t: &str) -> (String, String, String) {
    let mut it = t.splitn(3, '|');
    (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
}

/// Key/value row whose key explains itself on hover.
pub fn kvt(ui: &mut Ui, k: &str, v: &str, tip: &str) {
    let r = ui.label(RichText::new(k).color(DIM));
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
    ui.label(RichText::new(v).color(TEXT));
    ui.end_row();
}

pub fn fs_help(name: &str) -> &'static str {
    match name {
        "Joliet" => "Joliet: the Windows name table. Long names (up to 103 characters), any language. Best choice on Windows.",
        "ISO 9660" => "ISO 9660: the original CD name table. Short UPPERCASE names (like README.TXT). Every computer can read it.",
        "Rock Ridge" => "Rock Ridge: the Linux / Unix name table. Long, case-sensitive names plus Unix permissions.",
        "UDF" => "UDF: the modern DVD / Blu-ray file system. Used by Windows install discs and video discs; supports huge files.",
        "FAT" => "FAT: the floppy / USB stick file system.",
        "Partitions" => "This is a whole disk image; each partition is shown as a folder.",
        "ZIP" => "A ZIP archive.",
        _ => "",
    }
}

fn info_help(k: &str) -> &'static str {
    match k {
        "Sector layout" => "How the image file stores each disc sector. Plain ISO files use 2048 bytes per sector; raw BIN files keep extra error-correction bytes.",
        "UDF revision" => "Version of the UDF standard the disc was written with.",
        "FAT type" => "FAT12 (floppies), FAT16 or FAT32 (USB sticks).",
        "Ratio" => "Compressed size compared with the original size.",
        "Encryption" => "The image is password protected.",
        "Volumes" => "The image is split into several files (.001, .002 ...) that are read as one.",
        _ => "",
    }
}

fn lvl_tag(l: Lvl) -> (&'static str, Color32) {
    match l {
        Lvl::Info => ("  ..", DIM),
        Lvl::Ok => ("  OK", GREEN),
        Lvl::Warn => ("WARN", AMBER),
        Lvl::Err => ("FAIL", RED),
    }
}

pub fn kv(ui: &mut Ui, k: &str, v: &str) {
    ui.label(RichText::new(k).color(DIM));
    ui.label(RichText::new(v).color(TEXT));
    ui.end_row();
}

pub fn meter(ui: &mut Ui, frac: f32, h: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BG);
    let mut f = rect;
    f.set_width(w * frac.clamp(0.0, 1.0));
    ui.painter().rect_filled(f, 0.0, if frac > 0.9 { AMBER } else { GREEN_DIM });
}

#[allow(clippy::too_many_arguments)]
fn tree_node(ui: &mut Ui, e: &Entry, name: &str, path: &mut Vec<String>, depth: usize, cwd: &[String], expanded: &HashSet<Vec<String>>, nav: &mut Option<Vec<String>>, toggle: &mut Option<Vec<String>>) {
    let mut kids: Vec<&Entry> = e.children().map(|c| c.iter().filter(|k| k.is_dir()).collect()).unwrap_or_default();
    let open = expanded.contains(path.as_slice());
    let is_cwd = path.as_slice() == cwd;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.add_space(depth as f32 * 12.0);
        let t = if kids.is_empty() { "  " } else if open { "- " } else { "+ " };
        if ui.add(egui::Label::new(RichText::new(t).color(GREEN_DIM)).sense(Sense::click())).clicked() && !kids.is_empty() {
            *toggle = Some(path.clone());
        }
        let txt = RichText::new(if depth == 0 { format!("/ {}", name) } else { name.to_string() }).color(if is_cwd { Color32::WHITE } else if depth == 0 { GREEN } else { CYAN });
        let r = ui.selectable_label(is_cwd, txt);
        let r = if depth == 0 { r.on_hover_text("Top folder of the image. Click a folder to show its files; + / - opens or closes it.") } else { r };
        if r.clicked() {
            *nav = Some(path.clone());
        }
        if r.double_clicked() && !kids.is_empty() {
            *toggle = Some(path.clone());
        }
    });
    if open {
        kids.sort_by_key(|k| k.name.to_lowercase());
        for k in kids {
            path.push(k.name.clone());
            tree_node(ui, k, &k.name, path, depth + 1, cwd, expanded, nav, toggle);
            path.pop();
        }
    }
}
