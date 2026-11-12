// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! ISO 9660 reader with Joliet, Rock Ridge and El Torito boot catalogs.

use crate::model::{unix_from_civil, BootSpec, Entry, FileSrc, Kind};
use crate::source::{read_range, u16le, u32le, SectorSource, SECTOR};
use std::collections::HashSet;
use std::io;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Names {
    Plain,
    Joliet,
    RockRidge,
}

pub struct IsoInfo {
    pub pvd: Option<Vec<u8>>,
    pub joliet: Option<Vec<u8>>,
    pub boot_catalog: Option<u32>,
    pub rock_ridge: bool,
}

pub fn probe(src: &dyn SectorSource) -> IsoInfo {
    let mut info = IsoInfo { pvd: None, joliet: None, boot_catalog: None, rock_ridge: false };
    let mut buf = vec![0u8; 2048];
    for lba in 16..64u64 {
        if lba >= src.sector_count() || src.read_sectors(lba, &mut buf).is_err() {
            break;
        }
        if &buf[1..6] != b"CD001" {
            if &buf[1..6] == b"BEA01" || &buf[1..6] == b"NSR02" || &buf[1..6] == b"NSR03" || &buf[1..6] == b"TEA01" {
                continue;
            }
            break;
        }
        match buf[0] {
            0 => {
                if buf[7..30].starts_with(b"EL TORITO SPECIFICATION") {
                    info.boot_catalog = Some(u32le(&buf, 0x47));
                }
            }
            1 if info.pvd.is_none() => info.pvd = Some(buf.clone()),
            2 => {
                let esc = &buf[88..91];
                if esc == b"%/@" || esc == b"%/C" || esc == b"%/E" {
                    info.joliet = Some(buf.clone());
                }
            }
            255 => break,
            _ => {}
        }
    }
    if let Some(pvd) = &info.pvd {
        // check root "." record for SUSP "SP" entry
        let root = &pvd[156..190];
        let lba = u32le(root, 2) as u64;
        let mut d = vec![0u8; 2048];
        if read_range(src, lba * SECTOR, &mut d).is_ok() {
            let len = d[0] as usize;
            if len >= 34 {
                let fi = d[32] as usize;
                let su = 33 + fi + (1 - fi % 2);
                if su + 7 <= len && &d[su..su + 2] == b"SP" && d[su + 4] == 0xBE && d[su + 5] == 0xEF {
                    info.rock_ridge = true;
                }
            }
        }
    }
    info
}

pub fn label_of(vd: &[u8], joliet: bool) -> String {
    let raw = &vd[40..72];
    if joliet {
        let u: Vec<u16> = raw.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u).trim_end_matches(['\0', ' ']).to_string()
    } else {
        String::from_utf8_lossy(raw).trim_end_matches(['\0', ' ']).to_string()
    }
}

fn rec_time(r: &[u8]) -> i64 {
    let y = r[0] as i64 + 1900;
    let m = r[1].clamp(1, 12) as u32;
    let d = r[2].clamp(1, 31) as u32;
    let tz = r[6] as i8 as i64 * 15 * 60;
    unix_from_civil(y, m, d, r[3] as u32 % 24, r[4] as u32 % 60, r[5] as u32 % 60) - tz
}

/// Read the whole tree under the given volume descriptor.
pub fn read_tree(src: &dyn SectorSource, vd: &[u8], names: Names) -> io::Result<Entry> {
    let root = &vd[156..190];
    let lba = u32le(root, 2) as u64;
    let len = u32le(root, 10) as u64;
    let mut seen = HashSet::new();
    let kids = read_dir(src, lba, len, names, 0, &mut seen)?;
    let mut e = Entry::dir("", kids);
    e.lba = Some(lba);
    Ok(e)
}

