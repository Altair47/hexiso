// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Modal dialogs.

use crate::app::{es, kv, section, HexApp, Lvl};
use crate::drives::{self, DriveKind, Track};
use crate::jobs::Apply;
use crate::model::{human, Kind};
use crate::ops::{self, OutFormat, Sums};
use crate::source::{open_image, Opened};
use crate::theme::*;
use eframe::egui::{self, RichText, Ui};
use std::path::PathBuf;
use std::sync::Arc;

pub enum Then {
    New,
    #[allow(dead_code)]
    Open(PathBuf),
    OpenDialog,
    Exit,
    #[allow(dead_code)]
    Drive(char),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DrivePurpose {
    Open,
    Burn,
    MakeImage,
    Copy,
    Rip,
}

#[derive(Clone)]
pub enum SumTarget {
    Image,
    File(Vec<String>),
    External(PathBuf),
}

pub enum Dialog {
    Message { title: String, body: String, error: bool },
    Discard { then: Then },
    Password { path: PathBuf, pw: String },
    SavePassword { path: PathBuf, pw: String, pw2: String },
    Rename { old: String, new: String },
    NewFolder { name: String },
    Properties { label: String },
    Boot,
    Search { query: String, results: Vec<(Vec<String>, bool, u64)> },
    Floppy { big: bool },
    DrivePick { purpose: DrivePurpose, kinds: Vec<DriveKind> },
    BurnPath { path: PathBuf, drive: Option<char> },
    MakeImage { drive: char, fmt: OutFormat, pw: String },
    Rip { drive: char, tracks: Vec<(Track, bool)> },
    Usb { drive: Option<char>, format: bool, fs: String, confirm: String },
    Mounts { system: Vec<(String, String)> },
    AlreadyMounted { name: String, existing: Vec<(String, String)> },
    Convert { src: String, fmt: OutFormat, pw: String },
    Split { file: String, mb: String },
    Checksum { target: SumTarget, result: Option<Sums> },
    About,
}

impl Dialog {
    pub fn convert_default(app: &HexApp, fmt: OutFormat) -> Dialog {
        Dialog::Convert { src: app.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(), fmt, pw: String::new() }
    }
}

fn title_bar(ui: &mut Ui, t: &str) {
    ui.label(RichText::new(format!(">> {}", t.to_uppercase())).color(GREEN).strong());
    ui.separator();
}

fn btn(ui: &mut Ui, t: &str) -> bool {
    ui.button(format!("[ {} ]", t)).clicked()
}

fn fmt_name(f: OutFormat) -> &'static str {
    match f {
        OutFormat::Iso => "ISO  (standard, mountable)",
        OutFormat::Cso => "CSO  (compressed, block deflate)",
        OutFormat::Hxz => "HXZ  (compressed + optional password)",
        OutFormat::Zip => "ZIP",
        OutFormat::Floppy144 => "IMG 1.44MB floppy",
        OutFormat::Floppy288 => "IMG 2.88MB floppy",
    }
}

impl HexApp {
    pub fn draw_dialog(&mut self, ctx: &egui::Context) {
        if self.job.is_some() {
            return;
        }
        let Some(mut d) = self.dialog.take() else { return };
        let mut keep = true;
        let resp = egui::Modal::new(egui::Id::unique("hexiso-dialog")).show(ctx, |ui| {
            ui.set_min_width((self.win_w - 60.0).clamp(240.0, 460.0));
            ui.set_max_width((self.win_w - 40.0).clamp(260.0, 640.0));
            keep = self.dialog_body(ui, &mut d);
        });
        if resp.should_close() {
            keep = false;
        }
        if keep && self.dialog.is_none() {
            self.dialog = Some(d);
        }
    }

