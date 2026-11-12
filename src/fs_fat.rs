// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! FAT12/16/32 reader (with long names), MBR / GPT partition tables, and a FAT12 floppy writer.

use crate::model::{unix_from_civil, Document, Entry, FileSrc, Kind};
use crate::source::{read_range, u16le, u32le, u64le, SectorSource};
use std::io::{self, Read};

pub struct Fat {
    base: u64,
    bps: u64,
    spc: u64,
    fat_start: u64,
    root_start: u64,
    root_entries: u64,
    data_start: u64,
    kind: u8,
    fat: Vec<u8>,
    root_clus: u32,
    clusters: u64,
    pub label: String,
}

pub fn looks_like_fat(b: &[u8]) -> bool {
    if b.len() < 512 || b[510] != 0x55 || b[511] != 0xAA {
        return false;
    }
    let bps = u16le(b, 11);
    let spc = b[13];
    let nf = b[16];
    matches!(bps, 512 | 1024 | 2048 | 4096) && spc != 0 && spc.is_power_of_two() && (1..=2).contains(&nf) && u16le(b, 14) > 0
}

impl Fat {
    pub fn open(src: &dyn SectorSource, base: u64) -> io::Result<Self> {
        let mut b = vec![0u8; 512];
        read_range(src, base, &mut b)?;
        if !looks_like_fat(&b) {
            return Err(crate::source::bad("no FAT boot sector"));
        }
        let bps = u16le(&b, 11) as u64;
        let spc = b[13] as u64;
        let reserved = u16le(&b, 14) as u64;
        let nf = b[16] as u64;
        let root_entries = u16le(&b, 17) as u64;
        let mut total = u16le(&b, 19) as u64;
        if total == 0 {
            total = u32le(&b, 32) as u64;
        }
        let mut fatsz = u16le(&b, 22) as u64;
        if fatsz == 0 {
            fatsz = u32le(&b, 36) as u64;
        }
        let root_secs = (root_entries * 32).div_ceil(bps);
        let fat_start = reserved;
        let root_start = reserved + nf * fatsz;
        let data_start = root_start + root_secs;
        let clusters = total.saturating_sub(data_start) / spc;
        let kind = if clusters < 4085 { 12 } else if clusters < 65525 { 16 } else { 32 };
        let mut fat = vec![0u8; (fatsz * bps).min(256 << 20) as usize];
        read_range(src, base + fat_start * bps, &mut fat)?;
        let root_clus = if kind == 32 { u32le(&b, 44) } else { 0 };
        let lab_off = if kind == 32 { 71 } else { 43 };
        let label = String::from_utf8_lossy(&b[lab_off..lab_off + 11]).trim().to_string();
        Ok(Fat { base, bps, spc, fat_start, root_start, root_entries, data_start, kind, fat, root_clus, clusters, label })
    }

    pub fn kind_name(&self) -> String {
        format!("FAT{}", self.kind)
    }

    fn next(&self, c: u32) -> u32 {
        let c = c as usize;
        match self.kind {
            12 => {
                let o = c + c / 2;
                if o + 1 >= self.fat.len() {
                    return 0xFFFF_FFFF;
                }
                let v = u16le(&self.fat, o) as u32;
                let v = if c & 1 == 1 { v >> 4 } else { v & 0xFFF };
                if v >= 0xFF8 { 0xFFFF_FFFF } else { v }
            }
            16 => {
                if c * 2 + 1 >= self.fat.len() {
                    return 0xFFFF_FFFF;
                }
                let v = u16le(&self.fat, c * 2) as u32;
                if v >= 0xFFF8 { 0xFFFF_FFFF } else { v }
            }
            _ => {
                if c * 4 + 3 >= self.fat.len() {
                    return 0xFFFF_FFFF;
                }
                let v = u32le(&self.fat, c * 4) & 0x0FFF_FFFF;
                if v >= 0x0FFF_FFF8 { 0xFFFF_FFFF } else { v }
            }
        }
    }

