// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! UDF reader (1.02 - 2.60, physical and metadata partitions).

use crate::model::{unix_from_civil, Entry, FileSrc, Kind, SPARSE};
use crate::source::{read_range, u16le, u32le, u64le, SectorSource, SECTOR};
use std::collections::HashSet;
use std::io;

#[derive(Clone)]
enum PMap {
    Physical { part: u16 },
    Metadata { part: u16, extents: Vec<(u64, u64)> }, // extents in bytes relative to physical partition start
}

pub struct Udf<'a> {
    src: &'a dyn SectorSource,
    bs: u64,
    parts: Vec<(u16, u64)>, // partition number -> start sector
    maps: Vec<PMap>,
    pub label: String,
    pub revision: String,
    root_icb: (u32, u16),
}

fn bad(m: &str) -> io::Error {
    crate::source::bad(m)
}

pub fn dstring(b: &[u8]) -> String {
    if b.is_empty() {
        return String::new();
    }
    // last byte of a dstring field is its length
    let used = *b.last().unwrap() as usize;
    let s = if used > 0 && used < b.len() { &b[..used] } else { b };
    dchars(s)
}

pub fn dchars(s: &[u8]) -> String {
    if s.is_empty() {
        return String::new();
    }
    match s[0] {
        16 | 255 => {
            let u: Vec<u16> = s[1..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&u).trim_end_matches('\0').to_string()
        }
        _ => s[1..].iter().map(|&c| c as char).collect::<String>().trim_end_matches('\0').to_string(),
    }
}

fn ts(b: &[u8]) -> i64 {
    let year = i16::from_le_bytes([b[2], b[3]]) as i64;
    if year < 1970 {
        return 0;
    }
    let tz_raw = (u16le(b, 0) & 0x0FFF) as i16;
    let tz = if tz_raw & 0x0800 != 0 { tz_raw | !0x0FFF } else { tz_raw };
    let tz = if tz == -2047 { 0 } else { tz as i64 };
    unix_from_civil(year, b[4].clamp(1, 12) as u32, b[5].clamp(1, 31) as u32, b[6] as u32 % 24, b[7] as u32 % 60, b[8] as u32 % 60) - tz * 60
}

pub fn detect(src: &dyn SectorSource) -> bool {
    let mut b = vec![0u8; 2048];
    if src.sector_count() <= 256 || src.read_sectors(256, &mut b).is_err() {
        return false;
    }
    u16le(&b, 0) == 2
}

impl<'a> Udf<'a> {
    pub fn open(src: &'a dyn SectorSource) -> io::Result<Self> {
        let bs = SECTOR;
        let mut b = vec![0u8; 2048];
        src.read_sectors(256, &mut b)?;
        if u16le(&b, 0) != 2 {
            return Err(bad("no UDF anchor"));
        }
        let vds_len = u32le(&b, 16) as u64;
        let vds_loc = u32le(&b, 20) as u64;
        let mut parts = Vec::new();
        let mut lvd: Option<Vec<u8>> = None;
        let mut revision = String::new();
        for i in 0..(vds_len / bs).min(64) {
            src.read_sectors(vds_loc + i, &mut b)?;
            match u16le(&b, 0) {
                5 => parts.push((u16le(&b, 22), u32le(&b, 188) as u64)),
                6 => lvd = Some(b.clone()),
                8 => break,
                _ => {}
            }
        }
        let lvd = lvd.ok_or_else(|| bad("UDF logical volume missing"))?;
        let label = dstring(&lvd[84..212]);
        let lbs = u32le(&lvd, 212) as u64;
        if lbs != 2048 {
            return Err(bad("UDF block size other than 2048 is not supported"));
        }
        // domain identifier suffix holds the UDF revision
        let rev = u16le(&lvd, 216 + 24);
        if rev > 0 {
            revision = format!("{}.{:02x}", rev >> 8, rev & 0xFF);
        }
        let fsd_lbn = u32le(&lvd, 252);
        let fsd_part = u16le(&lvd, 256);
        let nmaps = u32le(&lvd, 268) as usize;
        let mut maps = Vec::new();
        let mut p = 440usize;
        for _ in 0..nmaps.min(8) {
            if p + 2 > lvd.len() {
                break;
            }
            let t = lvd[p];
            let l = lvd[p + 1] as usize;
            if l == 0 {
                break;
            }
            if t == 1 {
                maps.push(PMap::Physical { part: u16le(&lvd, p + 4) });
            } else if t == 2 {
                let ident = &lvd[p + 5..p + 28];
                let part = u16le(&lvd, p + 38);
                if ident.starts_with(b"*UDF Metadata Partition") {
                    let loc = u32le(&lvd, p + 40);
                    maps.push(PMap::Metadata { part, extents: vec![(u64::MAX, loc as u64)] });
                } else {
                    // sparable/virtual: treat as physical (good enough for pressed discs)
                    maps.push(PMap::Physical { part });
                }
            }
            p += l;
        }
        let mut udf = Udf { src, bs, parts, maps, label, revision, root_icb: (0, 0) };
        // resolve metadata files
        for i in 0..udf.maps.len() {
            if let PMap::Metadata { part, extents } = udf.maps[i].clone() {
                let loc = extents[0].1;
                let start = udf.part_start(part)?;
                let fe = udf.read_block_abs(start + loc)?;
                let (_, _, ext, _) = udf.parse_fe(&fe, i as u16, true)?;
                udf.maps[i] = PMap::Metadata { part, extents: ext };
            }
        }
        let fsd = udf.read_block(fsd_part, fsd_lbn)?;
        if u16le(&fsd, 0) != 256 {
            return Err(bad("UDF file set descriptor not found"));
        }
        udf.root_icb = (u32le(&fsd, 404), u16le(&fsd, 408));
        Ok(udf)
    }

