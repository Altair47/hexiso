// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! ISO 9660 + Joliet image writer with El Torito (BIOS + UEFI) boot and >4 GB files.

use crate::jobs::Progress;
use crate::model::{civil, now, BootSpec, Document, Entry, FileSrc, Kind};
use std::collections::HashSet;
use std::io::{self, Read, Write};

const SEC: u64 = 2048;
const MAX_EXTENT: u64 = 0xFFFF_F800;

struct WDir {
    parent: usize,
    iso_id: Vec<u8>,
    jol_id: Vec<u8>,
    subdirs: Vec<usize>,
    files: Vec<usize>,
    mtime: i64,
    iso_lba: u32,
    iso_size: u32,
    jol_lba: u32,
    jol_size: u32,
    hidden: bool,
}

struct WFile {
    iso_id: Vec<u8>,
    jol_id: Vec<u8>,
    size: u64,
    src: FileSrc,
    lba: u32,
    mtime: i64,
    hidden: bool,
    info_table: bool,
}

pub struct WriteOpts {
    pub label: String,
    pub joliet: bool,
}

fn both16(v: u16) -> [u8; 4] {
    let mut b = [0u8; 4];
    b[..2].copy_from_slice(&v.to_le_bytes());
    b[2..].copy_from_slice(&v.to_be_bytes());
    b
}
fn both32(v: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&v.to_le_bytes());
    b[4..].copy_from_slice(&v.to_be_bytes());
    b
}

fn rec_date(ts: i64) -> [u8; 7] {
    let (y, m, d, hh, mm, ss) = civil(ts.max(0));
    [(y - 1900).clamp(0, 255) as u8, m as u8, d as u8, hh as u8, mm as u8, ss as u8, 0]
}

fn vd_date(ts: i64) -> [u8; 17] {
    let (y, m, d, hh, mm, ss) = civil(ts);
    let s = format!("{:04}{:02}{:02}{:02}{:02}{:02}00", y, m, d, hh, mm, ss);
    let mut b = [0u8; 17];
    b[..16].copy_from_slice(s.as_bytes());
    b
}

fn iso_name(name: &str, is_dir: bool, used: &mut HashSet<Vec<u8>>) -> Vec<u8> {
    let up: String = name
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '.' { c } else { '_' })
        .collect();
    let (base, ext) = if is_dir {
        (up.replace('.', "_"), String::new())
    } else {
        match up.rfind('.') {
            Some(i) => (up[..i].replace('.', "_"), up[i + 1..].chars().take(8).collect()),
            None => (up.clone(), String::new()),
        }
    };
    let max = if is_dir { 31 } else { 30 };
    let make = |base: &str, suffix: &str| -> Vec<u8> {
        let room = max - suffix.len() - if ext.is_empty() { 0 } else { ext.len() + 1 };
        let mut b: String = base.chars().take(room).collect();
        b.push_str(suffix);
        if !ext.is_empty() || !is_dir {
            b.push('.');
            b.push_str(&ext);
        }
        if !is_dir {
            b.push_str(";1");
        }
        b.into_bytes()
    };
    let base = if base.is_empty() { "_".to_string() } else { base };
    let mut id = make(&base, "");
    let mut n = 1;
    while used.contains(&id) {
        n += 1;
        id = make(&base, &format!("~{}", n));
    }
    used.insert(id.clone());
    id
}

fn jol_name(name: &str, used: &mut HashSet<Vec<u8>>) -> Vec<u8> {
    let clean: String = name.chars().map(|c| if "*/:;?\\".contains(c) || (c as u32) < 32 { '_' } else { c }).collect();
    let enc = |s: &str| -> Vec<u8> { s.encode_utf16().take(103).flat_map(|u| u.to_be_bytes()).collect() };
    let mut id = enc(&clean);
    let mut n = 1;
    while used.contains(&id) {
        n += 1;
        let (b, e) = match clean.rfind('.') {
            Some(i) if i > 0 => (&clean[..i], &clean[i..]),
            _ => (&clean[..], ""),
        };
        let b: String = b.chars().take(90).collect();
        id = enc(&format!("{} ({}){}", b, n, e));
    }
    used.insert(id.clone());
    id
}

