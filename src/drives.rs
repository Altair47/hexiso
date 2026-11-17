// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)
// SPDX-License-Identifier: AGPL-3.0-only

//! Drives: list, mount (Windows virtual disk API), burn (Windows disc burner), format, eject,
//! audio CD ripping. All OS-specific code lives here; every public function has a non-Windows
//! version (an empty result or a clear error) so a Linux port only has to fill these in.

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

/// File systems `format_drive` accepts (exact spelling Windows expects).
pub const FORMATS: [&str; 3] = ["FAT32", "NTFS", "exFAT"];

/// The canonical spelling of a file system name from `FORMATS`, case-insensitive.
pub fn format_name(fs: &str) -> Option<&'static str> {
    FORMATS.iter().copied().find(|f| f.eq_ignore_ascii_case(fs))
}

#[cfg(windows)]
fn hidden_cmd(prog: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(prog);
    c.creation_flags(0x0800_0000);
    c
}

#[cfg(windows)]
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

#[cfg(windows)]
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn from_wide(b: &[u16]) -> String {
    String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
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
        let (mut total, mut free) = (0u64, 0u64);
        if ok {
            let mut fa = 0u64;
            unsafe {
                GetDiskFreeSpaceExW(root.as_ptr(), &mut fa, &mut total, &mut free);
            }
        }
        out.push(DriveInfo { letter, kind, label: if ok { from_wide(&name) } else { String::new() }, fs: if ok { from_wide(&fsn) } else { String::new() }, total, free, ready: ok });
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

// ------------------------------------------------------------------ virtual drives (Windows)

#[cfg(windows)]
mod vdisk {
    use super::{from_wide, wide};
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::Storage::Vhd::*;

    fn check(code: u32) -> io::Result<()> {
        if code == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
    }

    /// Open an image file (.iso / .vhd / .vhdx; the type comes from the extension).
    pub fn open(path: &Path, read_only: bool) -> io::Result<OwnedHandle> {
        let vst = VIRTUAL_STORAGE_TYPE { DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_UNKNOWN, VendorId: windows_sys::core::GUID::from_u128(0) };
        let mut p = OPEN_VIRTUAL_DISK_PARAMETERS { Version: OPEN_VIRTUAL_DISK_VERSION_2, ..Default::default() };
        p.Anonymous.Version2 = OPEN_VIRTUAL_DISK_PARAMETERS_0_1 { GetInfoOnly: 0, ReadOnly: read_only as i32, ResiliencyGuid: windows_sys::core::GUID::from_u128(0) };
        let mut h: HANDLE = std::ptr::null_mut();
        let p16 = wide(&path.to_string_lossy());
        check(unsafe { OpenVirtualDisk(&vst, p16.as_ptr(), VIRTUAL_DISK_ACCESS_NONE, OPEN_VIRTUAL_DISK_FLAG_NONE, &p, &mut h) })?;
        Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
    }

    /// `\\.\CDROM1` / `\\.\PhysicalDrive3` while attached, `None` when not attached.
    pub fn physical_path(h: &OwnedHandle) -> Option<String> {
        let mut buf = [0u16; 260];
        let mut size = (buf.len() * 2) as u32;
        let r = unsafe { GetVirtualDiskPhysicalPath(h.as_raw_handle() as _, &mut size, buf.as_mut_ptr()) };
        Some(from_wide(&buf)).filter(|s| r == 0 && !s.is_empty())
    }

    pub fn attach(h: &OwnedHandle, read_only: bool) -> io::Result<()> {
        let params = ATTACH_VIRTUAL_DISK_PARAMETERS { Version: ATTACH_VIRTUAL_DISK_VERSION_1, ..Default::default() };
        let flags = ATTACH_VIRTUAL_DISK_FLAG_PERMANENT_LIFETIME | if read_only { ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY } else { 0 };
        check(unsafe { AttachVirtualDisk(h.as_raw_handle() as _, std::ptr::null_mut(), flags, 0, &params, std::ptr::null()) })
    }

    pub fn detach(h: &OwnedHandle) -> io::Result<()> {
        check(unsafe { DetachVirtualDisk(h.as_raw_handle() as _, DETACH_VIRTUAL_DISK_FLAG_NONE, 0) })
    }

    /// Open a device (`\\.\X:`, `\\.\CDROM1`) without read/write access: enough for queries.
    fn open_device(name: &str) -> io::Result<OwnedHandle> {
        let n = wide(name);
        let h = unsafe { CreateFileW(n.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
    }

    /// Letters of local drives (network and missing drives skipped).
    pub fn local_letters() -> impl Iterator<Item = char> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26u32).filter(move |i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).filter(|l| {
            let root = wide(&format!("{}:\\", l));
            // removable, fixed, CD-ROM: virtual disks show up as these
            matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5)
        })
    }

    /// Drive letters backed by the image `key` (lower-case path, as `backing_file` spells it).
    pub fn letters_of(key: &str) -> Vec<char> {
        local_letters().filter(|&l| backing_file(l).is_some_and(|p| p.to_lowercase() == key)).collect()
    }

    /// Image file behind the volume `X:`, if it is a virtual disk.
    pub fn backing_file(letter: char) -> Option<String> {
        let h = open_device(&format!("\\\\.\\{}:", letter)).ok()?;
        // u64 storage keeps the structure (which holds pointers) aligned
        let mut buf = vec![0u64; 512];
        for _ in 0..2 {
            let info = buf.as_mut_ptr() as *mut STORAGE_DEPENDENCY_INFO;
            unsafe {
                (*info).Version = STORAGE_DEPENDENCY_INFO_VERSION_2;
            }
            let mut used = 0u32;
            let r = unsafe { GetStorageDependencyInformation(h.as_raw_handle() as _, GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES, (buf.len() * 8) as u32, info, &mut used) };
            if r == ERROR_INSUFFICIENT_BUFFER {
                buf = vec![0u64; (used as usize).div_ceil(8)];
                continue;
            }
            if r != 0 {
                return None;
            }
            let count = unsafe { (*info).NumberEntries } as usize;
            let first = unsafe { std::ptr::addr_of!((*info).Anonymous.Version2Entries) } as *const STORAGE_DEPENDENCY_INFO_TYPE_2;
            // the entry closest to the volume (nested images report their parents too)
            let e = (0..count).map(|i| unsafe { &*first.add(i) }).min_by_key(|e| e.AncestorLevel)?;
            let host = unsafe { pwstr(e.HostVolumeName) };
            let rel = unsafe { pwstr(e.DependentVolumeRelativePath) };
            if rel.is_empty() {
                return None;
            }
            let root = volume_root(&host).unwrap_or(host);
            return Some(format!("{}\\{}", root.trim_end_matches('\\'), rel.trim_start_matches('\\')));
        }
        None
    }

    /// # Safety
    /// `p` is null or points to a NUL-terminated UTF-16 string.
    unsafe fn pwstr(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
    }

    /// `C:\` for `\\?\Volume{...}\`.
    fn volume_root(vol: &str) -> Option<String> {
        let mut v = vol.to_string();
        if !v.ends_with('\\') {
            v.push('\\');
        }
        let v16 = wide(&v);
        let mut buf = [0u16; 1024];
        let mut len = 0u32;
        let ok = unsafe { GetVolumePathNamesForVolumeNameW(v16.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, &mut len) };
        Some(from_wide(&buf)).filter(|s| ok != 0 && !s.is_empty())
    }
}

