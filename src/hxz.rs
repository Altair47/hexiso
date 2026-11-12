// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! HXZ: this app's own compressed + optionally encrypted image container (our answer to DAA),
//! plus a CSO writer. Both compress blocks on all CPU cores.

use crate::source::{read_blocked, read_exact_at, u32le, u64le, SectorSource, SECTOR};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

pub const MAGIC: &[u8; 4] = b"HXZ1";
const HEADER: u64 = 96;
const BLOCK: usize = 64 * 1024;
const ITER: u32 = 200_000;

fn derive(pw: &str, salt: &[u8]) -> ChaCha20Poly1305 {
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), salt, ITER, &mut key);
    ChaCha20Poly1305::new(Key::from_slice(&key))
}

fn nonce(i: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&i.to_le_bytes());
    n
}

pub fn is_encrypted(path: &Path) -> io::Result<bool> {
    let f = File::open(path)?;
    let mut h = [0u8; 8];
    read_exact_at(&f, &mut h, 0)?;
    Ok(h[5] & 1 != 0)
}

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 32)
}

fn deflate(level: u32, data: &[u8]) -> Vec<u8> {
    let mut e = DeflateEncoder::new(Vec::with_capacity(data.len() / 2), Compression::new(level));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

// ------------------------------------------------------------------ writer

pub struct HxzWriter {
    f: File,
    buf: Vec<u8>,
    pending: Vec<Vec<u8>>,
    index: Vec<(u64, u32, u32)>,
    pos: u64,
    total: u64,
    cipher: Option<ChaCha20Poly1305>,
    salt: [u8; 16],
    level: u32,
}

impl HxzWriter {
    pub fn create(path: &Path, password: Option<&str>, level: u32) -> io::Result<Self> {
        let mut f = File::create(path)?;
        f.write_all(&[0u8; HEADER as usize])?;
        let mut salt = [0u8; 16];
        getrandom::getrandom(&mut salt).map_err(|e| io::Error::other(e.to_string()))?;
        let cipher = password.filter(|p| !p.is_empty()).map(|p| derive(p, &salt));
        Ok(HxzWriter { f, buf: Vec::with_capacity(BLOCK), pending: vec![], index: vec![], pos: HEADER, total: 0, cipher, salt, level })
    }

    fn flush_batch(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let blocks = std::mem::take(&mut self.pending);
        let base = self.index.len() as u64;
        let n = threads();
        let level = self.level;
        let cipher = &self.cipher;
        let results: Vec<(Vec<u8>, u32)> = std::thread::scope(|s| {
            let chunk = blocks.len().div_ceil(n);
            let handles: Vec<_> = blocks
                .chunks(chunk.max(1))
                .enumerate()
                .map(|(ci, ch)| {
                    s.spawn(move || {
                        ch.iter()
                            .enumerate()
                            .map(|(k, b)| {
                                let idx = base + (ci * chunk + k) as u64;
                                let c = deflate(level, b);
                                let (mut data, flag) = if c.len() < b.len() { (c, 1u32) } else { (b.clone(), 0u32) };
                                if let Some(ci) = cipher {
                                    data = ci.encrypt(Nonce::from_slice(&nonce(idx)), data.as_ref()).unwrap_or_default();
                                }
                                (data, flag)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        for (data, flag) in results {
            self.f.write_all(&data)?;
            self.index.push((self.pos, data.len() as u32, flag));
            self.pos += data.len() as u64;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<u64> {
        if !self.buf.is_empty() {
            let mut b = std::mem::take(&mut self.buf);
            b.resize(BLOCK, 0);
            self.pending.push(b);
        }
        self.flush_batch()?;
        let index_off = self.pos;
        let mut idx = Vec::with_capacity(self.index.len() * 16);
        for (o, l, fl) in &self.index {
            idx.extend_from_slice(&o.to_le_bytes());
            idx.extend_from_slice(&l.to_le_bytes());
            idx.extend_from_slice(&fl.to_le_bytes());
        }
        self.f.write_all(&idx)?;
        let mut h = vec![0u8; HEADER as usize];
        h[0..4].copy_from_slice(MAGIC);
        h[4] = 1;
        h[5] = self.cipher.is_some() as u8;
        h[8..12].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        h[12..20].copy_from_slice(&self.total.to_le_bytes());
        h[20..24].copy_from_slice(&(self.index.len() as u32).to_le_bytes());
        h[24..32].copy_from_slice(&index_off.to_le_bytes());
        h[32..48].copy_from_slice(&self.salt);
        if let Some(c) = &self.cipher {
            let chk = c.encrypt(Nonce::from_slice(&[0xFF; 12]), &[0u8; 16][..]).unwrap_or_default();
            h[48..48 + chk.len().min(32)].copy_from_slice(&chk[..chk.len().min(32)]);
        }
        self.f.seek(SeekFrom::Start(0))?;
        self.f.write_all(&h)?;
        self.f.flush()?;
        Ok(index_off + idx.len() as u64)
    }
}

impl Write for HxzWriter {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let n = data.len();
        self.total += n as u64;
        while !data.is_empty() {
            let take = (BLOCK - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == BLOCK {
                let b = std::mem::replace(&mut self.buf, Vec::with_capacity(BLOCK));
                self.pending.push(b);
                if self.pending.len() >= threads() * 8 {
                    self.flush_batch()?;
                }
            }
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ------------------------------------------------------------------ reader

pub struct HxzImage {
    f: File,
    total: u64,
    index: Vec<(u64, u32, u32)>,
    cipher: Option<ChaCha20Poly1305>,
    cache: Mutex<Option<(u64, Vec<u8>)>>,
    packed: u64,
}

impl HxzImage {
    pub fn open(path: &Path, password: Option<&str>) -> io::Result<Self> {
        let f = File::open(path)?;
        let mut h = vec![0u8; HEADER as usize];
        read_exact_at(&f, &mut h, 0)?;
        if &h[0..4] != MAGIC {
            return Err(crate::source::bad("not an HXZ image"));
        }
        let enc = h[5] & 1 != 0;
        let total = u64le(&h, 12);
        let count = u32le(&h, 20) as usize;
        let index_off = u64le(&h, 24);
        let cipher = if enc {
            let pw = password.ok_or_else(|| crate::source::bad("this image is password protected"))?;
            let c = derive(pw, &h[32..48]);
            let ok = c.decrypt(Nonce::from_slice(&[0xFF; 12]), &h[48..80]).map(|d| d == [0u8; 16]).unwrap_or(false);
            if !ok {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "wrong password"));
            }
            Some(c)
        } else {
            None
        };
        let mut raw = vec![0u8; count * 16];
        read_exact_at(&f, &mut raw, index_off)?;
        let index = raw.chunks(16).map(|c| (u64le(c, 0), u32le(c, 8), u32le(c, 12))).collect();
        let packed = f.metadata()?.len();
        Ok(HxzImage { f, total, index, cipher, cache: Mutex::new(None), packed })
    }
    fn block(&self, b: u64) -> io::Result<Vec<u8>> {
        if let Some((cb, d)) = &*self.cache.lock().unwrap()
            && *cb == b {
                return Ok(d.clone());
            }
        let Some(&(off, len, flag)) = self.index.get(b as usize) else {
            return Ok(vec![0; BLOCK]);
        };
        let mut data = vec![0u8; len as usize];
        read_exact_at(&self.f, &mut data, off)?;
        if let Some(c) = &self.cipher {
            data = c.decrypt(Nonce::from_slice(&nonce(b)), data.as_ref()).map_err(|_| crate::source::bad("block failed its integrity check"))?;
        }
        if flag & 1 != 0 {
            let mut out = Vec::with_capacity(BLOCK);
            flate2::read::DeflateDecoder::new(&data[..]).read_to_end(&mut out)?;
            data = out;
        }
        data.resize(BLOCK, 0);
        *self.cache.lock().unwrap() = Some((b, data.clone()));
        Ok(data)
    }
}

impl SectorSource for HxzImage {
    fn sector_count(&self) -> u64 {
        self.total.div_ceil(SECTOR)
    }
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()> {
        read_blocked(lba * SECTOR, buf, BLOCK as u64, |b| self.block(b))
    }
    fn format_name(&self) -> String {
        if self.cipher.is_some() { "HXZ (compressed, encrypted)".into() } else { "HXZ (compressed)".into() }
    }
    fn details(&self) -> Vec<(String, String)> {
        let mut v = vec![("Ratio".to_string(), format!("{:.1}%", self.packed as f64 * 100.0 / self.total.max(1) as f64))];
        if self.cipher.is_some() {
            v.push(("Encryption".into(), "ChaCha20-Poly1305, PBKDF2-SHA256 key".into()));
        }
        v
    }
}

// ------------------------------------------------------------------ CSO writer

pub struct CsoWriter {
    f: File,
    buf: Vec<u8>,
    pending: Vec<Vec<u8>>,
    index: Vec<u32>,
    pos: u64,
    align: u32,
    total_hint: u64,
    total: u64,
}

impl CsoWriter {
    pub fn create(path: &Path, total_hint: u64) -> io::Result<Self> {
        let mut align = 0u32;
        while (total_hint + (total_hint >> 4)) >> align >= 0x7FFF_FFFF {
            align += 1;
        }
        let blocks = total_hint.div_ceil(SECTOR);
        let mut f = File::create(path)?;
        let header = 24 + (blocks + 1) * 4;
        f.write_all(&vec![0u8; header as usize])?;
        Ok(CsoWriter { f, buf: Vec::with_capacity(SECTOR as usize), pending: vec![], index: vec![], pos: header, align, total_hint, total: 0 })
    }
    fn flush_batch(&mut self) -> io::Result<()> {
        let blocks = std::mem::take(&mut self.pending);
        if blocks.is_empty() {
            return Ok(());
        }
        let n = threads();
        let chunk = blocks.len().div_ceil(n).max(1);
        let results: Vec<Vec<u8>> = std::thread::scope(|s| {
            let hs: Vec<_> = blocks.chunks(chunk).map(|ch| s.spawn(move || ch.iter().map(|b| deflate(6, b)).collect::<Vec<_>>())).collect();
            hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        for (b, c) in blocks.iter().zip(results) {
            // align
            let a = 1u64 << self.align;
            if !self.pos.is_multiple_of(a) {
                let pad = a - self.pos % a;
                self.f.write_all(&vec![0u8; pad as usize])?;
                self.pos += pad;
            }
            let (data, plain) = if c.len() < b.len() { (c, 0u32) } else { (b.clone(), 0x8000_0000u32) };
            self.index.push(((self.pos >> self.align) as u32) | plain);
            self.f.write_all(&data)?;
            self.pos += data.len() as u64;
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<u64> {
        if !self.buf.is_empty() {
            let mut b = std::mem::take(&mut self.buf);
            b.resize(SECTOR as usize, 0);
            self.pending.push(b);
        }
        self.flush_batch()?;
        let a = 1u64 << self.align;
        if !self.pos.is_multiple_of(a) {
            let pad = a - self.pos % a;
            self.f.write_all(&vec![0u8; pad as usize])?;
            self.pos += pad;
        }
        self.index.push((self.pos >> self.align) as u32);
        let _ = self.total_hint;
        let mut h = vec![0u8; 24];
        h[0..4].copy_from_slice(b"CISO");
        h[4..8].copy_from_slice(&24u32.to_le_bytes());
        h[8..16].copy_from_slice(&self.total.to_le_bytes());
        h[16..20].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        h[20] = 1;
        h[21] = self.align as u8;
        self.f.seek(SeekFrom::Start(0))?;
        self.f.write_all(&h)?;
        let mut idx = Vec::with_capacity(self.index.len() * 4);
        for i in &self.index {
            idx.extend_from_slice(&i.to_le_bytes());
        }
        let room = (self.total_hint.div_ceil(SECTOR) + 1) as usize * 4;
        if idx.len() > room {
            return Err(io::Error::other("CSO index overflow"));
        }
        self.f.write_all(&idx)?;
        Ok(self.pos)
    }
}

impl Write for CsoWriter {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let n = data.len();
        self.total += n as u64;
        while !data.is_empty() {
            let take = (SECTOR as usize - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == SECTOR as usize {
                let b = std::mem::replace(&mut self.buf, Vec::with_capacity(SECTOR as usize));
                self.pending.push(b);
                if self.pending.len() >= threads() * 256 {
                    self.flush_batch()?;
                }
            }
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