fn read_dir(src: &dyn SectorSource, lba: u64, len: u64, names: Names, depth: u32, seen: &mut HashSet<u64>) -> io::Result<Vec<Entry>> {
    if depth > 64 || !seen.insert(lba) || len > 64 * 1024 * 1024 {
        return Ok(vec![]);
    }
    let mut data = vec![0u8; len as usize];
    read_range(src, lba * SECTOR, &mut data)?;
    let mut out: Vec<Entry> = Vec::new();
    let mut pending: Option<(String, Vec<(u64, u64)>, u64)> = None;
    let mut pos = 0usize;
    while pos < data.len() {
        let rl = data[pos] as usize;
        if rl == 0 {
            pos = (pos / 2048 + 1) * 2048;
            continue;
        }
        if rl < 34 || pos + rl > data.len() {
            break;
        }
        let r = &data[pos..pos + rl];
        pos += rl;
        let ext_attr = r[1] as u64;
        let elba = u32le(r, 2) as u64 + ext_attr;
        let dlen = u32le(r, 10) as u64;
        let flags = r[25];
        let fil = r[32] as usize;
        if 33 + fil > rl {
            continue;
        }
        let fi = &r[33..33 + fil];
        if fil == 1 && (fi[0] == 0 || fi[0] == 1) {
            continue;
        }
        let su_start = 33 + fil + (1 - fil % 2);
        let mut name = match names {
            Names::Joliet => {
                let u: Vec<u16> = fi.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&u)
            }
            _ => String::from_utf8_lossy(fi).to_string(),
        };
        if names == Names::RockRidge && su_start < rl {
            if let Some(n) = rr_name(src, &r[su_start..]) {
                name = n;
            } else {
                name = clean_iso(&name);
            }
        } else {
            name = clean_iso(&name);
        }
        let mtime = rec_time(&r[18..25]);
        let hidden = flags & 1 != 0;
        if flags & 0x02 != 0 {
            let kids = read_dir(src, elba, dlen, names, depth + 1, seen)?;
            out.push(Entry { name, mtime, kind: Kind::Dir(kids), lba: Some(elba), hidden });
            continue;
        }
        let ext = (elba * SECTOR, dlen);
        if flags & 0x80 != 0 {
            match &mut pending {
                Some((_, v, _)) => v.push(ext),
                None => pending = Some((name.clone(), vec![ext], elba)),
            }
            continue;
        }
        let (extents, first) = match pending.take() {
            Some((_, mut v, first)) => {
                v.push(ext);
                (v, first)
            }
            None => (vec![ext], elba),
        };
        let size: u64 = extents.iter().map(|e| e.1).sum();
        out.push(Entry { name, mtime, kind: Kind::File { size, src: FileSrc::Image(extents) }, lba: Some(first), hidden });
    }
    // de-duplicate names (should not happen, but keep the tree addressable)
    let mut names_seen = HashSet::new();
    for e in out.iter_mut() {
        let mut n = e.name.clone();
        let mut i = 1;
        while !names_seen.insert(n.to_lowercase()) {
            i += 1;
            n = format!("{} ({})", e.name, i);
        }
        e.name = n;
    }
    Ok(out)
}

fn clean_iso(n: &str) -> String {
    let mut s = n.to_string();
    if let Some(i) = s.rfind(';') {
        s.truncate(i);
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s.is_empty() {
        s = "_".into();
    }
    s
}

fn rr_name(src: &dyn SectorSource, su: &[u8]) -> Option<String> {
    let mut name = Vec::new();
    let mut found = false;
    let mut area = su.to_vec();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 16 {
            break;
        }
        let mut p = 0usize;
        let mut ce: Option<(u64, u64, u64)> = None;
        while p + 4 <= area.len() {
            let sig = &area[p..p + 2];
            let l = area[p + 2] as usize;
            if l < 4 || p + l > area.len() {
                break;
            }
            if sig == b"NM" && l >= 5 {
                let fl = area[p + 4];
                if fl & 0x06 == 0 {
                    name.extend_from_slice(&area[p + 5..p + l]);
                    found = true;
                }
            } else if sig == b"CE" && l >= 28 {
                ce = Some((u32le(&area, p + 4) as u64, u32le(&area, p + 12) as u64, u32le(&area, p + 20) as u64));
            } else if sig == b"ST" {
                break;
            }
            p += l;
        }
        match ce {
            Some((blk, off, len)) if len < 65536 => {
                let mut b = vec![0u8; len as usize];
                if read_range(src, blk * SECTOR + off, &mut b).is_err() {
                    break;
                }
                area = b;
            }
            _ => break,
        }
    }
    if found && !name.is_empty() { Some(String::from_utf8_lossy(&name).to_string()) } else { None }
}

