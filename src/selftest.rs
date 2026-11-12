// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! `hexiso --selftest <dir> [extra images...]`: round-trips the engine and prints a report.

use crate::jobs::Progress;
use crate::model::{Document, Entry, FileSrc, Kind};
use crate::ops::{self, OpenResult, OutFormat};
use crate::source::{open_image, Opened, SECTOR};
use std::io::Read;
use std::path::{Path, PathBuf};

fn rnd(seed: &mut u64) -> u8 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 24) as u8
}

fn mkfile(p: &Path, n: usize, seed: &mut u64) {
    let v: Vec<u8> = (0..n).map(|_| rnd(seed)).collect();
    std::fs::write(p, v).unwrap();
}

fn open_doc(p: &Path, pw: Option<&str>) -> Result<Document, String> {
    match ops::open_path(p, pw).map_err(|e| e.to_string())? {
        OpenResult::Doc(d) => Ok(*d),
        OpenResult::NeedPassword => Err("needs password".into()),
    }
}

/// Compare every file of `e` (image tree) with the disk folder `dir`.
fn compare(doc: &Document, e: &Entry, dir: &Path, errs: &mut Vec<String>, n: &mut usize) {
    let mut disk: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|x| x.ok()).collect();
    disk.sort_by_key(|x| x.file_name());
    let kids = e.children().cloned().unwrap_or_default();
    if kids.len() != disk.len() {
        errs.push(format!("{}: {} entries in image, {} on disk", dir.display(), kids.len(), disk.len()));
    }
    for d in disk {
        let name = d.file_name().to_string_lossy().to_string();
        let Some(k) = kids.iter().find(|k| k.name == name) else {
            errs.push(format!("missing in image: {}", d.path().display()));
            continue;
        };
        if d.path().is_dir() {
            compare(doc, k, &d.path(), errs, n);
        } else if let Kind::File { size, src } = &k.kind {
            let want = std::fs::read(d.path()).unwrap();
            let got = doc.read_all(src, *size).unwrap_or_default();
            *n += 1;
            if want != got {
                errs.push(format!("content differs: {} ({} vs {} bytes)", name, want.len(), got.len()));
            }
        }
    }
}

fn hash_src(p: &Path, pw: Option<&str>) -> Result<(u64, String), String> {
    let s = match open_image(p, pw).map_err(|e| e.to_string())? {
        Opened::Sectors(s) => s,
        _ => return Err("not sectors".into()),
    };
    let mut h = crc32fast::Hasher::new();
    let mut buf = vec![0u8; 256 * SECTOR as usize];
    let n = s.sector_count();
    let mut lba = 0;
    while lba < n {
        let k = 256.min(n - lba);
        let b = &mut buf[..(k * SECTOR) as usize];
        s.read_sectors(lba, b).map_err(|e| e.to_string())?;
        h.update(b);
        lba += k;
    }
    Ok((n, format!("{:08x}", h.finalize())))
}

