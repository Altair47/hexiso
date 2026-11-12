// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Drives: list, mount (Windows built-in), burn (Windows disc burner), audio CD ripping.

use crate::jobs::Progress;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub letter: char,
    pub kind: DriveKind,
    pub label: String,
    pub fs: String,
    pub total: u64,
    pub free: u64,
    pub ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Optical,
    Removable,
    Fixed,
    Other,
}

impl DriveKind {
    pub fn name(self) -> &'static str {
        match self {
            DriveKind::Optical => "OPTICAL",
            DriveKind::Removable => "USB/REMOVABLE",
            DriveKind::Fixed => "FIXED",
            DriveKind::Other => "OTHER",
        }
    }
}

fn hidden_cmd(prog: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(prog);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

fn ps(script: &str) -> io::Result<String> {
    let out = hidden_cmd("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).output()?;
    let so = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() {
        let se = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let first = se.lines().next().unwrap_or("command failed").to_string();
        return Err(io::Error::other(first));
    }
    Ok(so)
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub fn list() -> Vec<DriveInfo> {
    use windows_sys::Win32::Storage::FileSystem::*;
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = wide(&format!("{}:\\", letter));
        let t = unsafe { GetDriveTypeW(root.as_ptr()) };
        let kind = match t {
            5 => DriveKind::Optical,
            2 => DriveKind::Removable,
            3 => DriveKind::Fixed,
            _ => DriveKind::Other,
        };
        let mut name = [0u16; 261];
        let mut fsn = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(root.as_ptr(), name.as_mut_ptr(), 261, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fsn.as_mut_ptr(), 261)
        } != 0;
        let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
        let (mut total, mut free) = (0u64, 0u64);
        if ok {
            let mut fa = 0u64;
            unsafe {
                GetDiskFreeSpaceExW(root.as_ptr(), &mut fa, &mut total, &mut free);
            }
        }
        out.push(DriveInfo { letter, kind, label: if ok { s(&name) } else { String::new() }, fs: if ok { s(&fsn) } else { String::new() }, total, free, ready: ok });
    }
    out
}

#[cfg(not(windows))]
pub fn list() -> Vec<DriveInfo> {
    Vec::new()
}

#[cfg(windows)]
pub fn device_length(f: &std::fs::File) -> Option<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
    let mut len: i64 = 0;
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(f.as_raw_handle() as _, IOCTL_DISK_GET_LENGTH_INFO, std::ptr::null(), 0, &mut len as *mut i64 as *mut _, 8, &mut ret, std::ptr::null_mut())
    };
    if ok != 0 && len > 0 { Some(len as u64) } else { None }
}

/// Mount an ISO / VHD with Windows' built-in mounter. Returns the drive letter.
pub fn mount(path: &Path) -> io::Result<String> {
    let p = q(&path.to_string_lossy());
    let s = format!("$d = Mount-DiskImage -ImagePath {} -PassThru; Start-Sleep -Milliseconds 400; ($d | Get-Volume).DriveLetter", p);
    let out = ps(&s)?;
    let letter = out.lines().last().unwrap_or("").trim().to_string();
    Ok(if letter.is_empty() { "?".into() } else { letter })
}

/// Eject every drive Windows has made from this image file (the same file can be mounted several times).
pub fn unmount(path: &Path) -> io::Result<()> {
    let p = q(&path.to_string_lossy());
    let s = format!("for($i=0;$i -lt 26;$i++){{ $d = Get-DiskImage -ImagePath {p}; if(-not $d.Attached){{break}}; Dismount-DiskImage -ImagePath {p} | Out-Null }}");
    ps(&s).map(|_| ())
}

/// Images Windows has mounted right now: (image path, drive letter), one row per drive.
pub fn mounted() -> Vec<(String, String)> {
    let s = "Get-Volume | Where-Object DriveType -eq 'CD-ROM' | ForEach-Object { $img = $_ | Get-DiskImage -ErrorAction SilentlyContinue; if ($img -and $img.ImagePath) { \"$($img.ImagePath)|$($_.DriveLetter)\" } }";
    ps(s)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('|').map(|(a, b)| (a.trim().to_string(), b.trim().to_string())))
        .collect()
}

/// Hand an ISO to the Windows Disc Image Burner.
pub fn burn(path: &Path, drive: char) -> io::Result<()> {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let exe = Path::new(&sys).join("System32").join("isoburn.exe");
    if !exe.exists() {
        return Err(io::Error::other("Windows Disc Image Burner (isoburn.exe) is not installed on this system"));
    }
    Command::new(exe).arg("/Q").arg(format!("{}:", drive)).arg(path).spawn().map(|_| ())
}

pub fn format_drive(letter: char, fs: &str, label: &str) -> io::Result<()> {
    let s = format!("Format-Volume -DriveLetter {} -FileSystem {} -NewFileSystemLabel {} -Force -Confirm:$false | Out-Null", letter, fs, q(label));
    ps(&s).map(|_| ())
}