struct Layout {
    dirs: Vec<WDir>,
    files: Vec<WFile>,
}

fn flatten(e: &Entry, parent: usize, l: &mut Layout, iso_id: Vec<u8>, jol_id: Vec<u8>) -> usize {
    let me = l.dirs.len();
    l.dirs.push(WDir { parent, iso_id, jol_id, subdirs: vec![], files: vec![], mtime: e.mtime, iso_lba: 0, iso_size: 0, jol_lba: 0, jol_size: 0, hidden: e.hidden });
    let mut iso_used = HashSet::new();
    let mut jol_used = HashSet::new();
    if let Kind::Dir(kids) = &e.kind {
        for k in kids {
            let iid = iso_name(&k.name, k.is_dir(), &mut iso_used);
            let jid = jol_name(&k.name, &mut jol_used);
            match &k.kind {
                Kind::Dir(_) => {
                    let d = flatten(k, me, l, iid, jid);
                    l.dirs[me].subdirs.push(d);
                }
                Kind::File { size, src } => {
                    let fi = l.files.len();
                    l.files.push(WFile { iso_id: iid, jol_id: jid, size: *size, src: src.clone(), lba: 0, mtime: k.mtime, hidden: k.hidden, info_table: false });
                    l.dirs[me].files.push(fi);
                }
            }
        }
    }
    me
}

enum Child {
    D(usize),
    F(usize),
}