    fn clus_off(&self, c: u32) -> u64 {
        self.base + (self.data_start + (c as u64 - 2) * self.spc) * self.bps
    }

    fn chain(&self, first: u32, limit: Option<u64>) -> Vec<(u64, u64)> {
        let csize = self.spc * self.bps;
        let mut out: Vec<(u64, u64)> = Vec::new();
        let mut c = first;
        let mut left = limit.unwrap_or(u64::MAX);
        let mut guard = 0u64;
        while c >= 2 && (c as u64) < self.clusters + 2 && left > 0 && guard <= self.clusters {
            guard += 1;
            let off = self.clus_off(c);
            let take = csize.min(left);
            match out.last_mut() {
                Some(l) if l.0 + l.1 == off => l.1 += take,
                _ => out.push((off, take)),
            }
            left = left.saturating_sub(take);
            c = self.next(c);
        }
        out
    }

    pub fn read_tree(&self, src: &dyn SectorSource) -> io::Result<Entry> {
        let raw = if self.kind == 32 {
            read_extents(src, &self.chain(self.root_clus, None))?
        } else {
            let mut v = vec![0u8; (self.root_entries * 32) as usize];
            read_range(src, self.base + self.root_start * self.bps, &mut v)?;
            v
        };
        let kids = self.parse_dir(src, &raw, 0)?;
        Ok(Entry::dir("", kids))
    }

    fn parse_dir(&self, src: &dyn SectorSource, raw: &[u8], depth: u32) -> io::Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut lfn: Vec<(u8, Vec<u16>)> = Vec::new();
        for e in raw.chunks(32) {
            if e.len() < 32 || e[0] == 0 {
                break;
            }
            if e[0] == 0xE5 {
                lfn.clear();
                continue;
            }
            let attr = e[11];
            if attr == 0x0F {
                let mut u = Vec::new();
                for o in (1..11).step_by(2).chain((14..26).step_by(2)).chain((28..32).step_by(2)) {
                    u.push(u16le(e, o));
                }
                lfn.push((e[0] & 0x1F, u));
                continue;
            }
            if attr & 0x08 != 0 {
                lfn.clear();
                continue; // volume label
            }
            let short = {
                let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                if ext.is_empty() { base } else { format!("{}.{}", base, ext) }
            };
            let name = if lfn.is_empty() {
                short
            } else {
                lfn.sort_by_key(|x| x.0);
                let mut u: Vec<u16> = lfn.iter().flat_map(|x| x.1.clone()).collect();
                if let Some(p) = u.iter().position(|&c| c == 0) {
                    u.truncate(p);
                }
                u.retain(|&c| c != 0xFFFF);
                String::from_utf16_lossy(&u)
            };
            lfn.clear();
            if name == "." || name == ".." {
                continue;
            }
            let clus = ((u16le(e, 20) as u32) << 16) | u16le(e, 26) as u32;
            let size = u32le(e, 28) as u64;
            let t = u16le(e, 22);
            let d = u16le(e, 24);
            let mtime = unix_from_civil(1980 + (d >> 9) as i64, ((d >> 5) & 15).clamp(1, 12) as u32, (d & 31).clamp(1, 31) as u32, (t >> 11) as u32 % 24, ((t >> 5) & 63) as u32 % 60, ((t & 31) * 2) as u32 % 60);
            let hidden = attr & 0x02 != 0;
            let lba = if clus >= 2 { Some(self.clus_off(clus) / 2048) } else { None };
            if attr & 0x10 != 0 {
                let kids = if depth < 32 && clus >= 2 {
                    let raw = read_extents(src, &self.chain(clus, Some(4 << 20)))?;
                    self.parse_dir(src, &raw, depth + 1)?
                } else {
                    vec![]
                };
                out.push(Entry { name, mtime, kind: Kind::Dir(kids), lba, hidden });
            } else {
                let ext = if size > 0 { self.chain(clus, Some(size)) } else { vec![] };
                out.push(Entry { name, mtime, kind: Kind::File { size, src: FileSrc::Image(ext) }, lba, hidden });
            }
        }
        let _ = self.fat_start;
        Ok(out)
    }
}

