// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! High level operations: open, save, convert, extract, split, checksum, test.

use crate::fs_fat::{looks_like_fat, partitions, Fat};
use crate::fs_iso::{self, Names};
use crate::fs_udf::{self, Udf};
use crate::hxz::{CsoWriter, HxzWriter};
use crate::jobs::Progress;
use crate::model::{human, Document, Entry, FileSrc, Kind};
use crate::source::{open_image, read_range, Opened, Src, SECTOR};
use crate::writer::{estimate, write_iso, WriteOpts};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub enum OpenResult {
    Doc(Box<Document>),
    NeedPassword,
}

pub fn open_path(path: &Path, password: Option<&str>) -> io::Result<OpenResult> {
    match open_image(path, password)? {
        Opened::NeedPassword => Ok(OpenResult::NeedPassword),
        Opened::Zip => Ok(OpenResult::Doc(Box::new(open_zip(path)?))),
        Opened::Sectors(src) => {
            let mut d = doc_from_source(src, None)?;
            d.path = Some(path.to_path_buf());
            d.password = password.map(|s| s.to_string());
            Ok(OpenResult::Doc(Box::new(d)))
        }
    }
}

/// Which file system views a source offers.
pub fn fs_views(src: &Src) -> Vec<String> {
    let mut v = Vec::new();
    let s = src.as_ref();
    if fs_udf::detect(s) {
        v.push("UDF".into());
    }
    let info = fs_iso::probe(s);
    if info.joliet.is_some() {
        v.push("Joliet".into());
    }
    if info.rock_ridge {
        v.push("Rock Ridge".into());
    }
    if info.pvd.is_some() {
        v.push("ISO 9660".into());
    }
    let mut b = vec![0u8; 512];
    if v.is_empty() && read_range(s, 0, &mut b).is_ok() {
        if looks_like_fat(&b) {
            v.push("FAT".into());
        } else if !partitions(s).is_empty() {
            v.push("Partitions".into());
        }
    }
    v
}