    fn dialog_body(&mut self, ui: &mut Ui, d: &mut Dialog) -> bool {
        match d {
            Dialog::Message { title, body, error } => {
                title_bar(ui, title);
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.label(RichText::new(body.as_str()).color(if *error { RED } else { TEXT }));
                });
                ui.add_space(6.0);
                !btn(ui, "ok")
            }
            Dialog::Discard { .. } => {
                title_bar(ui, "unsaved changes");
                ui.label("The current image has changes that are not saved. Discard them?");
                ui.add_space(6.0);
                let mut choice = 0;
                ui.horizontal(|ui| {
                    if btn(ui, "discard") {
                        choice = 1;
                    }
                    if btn(ui, "save first") {
                        choice = 2;
                    }
                    if btn(ui, "cancel") {
                        choice = 3;
                    }
                });
                match choice {
                    1 => {
                        if let Dialog::Discard { then } = std::mem::replace(d, Dialog::About) {
                            self.doc.modified = false;
                            self.do_then(then);
                        }
                        false
                    }
                    2 => {
                        self.save();
                        false
                    }
                    3 => false,
                    _ => true,
                }
            }
            Dialog::Password { path, pw } => {
                title_bar(ui, "password required");
                ui.label(format!("{} is encrypted.", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                let r = ui.add(egui::TextEdit::singleline(pw).password(true).hint_text("password"));
                r.request_focus();
                let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "unlock") || go {
                        self.open_with_password(path.clone(), Some(pw.clone()));
                        keep = false;
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::SavePassword { path, pw, pw2 } => {
                title_bar(ui, "hxz options");
                ui.label("Leave empty for compression only, or set a password to encrypt (ChaCha20-Poly1305).");
                ui.add(egui::TextEdit::singleline(pw).password(true).hint_text("password (optional)"));
                ui.add(egui::TextEdit::singleline(pw2).password(true).hint_text("repeat password"));
                let mismatch = pw != pw2;
                if mismatch {
                    ui.label(RichText::new("passwords do not match").color(RED).small());
                }
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.add_enabled(!mismatch, egui::Button::new("[ save ]")).clicked() {
                        let p = if pw.is_empty() { None } else { Some(pw.clone()) };
                        self.save_to(path.clone(), OutFormat::Hxz, p);
                        keep = false;
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Rename { old, new } => {
                title_bar(ui, "rename");
                let r = ui.add(egui::TextEdit::singleline(new).desired_width(400.0));
                let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "rename") || go {
                        let (o, n) = (old.clone(), new.clone());
                        self.rename(&o, &n);
                        keep = false;
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::NewFolder { name } => {
                title_bar(ui, "new folder");
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(400.0));
                let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "create") || go {
                        let n = name.clone();
                        self.new_folder(&n);
                        keep = false;
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Properties { label } => {
                title_bar(ui, "image properties");
                egui::Grid::new("props").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    kv(ui, "file", &self.doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(not saved)".into()));
                    kv(ui, "format", &self.doc.format);
                    kv(ui, "file system", &self.doc.fs_name);
                    let (f, dd) = self.doc.root.count();
                    kv(ui, "content", &format!("{} files, {} folders, {}", f, dd.saturating_sub(1), human(self.doc.root.size())));
                    ui.label(RichText::new("volume label").color(DIM));
                    ui.add(egui::TextEdit::singleline(label).char_limit(32));
                    ui.end_row();
                    ui.label(RichText::new("target media").color(DIM));
                    let mut m = self.doc.media;
                    egui::ComboBox::from_id_salt("pmedia").selected_text(m.label()).show_ui(ui, |ui| {
                        for x in crate::model::Media::ALL {
                            ui.selectable_value(&mut m, x, x.label());
                        }
                    });
                    self.doc.media = m;
                    ui.end_row();
                });
                ui.label(RichText::new("New images are written as ISO 9660 + Joliet (long Unicode names, files over 4 GB).").color(DIM).small());
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "apply") {
                        if *label != self.doc.label {
                            self.doc.label = label.clone();
                            self.touch();
                            self.log(Lvl::Ok, format!("label set to {}", label));
                        }
                        keep = false;
                    }
                    if btn(ui, "close") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Boot => {
                title_bar(ui, "boot options (el torito)");
                if self.doc.boot.is_empty() {
                    ui.label(RichText::new("This image is not bootable.").color(DIM));
                }
                let mut remove: Option<usize> = None;
                let mut save: Option<usize> = None;
                let mut changed = false;
                for (i, b) in self.doc.boot.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if b.platform == 0xEF { "UEFI" } else { "BIOS" }).color(AMBER).strong());
                            ui.label(&b.label);
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{} | {}", human(b.size), match b.emulation { 0 => "no emulation", 1 => "1.2M floppy", 2 => "1.44M floppy", 3 => "2.88M floppy", _ => "hard disk" })).color(DIM));
                            if b.platform == 0 && ui.checkbox(&mut b.info_table, "patch boot-info table").changed() {
                                changed = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.small_button("extract boot image").clicked() {
                                save = Some(i);
                            }
                            if ui.small_button("remove").clicked() {
                                remove = Some(i);
                            }
                        });
                    });
                }
                if changed {
                    self.touch();
                }
                if let Some(i) = remove {
                    self.doc.boot.remove(i);
                    self.touch();
                    self.log(Lvl::Ok, "boot entry removed");
                }
                if let Some(i) = save {
                    let b = self.doc.boot[i].clone();
                    if let Some(p) = rfd::FileDialog::new().set_file_name(if b.platform == 0xEF { "efiboot.img" } else { "boot.bin" }).save_file() {
                        match self.doc.read_all(&b.src, b.size).and_then(|v| std::fs::write(&p, v)) {
                            Ok(_) => self.log(Lvl::Ok, format!("boot image saved to {}", p.display())),
                            Err(e) => self.log(Lvl::Err, format!("cannot save boot image: {}", e)),
                        }
                    }
                }
                ui.separator();
                ui.label(RichText::new("add a boot image from disk (or right-click a file in the list: 'Use as ... boot image')").color(DIM).small());
                ui.horizontal(|ui| {
                    for (label, efi) in [("add BIOS image...", false), ("add UEFI image...", true)] {
                        if ui.button(label).clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_file() {
                                let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                                let platform = if efi { 0xEF } else { 0 };
                                self.doc.boot.retain(|b| b.platform != platform);
                                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                let info_table = !efi && name.to_lowercase().contains("isolinux");
                                self.doc.boot.push(crate::model::BootSpec { platform, emulation: 0, load_sectors: if efi { 0 } else { 4 }, info_table, size, src: crate::model::FileSrc::Disk(p.clone()), tree_path: None, label: format!("[hidden] {}", name) });
                                self.doc.boot.sort_by_key(|b| b.platform);
                                self.touch();
                                self.log(Lvl::Ok, format!("{} boot image added: {}", if efi { "UEFI" } else { "BIOS" }, name));
                            }
                    }
                });
                ui.add_space(4.0);
                !btn(ui, "close")
            }
            Dialog::Search { query, results } => {
                title_bar(ui, "search in image");
                let r = ui.add(egui::TextEdit::singleline(query).hint_text("name or wildcard, e.g. *.efi").desired_width(420.0));
                r.request_focus();
                if r.changed() {
                    *results = if query.trim().is_empty() { vec![] } else { ops::search(&self.doc.root, query.trim()) };
                }
                ui.label(RichText::new(format!("{} match(es)", results.len())).color(DIM).small());
                let mut go: Option<(Vec<String>, bool)> = None;
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (p, dir, size) in results.iter().take(2000) {
                        let t = format!("/{}{}   {}", p.join("/"), if *dir { "/" } else { "" }, if *dir { String::new() } else { human(*size) });
                        if ui.selectable_label(false, RichText::new(t).color(if *dir { CYAN } else { TEXT })).clicked() {
                            go = Some((p.clone(), *dir));
                        }
                    }
                });
                let mut keep = true;
                if let Some((p, dir)) = go {
                    if dir {
                        self.cwd = p;
                        self.sel.clear();
                    } else {
                        let (last, parent) = p.split_last().unwrap();
                        self.cwd = parent.to_vec();
                        self.sel.clear();
                        self.sel.insert(last.clone());
                    }
                    let mut acc = vec![];
                    self.expanded.insert(acc.clone());
                    for x in &self.cwd {
                        acc.push(x.clone());
                        self.expanded.insert(acc.clone());
                    }
                    self.view = crate::app::View::Files;
                    keep = false;
                }
                if btn(ui, "close") {
                    keep = false;
                }
                keep
            }
            Dialog::Floppy { big } => {
                title_bar(ui, "make floppy image");
                ui.label("Writes the current file list as a FAT12 floppy disk image.");
                ui.radio_value(big, false, "1.44 MB (3.5\" HD)");
                ui.radio_value(big, true, "2.88 MB (3.5\" ED)");
                let need = self.doc.root.size();
                let cap = if *big { 2_949_120u64 } else { 1_474_560 };
                ui.label(RichText::new(format!("content {} of ~{}", human(need), human(cap - 33 * 512))).color(if need > cap { RED } else { DIM }));
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "save as...")
                        && let Some(p) = rfd::FileDialog::new().set_file_name("floppy.img").add_filter("Floppy image", &["img", "ima", "flp"]).save_file() {
                            self.save_to(p, if *big { OutFormat::Floppy288 } else { OutFormat::Floppy144 }, None);
                            keep = false;
                        }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::DrivePick { purpose, kinds } => {
                let purpose = *purpose;
                let kinds = kinds.clone();
                let t = match purpose {
                    DrivePurpose::Open => "open a drive",
                    DrivePurpose::Burn => "burn image to disc",
                    DrivePurpose::MakeImage => "make image from a drive",
                    DrivePurpose::Copy => "copy a disc",
                    DrivePurpose::Rip => "rip audio cd",
                };
                title_bar(ui, t);
                match purpose {
                    DrivePurpose::Burn => {
                        ui.label(format!("Source: {}", if self.doc.modified || self.doc.path.is_none() { "current compilation (written to a temporary ISO first)".to_string() } else { self.doc.title() }));
                        ui.label(RichText::new("Burning uses the Windows Disc Image Burner; insert a blank or rewritable disc.").color(DIM).small());
                    }
                    DrivePurpose::Copy => {
                        ui.label(RichText::new("Step 1 reads the source disc into a temporary ISO; step 2 burns it to a blank disc.").color(DIM).small());
                    }
                    _ => {}
                }
                let list: Vec<_> = self.drives.iter().filter(|x| kinds.contains(&x.kind)).cloned().collect();
                if list.is_empty() {
                    ui.label(RichText::new("No matching drives found.").color(AMBER));
                }
                let mut chosen: Option<char> = None;
                for dr in &list {
                    let t = format!("{}:  {:<14} {}  {}", dr.letter, dr.kind.name(), if dr.ready { dr.label.clone() } else { "(no media)".into() }, if dr.ready { human(dr.total) } else { String::new() });
                    if ui.selectable_label(false, RichText::new(t).color(if dr.kind == DriveKind::Optical { CYAN } else { TEXT })).clicked() {
                        chosen = Some(dr.letter);
                    }
                }
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.button("refresh").clicked() {
                        self.refresh_drives();
                    }
                    if purpose == DrivePurpose::Burn && ui.button("burn another image file...").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("ISO", &["iso"]).pick_file() {
                            *d = Dialog::BurnPath { path: p, drive: None };
                        }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                if let Dialog::BurnPath { .. } = d {
                    return true;
                }
                if let Some(l) = chosen {
                    let p = purpose;
                    keep = false;
                    match p {
                        DrivePurpose::Open => self.open_drive(l),
                        DrivePurpose::Burn => {
                            self.with_iso("Burning", move |_, path| {
                                es(drives::burn(&path, l))?;
                                Ok(Box::new(move |app: &mut HexApp| app.log(Lvl::Ok, format!("{} handed to the Windows burner for drive {}:", path.display(), l))) as Apply)
                            });
                        }
                        DrivePurpose::MakeImage => {
                            self.dialog = Some(Dialog::MakeImage { drive: l, fmt: OutFormat::Iso, pw: String::new() });
                        }
                        DrivePurpose::Copy => {
                            self.run("Reading source disc", move |prog| {
                                #[cfg(windows)]
                                {
                                    let src: crate::source::Src = Arc::new(es(crate::source::DriveSource::open(l))?);
                                    let tmp = crate::app::temp_dir("copy").join(format!("disc_{}.iso", crate::model::now()));
                                    es(ops::convert(&src, &tmp, OutFormat::Iso, None, prog))?;
                                    Ok(Box::new(move |app: &mut HexApp| {
                                        app.log(Lvl::Ok, format!("disc read into {}", tmp.display()));
                                        let _ = drives::eject(l);
                                        app.dialog = Some(Dialog::BurnPath { path: tmp, drive: None });
                                    }) as Apply)
                                }
                                #[cfg(not(windows))]
                                {
                                    let _ = (prog, l);
                                    Err("drive access is only available on Windows".to_string())
                                }
                            });
                        }
                        DrivePurpose::Rip => match drives::read_toc(l) {
                            Ok(t) => {
                                let audio = t.iter().filter(|x| x.audio).count();
                                self.log(Lvl::Ok, format!("toc: {} tracks, {} audio", t.len(), audio));
                                self.dialog = Some(Dialog::Rip { drive: l, tracks: t.into_iter().map(|x| { let a = x.audio; (x, a) }).collect() });
                            }
                            Err(e) => self.dialog = Some(Dialog::Message { title: "Cannot read the disc".into(), body: e.to_string(), error: true }),
                        },
                    }
                }
                keep
            }
            Dialog::BurnPath { path, drive } => {
                title_bar(ui, "burn image");
                ui.label(format!("Image: {}", path.display()));
                ui.label("Insert a blank disc and pick the burner:");
                for dr in self.drives.iter().filter(|x| x.kind == DriveKind::Optical) {
                    ui.radio_value(drive, Some(dr.letter), format!("{}:  {}", dr.letter, if dr.ready { dr.label.clone() } else { "(empty)".into() }));
                }
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.add_enabled(drive.is_some(), egui::Button::new("[ burn ]")).clicked() {
                        let l = drive.unwrap();
                        match drives::burn(path, l) {
                            Ok(_) => self.log(Lvl::Ok, format!("{} handed to the Windows burner for drive {}:", path.display(), l)),
                            Err(e) => self.log(Lvl::Err, e.to_string()),
                        }
                        keep = false;
                    }
                    if ui.button("refresh").clicked() {
                        self.refresh_drives();
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::MakeImage { drive, fmt, pw } => {
                title_bar(ui, "make image from drive");
                ui.label(format!("Source drive: {}:", drive));
                for f in [OutFormat::Iso, OutFormat::Cso, OutFormat::Hxz] {
                    ui.radio_value(fmt, f, fmt_name(f));
                }
                if *fmt == OutFormat::Hxz {
                    ui.add(egui::TextEdit::singleline(pw).password(true).hint_text("password (optional)"));
                }
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "choose output and start") {
                        let l = *drive;
                        let f = *fmt;
                        let pwo = if pw.is_empty() { None } else { Some(pw.clone()) };
                        if let Some(out) = rfd::FileDialog::new().set_file_name(format!("disc.{}", f.ext())).save_file() {
                            keep = false;
                            self.run("Making image", move |prog| {
                                #[cfg(windows)]
                                {
                                    let src: crate::source::Src = Arc::new(es(crate::source::DriveSource::open(l))?);
                                    let n = es(ops::convert(&src, &out, f, pwo.as_deref(), prog))?;
                                    Ok(Box::new(move |app: &mut HexApp| app.log(Lvl::Ok, format!("image written: {} ({})", out.display(), human(n)))) as Apply)
                                }
                                #[cfg(not(windows))]
                                {
                                    let _ = (prog, l, out, f, pwo);
                                    Err("drive access is only available on Windows".to_string())
                                }
                            });
                        }
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Rip { drive, tracks } => {
                title_bar(ui, "rip audio cd");
                ui.label(format!("Drive {}:  -> WAV 44.1 kHz / 16-bit / stereo", drive));
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (t, on) in tracks.iter_mut() {
                        let secs = t.sectors / 75;
                        ui.add_enabled(t.audio, egui::Checkbox::new(on, format!("track {:02}  {:>2}:{:02}  {}  {}", t.number, secs / 60, secs % 60, human(t.sectors as u64 * 2352), if t.audio { "audio" } else { "data" })));
                    }
                });
                let mut keep = true;
                ui.horizontal(|ui| {
                    let n = tracks.iter().filter(|x| x.1).count();
                    if ui.add_enabled(n > 0, egui::Button::new(format!("[ rip {} track(s) ]", n))).clicked()
                        && let Some(dest) = rfd::FileDialog::new().set_title("Save tracks to").pick_folder() {
                            let l = *drive;
                            let sel: Vec<Track> = tracks.iter().filter(|x| x.1).map(|x| x.0.clone()).collect();
                            keep = false;
                            self.run("Ripping audio", move |prog| {
                                let n = es(drives::rip_tracks(l, &sel, &dest, prog))?;
                                Ok(Box::new(move |app: &mut HexApp| {
                                    app.log(Lvl::Ok, format!("{} track(s) saved to {}", n, dest.display()));
                                    drives::open_in_shell(&dest);
                                }) as Apply)
                            });
                        }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Usb { drive, format, fs, confirm } => {
                title_bar(ui, "bootable usb drive");
                ui.label(format!("Source: {} ({})", self.doc.title(), human(self.doc.root.size())));
                ui.label(RichText::new("Copies every file of the image onto the stick. Works for UEFI installers (Windows 8+, most Linux ISOs).").color(DIM).small());
                let big = big_files(&self.doc.root);
                let list: Vec<_> = self.drives.iter().filter(|x| x.kind == DriveKind::Removable).cloned().collect();
                if list.is_empty() {
                    ui.label(RichText::new("No USB / removable drive found. Plug one in and press refresh.").color(AMBER));
                }
                for dr in &list {
                    ui.radio_value(drive, Some(dr.letter), format!("{}:  {} {} {}", dr.letter, if dr.label.is_empty() { "-" } else { &dr.label }, dr.fs, human(dr.total)));
                }
                ui.checkbox(format, "format the drive first (ERASES EVERYTHING ON IT)");
                if *format {
                    ui.horizontal(|ui| {
                        for f in ["FAT32", "NTFS", "exFAT"] {
                            ui.radio_value(fs, f.to_string(), f);
                        }
                    });
                }
                if !big.is_empty() && (fs.as_str() == "FAT32" || !*format) {
                    ui.label(RichText::new(format!("warning: {} file(s) are over 4 GB and cannot be stored on FAT32 (e.g. {}). Use NTFS or exFAT.", big.len(), big[0])).color(AMBER).small());
                }
                if *format {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("type ERASE to confirm").color(RED));
                        ui.add(egui::TextEdit::singleline(confirm).desired_width(90.0));
                    });
                }
                let ok = drive.is_some() && (!*format || confirm.as_str() == "ERASE") && self.has_content();
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.add_enabled(ok, egui::Button::new("[ write ]")).clicked() {
                        let l = drive.unwrap();
                        let doc = self.doc.clone();
                        let fmt = if *format { Some(fs.clone()) } else { None };
                        let label: String = doc.label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(11).collect();
                        keep = false;
                        self.run("Writing USB drive", move |prog| {
                            if let Some(f) = fmt {
                                prog.say(format!("formatting {}: as {}", l, f));
                                es(drives::format_drive(l, &f, if label.is_empty() { "HEXISO" } else { &label }))?;
                            }
                            let items = doc.root.children().cloned().unwrap_or_default();
                            let dest = PathBuf::from(format!("{}:\\", l));
                            let (n, failed) = es(ops::extract(&doc, &items, &dest, prog))?;
                            Ok(Box::new(move |app: &mut HexApp| {
                                app.log(if failed > 0 { Lvl::Warn } else { Lvl::Ok }, format!("usb {}: ready, {} files copied{}", l, n, if failed > 0 { format!(", {} failed", failed) } else { String::new() }));
                                app.refresh_drives();
                            }) as Apply)
                        });
                    }
                    if ui.button("refresh").clicked() {
                        self.refresh_drives();
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Mounts { system } => {
                title_bar(ui, "virtual drives");
                ui.label(RichText::new("Images are mounted with Windows' built-in virtual drive (no driver to install). Other formats are converted to ISO first.").color(DIM).small());
                section(ui, "MOUNTED NOW");
                if system.is_empty() {
                    ui.label(RichText::new("no mounted images").color(DIM));
                }
                let mut un: Option<PathBuf> = None;
                for (p, l) in system.iter() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}:", if l.is_empty() { "?" } else { l })).color(GREEN).strong());
                        ui.label(p.as_str());
                        if ui.small_button("unmount").clicked() {
                            un = Some(PathBuf::from(p));
                        }
                    });
                }
                let mut keep = true;
                if let Some(p) = un {
                    self.unmount(p);
                    keep = false;
                }
                ui.horizontal(|ui| {
                    if btn(ui, "mount current image") {
                        self.mount_current();
                        keep = false;
                    }
                    if btn(ui, "mount a file...")
                        && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["iso", "vhd", "vhdx"]).pick_file() {
                            self.run("Mounting", move |_| {
                                let l = es(drives::mount(&p))?;
                                Ok(Box::new(move |app: &mut HexApp| {
                                    app.log(Lvl::Ok, format!("mounted {} as {}:", p.display(), l));
                                    app.mounts.push((p, l));
                                    app.refresh_drives();
                                }) as Apply)
                            });
                            keep = false;
                        }
                    if ui.button("refresh").clicked() {
                        *system = drives::mounted();
                    }
                    if btn(ui, "close") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::AlreadyMounted { name, existing } => {
                title_bar(ui, "already mounted");
                let letters: Vec<String> = existing.iter().map(|e| format!("{}:", if e.1.is_empty() { "?" } else { &e.1 })).collect();
                ui.label(RichText::new(format!("{} is already mounted as {}.", name, letters.join(", "))).color(TEXT));
                ui.add_space(4.0);
                for (p, l) in existing.iter() {
                    ui.label(RichText::new(format!("  {}:  {}", if l.is_empty() { "?" } else { l }, p)).color(DIM).small());
                }
                ui.add_space(8.0);
                let mut keep = true;
                let paths: Vec<String> = {
                    let mut v: Vec<String> = existing.iter().map(|e| e.0.clone()).collect();
                    v.dedup();
                    v
                };
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("[ remount ]")
                        .on_hover_text("Eject the existing drive(s), then mount the image again as one fresh drive. Use this after changing the image.")
                        .clicked()
                    {
                        self.mount_now(paths.clone(), false);
                        keep = false;
                    }
                    if ui.button("[ mount another copy ]").on_hover_text("Keep the existing drive(s) and add one more drive letter with the same image.").clicked() {
                        self.mount_now(vec![], true);
                        keep = false;
                    }
                    if let Some(l) = existing.iter().find_map(|e| e.1.chars().next()).filter(|c| c.is_ascii_alphabetic())
                        && ui.button(format!("[ open {}: ]", l)).on_hover_text("Just open the drive that is already there.").clicked() {
                            drives::open_in_shell(std::path::Path::new(&format!("{}:\\", l)));
                            keep = false;
                        }
                    if ui.button("[ eject all ]").on_hover_text("Unmount every drive of this image.").clicked() {
                        let ps = paths.clone();
                        self.run("Ejecting", move |_| {
                            for p in &ps {
                                es(drives::unmount(std::path::Path::new(p)))?;
                            }
                            Ok(Box::new(move |app: &mut HexApp| {
                                for p in &ps {
                                    app.mounts.retain(|m| m.0.display().to_string().to_lowercase() != p.to_lowercase());
                                    app.log(Lvl::Ok, format!("ejected {}", p));
                                }
                                app.refresh_drives();
                            }) as Apply)
                        });
                        keep = false;
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Convert { src, fmt, pw } => {
                title_bar(ui, "convert / compress / encrypt");
                ui.horizontal(|ui| {
                    ui.label("source");
                    ui.add(egui::TextEdit::singleline(src).desired_width(380.0));
                    if ui.button("...").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["iso", "bin", "cue", "img", "nrg", "mdf", "cso", "hxz", "vhd", "001"]).pick_file() {
                            *src = p.display().to_string();
                        }
                });
                ui.label("output format");
                for f in [OutFormat::Iso, OutFormat::Cso, OutFormat::Hxz] {
                    ui.radio_value(fmt, f, fmt_name(f));
                }
                if *fmt == OutFormat::Hxz {
                    ui.add(egui::TextEdit::singleline(pw).password(true).hint_text("password (empty = compress only)"));
                }
                ui.label(RichText::new("BIN/CUE, NRG, MDF, CSO, HXZ, VHD and split .001 files become a plain 2048-byte-sector image. Compression uses every CPU core.").color(DIM).small());
                let mut keep = true;
                ui.horizontal(|ui| {
                    if ui.add_enabled(!src.trim().is_empty(), egui::Button::new("[ convert ]")).clicked() {
                        let sp = PathBuf::from(src.trim());
                        let f = *fmt;
                        let stem = sp.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
                        if let Some(out) = rfd::FileDialog::new().set_file_name(format!("{}.{}", stem, f.ext())).save_file() {
                            let pwo = if pw.is_empty() { None } else { Some(pw.clone()) };
                            let in_pw = self.doc.password.clone().filter(|_| self.doc.path.as_ref() == Some(&sp));
                            keep = false;
                            self.run("Converting", move |prog| {
                                let srcv = match es(open_image(&sp, in_pw.as_deref()))? {
                                    Opened::Sectors(s) => s,
                                    Opened::NeedPassword => return Err("open the encrypted source image first so its password is known".into()),
                                    Opened::Zip => return Err("a ZIP archive is not a disc image".into()),
                                };
                                let n = es(ops::convert(&srcv, &out, f, pwo.as_deref(), prog))?;
                                let inb = srcv.sector_count() * 2048;
                                Ok(Box::new(move |app: &mut HexApp| app.log(Lvl::Ok, format!("converted -> {} ({} -> {}, {:.1}%)", out.display(), human(inb), human(n), n as f64 * 100.0 / inb.max(1) as f64))) as Apply)
                            });
                        }
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Split { file, mb } => {
                title_bar(ui, "split into volumes");
                ui.horizontal(|ui| {
                    ui.label("file");
                    ui.add(egui::TextEdit::singleline(file).desired_width(380.0));
                    if ui.button("...").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_file() {
                            *file = p.display().to_string();
                        }
                });
                ui.horizontal(|ui| {
                    ui.label("volume size MB");
                    ui.add(egui::TextEdit::singleline(mb).desired_width(80.0));
                    for (l, v) in [("CD", "700"), ("DVD", "4480"), ("FAT32", "4095"), ("1 GB", "1024")] {
                        if ui.small_button(l).clicked() {
                            *mb = v.into();
                        }
                    }
                });
                ui.label(RichText::new("Creates name.001, name.002, ... Open the .001 file to read the joined image directly.").color(DIM).small());
                let mut keep = true;
                ui.horizontal(|ui| {
                    let size = mb.trim().parse::<u64>().unwrap_or(0) * 1024 * 1024;
                    if ui.add_enabled(size > 0 && !file.is_empty(), egui::Button::new("[ split ]")).clicked() {
                        let p = PathBuf::from(file.trim());
                        keep = false;
                        self.run("Splitting", move |prog| {
                            let parts = es(ops::split_file(&p, size, prog))?;
                            Ok(Box::new(move |app: &mut HexApp| app.log(Lvl::Ok, format!("split into {} volume(s), first: {}", parts.len(), parts.first().map(|x| x.display().to_string()).unwrap_or_default()))) as Apply)
                        });
                    }
                    if btn(ui, "cancel") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::Checksum { target, result } => {
                title_bar(ui, "checksums");
                let tname = match target {
                    SumTarget::Image => format!("whole image: {}", self.doc.title()),
                    SumTarget::File(p) => format!("file in image: /{}", p.join("/")),
                    SumTarget::External(p) => format!("file: {}", p.display()),
                };
                ui.label(tname);
                if let Some(s) = result {
                    egui::Grid::new("sums").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
                        for (k, v) in [("CRC32", &s.crc32), ("MD5", &s.md5), ("SHA-1", &s.sha1), ("SHA-256", &s.sha256)] {
                            ui.label(RichText::new(k).color(DIM));
                            ui.label(RichText::new(v.as_str()).color(GREEN));
                            if ui.small_button("copy").clicked() {
                                ui.ctx().copy_text(v.clone());
                            }
                            ui.end_row();
                        }
                    });
                    ui.label(RichText::new(format!("{} bytes hashed", s.bytes)).color(DIM).small());
                }
                let mut keep = true;
                ui.horizontal(|ui| {
                    if btn(ui, "compute") {
                        let t = target.clone();
                        let doc = self.doc.clone();
                        keep = false;
                        self.run("Computing checksums", move |prog| {
                            let sums = match &t {
                                SumTarget::External(p) => {
                                    let f = es(std::fs::File::open(p))?;
                                    let n = f.metadata().map(|m| m.len()).unwrap_or(0);
                                    es(ops::checksum(std::io::BufReader::new(f), n, prog))?
                                }
                                SumTarget::File(p) => {
                                    let e = doc.find(p).cloned().ok_or("file not found")?;
                                    let Kind::File { size, src } = e.kind else { return Err("pick a file, not a folder".into()) };
                                    let r = es(doc.reader(&src, size))?;
                                    es(ops::checksum(r, size, prog))?
                                }
                                SumTarget::Image => match (&doc.path, &doc.modified) {
                                    (Some(p), false) => {
                                        let f = es(std::fs::File::open(p))?;
                                        let n = f.metadata().map(|m| m.len()).unwrap_or(0);
                                        es(ops::checksum(std::io::BufReader::new(f), n, prog))?
                                    }
                                    _ => return Err("save the image first, then compute its checksum".into()),
                                },
                            };
                            Ok(Box::new(move |app: &mut HexApp| {
                                app.log(Lvl::Ok, format!("sha256 {}", sums.sha256));
                                app.dialog = Some(Dialog::Checksum { target: t, result: Some(sums) });
                            }) as Apply)
                        });
                    }
                    if ui.button("pick a file on disk...").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_file() {
                            *target = SumTarget::External(p);
                            *result = None;
                        }
                    if btn(ui, "close") {
                        keep = false;
                    }
                });
                keep
            }
            Dialog::About => {
                title_bar(ui, "about hexiso");
                ui.label(RichText::new(format!("HEXISO {} :: a fast disc image toolkit written in Rust", env!("CARGO_PKG_VERSION"))).color(GREEN));
                ui.horizontal(|ui| {
                    ui.label(RichText::new("created by Nick Petropoulos").color(TEXT));
                    ui.hyperlink_to("github.com/Altair47", "https://github.com/Altair47");
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("source, updates, issues:").color(DIM));
                    ui.hyperlink_to("github.com/Altair47/hexiso", "https://github.com/Altair47/hexiso");
                });
                ui.label(RichText::new("Free software under the GNU Affero General Public License v3.0 (AGPL-3.0). No warranty.").color(DIM).small());
                ui.add_space(4.0);
                ui.label("Open, edit, extract, build, convert, compress, encrypt, split, checksum, mount and burn disc images; read drives, rip audio CDs, write bootable USB sticks.");
                ui.add_space(4.0);
                ui.label(RichText::new("Read: ISO 9660, Joliet, Rock Ridge, UDF, El Torito, FAT12/16/32, MBR/GPT, BIN/CUE, NRG, MDF, CSO, HXZ, VHD (fixed), split volumes, ZIP.").color(DIM).small());
                ui.label(RichText::new("Write: ISO 9660 + Joliet (BIOS + UEFI boot, multi-extent files), CSO, HXZ, ZIP, FAT12 floppy.").color(DIM).small());
                ui.label(RichText::new("Shortcuts: Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+E extract, Del delete, F2 rename, Backspace up, F5 refresh drives.").color(DIM).small());
                ui.label(RichText::new(format!("Renderer: {}", self.renderer)).color(DIM).small());
                ui.add_space(6.0);
                !btn(ui, "close")
            }
        }
    }
}

fn big_files(e: &crate::model::Entry) -> Vec<String> {
    let mut out = Vec::new();
    fn go(e: &crate::model::Entry, out: &mut Vec<String>) {
        for k in e.children().into_iter().flatten() {
            match &k.kind {
                Kind::Dir(_) => go(k, out),
                Kind::File { size, .. } if *size >= 4 * 1024 * 1024 * 1024 => out.push(k.name.clone()),
                _ => {}
            }
        }
    }
    go(e, &mut out);
    out
}
