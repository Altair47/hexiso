// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Sector sources: every image format is turned into a flat run of 2048-byte user-data sectors.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const SECTOR: u64 = 2048;

pub trait SectorSource: Send + Sync {
    /// Number of 2048-byte sectors.
    fn sector_count(&self) -> u64;
    /// Read whole sectors starting at `lba`. `buf.len()` must be a multiple of 2048.
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()>;
    /// Short description of the container format.
    fn format_name(&self) -> String;
    /// Extra info lines for the properties panel.
    fn details(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

pub type Src = Arc<dyn SectorSource>;

/// Read an arbitrary byte range of user data (unaligned allowed).
pub fn read_range(src: &dyn SectorSource, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let first = offset / SECTOR;
    let skip = (offset % SECTOR) as usize;
    if skip == 0 && buf.len().is_multiple_of(SECTOR as usize) {
        return src.read_sectors(first, buf);
    }
    let end = offset + buf.len() as u64;
    let last = end.div_ceil(SECTOR);
    let count = (last - first) as usize;
    // read in chunks to bound memory
    let mut out_pos = 0usize;
    let mut lba = first;
    let mut skip_now = skip;
    let chunk_secs = 512usize;
    let mut tmp = vec![0u8; chunk_secs.min(count) * SECTOR as usize];
    while out_pos < buf.len() {
        let remaining_secs = (last - lba) as usize;
        let n = remaining_secs.min(chunk_secs);
        let t = &mut tmp[..n * SECTOR as usize];
        src.read_sectors(lba, t)?;
        let avail = t.len() - skip_now;
        let take = avail.min(buf.len() - out_pos);
        buf[out_pos..out_pos + take].copy_from_slice(&t[skip_now..skip_now + take]);
        out_pos += take;
        skip_now = 0;
        lba += n as u64;
    }
    Ok(())
}

pub fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        file.seek_read(buf, offset)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_at(buf, offset)
    }
}

pub fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = read_at(file, buf, offset)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected end of image"));
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// Several files read as one (split volumes .001 .002 ...).
pub struct MultiFile {
    parts: Vec<(File, u64, u64)>, // file, start, len
    pub total: u64,
    pub paths: Vec<PathBuf>,
}