pub fn doc_from_source(src: Src, view: Option<&str>) -> io::Result<Document> {
    let views = fs_views(&src);
    let mut doc = Document::new_blank();
    doc.format = src.format_name();
    doc.src = Some(src.clone());
    doc.fs_views = views.clone();
    let size = src.sector_count() * SECTOR;
    doc.media = crate::model::Media::ALL.iter().copied().find(|m| m.bytes() >= size).unwrap_or(crate::model::Media::Bd50);
    let mut want: Vec<String> = match view {
        Some(v) => vec![v.to_string()],
        None => vec![],
    };
    // preference: UDF when it holds more than the ISO tree (Windows discs), Rock Ridge, Joliet, ISO
    want.extend(["UDF", "Rock Ridge", "Joliet", "ISO 9660", "FAT", "Partitions"].iter().map(|s| s.to_string()));
    let s = src.as_ref();
    let info = fs_iso::probe(s);
    let mut last_err = None;
    let mut best: Option<(String, Entry, String)> = None;
    for w in want {
        if !views.contains(&w) {
            continue;
        }
        let r: io::Result<(Entry, String)> = match w.as_str() {
            "UDF" => Udf::open(s).and_then(|u| {
                let t = u.read_tree()?;
                doc.info.push(("UDF revision".into(), if u.revision.is_empty() { "?".into() } else { u.revision.clone() }));
                Ok((t, u.label.clone()))
            }),
            "Rock Ridge" => fs_iso::read_tree(s, info.pvd.as_ref().unwrap(), Names::RockRidge).map(|t| (t, fs_iso::label_of(info.pvd.as_ref().unwrap(), false))),
            "Joliet" => fs_iso::read_tree(s, info.joliet.as_ref().unwrap(), Names::Joliet).map(|t| (t, fs_iso::label_of(info.joliet.as_ref().unwrap(), true))),
            "ISO 9660" => fs_iso::read_tree(s, info.pvd.as_ref().unwrap(), Names::Plain).map(|t| (t, fs_iso::label_of(info.pvd.as_ref().unwrap(), false))),
            "FAT" => Fat::open(s, 0).and_then(|f| {
                let t = f.read_tree(s)?;
                doc.info.push(("FAT type".into(), f.kind_name()));
                Ok((t, f.label.clone()))
            }),
            "Partitions" => {
                let mut kids = Vec::new();
                for (i, (off, len, desc)) in partitions(s).into_iter().enumerate() {
                    let name = match Fat::open(s, off) {
                        Ok(f) => {
                            let mut t = f.read_tree(s).unwrap_or_else(|_| Entry::dir("", vec![]));
                            t.name = format!("Partition {} ({}, {})", i + 1, f.kind_name(), human(len));
                            kids.push(t);
                            continue;
                        }
                        Err(_) => format!("Partition {} ({}, {}) - unsupported file system", i + 1, desc, human(len)),
                    };
                    kids.push(Entry::dir(&name, vec![]));
                }
                Ok((Entry::dir("", kids), "DISK".into()))
            }
            _ => continue,
        };
        match r {
            Ok((t, label)) => {
                let better = match &best {
                    None => true,
                    Some((_, b, _)) => view.is_none() && b.count().0 < 4 && t.count().0 > b.count().0,
                };
                if best.is_none() || better {
                    best = Some((w.clone(), t, label));
                }
                if view.is_some() || best.as_ref().map(|b| b.1.count().0 >= 4).unwrap_or(false) {
                    break;
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    match best {
        Some((fs, tree, label)) => {
            doc.fs_name = fs;
            doc.root = tree;
            doc.label = label;
        }
        None => {
            if let Some(e) = last_err {
                return Err(e);
            }
            doc.fs_name = "none found".into();
            doc.info.push(("Note".into(), "No supported file system; sector view and conversion still work".into()));
        }
    }
    if let Some(cat) = info.boot_catalog {
        let iso_tree = if doc.fs_name == "UDF" {
            info.pvd.as_ref().and_then(|p| fs_iso::read_tree(s, p, Names::Plain).ok())
        } else {
            None
        };
        doc.boot = fs_iso::read_boot(s, cat, iso_tree.as_ref().unwrap_or(&doc.root));
        if iso_tree.is_some() {
            for b in doc.boot.iter_mut() {
                b.tree_path = None; // path came from the ISO view; keep the data as a hidden image
            }
        }
    }
    for (k, v) in src.details() {
        doc.info.push((k, v));
    }
    Ok(doc)
}

fn open_zip(path: &Path) -> io::Result<Document> {
    let f = File::open(path)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| io::Error::other(e.to_string()))?;
    let mut root = Entry::dir("", vec![]);
    for i in 0..z.len() {
        let zf = z.by_index_raw(i).map_err(|e| io::Error::other(e.to_string()))?;
        let name = zf.name().map_err(|e| io::Error::other(e.to_string()))?.replace('\\', "/");
        let is_dir = zf.is_dir();
        let size = zf.size();
        drop(zf);
        let parts: Vec<String> = name.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        if parts.is_empty() {
            continue;
        }
        let mut cur = &mut root;
        for (k, p) in parts.iter().enumerate() {
            let last = k + 1 == parts.len();
            let kids = cur.children_mut().unwrap();
            if last && !is_dir {
                kids.push(Entry { name: p.clone(), mtime: crate::model::now(), kind: Kind::File { size, src: FileSrc::Zip(i) }, lba: None, hidden: false });
                break;
            }
            let pos = match kids.iter().position(|e| &e.name == p && e.is_dir()) {
                Some(x) => x,
                None => {
                    kids.push(Entry::dir(p, vec![]));
                    kids.len() - 1
                }
            };
            cur = &mut kids[pos];
        }
    }
    let mut d = Document::new_blank();
    d.path = Some(path.to_path_buf());
    d.format = "ZIP archive".into();
    d.fs_name = "ZIP".into();
    d.label = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    d.root = root;
    d.zip = Some(Arc::new(crate::model::ZipHandle(Mutex::new(z))));
    Ok(d)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutFormat {
    Iso,
    Cso,
    Hxz,
    Zip,
    Floppy144,
    Floppy288,
}

impl OutFormat {
    pub fn from_path(p: &Path) -> OutFormat {
        match p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
            Some("cso") => OutFormat::Cso,
            Some("hxz") => OutFormat::Hxz,
            Some("zip") => OutFormat::Zip,
            Some("img") | Some("ima") | Some("flp") => OutFormat::Floppy144,
            _ => OutFormat::Iso,
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            OutFormat::Iso => "iso",
            OutFormat::Cso => "cso",
            OutFormat::Hxz => "hxz",
            OutFormat::Zip => "zip",
            OutFormat::Floppy144 | OutFormat::Floppy288 => "img",
        }
    }
}

fn temp_beside(target: &Path) -> PathBuf {
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut p = target.to_path_buf();
    let name = format!(".{}.hexiso-tmp", target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    p.set_file_name(name);
    p
}

/// Save a document tree. Writes to a temp file first so the source image can be the target.
pub fn save_doc(doc: &Document, target: &Path, fmt: OutFormat, password: Option<&str>, prog: &Progress) -> io::Result<u64> {
    let tmp = temp_beside(target);
    let r = (|| -> io::Result<u64> {
        let opts = WriteOpts { label: doc.label.clone(), joliet: true };
        match fmt {
            OutFormat::Iso => {
                let f = File::create(&tmp)?;
                write_iso(doc, &opts, f, prog)
            }
            OutFormat::Cso => {
                let mut w = CsoWriter::create(&tmp, estimate(doc))?;
                write_iso(doc, &opts, &mut w, prog)?;
                w.finish()
            }
            OutFormat::Hxz => {
                let mut w = HxzWriter::create(&tmp, password, 6)?;
                write_iso(doc, &opts, &mut w, prog)?;
                w.finish()
            }
            OutFormat::Zip => save_zip(doc, &tmp, prog),
            OutFormat::Floppy144 | OutFormat::Floppy288 => {
                prog.say("building FAT12 floppy");
                let img = crate::fs_fat::make_floppy(doc, &doc.label, fmt == OutFormat::Floppy288)?;
                std::fs::write(&tmp, &img)?;
                Ok(img.len() as u64)
            }
        }
    })();
    match r {
        Ok(n) => {
            let _ = std::fs::remove_file(target);
            std::fs::rename(&tmp, target)?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn save_zip(doc: &Document, path: &Path, prog: &Progress) -> io::Result<u64> {
    let f = File::create(path)?;
    let mut z = zip::ZipWriter::new(io::BufWriter::new(f));
    prog.set_total(doc.root.size());
    fn walk(doc: &Document, e: &Entry, prefix: &str, z: &mut zip::ZipWriter<io::BufWriter<File>>, prog: &Progress) -> io::Result<()> {
        for k in e.children().into_iter().flatten() {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let name = format!("{}{}", prefix, k.name);
            let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(k.size() > 0xFFFF_0000);
            match &k.kind {
                Kind::Dir(_) => {
                    z.add_directory(format!("{}/", name), opts).map_err(|e| io::Error::other(e.to_string()))?;
                    walk(doc, k, &format!("{}/", name), z, prog)?;
                }
                Kind::File { size, src } => {
                    z.start_file(name, opts).map_err(|e| io::Error::other(e.to_string()))?;
                    let mut r = doc.reader(src, *size)?;
                    let n = io::copy(&mut r, z)?;
                    prog.add(n);
                }
            }
        }
        Ok(())
    }
    walk(doc, &doc.root, "", &mut z, prog)?;
    let w = z.finish().map_err(|e| io::Error::other(e.to_string()))?;
    let n = w.into_inner().map_err(|e| io::Error::other(e.to_string()))?.metadata()?.len();
    Ok(n)
}

/// Copy sectors of a source into a new image file (ISO / CSO / HXZ).
pub fn convert(src: &Src, target: &Path, fmt: OutFormat, password: Option<&str>, prog: &Progress) -> io::Result<u64> {
    let total = src.sector_count() * SECTOR;
    prog.set_total(total);
    let tmp = temp_beside(target);
    enum W {
        F(io::BufWriter<File>),
        C(CsoWriter),
        H(HxzWriter),
    }
    let w = match fmt {
        OutFormat::Cso => W::C(CsoWriter::create(&tmp, total)?),
        OutFormat::Hxz => W::H(HxzWriter::create(&tmp, password, 6)?),
        OutFormat::Iso => W::F(io::BufWriter::with_capacity(4 << 20, File::create(&tmp)?)),
        _ => return Err(io::Error::other("choose ISO, CSO or HXZ as the output format")),
    };
    let mut w = w;
    let r = (|| -> io::Result<()> {
        let mut sink = |b: &[u8]| match &mut w {
            W::F(f) => f.write_all(b),
            W::C(c) => c.write_all(b),
            W::H(h) => h.write_all(b),
        };
        let chunk = 512u64;
        let mut buf = vec![0u8; (chunk * SECTOR) as usize];
        let mut lba = 0;
        let n = src.sector_count();
        while lba < n {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let k = chunk.min(n - lba);
            let b = &mut buf[..(k * SECTOR) as usize];
            src.read_sectors(lba, b)?;
            sink(b)?;
            lba += k;
            prog.set_done(lba * SECTOR);
        }
        Ok(())
    })();
    let r = r.and_then(|_| match w {
        W::F(mut f) => f.flush().map(|_| total),
        W::C(c) => c.finish(),
        W::H(h) => h.finish(),
    });
    match r {
        Ok(n) => {
            let _ = std::fs::remove_file(target);
            std::fs::rename(&tmp, target)?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Extract entries into a folder.
pub fn extract(doc: &Document, items: &[Entry], dest: &Path, prog: &Progress) -> io::Result<(u64, u64)> {
    let total: u64 = items.iter().map(|e| e.size()).sum();
    prog.set_total(total);
    let mut files = 0;
    let mut failed = 0;
    fn go(doc: &Document, e: &Entry, dest: &Path, prog: &Progress, files: &mut u64, failed: &mut u64) -> io::Result<()> {
        if prog.cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let safe: String = e.name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
        let p = dest.join(safe);
        match &e.kind {
            Kind::Dir(k) => {
                std::fs::create_dir_all(&p)?;
                for c in k {
                    go(doc, c, &p, prog, files, failed)?;
                }
            }
            Kind::File { size, src } => {
                prog.say(e.name.clone());
                let r = (|| -> io::Result<()> {
                    let mut r = doc.reader(src, *size)?;
                    let mut f = io::BufWriter::with_capacity(1 << 20, File::create(&p)?);
                    let mut buf = vec![0u8; 1 << 20];
                    loop {
                        let n = r.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        f.write_all(&buf[..n])?;
                        prog.add(n as u64);
                        if prog.cancelled() {
                            return Err(io::Error::other("cancelled"));
                        }
                    }
                    f.flush()
                })();
                match r {
                    Ok(_) => *files += 1,
                    Err(e) if e.to_string() == "cancelled" => return Err(e),
                    Err(_) => *failed += 1,
                }
            }
        }
        Ok(())
    }
    for e in items {
        go(doc, e, dest, prog, &mut files, &mut failed)?;
    }
    Ok((files, failed))
}

/// Read every file to verify the image.
pub fn test_image(doc: &Document, prog: &Progress) -> io::Result<(u64, Vec<String>)> {
    prog.set_total(doc.root.size());
    let mut ok = 0;
    let mut bad = Vec::new();
    fn go(doc: &Document, e: &Entry, path: &str, prog: &Progress, ok: &mut u64, bad: &mut Vec<String>) -> io::Result<()> {
        for k in e.children().into_iter().flatten() {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let p = format!("{}/{}", path, k.name);
            match &k.kind {
                Kind::Dir(_) => go(doc, k, &p, prog, ok, bad)?,
                Kind::File { size, src } => {
                    prog.say(p.clone());
                    let r = (|| -> io::Result<u64> {
                        let mut r = doc.reader(src, *size)?;
                        let mut buf = vec![0u8; 1 << 20];
                        let mut n = 0u64;
                        loop {
                            let k = r.read(&mut buf)?;
                            if k == 0 {
                                break;
                            }
                            n += k as u64;
                            prog.add(k as u64);
                        }
                        Ok(n)
                    })();
                    match r {
                        Ok(n) if n == *size => *ok += 1,
                        Ok(n) => bad.push(format!("{}: short read ({} of {} bytes)", p, n, size)),
                        Err(e) => bad.push(format!("{}: {}", p, e)),
                    }
                }
            }
        }
        Ok(())
    }
    go(doc, &doc.root, "", prog, &mut ok, &mut bad)?;
    Ok((ok, bad))
}

pub struct Sums {
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
    pub bytes: u64,
}

pub fn checksum(mut r: impl Read, total: u64, prog: &Progress) -> io::Result<Sums> {
    use md5::Digest;
    prog.set_total(total);
    let mut c = crc32fast::Hasher::new();
    let mut m = md5::Md5::new();
    let mut s1 = sha1::Sha1::new();
    let mut s2 = sha2::Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut n = 0u64;
    loop {
        if prog.cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        let k = r.read(&mut buf)?;
        if k == 0 {
            break;
        }
        let b = &buf[..k];
        std::thread::scope(|s| {
            s.spawn(|| m.update(b));
            s.spawn(|| s1.update(b));
            s.spawn(|| s2.update(b));
            c.update(b);
        });
        n += k as u64;
        prog.set_done(n);
    }
    let hex = |v: &[u8]| v.iter().map(|b| format!("{:02x}", b)).collect::<String>();
    Ok(Sums { crc32: format!("{:08x}", c.finalize()), md5: hex(&m.finalize()), sha1: hex(&s1.finalize()), sha256: hex(&s2.finalize()), bytes: n })
}

/// Split a file into volumes name.001, name.002...
pub fn split_file(src: &Path, part: u64, prog: &Progress) -> io::Result<Vec<PathBuf>> {
    let mut f = File::open(src)?;
    let total = f.metadata()?.len();
    prog.set_total(total);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut i = 1;
    let mut done = 0u64;
    while done < total {
        let p = PathBuf::from(format!("{}.{:03}", src.to_string_lossy(), i));
        let mut w = File::create(&p)?;
        let mut left = part.min(total - done);
        while left > 0 {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let n = (left as usize).min(buf.len());
            f.read_exact(&mut buf[..n])?;
            w.write_all(&buf[..n])?;
            left -= n as u64;
            done += n as u64;
            prog.set_done(done);
        }
        out.push(p);
        i += 1;
    }
    Ok(out)
}

/// Search names in the tree (case-insensitive substring or simple * wildcard).
pub fn search(root: &Entry, query: &str) -> Vec<(Vec<String>, bool, u64)> {
    let q = query.to_lowercase();
    let parts: Vec<&str> = q.split('*').collect();
    let matches = |n: &str| -> bool {
        let n = n.to_lowercase();
        if parts.len() == 1 {
            return n.contains(&q);
        }
        let mut pos = 0;
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() {
                continue;
            }
            match n[pos..].find(p) {
                Some(k) => {
                    if i == 0 && k != 0 {
                        return false;
                    }
                    pos += k + p.len();
                }
                None => return false,
            }
        }
        parts.last().map(|l| l.is_empty() || n.ends_with(l)).unwrap_or(true)
    };
    let mut out = Vec::new();
    fn go(e: &Entry, path: &mut Vec<String>, out: &mut Vec<(Vec<String>, bool, u64)>, m: &dyn Fn(&str) -> bool) {
        for k in e.children().into_iter().flatten() {
            path.push(k.name.clone());
            if m(&k.name) {
                out.push((path.clone(), k.is_dir(), k.size()));
            }
            if k.is_dir() && out.len() < 5000 {
                go(k, path, out, m);
            }
            path.pop();
        }
    }
    go(root, &mut Vec::new(), &mut out, &matches);
    out
}
