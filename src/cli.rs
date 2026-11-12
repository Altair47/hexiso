// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Command-line interface: `hexiso <command> [arguments] [options]`.
//! Every command exits 0 on success, 1 on failure, 2 on a usage mistake.

use crate::drives;
use crate::jobs::Progress;
use crate::model::{fmt_time, human, BootSpec, Document, Entry, FileSrc, Kind};
use crate::ops::{self, OpenResult, OutFormat};
use crate::source::{open_image, Opened, SECTOR};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const COMMANDS: &[&str] = &[
    "help", "version", "info", "ls", "extract", "create", "add", "rm", "rename", "mkdir", "label", "boot", "convert", "checksum", "test", "split", "mount", "unmount", "mounts",
    "drives", "burn", "make-image", "rip", "floppy", "usb", "selftest",
];

const HELP: &str = r#"HEXISO - disc image toolkit
Created by Nick Petropoulos (https://github.com/Altair47) - AGPL-3.0
Source, updates and issues: https://github.com/Altair47/hexiso

USAGE
  hexiso                          open the window
  hexiso <image>                  open the window with an image loaded
  hexiso <command> [args] [options]

READ
  info     <image>                         format, file system, label, sizes, boot info
  ls       <image> [path] [-r]             list a folder (-r: everything below it)
  extract  <image> <dest-dir> [paths...]   extract everything, or only the given paths
  checksum <file> [--in <path>]            CRC32 / MD5 / SHA-1 / SHA-256 (of a file, or of a file inside an image)
  test     <image>                         read every file back and report errors

BUILD / EDIT  (edits save over the image unless --out is given)
  create   <out> <files/folders...>        new image; format from extension: .iso .cso .hxz .zip .img(floppy)
  add      <image> <files/folders...> [--to <path>]
  rm       <image> <paths...>
  rename   <image> <path> <new-name>
  mkdir    <image> <path>
  label    <image> <new-label>
  boot     <image> [--bios <file>] [--uefi <file>] [--remove] [--extract <dir>]
  floppy   <out.img> <files/folders...> [--big]      FAT12 1.44 MB (--big: 2.88 MB)

CONVERT
  convert  <in> <out>                      any readable image -> .iso / .cso / .hxz
  split    <file> --size <MB>              file.001, file.002 ... (open the .001 to read it back)

DRIVES (Windows)
  drives                                   list drives
  mounts                                   images mounted right now
  mount    <image>                         mount as a virtual drive, prints the drive letter
  unmount  <image>                         eject every drive made from this image
  burn     <image> --drive <E>             hand the image to Windows' disc burner
  make-image --drive <E> <out>             read a disc / USB drive into an image file
  rip      --drive <E> <dest-dir> [--tracks 1,3,5]   audio CD -> WAV
  usb      <image> --drive <F> [--format FAT32|NTFS|exFAT] --yes   copy an install image onto a USB stick

OPTIONS
  --json              machine-readable output (info, ls, checksum, test, drives, mounts, mount)
  --fs <name>         read names from: UDF, Joliet, "Rock Ridge", "ISO 9660", FAT
  --password <pw>     password of an encrypted .hxz image
  --out <file>        write the edited image here instead of over the original
  --label <name>      volume label for create / floppy
  --bios <path>       BIOS boot image (a path inside the image, or a file on disk)
  --uefi <path>       UEFI boot image (a path inside the image, or a file on disk)
  --encrypt <pw>      password for .hxz output
  -q, --quiet         no progress output
  -h, --help          this help;  -V, --version  version

EXAMPLES
  hexiso info win11.iso --json
  hexiso extract ubuntu.iso C:\out casper/vmlinuz casper/initrd
  hexiso create backup.iso C:\photos C:\docs --label BACKUP
  hexiso add backup.iso notes.txt --to docs
  hexiso convert game.bin game.iso
  hexiso convert big.iso big.hxz --encrypt s3cret
  for /f %d in ('hexiso mount disk.iso') do dir %d:\
"#;

struct Args {
    pos: Vec<String>,
    opts: HashMap<String, String>,
    flags: Vec<String>,
}

const VALUE_OPTS: &[&str] = &["fs", "password", "out", "label", "bios", "uefi", "encrypt", "to", "in", "size", "drive", "tracks", "format", "extract"];

fn parse(raw: &[String]) -> Result<Args, String> {
    let mut a = Args { pos: vec![], opts: HashMap::new(), flags: vec![] };
    let mut i = 0;
    while i < raw.len() {
        let s = &raw[i];
        if let Some(name) = s.strip_prefix("--") {
            let (k, inline) = match name.split_once('=') {
                Some((k, v)) => (k.to_string(), Some(v.to_string())),
                None => (name.to_string(), None),
            };
            if VALUE_OPTS.contains(&k.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        raw.get(i).cloned().ok_or(format!("--{} needs a value", k))?
                    }
                };
                a.opts.insert(k, v);
            } else {
                a.flags.push(k);
            }
        } else if s.len() > 1 && s.starts_with('-') && !s.starts_with("-/") {
            for c in s[1..].chars() {
                a.flags.push(match c {
                    'r' => "recursive".into(),
                    'q' => "quiet".into(),
                    'h' => "help".into(),
                    'V' => "version".into(),
                    'y' => "yes".into(),
                    other => format!("-{}", other),
                });
            }
        } else {
            a.pos.push(s.clone());
        }
        i += 1;
    }
    Ok(a)
}