impl MultiFile {
    pub fn open(paths: Vec<PathBuf>) -> io::Result<Self> {
        let mut parts = Vec::new();
        let mut total = 0;
        for p in &paths {
            let f = File::open(p)?;
            let len = f.metadata()?.len();
            parts.push((f, total, len));
            total += len;
        }
        Ok(MultiFile { parts, total, paths })
    }
    pub fn single(path: &Path) -> io::Result<Self> {
        Self::open(vec![path.to_path_buf()])
    }
    pub fn read_at(&self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            if offset >= self.total {
                // past the end: zeros (truncated images)
                buf.fill(0);
                return Ok(());
            }
            let idx = match self.parts.iter().position(|(_, s, l)| offset >= *s && offset < s + l) {
                Some(i) => i,
                None => {
                    buf.fill(0);
                    return Ok(());
                }
            };
            let (f, s, l) = &self.parts[idx];
            let local = offset - s;
            let n = ((l - local) as usize).min(buf.len());
            read_exact_at(f, &mut buf[..n], local)?;
            offset += n as u64;
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

/// Plain or raw-sector image inside a (possibly split) file.
pub struct RawImage {
    pub file: MultiFile,
    pub header: u64,
    pub sector_size: u64,
    pub data_off: u64,
    pub count: u64,
    pub name: String,
}

impl SectorSource for RawImage {
    fn sector_count(&self) -> u64 {
        self.count
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        if self.sector_size == SECTOR && self.data_off == 0 {
            return self.file.read_at(self.header + lba * SECTOR, buf);
        }
        // raw sectors: read a batch of raw sectors then pick user data
        let n = buf.len() / SECTOR as usize;
        let batch = 64usize;
        let mut raw = vec![0u8; batch.min(n.max(1)) * self.sector_size as usize];
        let mut done = 0usize;
        while done < n {
            let k = (n - done).min(batch);
            let r = &mut raw[..k * self.sector_size as usize];
            self.file.read_at(self.header + (lba + done as u64) * self.sector_size, r)?;
            for i in 0..k {
                let s = i * self.sector_size as usize + self.data_off as usize;
                let d = (done + i) * SECTOR as usize;
                buf[d..d + SECTOR as usize].copy_from_slice(&r[s..s + SECTOR as usize]);
            }
            done += k;
        }
        Ok(())
    }
    fn format_name(&self) -> String {
        self.name.clone()
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut v = vec![
            ("Sector layout".into(), format!("{} bytes / sector, data at +{}", self.sector_size, self.data_off)),
        ];
        if self.header > 0 {
            v.push(("Header".into(), format!("{} bytes", self.header)));
        }
        if self.file.paths.len() > 1 {
            v.push(("Volumes".into(), format!("{} parts", self.file.paths.len())));
        }
        v
    }
}

// ------------------------------------------------------------------ CSO (compressed ISO)

pub struct CsoImage {
    file: File,
    index: Vec<u32>,
    align: u32,
    block: u64,
    total: u64,
    cache: Mutex<Option<(u64, Vec<u8>)>>,
}

impl CsoImage {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let mut h = [0u8; 24];
        read_exact_at(&file, &mut h, 0)?;
        if &h[0..4] != b"CISO" {
            return Err(bad("not a CSO file"));
        }
        let header_size = u32le(&h, 4).max(24) as u64;
        let total = u64le(&h, 8);
        let block = u32le(&h, 16) as u64;
        let align = h[21] as u32;
        if block == 0 || !block.is_multiple_of(512) {
            return Err(bad("CSO block size is invalid"));
        }
        let blocks = total.div_ceil(block);
        let mut raw = vec![0u8; ((blocks + 1) * 4) as usize];
        read_exact_at(&file, &mut raw, header_size)?;
        let index = raw.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        Ok(CsoImage { file, index, align, block, total, cache: Mutex::new(None) })
    }
    fn block_data(&self, b: u64) -> io::Result<Vec<u8>> {
        if let Some((cb, d)) = &*self.cache.lock().unwrap()
            && *cb == b {
                return Ok(d.clone());
            }
        let i = b as usize;
        if i + 1 >= self.index.len() {
            return Ok(vec![0; self.block as usize]);
        }
        let e0 = self.index[i];
        let e1 = self.index[i + 1];
        let plain = e0 & 0x8000_0000 != 0;
        let start = ((e0 & 0x7FFF_FFFF) as u64) << self.align;
        let end = ((e1 & 0x7FFF_FFFF) as u64) << self.align;
        let len = end.saturating_sub(start) as usize;
        let mut comp = vec![0u8; len];
        read_exact_at(&self.file, &mut comp, start)?;
        let data = if plain {
            comp.resize(self.block as usize, 0);
            comp
        } else {
            let mut out = Vec::with_capacity(self.block as usize);
            let mut d = flate2::read::DeflateDecoder::new(&comp[..]);
            d.read_to_end(&mut out)?;
            out.resize(self.block as usize, 0);
            out
        };
        *self.cache.lock().unwrap() = Some((b, data.clone()));
        Ok(data)
    }
}

impl SectorSource for CsoImage {
    fn sector_count(&self) -> u64 {
        self.total / SECTOR
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        read_blocked(lba * SECTOR, buf, self.block, |b| self.block_data(b))
    }
    fn format_name(&self) -> String {
        "CSO (compressed ISO)".into()
    }
    fn details(&self) -> Vec<(String, String)> {
        let packed = self.file.metadata().map(|m| m.len()).unwrap_or(0);
        vec![
            ("Block size".into(), format!("{} bytes", self.block)),
            ("Ratio".into(), format!("{:.1}%", packed as f64 * 100.0 / self.total.max(1) as f64)),
        ]
    }
}

pub fn read_blocked(
    mut off: u64,
    buf: &mut [u8],
    block: u64,
    mut get: impl FnMut(u64) -> io::Result<Vec<u8>>,
) -> io::Result<()> {
    let mut pos = 0usize;
    while pos < buf.len() {
        let b = off / block;
        let inner = (off % block) as usize;
        let data = get(b)?;
        let take = (data.len() - inner).min(buf.len() - pos);
        buf[pos..pos + take].copy_from_slice(&data[inner..inner + take]);
        pos += take;
        off += take as u64;
    }
    Ok(())
}

// ------------------------------------------------------------------ physical drives

#[cfg(windows)]
pub struct DriveSource {
    file: File,
    count: u64,
    letter: char,
}

#[cfg(windows)]
impl DriveSource {
    pub fn open(letter: char) -> io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        let path = format!("\\\\.\\{}:", letter);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)?;
        let len = crate::drives::device_length(&file).unwrap_or(0);
        if len == 0 {
            return Err(bad("no readable media in this drive"));
        }
        Ok(DriveSource { file, count: len / SECTOR, letter })
    }
}