    fn part_start(&self, num: u16) -> io::Result<u64> {
        self.parts.iter().find(|p| p.0 == num).map(|p| p.1).ok_or_else(|| bad("UDF partition missing"))
    }

    fn read_block_abs(&self, sector: u64) -> io::Result<Vec<u8>> {
        let mut b = vec![0u8; self.bs as usize];
        self.src.read_sectors(sector, &mut b)?;
        Ok(b)
    }

    /// Byte offset (in user data) of logical block `lbn` of partition reference `pref`.
    fn lbn_offset(&self, pref: u16, lbn: u32) -> io::Result<u64> {
        let map = self.maps.get(pref as usize).ok_or_else(|| bad("UDF partition reference invalid"))?;
        match map {
            PMap::Physical { part } => Ok((self.part_start(*part)? + lbn as u64) * self.bs),
            PMap::Metadata { part, extents } => {
                // the metadata file's extents are already absolute byte offsets
                self.part_start(*part)?;
                let mut off = lbn as u64 * self.bs;
                for (o, l) in extents {
                    if off < *l {
                        return Ok(*o + off);
                    }
                    off -= l;
                }
                Err(bad("UDF metadata block out of range"))
            }
        }
    }

    fn read_block(&self, pref: u16, lbn: u32) -> io::Result<Vec<u8>> {
        let off = self.lbn_offset(pref, lbn)?;
        let mut b = vec![0u8; self.bs as usize];
        read_range(self.src, off, &mut b)?;
        Ok(b)
    }

    /// Returns (is_dir, size, extents (absolute byte offsets), mtime). Inline data is returned with
    /// a negative marker via `inline`.
    fn parse_fe(&self, fe: &[u8], pref: u16, meta_file: bool) -> io::Result<(bool, u64, Vec<(u64, u64)>, i64)> {
        let (r, _inl) = self.parse_fe_full(fe, pref, meta_file)?;
        Ok(r)
    }