impl Args {
    fn flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }
    fn opt(&self, k: &str) -> Option<&str> {
        self.opts.get(k).map(|s| s.as_str())
    }
    fn need(&self, i: usize, what: &str) -> Result<&str, String> {
        self.pos.get(i).map(|s| s.as_str()).ok_or_else(|| format!("missing {}", what))
    }
}

pub fn js(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Run `f` while printing a progress line to stderr.
fn progress<T: Send>(quiet: bool, label: &str, f: impl FnOnce(&Progress) -> T + Send) -> T {
    let prog = Progress::default();
    let done = AtomicBool::new(false);
    let show = !quiet && std::io::stderr().is_terminal();
    
    std::thread::scope(|s| {
        if show {
            let (p, d) = (&prog, &done);
            s.spawn(move || {
                let t0 = std::time::Instant::now();
                while !d.load(Ordering::Relaxed) {
                    let st = p.status.lock().unwrap().clone();
                    let pct = if p.total() > 0 { format!("{:5.1}%", p.fraction() * 100.0) } else { "  ...".into() };
                    let speed = p.done() as f64 / t0.elapsed().as_secs_f64().max(0.01);
                    let mut line = format!("{} {} {} / {}  {}/s  {}", label, pct, human(p.done()), human(p.total()), human(speed as u64), st);
                    line.truncate(110);
                    eprint!("\r{:<110}", line);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                eprint!("\r{:<110}\r", "");
            });
        }
        let r = f(&prog);
        done.store(true, Ordering::Relaxed);
        r
    })
}

fn es<T>(r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

fn split_path(p: &str) -> Vec<String> {
    p.split(['/', '\\']).filter(|s| !s.is_empty() && *s != ".").map(|s| s.to_string()).collect()
}

fn open_doc(path: &str, a: &Args) -> Result<Document, String> {
    let p = PathBuf::from(path);
    if !p.exists() {
        return Err(format!("{}: no such file", path));
    }
    let pw = a.opt("password");
    let mut d = match es(ops::open_path(&p, pw))? {
        OpenResult::Doc(d) => *d,
        OpenResult::NeedPassword => return Err(format!("{} is encrypted: pass --password", path)),
    };
    if let Some(fs) = a.opt("fs") {
        let src = d.src.clone().ok_or("--fs only works on disc images")?;
        let view = d.fs_views.iter().find(|v| v.eq_ignore_ascii_case(fs)).cloned().ok_or_else(|| format!("no '{}' file system here (available: {})", fs, d.fs_views.join(", ")))?;
        let mut d2 = es(ops::doc_from_source(src, Some(&view)))?;
        d2.path = d.path.take();
        d2.password = d.password.take();
        d = d2;
    }
    Ok(d)
}

fn find<'a>(d: &'a Document, p: &str) -> Result<&'a Entry, String> {
    let parts = split_path(p);
    if parts.is_empty() {
        return Ok(&d.root);
    }
    d.find(&parts).ok_or_else(|| format!("'{}' is not in the image", p))
}

/// Save an edited document: over the original (if it is a writable format) or to --out.
fn save_edit(d: &Document, a: &Args) -> Result<(), String> {
    let target = match a.opt("out") {
        Some(o) => PathBuf::from(o),
        None => {
            let p = d.path.clone().ok_or("nothing to save")?;
            if !crate::app::HexApp::savable(&p) {
                return Err(format!("{} can't be rewritten in place; pass --out <file.iso>", p.display()));
            }
            p
        }
    };
    let fmt = OutFormat::from_path(&target);
    let pw = a.opt("encrypt").or(if a.opt("out").is_none() { a.opt("password") } else { None }).map(|s| s.to_string());
    let n = progress(a.flag("quiet"), "saving", |p| ops::save_doc(d, &target, fmt, pw.as_deref(), p)).map_err(|e| e.to_string())?;
    println!("{} ({})", target.display(), human(n));
    Ok(())
}

fn add_sources(dir: &mut Entry, sources: &[String]) -> Result<usize, String> {
    let kids = dir.children_mut().ok_or("target is not a folder")?;
    let mut n = 0;
    for s in sources {
        let e = Entry::from_disk(Path::new(s)).map_err(|e| format!("{}: {}", s, e))?;
        kids.retain(|k| !k.name.eq_ignore_ascii_case(&e.name));
        kids.push(e);
        n += 1;
    }
    Ok(n)
}

fn boot_spec(d: &Document, val: &str, efi: bool) -> Result<BootSpec, String> {
    let platform = if efi { 0xEF } else { 0 };
    let parts = split_path(val);
    if let Some(Entry { kind: Kind::File { size, src }, name, .. }) = d.find(&parts) {
        return Ok(BootSpec { platform, emulation: 0, load_sectors: if efi { 0 } else { 4 }, info_table: !efi && name.to_lowercase().contains("isolinux"), size: *size, src: src.clone(), tree_path: Some(parts.clone()), label: parts.join("/") });
    }
    let p = PathBuf::from(val);
    let md = std::fs::metadata(&p).map_err(|_| format!("boot image '{}' is neither in the image nor on disk", val))?;
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    Ok(BootSpec { platform, emulation: 0, load_sectors: if efi { 0 } else { 4 }, info_table: !efi && name.to_lowercase().contains("isolinux"), size: md.len(), src: FileSrc::Disk(p), tree_path: None, label: format!("[hidden] {}", name) })
}

fn apply_boot_opts(d: &mut Document, a: &Args) -> Result<bool, String> {
    let mut changed = false;
    for (k, efi) in [("bios", false), ("uefi", true)] {
        if let Some(v) = a.opt(k) {
            let b = boot_spec(d, v, efi)?;
            d.boot.retain(|x| x.platform != b.platform);
            d.boot.push(b);
            d.boot.sort_by_key(|x| x.platform);
            changed = true;
        }
    }
    Ok(changed)
}

fn walk(e: &Entry, prefix: &str, rec: bool, out: &mut Vec<(String, bool, u64, i64, Option<u64>)>) {
    for k in e.children().into_iter().flatten() {
        let p = if prefix.is_empty() { k.name.clone() } else { format!("{}/{}", prefix, k.name) };
        out.push((p.clone(), k.is_dir(), k.size(), k.mtime, k.lba));
        if rec && k.is_dir() {
            walk(k, &p, rec, out);
        }
    }
}

fn drive_letter(a: &Args) -> Result<char, String> {
    let d = a.opt("drive").ok_or("pass --drive <letter>")?;
    d.chars().next().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).ok_or_else(|| format!("bad drive '{}'", d))
}