#[cfg(windows)]
impl SectorSource for DriveSource {
    fn sector_count(&self) -> u64 {
        self.count
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        match read_exact_at(&self.file, buf, lba * SECTOR) {
            Ok(()) => Ok(()),
            Err(_) => {
                // fall back sector by sector, zero-filling unreadable ones
                for (i, chunk) in buf.chunks_mut(SECTOR as usize).enumerate() {
                    if read_exact_at(&self.file, chunk, (lba + i as u64) * SECTOR).is_err() {
                        chunk.fill(0);
                    }
                }
                Ok(())
            }
        }
    }
    fn format_name(&self) -> String {
        format!("Drive {}:", self.letter)
    }
}

// ------------------------------------------------------------------ helpers

pub fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}
pub fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
pub fn u64le(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

// ------------------------------------------------------------------ detection

pub enum Opened {
    Sectors(Src),
    NeedPassword,
    Zip,
}

fn split_parts(path: &Path) -> Vec<PathBuf> {
    let s = path.to_string_lossy().to_string();
    if let Some(stem) = s.strip_suffix(".001") {
        let mut v = vec![];
        for i in 1..1000 {
            let p = PathBuf::from(format!("{}.{:03}", stem, i));
            if p.exists() {
                v.push(p);
            } else {
                break;
            }
        }
        return v;
    }
    vec![path.to_path_buf()]
}

fn has_sig(f: &MultiFile, header: u64, ss: u64, doff: u64) -> bool {
    let mut b = [0u8; 8];
    let at = |lba: u64| header + lba * ss + doff;
    if f.read_at(at(16), &mut b).is_ok() && (&b[1..6] == b"CD001" || &b[1..6] == b"BEA01") {
        return true;
    }
    if f.read_at(at(17), &mut b).is_ok() && &b[1..6] == b"BEA01" {
        return true;
    }
    let mut t = [0u8; 16];
    if f.read_at(at(256), &mut t).is_ok() && u16le(&t, 0) == 2 && u32le(&t, 12) == 256 {
        return true;
    }
    false
}

pub fn open_image(path: &Path, password: Option<&str>) -> io::Result<Opened> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "cue" {
        return open_cue(path).map(Opened::Sectors);
    }
    let mut magic = [0u8; 8];
    {
        let f = File::open(path)?;
        let _ = read_at(&f, &mut magic, 0);
    }
    if &magic[0..4] == b"CISO" {
        return Ok(Opened::Sectors(Arc::new(CsoImage::open(path)?)));
    }
    if &magic[0..4] == crate::hxz::MAGIC {
        return match password {
            _ if !crate::hxz::is_encrypted(path)? => Ok(Opened::Sectors(Arc::new(crate::hxz::HxzImage::open(path, None)?))),
            None => Ok(Opened::NeedPassword),
            Some(p) => Ok(Opened::Sectors(Arc::new(crate::hxz::HxzImage::open(path, Some(p))?))),
        };
    }
    if &magic[0..4] == b"PK\x03\x04" || &magic[0..4] == b"PK\x05\x06" {
        return Ok(Opened::Zip);
    }
    let parts = split_parts(path);
    let f = MultiFile::open(parts)?;
    let total = f.total;
    // VHD fixed: footer "conectix" at end
    let mut foot = [0u8; 8];
    let mut data_len = total;
    if total > 512 && f.read_at(total - 512, &mut foot).is_ok() && &foot == b"conectix" {
        data_len = total - 512;
    }
    let candidates: [(u64, u64, u64, &str); 10] = [
        (0, 2048, 0, "ISO 9660 image"),
        (0, 2352, 16, "BIN raw (Mode 1/2352)"),
        (0, 2352, 24, "BIN raw (Mode 2/2352)"),
        (0, 2336, 8, "Raw Mode 2/2336"),
        (0, 2448, 16, "Raw + subchannel (2448)"),
        (0, 2448, 24, "Raw Mode 2 + subchannel"),
        (307200, 2048, 0, "NRG image"),
        (352800, 2352, 16, "NRG raw image"),
        (352800, 2352, 24, "NRG raw Mode 2"),
        (0x8000, 2048, 0, "Image with 32K header"),
    ];
    for (header, ss, doff, name) in candidates {
        if has_sig(&f, header, ss, doff) {
            let mut name = name.to_string();
            if ext == "nrg" && header == 0 {
                name = "NRG image".into();
            } else if ext == "mdf" {
                name = format!("MDF image ({})", ss);
            } else if ext == "img" && ss == 2048 {
                name = "IMG image".into();
            }
            let count = (data_len.saturating_sub(header)) / ss;
            return Ok(Opened::Sectors(Arc::new(RawImage { file: f, header, sector_size: ss, data_off: doff, count, name })));
        }
    }
    // hard disk / floppy images: anything with a boot signature
    let mut s0 = [0u8; 512];
    f.read_at(0, &mut s0)?;
    let name = if &foot == b"conectix" {
        "VHD (fixed) disk image".to_string()
    } else if s0[510] == 0x55 && s0[511] == 0xAA {
        if total <= 2_949_120 { "Floppy disk image".into() } else { "Hard disk image".into() }
    } else {
        "Unknown raw image".into()
    };
    let count = data_len.div_ceil(SECTOR);
    Ok(Opened::Sectors(Arc::new(RawImage { file: f, header: 0, sector_size: 2048, data_off: 0, count, name })))
}

