---
name: hexiso-testing
description: HEXISO's testing setup (engine self-test, PowerShell smoke test, screenshots, renderer and performance checks, environment variables) and how to extend it. Use to verify any change before finishing.
---

# Testing HEXISO

There are no `cargo test` unit tests yet; verification is end-to-end and exercises the real binary.

## 1. Engine self-test (seconds, no window)

```powershell
cargo run --release -- selftest [work-dir] [extra-images...]
```

`src/selftest.rs` builds a sample tree (long names, Unicode, nested folders, empty and 5 MB files),
writes an ISO with a BIOS boot entry, re-reads it through Joliet and ISO 9660 and compares every byte,
converts to CSO / HXZ / encrypted HXZ and compares sector hashes, checks a wrong password is refused,
edits the ISO in place (delete + rename + add) and verifies, round-trips a FAT12 floppy and a ZIP,
splits and re-reads volumes, and computes checksums. Extra image paths are opened and their first files
read in full: use this with real-world ISOs (Windows / Linux installers) when touching readers.
Prints `[ ok ]` / `[FAIL]` lines and `ALL PASSED`; exit code 0 only when everything passed.

## 2. Smoke test (a few minutes)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 [-SkipBuild] [-Mount] [-Exe path]
```

Builds the release exe, then: self-test; every CLI command family with JSON parsing and byte
comparisons (create / info / ls / add --to / rename / rm / label / boot / extract incl. Unicode
names / convert CSO+HXZ / password enforcement / test / split / floppy / exit codes / drives);
optionally mount → list → unmount; window screenshots with **both renderers**; redraw speed per
renderer; **idle CPU must stay below 0.2 CPU-seconds in 4 s**. Work files go to a fresh folder in
`%TEMP%\hexiso-smoke`. Windows PowerShell 5.1 reads scripts as ANSI: keep the script ASCII-only (build
Unicode strings from `[char]` codes).

GPU checks are skipped automatically when the desktop session is disconnected (remote desktop closed):
Direct3D cannot present there. That is an environment limit, not a bug.

## 3. Looking at the window

```powershell
hexiso --screenshot out.png [image.iso] [--view sectors] [--lba 16]
```

Draws ~120 frames, saves a PNG and exits (window opens unfocused and behind others; on Windows it keeps
painting while covered). Always open the PNG after UI changes. `HEXISO_SIZE=480x340` checks the
minimum size (also try 640x460 and 900x560: the layout changes at several breakpoints). Automated UI checks use exactly this option, so it must never break.

## 4. Environment variables

| Variable | Effect |
|---|---|
| `HEXISO_RENDERER=cpu\|gpu` | force a renderer |
| `HEXISO_BENCH=1` | redraw continuously for 5 s, print `bench: N frames ... fps`, exit |
| `HEXISO_DEBUG_REPAINT=1` | print each frame's repaint causes (find what keeps the app busy) |
| `HEXISO_SIZE=WxH` | initial window size |
| `HEXISO_VSYNC=1` | GPU renderer: wait for vsync |
| `HEXISO_FOREGROUND=1` | don't detach from the terminal when opening the window (for logs) |

## 5. Manual checklist for risky changes

* Open a Windows install ISO (UDF + ISO 9660) and a Linux ISO (Rock Ridge, El Torito) → names, sizes,
  boot entries, "names from" switching.
* Edit + save over the original, re-open, `test` → no errors; mount it and browse in Explorer.
* Start from Explorer (double-click) and from a terminal (terminal is not blocked).

## Extending

* Engine features → add a `check(...)` block to `selftest.rs` (deterministic data, byte comparison).
* CLI / OS behaviour → add a `Check "name" { ... }` block to `smoke-test.ps1`; return a short
  description, or a string starting with `FAIL`.
* Keep tests independent of the machine: no fixed drive letters, temp folders only.