fn iso_for(path: &Path, a: &Args) -> Result<PathBuf, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if matches!(ext.as_str(), "iso" | "vhd" | "vhdx") {
        return Ok(path.to_path_buf());
    }
    let src = match es(open_image(path, a.opt("password")))? {
        Opened::Sectors(s) => s,
        Opened::NeedPassword => return Err("image is encrypted: pass --password".into()),
        Opened::Zip => return Err("a ZIP archive is not a disc image".into()),
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
    let tmp = crate::app::temp_dir("iso").join(format!("{}.iso", stem));
    progress(a.flag("quiet"), "preparing ISO", |p| ops::convert(&src, &tmp, OutFormat::Iso, None, p)).map_err(|e| e.to_string())?;
    Ok(tmp)
}

/// Returns the process exit code.
pub fn run(raw: Vec<String>) -> i32 {
    let a = match parse(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("hexiso: {}\nrun 'hexiso help' for usage", e);
            return 2;
        }
    };
    let cmd = a.pos.first().cloned().unwrap_or_default();
    let rest = Args { pos: a.pos.iter().skip(1).cloned().collect(), opts: a.opts.clone(), flags: a.flags.clone() };
    if a.flag("help") || cmd == "help" || cmd.is_empty() {
        print!("{}", HELP);
        return 0;
    }
    if a.flag("version") || cmd == "version" {
        println!("hexiso {} - by Nick Petropoulos (https://github.com/Altair47) - AGPL-3.0", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    match command(&cmd, &rest) {
        Ok(()) => 0,
        Err(e) if e.starts_with("usage:") => {
            eprintln!("hexiso {}: {}", cmd, e);
            2
        }
        Err(e) => {
            eprintln!("hexiso {}: {}", cmd, e);
            1
        }
    }
}

fn command(cmd: &str, a: &Args) -> Result<(), String> {
    let json = a.flag("json");
    let quiet = a.flag("quiet");
    match cmd {
        "info" => {
            let path = a.need(0, "usage: hexiso info <image>")?;
            let d = open_doc(path, a)?;
            let (files, dirs) = d.root.count();
            let image_bytes = d.src.as_ref().map(|s| s.sector_count() * SECTOR);
            if json {
                let boot: Vec<String> = d.boot.iter().map(|b| format!("{{\"platform\":{},\"image\":{},\"size\":{},\"in_tree\":{}}}", js(if b.platform == 0xEF { "uefi" } else { "bios" }), js(&b.label), b.size, b.tree_path.is_some())).collect();
                let info: Vec<String> = d.info.iter().map(|(k, v)| format!("{}:{}", js(k), js(v))).collect();
                let views: Vec<String> = d.fs_views.iter().map(|v| js(v)).collect();
                println!(
                    "{{\"path\":{},\"format\":{},\"filesystem\":{},\"filesystems\":[{}],\"label\":{},\"files\":{},\"folders\":{},\"data_bytes\":{},\"image_bytes\":{},\"boot\":[{}],\"details\":{{{}}}}}",
                    js(path), js(&d.format), js(&d.fs_name), views.join(","), js(&d.label), files, dirs.saturating_sub(1), d.root.size(),
                    image_bytes.map(|b| b.to_string()).unwrap_or_else(|| "null".into()), boot.join(","), info.join(",")
                );
            } else {
                println!("file        {}", path);
                println!("format      {}", d.format);
                println!("file system {}   (available: {})", d.fs_name, d.fs_views.join(", "));
                println!("label       {}", d.label);
                println!("content     {} files, {} folders, {}", files, dirs.saturating_sub(1), human(d.root.size()));
                if let Some(b) = image_bytes {
                    println!("image size  {} ({} sectors)", human(b), b / SECTOR);
                }
                if d.boot.is_empty() {
                    println!("bootable    no");
                }
                for b in &d.boot {
                    println!("boot        {} {} ({})", if b.platform == 0xEF { "UEFI" } else { "BIOS" }, b.label, human(b.size));
                }
                for (k, v) in &d.info {
                    println!("{:<11} {}", k.to_lowercase(), v);
                }
            }
            Ok(())
        }
        "ls" => {
            let path = a.need(0, "usage: hexiso ls <image> [path] [-r]")?;
            let d = open_doc(path, a)?;
            let inner = a.pos.get(1).map(|s| s.as_str()).unwrap_or("/");
            let e = find(&d, inner)?;
            let mut rows = Vec::new();
            if e.is_dir() {
                walk(e, &split_path(inner).join("/"), a.flag("recursive"), &mut rows);
            } else {
                rows.push((split_path(inner).join("/"), false, e.size(), e.mtime, e.lba));
            }
            if json {
                let v: Vec<String> = rows.iter().map(|(p, dir, s, t, l)| format!("{{\"path\":{},\"type\":{},\"size\":{},\"modified\":{},\"lba\":{}}}", js(p), js(if *dir { "dir" } else { "file" }), s, t, l.map(|x| x.to_string()).unwrap_or_else(|| "null".into()))).collect();
                println!("[{}]", v.join(","));
            } else {
                for (p, dir, s, t, _) in rows {
                    println!("{} {:>12} {}  {}{}", if dir { "d" } else { "-" }, s, fmt_time(t), p, if dir { "/" } else { "" });
                }
            }
            Ok(())
        }
        "extract" => {
            let path = a.need(0, "usage: hexiso extract <image> <dest-dir> [paths...]")?;
            let dest = PathBuf::from(a.need(1, "usage: hexiso extract <image> <dest-dir> [paths...]")?);
            let d = open_doc(path, a)?;
            let items: Vec<Entry> = if a.pos.len() > 2 {
                a.pos[2..].iter().map(|p| find(&d, p).cloned()).collect::<Result<_, _>>()?
            } else {
                d.root.children().cloned().unwrap_or_default()
            };
            es(std::fs::create_dir_all(&dest))?;
            let (n, failed) = progress(quiet, "extracting", |p| ops::extract(&d, &items, &dest, p)).map_err(|e| e.to_string())?;
            println!("extracted {} file(s) to {}{}", n, dest.display(), if failed > 0 { format!(", {} failed", failed) } else { String::new() });
            if failed > 0 { Err(format!("{} file(s) could not be read", failed)) } else { Ok(()) }
        }
        "create" | "floppy" => {
            let out = PathBuf::from(a.need(0, "usage: hexiso create <out> <files/folders...>")?);
            if a.pos.len() < 2 {
                return Err("usage: give at least one file or folder to put in the image".into());
            }
            let mut d = Document::new_blank();
            d.label = a.opt("label").map(|s| s.to_string()).unwrap_or_else(|| out.file_stem().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_else(|| "NEW_IMAGE".into()));
            add_sources(&mut d.root, &a.pos[1..])?;
            apply_boot_opts(&mut d, a)?;
            let fmt = if cmd == "floppy" { if a.flag("big") { OutFormat::Floppy288 } else { OutFormat::Floppy144 } } else { OutFormat::from_path(&out) };
            let n = progress(quiet, "writing", |p| ops::save_doc(&d, &out, fmt, a.opt("encrypt"), p)).map_err(|e| e.to_string())?;
            println!("{} ({}, {} files)", out.display(), human(n), d.root.count().0);
            Ok(())
        }
        "add" => {
            let path = a.need(0, "usage: hexiso add <image> <files/folders...> [--to path]")?;
            if a.pos.len() < 2 {
                return Err("usage: give at least one file or folder to add".into());
            }
            let mut d = open_doc(path, a)?;
            let to = split_path(a.opt("to").unwrap_or("/"));
            // create the target folders if needed
            let mut cur = &mut d.root;
            for part in &to {
                let kids = cur.children_mut().ok_or("--to points into a file")?;
                let i = match kids.iter().position(|k| k.name.eq_ignore_ascii_case(part) && k.is_dir()) {
                    Some(i) => i,
                    None => {
                        kids.push(Entry::dir(part, vec![]));
                        kids.len() - 1
                    }
                };
                cur = &mut kids[i];
            }
            let n = add_sources(cur, &a.pos[1..])?;
            eprintln!("added {} item(s) to /{}", n, to.join("/"));
            save_edit(&d, a)
        }
        "rm" => {
            let path = a.need(0, "usage: hexiso rm <image> <paths...>")?;
            if a.pos.len() < 2 {
                return Err("usage: give the paths to remove".into());
            }
            let mut d = open_doc(path, a)?;
            for p in &a.pos[1..] {
                let parts = split_path(p);
                let (last, parent) = parts.split_last().ok_or("can't remove the top folder")?;
                let kids = d.dir_at_mut(parent).and_then(|e| e.children_mut()).ok_or_else(|| format!("'{}' is not in the image", p))?;
                let before = kids.len();
                kids.retain(|k| &k.name != last);
                if kids.len() == before {
                    return Err(format!("'{}' is not in the image", p));
                }
                d.boot.retain(|b| b.tree_path.as_ref() != Some(&parts));
            }
            save_edit(&d, a)
        }
        "rename" => {
            let path = a.need(0, "usage: hexiso rename <image> <path> <new-name>")?;
            let from = split_path(a.need(1, "usage: hexiso rename <image> <path> <new-name>")?);
            let new = a.need(2, "usage: hexiso rename <image> <path> <new-name>")?.to_string();
            let mut d = open_doc(path, a)?;
            let (last, parent) = from.split_last().ok_or("can't rename the top folder")?;
            let kids = d.dir_at_mut(parent).and_then(|e| e.children_mut()).ok_or("folder not found")?;
            if kids.iter().any(|k| k.name == new) {
                return Err(format!("'{}' already exists", new));
            }
            kids.iter_mut().find(|k| &k.name == last).ok_or("not found")?.name = new;
            save_edit(&d, a)
        }
        "mkdir" => {
            let path = a.need(0, "usage: hexiso mkdir <image> <path>")?;
            let parts = split_path(a.need(1, "usage: hexiso mkdir <image> <path>")?);
            let mut d = open_doc(path, a)?;
            let mut cur = &mut d.root;
            for part in &parts {
                let kids = cur.children_mut().ok_or("path goes through a file")?;
                let i = match kids.iter().position(|k| &k.name == part && k.is_dir()) {
                    Some(i) => i,
                    None => {
                        kids.push(Entry::dir(part, vec![]));
                        kids.len() - 1
                    }
                };
                cur = &mut kids[i];
            }
            save_edit(&d, a)
        }
        "label" => {
            let path = a.need(0, "usage: hexiso label <image> <new-label>")?;
            let label = a.need(1, "usage: hexiso label <image> <new-label>")?.to_string();
            let mut d = open_doc(path, a)?;
            d.label = label;
            save_edit(&d, a)
        }
        "boot" => {
            let path = a.need(0, "usage: hexiso boot <image> [--bios f] [--uefi f] [--remove] [--extract dir]")?;
            let mut d = open_doc(path, a)?;
            if let Some(dir) = a.opt("extract") {
                es(std::fs::create_dir_all(dir))?;
                for b in &d.boot {
                    let name = if b.platform == 0xEF { "uefi-boot.img" } else { "bios-boot.bin" };
                    let data = es(d.read_all(&b.src, b.size))?;
                    es(std::fs::write(Path::new(dir).join(name), data))?;
                    println!("{}", Path::new(dir).join(name).display());
                }
                return Ok(());
            }
            let mut changed = false;
            if a.flag("remove") {
                d.boot.clear();
                changed = true;
            }
            changed |= apply_boot_opts(&mut d, a)?;
            if changed {
                return save_edit(&d, a);
            }
            if d.boot.is_empty() {
                println!("not bootable");
            }
            for b in &d.boot {
                println!("{} {} ({} bytes)", if b.platform == 0xEF { "UEFI" } else { "BIOS" }, b.label, b.size);
            }
            Ok(())
        }
        "convert" => {
            let inp = PathBuf::from(a.need(0, "usage: hexiso convert <in> <out>")?);
            let out = PathBuf::from(a.need(1, "usage: hexiso convert <in> <out>")?);
            let fmt = OutFormat::from_path(&out);
            if !matches!(fmt, OutFormat::Iso | OutFormat::Cso | OutFormat::Hxz) {
                return Err("output must end in .iso, .cso or .hxz".into());
            }
            let src = match es(open_image(&inp, a.opt("password")))? {
                Opened::Sectors(s) => s,
                Opened::NeedPassword => return Err("input is encrypted: pass --password".into()),
                Opened::Zip => return Err("a ZIP archive is not a disc image".into()),
            };
            let n = progress(quiet, "converting", |p| ops::convert(&src, &out, fmt, a.opt("encrypt"), p)).map_err(|e| e.to_string())?;
            println!("{} ({} -> {})", out.display(), human(src.sector_count() * SECTOR), human(n));
            Ok(())
        }
        "checksum" => {
            let path = a.need(0, "usage: hexiso checksum <file> [--in <path-inside-image>]")?;
            let sums = match a.opt("in") {
                Some(inner) => {
                    let d = open_doc(path, a)?;
                    let e = find(&d, inner)?;
                    let Kind::File { size, src } = &e.kind else { return Err("--in must name a file".into()) };
                    let r = es(d.reader(src, *size))?;
                    progress(quiet, "hashing", |p| ops::checksum(r, *size, p)).map_err(|e| e.to_string())?
                }
                None => {
                    let f = es(std::fs::File::open(path))?;
                    let n = f.metadata().map(|m| m.len()).unwrap_or(0);
                    progress(quiet, "hashing", |p| ops::checksum(std::io::BufReader::with_capacity(4 << 20, f), n, p)).map_err(|e| e.to_string())?
                }
            };
            if json {
                println!("{{\"bytes\":{},\"crc32\":{},\"md5\":{},\"sha1\":{},\"sha256\":{}}}", sums.bytes, js(&sums.crc32), js(&sums.md5), js(&sums.sha1), js(&sums.sha256));
            } else {
                println!("CRC32   {}\nMD5     {}\nSHA-1   {}\nSHA-256 {}", sums.crc32, sums.md5, sums.sha1, sums.sha256);
            }
            Ok(())
        }
        "test" => {
            let path = a.need(0, "usage: hexiso test <image>")?;
            let d = open_doc(path, a)?;
            let (ok, bad) = progress(quiet, "testing", |p| ops::test_image(&d, p)).map_err(|e| e.to_string())?;
            if json {
                println!("{{\"ok\":{},\"errors\":[{}]}}", ok, bad.iter().map(|b| js(b)).collect::<Vec<_>>().join(","));
            } else {
                println!("{} file(s) OK, {} with errors", ok, bad.len());
                for b in &bad {
                    println!("  {}", b);
                }
            }
            if bad.is_empty() { Ok(()) } else { Err(format!("{} file(s) failed", bad.len())) }
        }
        "split" => {
            let path = PathBuf::from(a.need(0, "usage: hexiso split <file> --size <MB>")?);
            let mb: u64 = a.opt("size").ok_or("usage: pass --size <MB>")?.parse().map_err(|_| "usage: --size must be a number of MB")?;
            let parts = progress(quiet, "splitting", |p| ops::split_file(&path, mb * 1024 * 1024, p)).map_err(|e| e.to_string())?;
            for p in parts {
                println!("{}", p.display());
            }
            Ok(())
        }
        "drives" => {
            let list = drives::list();
            if json {
                let v: Vec<String> = list.iter().map(|d| format!("{{\"letter\":{},\"kind\":{},\"label\":{},\"filesystem\":{},\"total\":{},\"free\":{},\"ready\":{}}}", js(&d.letter.to_string()), js(&d.kind.name().to_lowercase()), js(&d.label), js(&d.fs), d.total, d.free, d.ready)).collect();
                println!("[{}]", v.join(","));
            } else {
                for d in list {
                    println!("{}:  {:<14} {:<6} {:<16} {}", d.letter, d.kind.name(), d.fs, if d.ready { d.label.clone() } else { "(no media)".into() }, if d.ready { human(d.total) } else { String::new() });
                }
            }
            Ok(())
        }
        "mounts" => {
            let m = drives::mounted();
            if json {
                println!("[{}]", m.iter().map(|(p, l)| format!("{{\"image\":{},\"drive\":{}}}", js(p), js(l))).collect::<Vec<_>>().join(","));
            } else if m.is_empty() {
                println!("no images mounted");
            } else {
                for (p, l) in m {
                    println!("{}:  {}", l, p);
                }
            }
            Ok(())
        }
        "mount" => {
            let path = PathBuf::from(a.need(0, "usage: hexiso mount <image>")?);
            let iso = iso_for(&path, a)?;
            let full = std::fs::canonicalize(&iso).unwrap_or(iso.clone());
            let full_s = full.display().to_string().trim_start_matches(r"\\?\").to_string();
            if let Some((_, l)) = drives::mounted().into_iter().find(|(p, _)| p.eq_ignore_ascii_case(&full_s))
                && !a.flag("again") {
                    if json { println!("{{\"drive\":{},\"already\":true}}", js(&l)) } else { println!("{}", l) }
                    eprintln!("(already mounted; pass --again to add another drive)");
                    return Ok(());
                }
            let l = es(drives::mount(Path::new(&full_s)))?;
            if json { println!("{{\"drive\":{},\"already\":false}}", js(&l)) } else { println!("{}", l) }
            Ok(())
        }
        "unmount" | "eject" => {
            let path = PathBuf::from(a.need(0, "usage: hexiso unmount <image>")?);
            let full = std::fs::canonicalize(&path).map(|p| p.display().to_string().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| path.display().to_string());
            es(drives::unmount(Path::new(&full)))?;
            println!("unmounted {}", full);
            Ok(())
        }
        "burn" => {
            let path = PathBuf::from(a.need(0, "usage: hexiso burn <image> --drive <E>")?);
            let l = drive_letter(a)?;
            let iso = iso_for(&path, a)?;
            es(drives::burn(&iso, l))?;
            println!("{} handed to the Windows disc burner for drive {}:", iso.display(), l);
            Ok(())
        }
        "make-image" => {
            let l = drive_letter(a)?;
            let out = PathBuf::from(a.need(0, "usage: hexiso make-image --drive <E> <out>")?);
            #[cfg(windows)]
            {
                let src: crate::source::Src = std::sync::Arc::new(es(crate::source::DriveSource::open(l))?);
                let n = progress(quiet, "reading disc", |p| ops::convert(&src, &out, OutFormat::from_path(&out), a.opt("encrypt"), p)).map_err(|e| e.to_string())?;
                println!("{} ({})", out.display(), human(n));
                Ok(())
            }
            #[cfg(not(windows))]
            {
                let _ = (l, out);
                Err("only available on Windows".into())
            }
        }
        "rip" => {
            let l = drive_letter(a)?;
            let dest = PathBuf::from(a.need(0, "usage: hexiso rip --drive <E> <dest-dir> [--tracks 1,2]")?);
            es(std::fs::create_dir_all(&dest))?;
            let toc = es(drives::read_toc(l))?;
            let want: Option<Vec<u8>> = a.opt("tracks").map(|t| t.split(',').filter_map(|x| x.trim().parse().ok()).collect());
            let tracks: Vec<_> = toc.into_iter().filter(|t| t.audio && want.as_ref().map(|w| w.contains(&t.number)).unwrap_or(true)).collect();
            if tracks.is_empty() {
                return Err("no audio tracks to rip".into());
            }
            let n = progress(quiet, "ripping", |p| drives::rip_tracks(l, &tracks, &dest, p)).map_err(|e| e.to_string())?;
            println!("{} track(s) saved to {}", n, dest.display());
            Ok(())
        }
        "usb" => {
            let path = a.need(0, "usage: hexiso usb <image> --drive <F> [--format FAT32] --yes")?;
            let l = drive_letter(a)?;
            let info = drives::list().into_iter().find(|d| d.letter == l).ok_or_else(|| format!("drive {}: not found", l))?;
            if info.kind != drives::DriveKind::Removable && !a.flag("force") {
                return Err(format!("{}: is not a removable drive (pass --force if you are sure)", l));
            }
            if !a.flag("yes") {
                return Err(format!("this writes to drive {}: ({}). Re-run with --yes to confirm", l, info.label));
            }
            let d = open_doc(path, a)?;
            if let Some(f) = a.opt("format") {
                eprintln!("formatting {}: as {} ...", l, f);
                let label: String = d.label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(11).collect();
                es(drives::format_drive(l, f, if label.is_empty() { "HEXISO" } else { &label }))?;
            }
            let items = d.root.children().cloned().unwrap_or_default();
            let dest = PathBuf::from(format!("{}:\\", l));
            let (n, failed) = progress(quiet, "copying", |p| ops::extract(&d, &items, &dest, p)).map_err(|e| e.to_string())?;
            println!("{} file(s) copied to {}:", n, l);
            if failed > 0 { Err(format!("{} file(s) failed", failed)) } else { Ok(()) }
        }
        "selftest" => {
            let dir = a.pos.first().map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("hexiso-selftest"));
            let extra = a.pos.iter().skip(1).map(PathBuf::from).collect();
            if crate::selftest::run(dir, extra) == 0 { Ok(()) } else { Err("self-test failed".into()) }
        }
        other => Err(format!("usage: unknown command '{}' (run 'hexiso help')", other)),
    }
}
