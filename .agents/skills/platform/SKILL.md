---
name: hexiso-platform
description: How HEXISO integrates with Windows (drives.rs: Win32 calls, virtual disk API, PowerShell Format-Volume, isoburn, console handling) and how to port the drive features to Linux. Use when touching drives, mounting, burning, formatting, ripping or start-up/console behaviour.
---

# Platform integration

Everything that reads, writes or converts images is portable Rust. OS integration is isolated in
`src/drives.rs` (plus console handling in `main.rs` and the icon resource in `build.rs`).

## Windows today

| Feature | Mechanism | Function |
|---|---|---|
| List drives | `GetLogicalDrives`, `GetDriveTypeW`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW` (`windows-sys`) | `drives::list` (always called off the UI thread) |
| Device size | `DeviceIoControl(IOCTL_DISK_GET_LENGTH_INFO)` on `\\.\X:` | `device_length` |
| Read discs / USB | `\\.\X:` opened with Rust's `File` + positional reads | `source::DriveSource` |
| Audio CD | `IOCTL_CDROM_READ_TOC`, `IOCTL_CDROM_RAW_READ` (2352-byte sectors, 3 retries, zero-fill) | `read_toc`, `rip_tracks` |
| Mount / unmount | virtual disk API (`Win32_Storage_Vhd`): `OpenVirtualDisk` (version 2 parameters, type from the extension), `AttachVirtualDisk` (permanent lifetime; read-only for ISO), `DetachVirtualDisk` looped while `GetVirtualDiskPhysicalPath` reports it attached | `mount`, `unmount` |
| Drive letter after mount | the local letter that was not there before the attach and whose `backing_file` is the image, polled up to 10 s | `mount` |
| List mounted images | for each local letter, `GetStorageDependencyInformation(HOST_VOLUMES)` on `\\.\X:` → host volume + relative path, volume GUID → letter with `GetVolumePathNamesForVolumeNameW` | `mounted` |
| Burn | starts `%SystemRoot%\System32\isoburn.exe /Q X: image` | `burn` |
| Format | PowerShell `Format-Volume`; `fs` must be one of `drives::FORMATS` (checked with `format_name`) | `format_drive` |
| Eject | mounted image → `unmount`; otherwise `DeviceIoControl` `FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`, `IOCTL_STORAGE_MEDIA_REMOVAL`, `IOCTL_STORAGE_EJECT_MEDIA` | `eject` |
| Open files / folders | `ShellExecuteW` (`open`), `explorer /select,` | `open_in_shell`, `reveal_in_shell` |

**PowerShell is used for one thing only: `Format-Volume`** (rare, destructive, documented; the
start-up cost does not matter there). Do not add new PowerShell calls: use the Win32 API through
`windows-sys` (add the feature to `Cargo.toml`). `ps()` runs it hidden (`CREATE_NO_WINDOW`) with
`-NoProfile -NonInteractive`; arguments are single-quoted with `q()`, and anything that is not a
free-form string (file system names, drive letters) is checked against a fixed list first. The first
stderr line becomes the error message.

Mount notes: Windows mounts `.iso`, `.vhd`, `.vhdx` only (other formats are converted to
`%TEMP%\hexiso\iso\*.iso` first); `mount` refuses other extensions. VHD/VHDX are attached writable,
ISO read-only. Attaching needs an administrator account (error 1314 for standard users, also with
PowerShell `Mount-DiskImage`); `explain_mount_error` turns that into plain advice. `unmount` of an
image that is not mounted succeeds.

**Never mount the same file twice** (same path or a hard link): Windows then treats every drive of
that file as the first one: `GetVirtualDiskPhysicalPath`, `DetachVirtualDisk`, Explorer's Eject and
`Dismount-DiskImage` all act on the first attachment, so one copy can't be ejected on its own. A
second drive of a read-only ISO adds nothing, so HEXISO has no "mount another copy".

Console: the exe is a console-subsystem program so the CLI gets stdout / exit codes. For the window,
`detach_console` frees a console it owns, or relaunches itself detached (`HEXISO_FOREGROUND=1` set
on the child) when it shares a terminal, so terminals are never blocked. `--screenshot` never
relaunches.

## Porting to Linux (not done yet)

The crate should compile on Linux already (Windows code is `#[cfg(windows)]`, `read_at` has a Unix
branch, the CLI and engine are portable, `soft.rs` uses winit + softbuffer which support X11/Wayland).
Every public function in `drives.rs` already has a `#[cfg(not(windows))]` twin that returns an empty
list or a "not available on this system yet" error; a port replaces those twins. What has to be
written, all in `drives.rs` (+ `DriveSource` in `source.rs`):

| Feature | Linux approach |
|---|---|
| List drives | `lsblk --json -o NAME,TYPE,RM,SIZE,LABEL,FSTYPE,MOUNTPOINT` or `/sys/block` |
| Device size / read | open `/dev/sr0`, `/dev/sdX`; `ioctl(BLKGETSIZE64)` |
| Audio CD | `CDROMREADTOCHDR`, `CDROMREADTOCENTRY`, `CDROMREADAUDIO` ioctls |
| Mount | `udisksctl loop-setup -f img` + `udisksctl mount -b /dev/loopN` (no root) |
| Burn | `xorriso -as cdrecord` / `growisofs`, or libburn |
| Format | `mkfs.vfat` / `mkfs.exfat` / `mkfs.ntfs` via udisks |
| Eject | `ioctl(CDROMEJECT)` / `udisksctl power-off` |
| Open in file manager | `xdg-open` (already used, also for `reveal_in_shell`) |

Keep the same function signatures so `app.rs`, `dialogs.rs` and `cli.rs` don't change, and keep a
clear error for anything unsupported.