fn open_cue(path: &Path) -> io::Result<Src> {
    let text = std::fs::read_to_string(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut bin: Option<PathBuf> = None;
    let mut mode = String::new();
    for line in text.lines() {
        let l = line.trim();
        let up = l.to_uppercase();
        if up.starts_with("FILE") && bin.is_none() {
            let name = if let (Some(a), Some(b)) = (l.find('"'), l.rfind('"')) {
                if b > a { l[a + 1..b].to_string() } else { String::new() }
            } else {
                l.split_whitespace().nth(1).unwrap_or("").to_string()
            };
            bin = Some(dir.join(name));
        } else if up.starts_with("TRACK") && mode.is_empty() {
            mode = up.split_whitespace().nth(2).unwrap_or("").to_string();
        }
    }
    let bin = bin.ok_or_else(|| bad("cue sheet has no FILE line"))?;
    let (ss, doff) = match mode.as_str() {
        "MODE1/2048" => (2048, 0),
        "MODE1/2352" => (2352, 16),
        "MODE2/2352" => (2352, 24),
        "MODE2/2336" => (2336, 8),
        "AUDIO" => return Err(bad("this cue sheet describes an audio CD (no data track)")),
        _ => (2352, 16),
    };
    let f = MultiFile::single(&bin)?;
    let count = f.total / ss;
    Ok(Arc::new(RawImage { file: f, header: 0, sector_size: ss, data_off: doff, count, name: format!("BIN/CUE ({})", mode) }))
}