pub fn run(dir: PathBuf, extra: Vec<PathBuf>) -> i32 {
    let mut fails = 0;
    let mut check = |name: &str, r: Result<String, String>| match r {
        Ok(s) => println!("[ ok ] {:<34} {}", name, s),
        Err(e) => {
            fails += 1;
            println!("[FAIL] {:<34} {}", name, e)
        }
    };
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    if let Err(e) = std::fs::create_dir_all(src.join("deep/er/still deeper/and a very long folder name that exceeds iso limits")) {
        println!("[FAIL] cannot use work folder {}: {}", dir.display(), e);
        return 1;
    }
    std::fs::create_dir_all(src.join("Ünïcødé ✓")).unwrap();
    let mut seed = 0x1234_5678_9abc_def0u64;
    mkfile(&src.join("readme.txt"), 1234, &mut seed);
    mkfile(&src.join("empty.bin"), 0, &mut seed);
    mkfile(&src.join("boot.img"), 2048, &mut seed);
    mkfile(&src.join("big data file with spaces.dat"), 5 * 1024 * 1024 + 7, &mut seed);
    mkfile(&src.join("Ünïcødé ✓/naïve – file.txt"), 333, &mut seed);
    mkfile(&src.join("deep/er/still deeper/and a very long folder name that exceeds iso limits/x.y.z.tar.gz"), 70000, &mut seed);
    for i in 0..60 {
        mkfile(&src.join(format!("deep/file_{:03}.bin", i)), (i * 977) % 9000, &mut seed);
    }
    let prog = Progress::default();

    // 1. build ISO from disk
    let mut doc = Document::new_blank();
    doc.label = "SELFTEST".into();
    for e in std::fs::read_dir(&src).unwrap().filter_map(|x| x.ok()) {
        doc.root.children_mut().unwrap().push(Entry::from_disk(&e.path()).unwrap());
    }
    doc.boot.push(crate::model::BootSpec { platform: 0, emulation: 0, load_sectors: 4, info_table: false, size: 2048, src: FileSrc::Disk(src.join("boot.img")), tree_path: Some(vec!["boot.img".into()]), label: "boot.img".into() });
    let iso = dir.join("t.iso");
    check("write ISO", ops::save_doc(&doc, &iso, OutFormat::Iso, None, &prog).map(|n| format!("{} bytes", n)).map_err(|e| e.to_string()));
    let t0 = std::time::Instant::now();
    match open_doc(&iso, None) {
        Ok(d) => {
            let mut errs = vec![];
            let mut n = 0;
            compare(&d, &d.root, &src, &mut errs, &mut n);
            check("read back (Joliet) + compare", if errs.is_empty() { Ok(format!("{} files identical, fs={} label={} in {:?}", n, d.fs_name, d.label, t0.elapsed())) } else { Err(errs.join("; ")) });
            check("boot catalog", if d.boot.len() == 1 && d.boot[0].tree_path.is_some() { Ok(format!("{} entry -> {}", d.boot.len(), d.boot[0].label)) } else { Err(format!("{} entries", d.boot.len())) });
            check("fs views", Ok(d.fs_views.join(", ")));
            let plain = ops::doc_from_source(d.src.clone().unwrap(), Some("ISO 9660")).map_err(|e| e.to_string());
            check("ISO 9660 view", plain.map(|p| format!("{} files, names like {:?}", p.root.count().0, p.root.children().unwrap().iter().map(|e| e.name.clone()).take(3).collect::<Vec<_>>())));
        }
        Err(e) => check("read back", Err(e)),
    }
    let (n_iso, h_iso) = hash_src(&iso, None).unwrap_or((0, String::new()));

    // 2. convert to CSO / HXZ
    let src_iso = match open_image(&iso, None) {
        Ok(Opened::Sectors(s)) => s,
        _ => panic!("cannot reopen"),
    };
    for (name, fmt, pw) in [("cso", OutFormat::Cso, None), ("hxz", OutFormat::Hxz, None), ("enc.hxz", OutFormat::Hxz, Some("s3cret"))] {
        let out = dir.join(format!("t.{}", name));
        let t = std::time::Instant::now();
        let r = ops::convert(&src_iso, &out, fmt, pw, &prog).map_err(|e| e.to_string()).and_then(|sz| {
            let (n, h) = hash_src(&out, pw)?;
            if n == n_iso && h == h_iso { Ok(format!("{} -> {} bytes, sectors identical, {:?}", n_iso * SECTOR, sz, t.elapsed())) } else { Err(format!("hash {} != {}", h, h_iso)) }
        });
        check(&format!("convert -> {}", name), r);
    }
    check("wrong password refused", match open_image(&dir.join("t.enc.hxz"), Some("nope")) { Err(e) => Ok(e.to_string()), Ok(_) => Err("accepted a wrong password".into()) });

    // 3. edit in place: delete + add + rename, overwrite the source file
    let r = (|| -> Result<String, String> {
        let mut d = open_doc(&iso, None)?;
        let kids = d.root.children_mut().unwrap();
        kids.retain(|e| e.name != "readme.txt");
        kids.iter_mut().find(|e| e.name == "empty.bin").unwrap().name = "renamed.bin".into();
        let extra = dir.join("added.txt");
        std::fs::write(&extra, b"added later").unwrap();
        kids.push(Entry::from_disk(&extra).unwrap());
        d.modified = true;
        ops::save_doc(&d, &iso, OutFormat::Iso, None, &prog).map_err(|e| e.to_string())?;
        let d2 = open_doc(&iso, None)?;
        let names: Vec<String> = d2.root.children().unwrap().iter().map(|e| e.name.clone()).collect();
        let big = d2.root.children().unwrap().iter().find(|e| e.name == "big data file with spaces.dat").ok_or("big file lost")?;
        let Kind::File { size, src } = &big.kind else { return Err("not a file".into()) };
        let same = d2.read_all(src, *size).map_err(|e| e.to_string())? == std::fs::read(src_path(&dir)).unwrap();
        if names.contains(&"added.txt".to_string()) && names.contains(&"renamed.bin".to_string()) && !names.contains(&"readme.txt".to_string()) && same && d2.boot.len() == 1 {
            Ok(format!("{:?}", names))
        } else {
            Err(format!("names {:?}, big file same={}, boot={}", names, same, d2.boot.len()))
        }
    })();
    check("edit + save over source", r);

    // 4. floppy
    let r = (|| -> Result<String, String> {
        let mut d = Document::new_blank();
        d.label = "FLOPPY".into();
        let fsrc = src.join("Ünïcødé ✓");
        d.root.children_mut().unwrap().push(Entry::from_disk(&fsrc).unwrap());
        d.root.children_mut().unwrap().push(Entry::from_disk(&src.join("readme.txt")).unwrap());
        let f = dir.join("f.img");
        ops::save_doc(&d, &f, OutFormat::Floppy144, None, &prog).map_err(|e| e.to_string())?;
        let d2 = open_doc(&f, None)?;
        let mut errs = vec![];
        let mut n = 0;
        let tmp = dir.join("fcmp");
        std::fs::create_dir_all(tmp.join("Ünïcødé ✓")).unwrap();
        std::fs::copy(src.join("readme.txt"), tmp.join("readme.txt")).unwrap();
        std::fs::copy(fsrc.join("naïve – file.txt"), tmp.join("Ünïcødé ✓/naïve – file.txt")).unwrap();
        compare(&d2, &d2.root, &tmp, &mut errs, &mut n);
        if errs.is_empty() { Ok(format!("{} files, fs={}", n, d2.fs_name)) } else { Err(errs.join("; ")) }
    })();
    check("FAT12 floppy round trip", r);

    // 5. zip
    let r = (|| -> Result<String, String> {
        let mut d = Document::new_blank();
        for e in std::fs::read_dir(&src).unwrap().filter_map(|x| x.ok()) {
            d.root.children_mut().unwrap().push(Entry::from_disk(&e.path()).unwrap());
        }
        let z = dir.join("t.zip");
        ops::save_doc(&d, &z, OutFormat::Zip, None, &prog).map_err(|e| e.to_string())?;
        let d2 = open_doc(&z, None)?;
        let mut errs = vec![];
        let mut n = 0;
        compare(&d2, &d2.root, &src, &mut errs, &mut n);
        if errs.is_empty() { Ok(format!("{} files", n)) } else { Err(errs.join("; ")) }
    })();
    check("ZIP round trip", r);

    // 6. split volumes
    let r = (|| -> Result<String, String> {
        let parts = ops::split_file(&iso, 1024 * 1024, &prog).map_err(|e| e.to_string())?;
        let whole = hash_src(&iso, None)?;
        let joined = hash_src(&parts[0], None)?;
        if whole == joined { Ok(format!("{} volumes, joined read identical", parts.len())) } else { Err("joined image differs".into()) }
    })();
    check("split + read .001", r);

    // 7. checksum
    let r = std::fs::File::open(&iso).map_err(|e| e.to_string()).and_then(|f| ops::checksum(f, 0, &prog).map_err(|e| e.to_string())).map(|s| format!("sha256 {}", s.sha256));
    check("checksum", r);

    // 8. external images given on the command line
    for p in extra {
        let t = std::time::Instant::now();
        let r = open_doc(&p, None).and_then(|d| {
            let (f, ds) = d.root.count();
            let mut sample = String::new();
            // read the first few files fully
            let mut read = 0u64;
            fn first_files(e: &Entry, out: &mut Vec<(String, u64, FileSrc)>) {
                for k in e.children().into_iter().flatten() {
                    if out.len() >= 25 {
                        return;
                    }
                    match &k.kind {
                        Kind::File { size, src } => out.push((k.name.clone(), *size, src.clone())),
                        Kind::Dir(_) => first_files(k, out),
                    }
                }
            }
            let mut v = vec![];
            first_files(&d.root, &mut v);
            for (n, s, src) in &v {
                let mut r = d.reader(src, *s).map_err(|e| e.to_string())?;
                let mut b = Vec::new();
                r.read_to_end(&mut b).map_err(|e| e.to_string())?;
                if b.len() as u64 != *s {
                    return Err(format!("{}: short read", n));
                }
                read += *s;
                if sample.len() < 120 {
                    sample.push_str(&format!("{} ", n));
                }
            }
            Ok(format!("{} / {} views={:?} label={} {} files {} dirs boot={} read {} bytes ok in {:?} [{}]", d.format, d.fs_name, d.fs_views, d.label, f, ds - 1, d.boot.len(), read, t.elapsed(), sample.trim()))
        });
        check(&format!("open {}", p.file_name().unwrap().to_string_lossy()), r);
    }
    println!("{}", if fails == 0 { "ALL PASSED" } else { "SOME CHECKS FAILED" });
    fails
}

fn src_path(dir: &Path) -> PathBuf {
    dir.join("src").join("big data file with spaces.dat")
}
