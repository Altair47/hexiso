// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! HexApp actions: open, save, edit, extract, mount, test, keyboard shortcuts.
//! Everything slow runs through `HexApp::run` on a background thread.

use super::*;

impl HexApp {
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
                app.log_failed(&failed);
                app.log(if failed.is_empty() { Lvl::Ok } else { Lvl::Warn }, format!("extracted {} file(s) to {}{}", n, dest.display(), if failed.is_empty() { String::new() } else { format!(", {} failed", failed.len()) }));
                drives::open_in_shell(&dest);
            }) as Apply)
        });
    }

    pub fn open_entry(&mut self, e: Entry) {
        let doc = self.doc.clone();
        self.run("Opening file", move |prog| {
            let dir = temp_dir(&format!("open/{}", crate::model::now()));
            let (_, failed) = es(ops::extract(&doc, std::slice::from_ref(&e), &dir, prog))?;
            if let Some(f) = failed.into_iter().next() {
                return Err(f);
            }
            let p = dir.join(ops::safe_name(&e.name, &mut Default::default()));
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
        // temporary ISO made for mounting this image (non-ISO formats, unsaved edits)
        let mp = Path::new(&m);
        let name = mp.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let parent = mp.parent().map(|x| x.display().to_string()).unwrap_or_default();
        !stem.is_empty() && parent == temp_dir("iso").display().to_string().to_lowercase() && name.starts_with(&stem.to_lowercase())
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
                    app.mount_now(vec![]);
                } else {
                    app.log(Lvl::Warn, format!("{} is already mounted as {}", name, mine.iter().map(|m| format!("{}:", m.1)).collect::<Vec<_>>().join(", ")));
                    app.dialog = Some(Dialog::AlreadyMounted { name, existing: mine });
                }
            }) as Apply)
        });
    }

    /// Mount the open image. `unmount_first`: image paths to eject before mounting (remount).
    pub fn mount_now(&mut self, unmount_first: Vec<String>) {
        let doc = self.doc.clone();
        let ext = doc.path.as_ref().and_then(|p| p.extension()).map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let direct = doc.path.clone().filter(|_| !doc.modified && matches!(ext.as_str(), "iso" | "vhd" | "vhdx"));
        let title = if !unmount_first.is_empty() { "Remounting" } else { "Mounting" };
        self.run(title, move |prog| {
            for p in &unmount_first {
                prog.say(format!("ejecting {}", p));
                es(drives::unmount(Path::new(p)))?;
            }
            let path = match direct {
                Some(p) => p,
                None => {
                    let stem = Path::new(&doc.title()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
                    let tmp = temp_dir("iso").join(format!("{}.iso", stem));
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

    pub(super) fn handle_drop(&mut self, ctx: &egui::Context) {
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

    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
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

    pub(super) fn activate(&mut self, name: &str) {
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
}
