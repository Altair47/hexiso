// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! The editable file tree of an open image.

use crate::source::{read_range, Src};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Extent offset meaning "sparse, reads as zeros".
pub const SPARSE: u64 = u64::MAX;

#[derive(Clone)]
pub enum FileSrc {
    /// Byte extents (offset, len) inside the image's user data.
    Image(Vec<(u64, u64)>),
    Disk(PathBuf),
    Mem(Arc<Vec<u8>>),
    Zip(usize),
}

#[derive(Clone)]
pub enum Kind {
    Dir(Vec<Entry>),
    File { size: u64, src: FileSrc },
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub mtime: i64,
    pub kind: Kind,
    /// First sector in the source image, if any.
    pub lba: Option<u64>,
    pub hidden: bool,
}

impl Entry {
    pub fn dir(name: &str, children: Vec<Entry>) -> Self {
        Entry { name: name.into(), mtime: now(), kind: Kind::Dir(children), lba: None, hidden: false }
    }
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Dir(_))
    }
    pub fn size(&self) -> u64 {
        match &self.kind {
            Kind::Dir(c) => c.iter().map(|e| e.size()).sum(),
            Kind::File { size, .. } => *size,
        }
    }
    pub fn count(&self) -> (u64, u64) {
        match &self.kind {
            Kind::Dir(c) => {
                let mut f = 0;
                let mut d = 1;
                for e in c {
                    let (a, b) = e.count();
                    f += a;
                    d += b;
                }
                (f, d)
            }
            Kind::File { .. } => (1, 0),
        }
    }
    pub fn children(&self) -> Option<&Vec<Entry>> {
        match &self.kind {
            Kind::Dir(c) => Some(c),
            _ => None,
        }
    }
    pub fn children_mut(&mut self) -> Option<&mut Vec<Entry>> {
        match &mut self.kind {
            Kind::Dir(c) => Some(c),
            _ => None,
        }
    }
    pub fn type_name(&self) -> String {
        if self.is_dir() {
            return "Folder".into();
        }
        match Path::new(&self.name).extension() {
            Some(e) => format!("{} file", e.to_string_lossy().to_uppercase()),
            None => "File".into(),
        }
    }
    /// Load a file or folder from the local disk.
    pub fn from_disk(path: &Path) -> io::Result<Entry> {
        let md = std::fs::metadata(path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "disk".into());
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(now);
        if md.is_dir() {
            let mut kids = Vec::new();
            let mut list: Vec<_> = std::fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            list.sort_by_key(|e| e.file_name());
            for e in list {
                if let Ok(c) = Entry::from_disk(&e.path()) {
                    kids.push(c);
                }
            }
            Ok(Entry { name, mtime, kind: Kind::Dir(kids), lba: None, hidden: false })
        } else {
            Ok(Entry { name, mtime, kind: Kind::File { size: md.len(), src: FileSrc::Disk(path.to_path_buf()) }, lba: None, hidden: false })
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct BootSpec {
    pub platform: u8, // 0 = BIOS x86, 0xEF = UEFI
    pub emulation: u8, // 0 = no emulation
    pub load_sectors: u16, // 512-byte virtual sectors, 0 = auto
    pub info_table: bool,
    pub size: u64,
    pub src: FileSrc,
    /// When the boot image is also a file in the tree.
    pub tree_path: Option<Vec<String>>,
    pub label: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Media {
    Cd700,
    Dvd5,
    Dvd9,
    Bd25,
    Bd50,
}

impl Media {
    pub const ALL: [Media; 5] = [Media::Cd700, Media::Dvd5, Media::Dvd9, Media::Bd25, Media::Bd50];
    pub fn bytes(self) -> u64 {
        match self {
            Media::Cd700 => 737_280_000,
            Media::Dvd5 => 4_700_372_992,
            Media::Dvd9 => 8_547_991_552,
            Media::Bd25 => 25_025_314_816,
            Media::Bd50 => 50_050_629_632,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Media::Cd700 => "CD 700MB",
            Media::Dvd5 => "DVD 4.7GB",
            Media::Dvd9 => "DVD-DL 8.5GB",
            Media::Bd25 => "BD 25GB",
            Media::Bd50 => "BD-DL 50GB",
        }
    }
}

pub struct ZipHandle(pub Mutex<zip::ZipArchive<File>>);

#[derive(Clone)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub format: String,
    pub fs_name: String,
    pub fs_views: Vec<String>,
    pub src: Option<Src>,
    pub zip: Option<Arc<ZipHandle>>,
    pub root: Entry,
    pub label: String,
    pub boot: Vec<BootSpec>,
    pub modified: bool,
    pub media: Media,
    pub info: Vec<(String, String)>,
    pub password: Option<String>,
}

impl Document {
    pub fn new_blank() -> Self {
        Document {
            path: None,
            format: "New data image".into(),
            fs_name: "ISO 9660 + Joliet".into(),
            fs_views: vec![],
            src: None,
            zip: None,
            root: Entry::dir("", vec![]),
            label: "NEW_IMAGE".into(),
            boot: vec![],
            modified: false,
            media: Media::Dvd5,
            info: vec![],
            password: None,
        }
    }
    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            None => "untitled.iso".into(),
        }
    }
    pub fn dir_at(&self, path: &[String]) -> Option<&Entry> {
        let mut cur = &self.root;
        for p in path {
            cur = cur.children()?.iter().find(|e| &e.name == p && e.is_dir())?;
        }
        Some(cur)
    }
    pub fn dir_at_mut(&mut self, path: &[String]) -> Option<&mut Entry> {
        let mut cur = &mut self.root;
        for p in path {
            cur = cur.children_mut()?.iter_mut().find(|e| &e.name == p && e.is_dir())?;
        }
        Some(cur)
    }
    pub fn find(&self, path: &[String]) -> Option<&Entry> {
        let (last, dir) = path.split_last()?;
        self.dir_at(dir)?.children()?.iter().find(|e| &e.name == last)
    }

    /// A reader over a file's content.
    pub fn reader(&self, src: &FileSrc, size: u64) -> io::Result<Box<dyn Read + Send>> {
        Ok(match src {
            FileSrc::Disk(p) => Box::new(File::open(p)?.take(size)),
            FileSrc::Mem(m) => Box::new(io::Cursor::new(m.clone().as_ref().clone())),
            FileSrc::Image(ext) => {
                let s = self.src.clone().ok_or_else(|| crate::source::bad("image source is closed"))?;
                Box::new(ExtentReader { src: s, extents: ext.clone(), idx: 0, pos: 0, buf: Vec::new(), bpos: 0 })
            }
            FileSrc::Zip(i) => {
                let z = self.zip.clone().ok_or_else(|| crate::source::bad("archive is closed"))?;
                let mut a = z.0.lock().unwrap();
                let mut f = a.by_index(*i).map_err(|e| io::Error::other(e.to_string()))?;
                let mut v = Vec::with_capacity(size as usize);
                f.read_to_end(&mut v)?;
                Box::new(io::Cursor::new(v))
            }
        })
    }

    pub fn read_all(&self, src: &FileSrc, size: u64) -> io::Result<Vec<u8>> {
        let mut v = Vec::with_capacity(size.min(1 << 30) as usize);
        self.reader(src, size)?.read_to_end(&mut v)?;
        Ok(v)
    }
}