fn children_sorted(l: &Layout, d: usize, joliet: bool) -> Vec<Child> {
    let mut v: Vec<(Vec<u8>, Child)> = Vec::new();
    for &s in &l.dirs[d].subdirs {
        v.push((if joliet { l.dirs[s].jol_id.clone() } else { l.dirs[s].iso_id.clone() }, Child::D(s)));
    }
    for &f in &l.dirs[d].files {
        v.push((if joliet { l.files[f].jol_id.clone() } else { l.files[f].iso_id.clone() }, Child::F(f)));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

fn rec(lba: u32, len: u32, ts: i64, flags: u8, id: &[u8]) -> Vec<u8> {
    let pad = if id.len().is_multiple_of(2) { 1 } else { 0 };
    let rl = 33 + id.len() + pad;
    let mut r = vec![0u8; rl];
    r[0] = rl as u8;
    r[2..10].copy_from_slice(&both32(lba));
    r[10..18].copy_from_slice(&both32(len));
    r[18..25].copy_from_slice(&rec_date(ts));
    r[25] = flags;
    r[28..32].copy_from_slice(&both16(1));
    r[32] = id.len() as u8;
    r[33..33 + id.len()].copy_from_slice(id);
    r
}

/// Directory records of dir `d`; LBAs must be assigned for real output, sizes don't depend on them.
fn dir_bytes(l: &Layout, d: usize, joliet: bool) -> Vec<u8> {
    let dir = &l.dirs[d];
    let par = &l.dirs[dir.parent];
    let (me_lba, me_size, p_lba, p_size) = if joliet {
        (dir.jol_lba, dir.jol_size, par.jol_lba, par.jol_size)
    } else {
        (dir.iso_lba, dir.iso_size, par.iso_lba, par.iso_size)
    };
    let mut recs = vec![rec(me_lba, me_size, dir.mtime, 2, &[0]), rec(p_lba, p_size, par.mtime, 2, &[1])];
    for c in children_sorted(l, d, joliet) {
        match c {
            Child::D(s) => {
                let sd = &l.dirs[s];
                let (lba, size, id) = if joliet { (sd.jol_lba, sd.jol_size, &sd.jol_id) } else { (sd.iso_lba, sd.iso_size, &sd.iso_id) };
                recs.push(rec(lba, size, sd.mtime, 2 | sd.hidden as u8, id));
            }
            Child::F(f) => {
                let fl = &l.files[f];
                let id = if joliet { &fl.jol_id } else { &fl.iso_id };
                let mut left = fl.size;
                let mut lba = fl.lba;
                loop {
                    let part = left.min(MAX_EXTENT);
                    let last = left - part == 0;
                    let flags = (fl.hidden as u8) | if last { 0 } else { 0x80 };
                    recs.push(rec(lba, part as u32, fl.mtime, flags, id));
                    if last {
                        break;
                    }
                    left -= part;
                    lba += (part / SEC) as u32;
                }
            }
        }
    }
    let mut out = Vec::new();
    for r in recs {
        let used = out.len() % SEC as usize;
        if used + r.len() > SEC as usize {
            out.resize(out.len() + (SEC as usize - used), 0);
        }
        out.extend_from_slice(&r);
    }
    let rem = out.len() % SEC as usize;
    if rem != 0 {
        out.resize(out.len() + SEC as usize - rem, 0);
    }
    out
}

/// BFS order of directories (path table order).
fn bfs(l: &Layout, joliet: bool) -> Vec<usize> {
    let mut order = vec![0usize];
    let mut i = 0;
    while i < order.len() {
        let d = order[i];
        let mut subs: Vec<usize> = l.dirs[d].subdirs.clone();
        subs.sort_by(|a, b| {
            let (x, y) = if joliet { (&l.dirs[*a].jol_id, &l.dirs[*b].jol_id) } else { (&l.dirs[*a].iso_id, &l.dirs[*b].iso_id) };
            x.cmp(y)
        });
        order.extend(subs);
        i += 1;
    }
    order
}

fn path_table(l: &Layout, order: &[usize], joliet: bool, be: bool) -> Vec<u8> {
    let mut num = vec![0u16; l.dirs.len()];
    for (i, d) in order.iter().enumerate() {
        num[*d] = (i + 1) as u16;
    }
    let mut out = Vec::new();
    for &d in order {
        let dir = &l.dirs[d];
        let id: Vec<u8> = if d == 0 { vec![0] } else if joliet { dir.jol_id.clone() } else { dir.iso_id.clone() };
        let lba = if joliet { dir.jol_lba } else { dir.iso_lba };
        let parent = num[dir.parent];
        out.push(id.len() as u8);
        out.push(0);
        if be {
            out.extend_from_slice(&lba.to_be_bytes());
            out.extend_from_slice(&parent.to_be_bytes());
        } else {
            out.extend_from_slice(&lba.to_le_bytes());
            out.extend_from_slice(&parent.to_le_bytes());
        }
        out.extend_from_slice(&id);
        if id.len() % 2 == 1 {
            out.push(0);
        }
    }
    out
}

fn secs(n: u64) -> u32 {
    n.div_ceil(SEC) as u32
}

fn pad_text(dst: &mut [u8], s: &str, joliet: bool) {
    if joliet {
        dst.fill(0);
        for c in dst.as_chunks_mut::<2>().0 {
            c[1] = b' ';
        }
        for (i, u) in s.encode_utf16().take(dst.len() / 2).enumerate() {
            dst[i * 2..i * 2 + 2].copy_from_slice(&u.to_be_bytes());
        }
    } else {
        dst.fill(b' ');
        for (i, b) in s.bytes().take(dst.len()).enumerate() {
            dst[i] = b;
        }
    }
}

struct Pos<W: Write> {
    w: W,
    pos: u64,
}
impl<W: Write> Pos<W> {
    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.w.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }
    fn seek_to(&mut self, lba: u32) -> io::Result<()> {
        let target = lba as u64 * SEC;
        if self.pos > target {
            return Err(io::Error::other(format!("layout error at sector {}", lba)));
        }
        let zeros = vec![0u8; 65536];
        while self.pos < target {
            let n = ((target - self.pos) as usize).min(zeros.len());
            self.put(&zeros[..n])?;
        }
        Ok(())
    }
}

fn find_file(l: &Layout, root: &Entry, path: &[String]) -> Option<usize> {
    // walk names in the source tree in parallel with the layout (same order as flatten)
    let mut d = 0usize;
    let mut cur = root;
    for (i, name) in path.iter().enumerate() {
        let kids = cur.children()?;
        let last = i + 1 == path.len();
        let (mut di, mut fi) = (0usize, 0usize);
        for k in kids {
            if &k.name == name {
                if last && !k.is_dir() {
                    return l.dirs[d].files.get(fi).copied();
                }
                if !last && k.is_dir() {
                    d = l.dirs[d].subdirs[di];
                    cur = k;
                    break;
                }
                return None;
            }
            if k.is_dir() {
                di += 1;
            } else {
                fi += 1;
            }
        }
    }
    None
}

/// Total size the image will have (sectors), quickly.
pub fn estimate(doc: &Document) -> u64 {
    fn walk(e: &Entry) -> u64 {
        match &e.kind {
            Kind::Dir(k) => 2 + k.iter().map(walk).sum::<u64>(),
            Kind::File { size, .. } => size.div_ceil(SEC),
        }
    }
    (walk(&doc.root) * 2 + 24 + doc.boot.iter().map(|b| b.size.div_ceil(SEC)).sum::<u64>()) * SEC
}

pub fn write_iso<W: Write>(doc: &Document, opts: &WriteOpts, out: W, prog: &Progress) -> io::Result<u64> {
    let mut l = Layout { dirs: vec![], files: vec![] };
    flatten(&doc.root, 0, &mut l, vec![0], vec![0]);
    let iso_order = bfs(&l, false);
    let jol_order = bfs(&l, true);

    // boot images
    struct B {
        spec: BootSpec,
        file: Option<usize>,
        lba: u32,
    }
    let mut boots: Vec<B> = doc
        .boot
        .iter()
        .map(|b| B { spec: b.clone(), file: b.tree_path.as_ref().and_then(|p| find_file(&l, &doc.root, p)), lba: 0 })
        .collect();
    for b in &boots {
        if let (Some(f), true) = (b.file, b.spec.info_table) {
            l.files[f].info_table = true;
        }
    }

    // sizes
    for d in 0..l.dirs.len() {
        l.dirs[d].iso_size = dir_bytes(&l, d, false).len() as u32;
        if opts.joliet {
            l.dirs[d].jol_size = dir_bytes(&l, d, true).len() as u32;
        }
    }
    let pt_iso = path_table(&l, &iso_order, false, false).len() as u64;
    let pt_jol = if opts.joliet { path_table(&l, &jol_order, true, false).len() as u64 } else { 0 };

    let mut lba: u32 = 16;
    let pvd_lba = lba;
    lba += 1;
    let br_lba = if boots.is_empty() { None } else { lba += 1; Some(lba - 1) };
    let svd_lba = if opts.joliet { lba += 1; Some(lba - 1) } else { None };
    let term_lba = lba;
    lba += 1;
    let lpt = lba;
    lba += secs(pt_iso);
    let mpt = lba;
    lba += secs(pt_iso);
    let (jlpt, jmpt) = if opts.joliet {
        let a = lba;
        lba += secs(pt_jol);
        let b = lba;
        lba += secs(pt_jol);
        (a, b)
    } else {
        (0, 0)
    };
    for &d in &iso_order {
        l.dirs[d].iso_lba = lba;
        lba += secs(l.dirs[d].iso_size as u64);
    }
    if opts.joliet {
        for &d in &jol_order {
            l.dirs[d].jol_lba = lba;
            lba += secs(l.dirs[d].jol_size as u64);
        }
    }
    let cat_lba = if boots.is_empty() { 0 } else { lba += 1; lba - 1 };
    for b in boots.iter_mut() {
        if b.file.is_none() {
            b.lba = lba;
            lba += secs(b.spec.size).max(1);
        }
    }
    // files in directory order (boot files first so loaders find them early)
    let mut file_order: Vec<usize> = boots.iter().filter_map(|b| b.file).collect();
    let mut seen: HashSet<usize> = file_order.iter().copied().collect();
    for &d in &iso_order {
        for &f in &l.dirs[d].files {
            if seen.insert(f) {
                file_order.push(f);
            }
        }
    }
    for &f in &file_order {
        l.files[f].lba = lba;
        lba += secs(l.files[f].size);
    }
    for b in boots.iter_mut() {
        if let Some(f) = b.file {
            b.lba = l.files[f].lba;
        }
    }
    let total = lba; // no trailing padding
    let total_bytes = total as u64 * SEC;
    prog.set_total(total_bytes);

    let mut w = Pos { w: io::BufWriter::with_capacity(4 << 20, out), pos: 0 };
    w.seek_to(16)?;
    let t = now();
    // volume descriptors
    let mk_vd = |kind: u8, joliet: bool, root: &WDir, pt_size: u64, lp: u32, mp: u32| -> Vec<u8> {
        let mut v = vec![0u8; 2048];
        v[0] = kind;
        v[1..6].copy_from_slice(b"CD001");
        v[6] = 1;
        pad_text(&mut v[8..40], "", joliet);
        let lab = if joliet { opts.label.clone() } else { opts.label.to_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect() };
        pad_text(&mut v[40..72], &lab, joliet);
        v[80..88].copy_from_slice(&both32(total));
        if joliet {
            v[88..91].copy_from_slice(b"%/E");
        }
        v[120..124].copy_from_slice(&both16(1));
        v[124..128].copy_from_slice(&both16(1));
        v[128..132].copy_from_slice(&both16(2048));
        v[132..140].copy_from_slice(&both32(pt_size as u32));
        v[140..144].copy_from_slice(&lp.to_le_bytes());
        v[148..152].copy_from_slice(&mp.to_be_bytes());
        let (rl, rs) = if joliet { (root.jol_lba, root.jol_size) } else { (root.iso_lba, root.iso_size) };
        let r = rec(rl, rs, root.mtime, 2, &[0]);
        v[156..190].copy_from_slice(&r);
        pad_text(&mut v[190..318], "", joliet);
        pad_text(&mut v[318..446], "", joliet);
        pad_text(&mut v[446..574], "", joliet);
        pad_text(&mut v[574..702], "HEXISO DISC IMAGE TOOLKIT", joliet);
        pad_text(&mut v[702..813], "", joliet);
        v[813..830].copy_from_slice(&vd_date(t));
        v[830..847].copy_from_slice(&vd_date(t));
        v[847..863].copy_from_slice(b"0000000000000000");
        v[864..880].copy_from_slice(b"0000000000000000");
        v[881] = 1;
        v
    };
    w.seek_to(pvd_lba)?;
    w.put(&mk_vd(1, false, &l.dirs[0], pt_iso, lpt, mpt))?;
    if let Some(b) = br_lba {
        let mut v = vec![0u8; 2048];
        v[1..6].copy_from_slice(b"CD001");
        v[6] = 1;
        v[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        v[0x47..0x4B].copy_from_slice(&cat_lba.to_le_bytes());
        w.seek_to(b)?;
        w.put(&v)?;
    }
    if let Some(s) = svd_lba {
        w.seek_to(s)?;
        w.put(&mk_vd(2, true, &l.dirs[0], pt_jol, jlpt, jmpt))?;
    }
    w.seek_to(term_lba)?;
    let mut v = vec![0u8; 2048];
    v[0] = 255;
    v[1..6].copy_from_slice(b"CD001");
    v[6] = 1;
    w.put(&v)?;
    w.seek_to(lpt)?;
    w.put(&path_table(&l, &iso_order, false, false))?;
    w.seek_to(mpt)?;
    w.put(&path_table(&l, &iso_order, false, true))?;
    if opts.joliet {
        w.seek_to(jlpt)?;
        w.put(&path_table(&l, &jol_order, true, false))?;
        w.seek_to(jmpt)?;
        w.put(&path_table(&l, &jol_order, true, true))?;
    }
    for &d in &iso_order {
        w.seek_to(l.dirs[d].iso_lba)?;
        w.put(&dir_bytes(&l, d, false))?;
    }
    if opts.joliet {
        for &d in &jol_order {
            w.seek_to(l.dirs[d].jol_lba)?;
            w.put(&dir_bytes(&l, d, true))?;
        }
    }
    if !boots.is_empty() {
        w.seek_to(cat_lba)?;
        w.put(&boot_catalog(&boots.iter().map(|b| (b.spec.clone(), b.lba)).collect::<Vec<_>>()))?;
        for b in &boots {
            if b.file.is_none() {
                w.seek_to(b.lba)?;
                let mut data = doc.read_all(&b.spec.src, b.spec.size)?;
                if b.spec.info_table {
                    patch_info_table(&mut data, b.lba);
                }
                w.put(&data)?;
            }
        }
    }
    let mut buf = vec![0u8; 1 << 20];
    for &f in &file_order {
        let fl = &l.files[f];
        w.seek_to(fl.lba)?;
        if fl.size == 0 {
            continue;
        }
        if fl.info_table {
            let mut data = doc.read_all(&fl.src, fl.size)?;
            patch_info_table(&mut data, fl.lba);
            w.put(&data)?;
            prog.add(fl.size);
            continue;
        }
        let mut r = doc.reader(&fl.src, fl.size)?;
        let mut left = fl.size;
        while left > 0 {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let n = (left as usize).min(buf.len());
            r.read_exact(&mut buf[..n]).map_err(|e| io::Error::new(e.kind(), format!("reading {}: {}", String::from_utf8_lossy(&fl.iso_id), e)))?;
            w.put(&buf[..n])?;
            left -= n as u64;
            prog.set_done(w.pos);
        }
    }
    w.seek_to(total)?;
    w.w.flush()?;
    prog.set_done(total_bytes);
    Ok(total_bytes)
}

fn patch_info_table(data: &mut [u8], lba: u32) {
    if data.len() < 64 {
        return;
    }
    let mut sum: u32 = 0;
    for c in data[64..].chunks(4) {
        let mut a = [0u8; 4];
        a[..c.len()].copy_from_slice(c);
        sum = sum.wrapping_add(u32::from_le_bytes(a));
    }
    data[8..12].copy_from_slice(&16u32.to_le_bytes());
    data[12..16].copy_from_slice(&lba.to_le_bytes());
    let len = data.len() as u32;
    data[16..20].copy_from_slice(&len.to_le_bytes());
    data[20..24].copy_from_slice(&sum.to_le_bytes());
    for b in &mut data[24..64] {
        *b = 0;
    }
}

fn boot_catalog(boots: &[(BootSpec, u32)]) -> Vec<u8> {
    let mut cat = vec![0u8; 2048];
    let first = &boots[0].0;
    cat[0] = 1;
    cat[1] = first.platform;
    cat[4..28].copy_from_slice(b"HEXISO BOOT CATALOG     ");
    cat[30] = 0x55;
    cat[31] = 0xAA;
    let mut sum: u16 = 0;
    for c in cat[0..32].chunks(2) {
        sum = sum.wrapping_add(u16::from_le_bytes([c[0], c[1]]));
    }
    cat[28..30].copy_from_slice(&(0u16.wrapping_sub(sum)).to_le_bytes());
    let entry = |b: &BootSpec, lba: u32| -> [u8; 32] {
        let mut e = [0u8; 32];
        e[0] = 0x88;
        e[1] = b.emulation;
        let count = if b.emulation != 0 {
            1
        } else if b.load_sectors > 0 {
            b.load_sectors
        } else if b.platform == 0 {
            4
        } else {
            b.size.div_ceil(512).min(0xFFFF) as u16
        };
        e[6..8].copy_from_slice(&count.to_le_bytes());
        e[8..12].copy_from_slice(&lba.to_le_bytes());
        e
    };
    cat[32..64].copy_from_slice(&entry(first, boots[0].1));
    let mut p = 64;
    let rest = &boots[1..];
    for (i, (b, lba)) in rest.iter().enumerate() {
        if p + 64 > 2048 {
            break;
        }
        cat[p] = if i + 1 == rest.len() { 0x91 } else { 0x90 };
        cat[p + 1] = b.platform;
        cat[p + 2..p + 4].copy_from_slice(&1u16.to_le_bytes());
        p += 32;
        cat[p..p + 32].copy_from_slice(&entry(b, *lba));
        p += 32;
    }
    cat
}