fn read_extents(src: &dyn SectorSource, ext: &[(u64, u64)]) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    for (o, l) in ext {
        let s = v.len();
        v.resize(s + *l as usize, 0);
        read_range(src, *o, &mut v[s..])?;
    }
    Ok(v)
}

/// Partitions of an MBR / GPT disk: (byte offset, byte length, description)
pub fn partitions(src: &dyn SectorSource) -> Vec<(u64, u64, String)> {
    let mut out = Vec::new();
    let mut b = vec![0u8; 512];
    if read_range(src, 0, &mut b).is_err() || b[510] != 0x55 || b[511] != 0xAA {
        return out;
    }
    let mut gpt = false;
    for i in 0..4 {
        let e = &b[446 + i * 16..462 + i * 16];
        let t = e[4];
        let start = u32le(e, 8) as u64 * 512;
        let len = u32le(e, 12) as u64 * 512;
        if t == 0xEE {
            gpt = true;
        } else if t != 0 && len > 0 {
            out.push((start, len, format!("MBR type 0x{:02X}", t)));
        }
    }
    if gpt {
        out.clear();
        let mut h = vec![0u8; 512];
        if read_range(src, 512, &mut h).is_ok() && &h[0..8] == b"EFI PART" {
            let lba = u64le(&h, 72);
            let n = u32le(&h, 80).min(256) as u64;
            let sz = u32le(&h, 84) as u64;
            let mut t = vec![0u8; (n * sz) as usize];
            if read_range(src, lba * 512, &mut t).is_ok() {
                for e in t.chunks(sz as usize) {
                    if e.len() < 128 || e[0..16].iter().all(|&x| x == 0) {
                        continue;
                    }
                    let first = u64le(e, 32);
                    let last = u64le(e, 40);
                    let u: Vec<u16> = e[56..128].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                    out.push((first * 512, (last - first + 1) * 512, format!("GPT {}", String::from_utf16_lossy(&u))));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ floppy writer

fn short_name(name: &str, used: &mut Vec<[u8; 11]>) -> [u8; 11] {
    let up = name.to_uppercase();
    let (base, ext) = match up.rfind('.') {
        Some(i) if i > 0 => (&up[..i], &up[i + 1..]),
        _ => (&up[..], ""),
    };
    let clean = |s: &str| -> Vec<u8> {
        s.bytes().filter(|c| c.is_ascii_alphanumeric() || b"$%'-_@~`!(){}^#&".contains(c)).collect()
    };
    let b = clean(base);
    let e = clean(ext);
    let mut out = [b' '; 11];
    let fits = b.len() <= 8 && e.len() <= 3 && !b.is_empty() && clean(base).len() == base.len();
    for (i, c) in e.iter().take(3).enumerate() {
        out[8 + i] = *c;
    }
    if fits && !used.contains(&{
        let mut t = out;
        for (i, c) in b.iter().enumerate() {
            t[i] = *c;
        }
        t
    }) {
        for (i, c) in b.iter().enumerate() {
            out[i] = *c;
        }
    } else {
        for n in 1..1000 {
            let tail = format!("~{}", n);
            let keep = 8 - tail.len();
            let mut t = out;
            t[..8].fill(b' ');
            let mut k = 0;
            for c in b.iter().take(keep) {
                t[k] = *c;
                k += 1;
            }
            if k == 0 {
                t[0] = b'_';
                k = 1;
            }
            for c in tail.bytes() {
                t[k] = c;
                k += 1;
            }
            if !used.contains(&t) {
                out = t;
                break;
            }
        }
    }
    used.push(out);
    out
}

fn lfn_checksum(s: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &c in s {
        sum = (if sum & 1 != 0 { 0x80u8 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(c);
    }
    sum
}

fn dos_time(ts: i64) -> (u16, u16) {
    let (y, m, d, hh, mm, ss) = crate::model::civil(ts.max(315532800));
    let date = (((y - 1980).clamp(0, 127) as u16) << 9) | ((m as u16) << 5) | d as u16;
    let time = ((hh as u16) << 11) | ((mm as u16) << 5) | (ss as u16 / 2);
    (time, date)
}

struct FloppyW {
    img: Vec<u8>,
    next: u32,
    total_clusters: u32,
    data_off: usize,
    fat_off: usize,
    fat_bytes: usize,
}

impl FloppyW {
    fn set_fat(&mut self, c: u32, v: u32) {
        let c = c as usize;
        let o = self.fat_off + c + c / 2;
        let cur = u16::from_le_bytes([self.img[o], self.img[o + 1]]);
        let nv = if c & 1 == 1 { (cur & 0x000F) | ((v as u16 & 0xFFF) << 4) } else { (cur & 0xF000) | (v as u16 & 0xFFF) };
        self.img[o..o + 2].copy_from_slice(&nv.to_le_bytes());
    }
    fn alloc(&mut self, bytes: usize) -> io::Result<u32> {
        let n = bytes.div_ceil(512).max(1) as u32;
        if self.next + n > self.total_clusters + 2 {
            return Err(io::Error::other("files do not fit on this floppy size"));
        }
        let first = self.next;
        for i in 0..n {
            let c = first + i;
            self.set_fat(c, if i + 1 == n { 0xFFF } else { c + 1 });
        }
        self.next += n;
        Ok(first)
    }
    fn coff(&self, c: u32) -> usize {
        self.data_off + (c as usize - 2) * 512
    }
}

fn dir_records(entries: &[Entry], doc: &Document, w: &mut FloppyW, me: u32, parent: u32, is_root: bool) -> io::Result<Vec<u8>> {
    let mut used = Vec::new();
    let mut recs: Vec<u8> = Vec::new();
    if !is_root {
        for (n, c) in [(b".          ", me), (b"..         ", parent)] {
            let mut r = [0u8; 32];
            r[..11].copy_from_slice(n);
            r[11] = 0x10;
            r[26..28].copy_from_slice(&(c as u16).to_le_bytes());
            recs.extend_from_slice(&r);
        }
    }
    for e in entries {
        let sn = short_name(&e.name, &mut used);
        let needs_lfn = {
            let s = String::from_utf8_lossy(&sn[..8]).trim_end().to_string();
            let x = String::from_utf8_lossy(&sn[8..]).trim_end().to_string();
            let full = if x.is_empty() { s } else { format!("{}.{}", s, x) };
            full != e.name
        };
        if needs_lfn {
            let mut u: Vec<u16> = e.name.encode_utf16().collect();
            u.truncate(255);
            let parts = u.len().div_ceil(13);
            u.push(0);
            while !u.len().is_multiple_of(13) {
                u.push(0xFFFF);
            }
            let ck = lfn_checksum(&sn);
            for i in (0..parts).rev() {
                let mut r = [0u8; 32];
                r[0] = (i as u8 + 1) | if i + 1 == parts { 0x40 } else { 0 };
                r[11] = 0x0F;
                r[13] = ck;
                let seg = &u[i * 13..i * 13 + 13];
                let pos: Vec<usize> = (1..11).step_by(2).chain((14..26).step_by(2)).chain((28..32).step_by(2)).collect();
                for (k, p) in pos.iter().enumerate() {
                    r[*p..*p + 2].copy_from_slice(&seg[k].to_le_bytes());
                }
                recs.extend_from_slice(&r);
            }
        }
        let mut r = [0u8; 32];
        r[..11].copy_from_slice(&sn);
        let (t, d) = dos_time(e.mtime);
        r[22..24].copy_from_slice(&t.to_le_bytes());
        r[24..26].copy_from_slice(&d.to_le_bytes());
        match &e.kind {
            Kind::Dir(kids) => {
                r[11] = 0x10;
                let need = (2 + kids.iter().map(|k| 1 + k.name.encode_utf16().count().div_ceil(13)).sum::<usize>()) * 32;
                let c = w.alloc(need)?;
                r[26..28].copy_from_slice(&(c as u16).to_le_bytes());
                let sub = dir_records(kids, doc, w, c, if is_root { 0 } else { me }, false)?;
                let o = w.coff(c);
                let n = sub.len().min(need.div_ceil(512) * 512);
                w.img[o..o + n].copy_from_slice(&sub[..n]);
            }
            Kind::File { size, src } => {
                r[11] = 0x20;
                if *size > 0 {
                    let c = w.alloc(*size as usize)?;
                    r[26..28].copy_from_slice(&(c as u16).to_le_bytes());
                    let mut rd = doc.reader(src, *size)?;
                    let o = w.coff(c);
                    rd.read_exact(&mut w.img[o..o + *size as usize])?;
                }
                r[28..32].copy_from_slice(&(*size as u32).to_le_bytes());
            }
        }
        recs.extend_from_slice(&r);
    }
    Ok(recs)
}

/// Build a FAT12 floppy image (1.44 MB or 2.88 MB) from a document tree.
pub fn make_floppy(doc: &Document, label: &str, big: bool) -> io::Result<Vec<u8>> {
    let (total, spt, fatsz, spc_root) = if big { (5760u32, 36u16, 9u16, 240u16) } else { (2880u32, 18u16, 9u16, 224u16) };
    let mut img = vec![0u8; total as usize * 512];
    let b = &mut img[..512];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"HEXISO  ");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = 1;
    b[14..16].copy_from_slice(&1u16.to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&spc_root.to_le_bytes());
    b[19..21].copy_from_slice(&(total as u16).to_le_bytes());
    b[21] = 0xF0;
    b[22..24].copy_from_slice(&fatsz.to_le_bytes());
    b[24..26].copy_from_slice(&spt.to_le_bytes());
    b[26..28].copy_from_slice(&2u16.to_le_bytes());
    b[38] = 0x29;
    b[39..43].copy_from_slice(&(crate::model::now() as u32).to_le_bytes());
    let mut lab = [b' '; 11];
    for (i, c) in label.to_uppercase().bytes().filter(|c| c.is_ascii_graphic()).take(11).enumerate() {
        lab[i] = c;
    }
    b[43..54].copy_from_slice(&lab);
    b[54..62].copy_from_slice(b"FAT12   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    let fat_off = 512;
    let root_off = 512 + 2 * fatsz as usize * 512;
    let data_off = root_off + spc_root as usize * 32;
    let total_clusters = total - (data_off / 512) as u32;
    let mut w = FloppyW { img, next: 2, total_clusters, data_off, fat_off, fat_bytes: fatsz as usize * 512 };
    w.img[fat_off] = 0xF0;
    w.img[fat_off + 1] = 0xFF;
    w.img[fat_off + 2] = 0xFF;
    let kids = doc.root.children().cloned().unwrap_or_default();
    let mut recs = Vec::new();
    // volume label entry
    let mut r = [0u8; 32];
    r[..11].copy_from_slice(&lab);
    r[11] = 0x08;
    recs.extend_from_slice(&r);
    recs.extend(dir_records(&kids, doc, &mut w, 0, 0, true)?);
    if recs.len() > spc_root as usize * 32 {
        return Err(io::Error::other("too many files in the floppy's root folder"));
    }
    w.img[root_off..root_off + recs.len()].copy_from_slice(&recs);
    // second FAT copy
    let (a, bb) = w.img.split_at_mut(fat_off + w.fat_bytes);
    bb[..w.fat_bytes].copy_from_slice(&a[fat_off..fat_off + w.fat_bytes]);
    Ok(w.img)
}