pub struct ExtentReader {
    src: Src,
    extents: Vec<(u64, u64)>,
    idx: usize,
    pos: u64,
    buf: Vec<u8>,
    bpos: usize,
}

impl Read for ExtentReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.bpos >= self.buf.len() {
            // refill
            loop {
                if self.idx >= self.extents.len() {
                    return Ok(0);
                }
                let (off, len) = self.extents[self.idx];
                if self.pos >= len {
                    self.idx += 1;
                    self.pos = 0;
                    continue;
                }
                let n = (len - self.pos).min(1 << 20) as usize;
                self.buf.resize(n, 0);
                if off == SPARSE {
                    self.buf.fill(0);
                } else {
                    read_range(self.src.as_ref(), off + self.pos, &mut self.buf)?;
                }
                self.pos += n as u64;
                self.bpos = 0;
                break;
            }
        }
        let n = (self.buf.len() - self.bpos).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.bpos..self.bpos + n]);
        self.bpos += n;
        Ok(n)
    }
}

pub fn human(n: u64) -> String {
    let f = n as f64;
    if n < 1024 {
        format!("{} B", n)
    } else if f < 1024.0 * 1024.0 {
        format!("{:.1} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / 1048576.0)
    } else {
        format!("{:.2} GB", f / 1073741824.0)
    }
}

/// Civil date from unix seconds: (y, m, d, hh, mm, ss)
pub fn civil(ts: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, (secs / 3600) as u32, ((secs % 3600) / 60) as u32, (secs % 60) as u32)
}

pub fn unix_from_civil(y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64
}

pub fn fmt_time(ts: i64) -> String {
    if ts <= 0 {
        return "-".into();
    }
    let (y, m, d, hh, mm, _) = civil(ts);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hh, mm)
}