pub fn eject(letter: char) -> io::Result<()> {
    let s = format!("(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{}:').InvokeVerb('Eject')", letter);
    ps(&s).map(|_| ())
}

pub fn open_in_shell(path: &Path) {
    #[cfg(windows)]
    {
        let _ = hidden_cmd("cmd").arg("/C").arg("start").arg("").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

// ------------------------------------------------------------------ audio CD

#[derive(Clone, Debug)]
pub struct Track {
    pub number: u8,
    pub start: u32,
    pub sectors: u32,
    pub audio: bool,
}

#[cfg(windows)]
fn open_raw(letter: char) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).share_mode(3).open(format!("\\\\.\\{}:", letter))
}

#[cfg(windows)]
pub fn read_toc(letter: char) -> io::Result<Vec<Track>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const IOCTL_CDROM_READ_TOC: u32 = 0x0002_4000;
    let f = open_raw(letter)?;
    let mut toc = vec![0u8; 804];
    let mut ret = 0u32;
    let ok = unsafe { DeviceIoControl(f.as_raw_handle() as _, IOCTL_CDROM_READ_TOC, std::ptr::null(), 0, toc.as_mut_ptr() as *mut _, 804, &mut ret, std::ptr::null_mut()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let first = toc[2];
    let last = toc[3];
    let n = (last - first + 2) as usize; // includes lead-out
    let mut raw = Vec::new();
    for i in 0..n.min(100) {
        let e = &toc[4 + i * 8..12 + i * 8];
        let ctrl = e[1] >> 4;
        let lba = ((e[5] as u32 * 60 + e[6] as u32) * 75 + e[7] as u32).saturating_sub(150);
        raw.push((e[2], lba, ctrl & 4 == 0));
    }
    let mut tracks = Vec::new();
    for w in raw.windows(2) {
        let (num, start, audio) = w[0];
        tracks.push(Track { number: num, start, sectors: w[1].1.saturating_sub(start), audio });
    }
    Ok(tracks)
}

#[cfg(not(windows))]
pub fn read_toc(_letter: char) -> io::Result<Vec<Track>> {
    Err(io::Error::other("audio ripping is only available on Windows"))
}

fn wav_header(bytes: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + bytes).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes());
    h.extend_from_slice(&2u16.to_le_bytes());
    h.extend_from_slice(&44100u32.to_le_bytes());
    h.extend_from_slice(&(44100u32 * 4).to_le_bytes());
    h.extend_from_slice(&4u16.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&bytes.to_le_bytes());
    h
}

#[cfg(windows)]
pub fn rip_tracks(letter: char, tracks: &[Track], dest: &Path, prog: &Progress) -> io::Result<usize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    const IOCTL_CDROM_RAW_READ: u32 = 0x0002_403E;
    let f = open_raw(letter)?;
    let total: u64 = tracks.iter().map(|t| t.sectors as u64 * 2352).sum();
    prog.set_total(total);
    let mut done = 0;
    for t in tracks {
        prog.say(format!("track {:02}", t.number));
        let path = dest.join(format!("Track{:02}.wav", t.number));
        let mut out = io::BufWriter::new(std::fs::File::create(&path)?);
        out.write_all(&wav_header(t.sectors * 2352))?;
        let mut lba = t.start;
        let end = t.start + t.sectors;
        let mut buf = vec![0u8; 2352 * 20];
        while lba < end {
            if prog.cancelled() {
                return Err(io::Error::other("cancelled"));
            }
            let n = (end - lba).min(20);
            #[repr(C)]
            struct RawReadInfo {
                disk_offset: i64,
                sector_count: u32,
                track_mode: u32,
            }
            let info = RawReadInfo { disk_offset: lba as i64 * 2048, sector_count: n, track_mode: 2 };
            let mut ret = 0u32;
            let mut ok = 0;
            for _ in 0..3 {
                ok = unsafe {
                    DeviceIoControl(f.as_raw_handle() as _, IOCTL_CDROM_RAW_READ, &info as *const _ as *const _, std::mem::size_of::<RawReadInfo>() as u32, buf.as_mut_ptr() as *mut _, n * 2352, &mut ret, std::ptr::null_mut())
                };
                if ok != 0 {
                    break;
                }
            }
            if ok == 0 {
                buf[..(n * 2352) as usize].fill(0);
            }
            out.write_all(&buf[..(n * 2352) as usize])?;
            lba += n;
            prog.add(n as u64 * 2352);
        }
        out.flush()?;
        done += 1;
    }
    Ok(done)
}

#[cfg(not(windows))]
pub fn rip_tracks(_letter: char, _tracks: &[Track], _dest: &Path, _prog: &Progress) -> io::Result<usize> {
    Err(io::Error::other("audio ripping is only available on Windows"))
}