/// Parse the El Torito boot catalog.
pub fn read_boot(src: &dyn SectorSource, cat_lba: u32, tree: &Entry) -> Vec<BootSpec> {
    let mut out = Vec::new();
    let mut cat = vec![0u8; 2048];
    if src.read_sectors(cat_lba as u64, &mut cat).is_err() {
        return out;
    }
    if cat[0] != 1 || cat[30] != 0x55 || cat[31] != 0xAA {
        return out;
    }
    let mut platform = cat[1];
    let push = |e: &[u8], platform: u8, out: &mut Vec<BootSpec>| {
        if e[0] != 0x88 {
            return;
        }
        let media = e[1] & 0x0F;
        let count = u16le(e, 6);
        let rba = u32le(e, 8) as u64;
        let mut path = Vec::new();
        let tree_hit = find_by_offset(tree, rba * SECTOR, &mut path);
        let (size, tree_path) = match tree_hit {
            Some(sz) => (sz, Some(path.clone())),
            None => {
                let sz = match media {
                    1 => 1_228_800,
                    2 => 1_474_560,
                    3 => 2_949_120,
                    _ => {
                        let mut s = count as u64 * 512;
                        if count <= 1 {
                            // maybe a FAT image (UEFI): read its BPB
                            let mut b = [0u8; 512];
                            if read_range(src, rba * SECTOR, &mut b).is_ok() && b[510] == 0x55 && b[511] == 0xAA {
                                let bps = u16le(&b, 11) as u64;
                                let mut tot = u16le(&b, 19) as u64;
                                if tot == 0 {
                                    tot = u32le(&b, 32) as u64;
                                }
                                if bps >= 512 && tot > 0 {
                                    s = bps * tot;
                                }
                            }
                        }
                        s.max(512)
                    }
                };
                (sz, None)
            }
        };
        let mut info_table = false;
        let mut b = [0u8; 16];
        if read_range(src, rba * SECTOR, &mut b).is_ok() && u32le(&b, 8) == 16 && u32le(&b, 12) as u64 == rba {
            info_table = true;
        }
        let label = match &tree_path {
            Some(p) => p.join("/"),
            None => format!("[hidden image @ LBA {}]", rba),
        };
        out.push(BootSpec {
            platform,
            emulation: media,
            load_sectors: count,
            info_table,
            size,
            src: FileSrc::Image(vec![(rba * SECTOR, size)]),
            tree_path,
            label,
        });
    };
    push(&cat[32..64], platform, &mut out);
    let mut p = 64;
    while p + 32 <= 2048 {
        let h = cat[p];
        if h == 0x90 || h == 0x91 {
            platform = cat[p + 1];
            let n = u16le(&cat, p + 2) as usize;
            p += 32;
            for _ in 0..n {
                if p + 32 > 2048 {
                    break;
                }
                let e = cat[p..p + 32].to_vec();
                push(&e, platform, &mut out);
                p += 32;
            }
            if h == 0x91 {
                break;
            }
        } else {
            break;
        }
    }
    out
}

fn find_by_offset(e: &Entry, off: u64, path: &mut Vec<String>) -> Option<u64> {
    if let Kind::Dir(c) = &e.kind {
        for k in c {
            path.push(k.name.clone());
            match &k.kind {
                Kind::File { size, src: FileSrc::Image(x) } if x.first().map(|f| f.0) == Some(off) => return Some(*size),
                Kind::Dir(_) => {
                    if let Some(s) = find_by_offset(k, off, path) {
                        return Some(s);
                    }
                }
                _ => {}
            }
            path.pop();
        }
    }
    None
}