    #[allow(clippy::type_complexity)]
    fn parse_fe_full(&self, fe: &[u8], pref: u16, meta_file: bool) -> io::Result<((bool, u64, Vec<(u64, u64)>, i64), Option<Vec<u8>>)> {
        let tag = u16le(fe, 0);
        let (l_ea, l_ad, base, mtime) = match tag {
            261 => (u32le(fe, 168) as usize, u32le(fe, 172) as usize, 176usize, ts(&fe[84..96])),
            266 => (u32le(fe, 208) as usize, u32le(fe, 212) as usize, 216usize, ts(&fe[92..104])),
            _ => return Err(bad("not a UDF file entry")),
        };
        let ftype = fe[27];
        let is_dir = ftype == 4;
        let size = u64le(fe, 56);
        let flags = u16le(fe, 34);
        let atype = flags & 7;
        let start = base + l_ea;
        let end = (start + l_ad).min(fe.len());
        let mut extents = Vec::new();
        if atype == 3 {
            let data = fe[start..end].to_vec();
            return Ok(((is_dir, size, vec![], mtime), Some(data)));
        }
        let mut ads = fe[start..end].to_vec();
        let mut guard = 0;
        'outer: loop {
            guard += 1;
            if guard > 1024 {
                break;
            }
            let step = if atype == 0 { 8 } else if atype == 1 { 16 } else { 20 };
            let mut p = 0;
            let mut next: Option<Vec<u8>> = None;
            while p + step <= ads.len() {
                let raw = u32le(&ads, p);
                let len = (raw & 0x3FFF_FFFF) as u64;
                let et = raw >> 30;
                if len == 0 {
                    break 'outer;
                }
                let (lbn, part) = if atype == 0 {
                    (u32le(&ads, p + 4), pref)
                } else {
                    (u32le(&ads, p + 4), u16le(&ads, p + 8))
                };
                if et == 3 {
                    let b = if meta_file {
                        let s = self.part_start(match &self.maps[part as usize] {
                            PMap::Physical { part } | PMap::Metadata { part, .. } => *part,
                        })?;
                        self.read_block_abs(s + lbn as u64)?
                    } else {
                        self.read_block(part, lbn)?
                    };
                    let l = u32le(&b, 20) as usize;
                    next = Some(b[24..(24 + l).min(b.len())].to_vec());
                    break;
                }
                let off = if et == 0 {
                    if meta_file {
                        let s = self.part_start(match &self.maps[part as usize] {
                            PMap::Physical { part } | PMap::Metadata { part, .. } => *part,
                        })?;
                        (s + lbn as u64) * self.bs
                    } else {
                        self.lbn_offset(part, lbn)?
                    }
                } else {
                    SPARSE
                };
                extents.push((off, len));
                p += step;
            }
            match next {
                Some(n) => ads = n,
                None => break,
            }
        }
        // clip extents to the information length
        let mut left = size;
        let mut clipped = Vec::new();
        for (o, l) in extents {
            if left == 0 {
                break;
            }
            let t = l.min(left);
            clipped.push((o, t));
            left -= t;
        }
        Ok(((is_dir, size, clipped, mtime), None))
    }

    pub fn read_tree(&self) -> io::Result<Entry> {
        let mut seen = HashSet::new();
        let (lbn, pref) = self.root_icb;
        let kids = self.read_dir(pref, lbn, 0, &mut seen)?;
        Ok(Entry::dir("", kids))
    }

    fn content(&self, ext: &[(u64, u64)], inline: Option<Vec<u8>>, size: u64) -> io::Result<Vec<u8>> {
        if let Some(d) = inline {
            return Ok(d);
        }
        let mut out = vec![0u8; size.min(256 << 20) as usize];
        let mut pos = 0usize;
        for (o, l) in ext {
            let n = (*l as usize).min(out.len() - pos);
            if *o != SPARSE {
                read_range(self.src, *o, &mut out[pos..pos + n])?;
            }
            pos += n;
        }
        Ok(out)
    }

    fn read_dir(&self, pref: u16, lbn: u32, depth: u32, seen: &mut HashSet<u64>) -> io::Result<Vec<Entry>> {
        let key = self.lbn_offset(pref, lbn)?;
        if depth > 64 || !seen.insert(key) {
            return Ok(vec![]);
        }
        let fe = self.read_block(pref, lbn)?;
        let ((_, size, ext, _), inl) = self.parse_fe_full(&fe, pref, false)?;
        let data = self.content(&ext, inl, size)?;
        let mut out = Vec::new();
        let mut p = 0usize;
        let mut names = HashSet::new();
        while p + 38 <= data.len() {
            if u16le(&data, p) != 257 {
                break;
            }
            let chars = data[p + 18];
            let l_fi = data[p + 19] as usize;
            let icb_lbn = u32le(&data, p + 24);
            let icb_part = u16le(&data, p + 28);
            let l_iu = u16le(&data, p + 36) as usize;
            let ns = p + 38 + l_iu;
            let total = (38 + l_iu + l_fi + 3) & !3;
            let name_raw = if ns + l_fi <= data.len() { &data[ns..ns + l_fi] } else { &[][..] };
            p += total;
            if chars & 0x08 != 0 || chars & 0x04 != 0 || l_fi == 0 {
                continue; // parent or deleted
            }
            let mut name = dchars(name_raw);
            if name.is_empty() {
                name = "_".into();
            }
            let base = name.clone();
            let mut i = 1;
            while !names.insert(name.to_lowercase()) {
                i += 1;
                name = format!("{} ({})", base, i);
            }
            let hidden = chars & 0x01 != 0;
            let fe = match self.read_block(icb_part, icb_lbn) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let parsed = match self.parse_fe_full(&fe, icb_part, false) {
                Ok(x) => x,
                Err(_) => continue,
            };
            let ((is_dir, fsize, fext, mtime), inl) = parsed;
            let lba = fext.first().filter(|e| e.0 != SPARSE).map(|e| e.0 / SECTOR);
            if is_dir || chars & 0x02 != 0 {
                let kids = self.read_dir(icb_part, icb_lbn, depth + 1, seen).unwrap_or_default();
                out.push(Entry { name, mtime, kind: Kind::Dir(kids), lba, hidden });
            } else {
                let src = match inl {
                    Some(d) => {
                        let mut d = d;
                        d.truncate(fsize as usize);
                        FileSrc::Mem(std::sync::Arc::new(d))
                    }
                    None => FileSrc::Image(fext),
                };
                out.push(Entry { name, mtime, kind: Kind::File { size: fsize, src }, lba, hidden });
            }
        }
        Ok(out)
    }
}
