---
name: hexiso-platform
description: How HEXISO integrates with Windows (drives.rs: Win32 calls, PowerShell, isoburn, console handling) and how to port the drive features to Linux. Use when touching drives, mounting, burning, formatting, ripping or start-up/console behaviour.
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
| Mount / unmount / list | PowerShell `Mount-DiskImage`, `Dismount-DiskImage` (looped until detached), `Get-Volume \| Get-DiskImage` | `mount`, `unmount`, `mounted` |
| Burn | starts `%SystemRoot%\System32\isoburn.exe /Q X: image` | `burn` |
| Format | PowerShell `Format-Volume` | `format_drive` |
| Eject | PowerShell, Shell COM `Eject` verb | `eject` |
| Open files / folders | `cmd /c start`, `explorer /select,` | `open_in_shell`, `doc_header` |

PowerShell is run hidden (`CREATE_NO_WINDOW`) with `-NoProfile -NonInteractive` through `ps()`;
arguments are single-quoted with `q()` (doubles embedded quotes). **Always quote user paths with
`q()`.** The first stderr line becomes the error message.

Mount notes: Windows mounts `.iso`, `.vhd`, `.vhdx` only (other formats are converted to
`%TEMP%\hexiso\iso\*.iso` first); the same file can be attached several times, so `unmount` loops;
"mount another copy" uses a hard link in `%TEMP%\hexiso\mount`.

Console: the exe is a console-subsystem program so the CLI gets stdout / exit codes. For the window,
`detach_console` frees a console it owns, or relaunches itself detached (`HEXISO_FOREGROUND=1` set
on the child) when it shares a terminal, so terminals are never blocked. `--screenshot` never
relaunches.

## Porting to Linux (not done yet)

The crate should compile on Linux already (Windows code is `#[cfg(windows)]`, `read_at` has a Unix
branch, the CLI and engine are portable, `soft.rs` uses winit + softbuffer which support X11/Wayland).
What has to be written, all in `drives.rs` (+ `DriveSource` in `source.rs`):

| Feature | Linux approach |
|---|---|
| List drives | `lsblk --json -o NAME,TYPE,RM,SIZE,LABEL,FSTYPE,MOUNTPOINT` or `/sys/block` |
| Device size / read | open `/dev/sr0`, `/dev/sdX`; `ioctl(BLKGETSIZE64)` |
| Audio CD | `CDROMREADTOCHDR`, `CDROMREADTOCENTRY`, `CDROMREADAUDIO` ioctls |
| Mount | `udisksctl loop-setup -f img` + `udisksctl mount -b /dev/loopN` (no root) |
| Burn | `xorriso -as cdrecord` / `growisofs`, or libburn |
| Format | `mkfs.vfat` / `mkfs.exfat` / `mkfs.ntfs` via udisks |
| Open in file manager | `xdg-open` (already used) |

Keep the same function signatures so `app.rs`, `dialogs.rs` and `cli.rs` don't change, and keep a
clear error for anything unsupported.