/// Turn "a required privilege is not held" into advice a person can act on.
#[cfg(windows)]
fn explain_mount_error(e: io::Error) -> io::Error {
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    if e.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) {
        return io::Error::new(io::ErrorKind::PermissionDenied, "mounting needs an administrator account: Windows only lets administrators attach virtual drives. Sign in with an administrator account or ask one to mount it.");
    }
    e
}

/// Mount an ISO / VHD / VHDX with Windows' built-in virtual drive. Returns the drive letter
/// (`?` when Windows attached the image but gave it no letter in time).
#[cfg(windows)]
pub fn mount(path: &Path) -> io::Result<String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "iso" | "vhd" | "vhdx") {
        return Err(io::Error::other("Windows can only mount .iso, .vhd and .vhdx files"));
    }
    // ISO is always read-only; VHD / VHDX are attached writable, like Explorer does
    let read_only = ext == "iso";
    let want = image_key(path);
    // drives already backed by this file (normally none) are not the new one
    let before = vdisk::letters_of(&want);
    let h = vdisk::open(path, read_only).map_err(explain_mount_error)?;
    vdisk::attach(&h, read_only).map_err(explain_mount_error)?;
    // the new drive is a letter backed by this file that wasn't before. A letter that was just
    // freed (remount) can come straight back, so letters are not compared, only their image.
    // (Asking the virtual disk handle for its device is not enough: when the same file is
    // attached twice, Windows answers with the first attachment for every handle.)
    for _ in 0..200 {
        if let Some(l) = vdisk::letters_of(&want).into_iter().find(|l| !before.contains(l)) {
            return Ok(l.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok("?".into())
}

/// How `backing_file` spells an image path, for comparisons.
#[cfg(windows)]
fn image_key(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").to_lowercase()
}

#[cfg(not(windows))]
pub fn mount(_path: &Path) -> io::Result<String> {
    Err(io::Error::other("mounting is not available on this system yet"))
}

/// Eject every drive Windows made from this image file. Not mounted is not an error.
#[cfg(windows)]
pub fn unmount(path: &Path) -> io::Result<()> {
    let h = vdisk::open(path, true).map_err(explain_mount_error)?;
    // the same file can be attached more than once
    for _ in 0..26 {
        if vdisk::physical_path(&h).is_none() {
            break;
        }
        vdisk::detach(&h).map_err(explain_mount_error)?;
    }
    // Windows removes the drive letter a moment after the detach: wait for it, so a remount
    // straight after sees a clean state
    let want = image_key(path);
    for _ in 0..100 {
        if vdisk::letters_of(&want).is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn unmount(_path: &Path) -> io::Result<()> {
    Err(io::Error::other("mounting is not available on this system yet"))
}

/// Images mounted right now: (image path, drive letter), one row per drive.
#[cfg(windows)]
pub fn mounted() -> Vec<(String, String)> {
    vdisk::local_letters().filter_map(|l| vdisk::backing_file(l).map(|p| (p, l.to_string()))).collect()
}

#[cfg(not(windows))]
pub fn mounted() -> Vec<(String, String)> {
    Vec::new()
}

/// Hand an ISO to the Windows Disc Image Burner.
#[cfg(windows)]
pub fn burn(path: &Path, drive: char) -> io::Result<()> {
    let sys = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let exe = Path::new(&sys).join("System32").join("isoburn.exe");
    if !exe.exists() {
        return Err(io::Error::other("Windows Disc Image Burner (isoburn.exe) is not installed on this system"));
    }
    Command::new(exe).arg("/Q").arg(format!("{}:", drive)).arg(path).spawn().map(|_| ())
}

#[cfg(not(windows))]
pub fn burn(_path: &Path, _drive: char) -> io::Result<()> {
    Err(io::Error::other("burning is not available on this system yet"))
}

/// Erase drive `letter` and create an empty file system. `fs` must be one of `FORMATS`.
/// Runs PowerShell `Format-Volume`, the documented Windows storage API for this.
#[cfg(windows)]
pub fn format_drive(letter: char, fs: &str, label: &str) -> io::Result<()> {
    let fs = format_name(fs).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("unknown file system '{}' (use {})", fs, FORMATS.join(", "))))?;
    if !letter.is_ascii_alphabetic() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid drive letter"));
    }
    let s = format!("Format-Volume -DriveLetter {} -FileSystem {} -NewFileSystemLabel {} -Force -Confirm:$false | Out-Null", letter, fs, q(label));
    ps(&s).map(|_| ())
}

#[cfg(not(windows))]
pub fn format_drive(_letter: char, _fs: &str, _label: &str) -> io::Result<()> {
    Err(io::Error::other("formatting is not available on this system yet"))
}

/// Eject the media in drive `letter`: opens the disc tray, prepares a USB stick for removal,
/// or unmounts a mounted image.
#[cfg(windows)]
pub fn eject(letter: char) -> io::Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::*;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::*;
    if let Some(img) = vdisk::backing_file(letter) {
        return unmount(Path::new(&img));
    }
    let n = wide(&format!("\\\\.\\{}:", letter));
    let h = unsafe { CreateFileW(n.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let h = unsafe { OwnedHandle::from_raw_handle(h as _) };
    let ioctl = |code: u32, input: &[u8]| {
        let mut ret = 0u32;
        let ok = unsafe { DeviceIoControl(h.as_raw_handle() as _, code, input.as_ptr() as *const _, input.len() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) };
        if ok != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    };
    // lock + dismount flushes cached writes; an empty tray has nothing to lock
    if ioctl(FSCTL_LOCK_VOLUME, &[]).is_ok() {
        let _ = ioctl(FSCTL_DISMOUNT_VOLUME, &[]);
    }
    let _ = ioctl(IOCTL_STORAGE_MEDIA_REMOVAL, &[0]); // allow removal
    ioctl(IOCTL_STORAGE_EJECT_MEDIA, &[])
}

#[cfg(not(windows))]
pub fn eject(_letter: char) -> io::Result<()> {
    Err(io::Error::other("ejecting is not available on this system yet"))
}

/// Open a file or folder with its default program (Explorer for folders).
pub fn open_in_shell(path: &Path) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let p = wide(&path.to_string_lossy());
        let verb = wide("open");
        unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), p.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

/// Show a file selected in its folder.
pub fn reveal_in_shell(path: &Path) {
    #[cfg(windows)]
    {
        let _ = Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    }
    #[cfg(not(windows))]
    {
        if let Some(dir) = path.parent() {
            let _ = Command::new("xdg-open").arg(dir).spawn();
        }
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
                return Err(crate::jobs::cancelled());
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
