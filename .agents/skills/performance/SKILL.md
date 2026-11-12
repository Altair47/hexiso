---
name: hexiso-performance
description: Performance rules and measured budgets for HEXISO (idle CPU, frame time, both renderers, background work, caching, streaming I/O, multi-core compression). Use for any change that could add CPU, memory, frame time or startup cost.
---

# Performance

HEXISO's promise is **superlight and smooth**: idle at 0% CPU, instant UI, fast I/O, works well
on machines without a GPU.

## Budgets (measured on a 6-core VM without a GPU)

| Metric | Budget | Measured |
|---|---|---|
| Idle CPU | < 0.2 CPU-s per 4 s | 0.000 |
| egui frame build (`frame_ms`, bottom-right of status bar) | < 4 ms | ~1-2 ms |
| CPU renderer full redraw | ≥ 30 fps (smoke test) | ~500-630 fps |
| Window move (CPU renderer) | no redraw | 0.016 CPU-s for 3 s of dragging |
| 20,000-file ISO (201 folders) | open < 1 s, window ≥ 100 fps | create 2.0 s · open+info 0.10 s · `ls -r` 0.25 s · read all 0.32 s · window 430 fps |

Windows' software Direct3D driver ("Microsoft Basic Render Driver") manages only ~5 fps for this UI,
which is why `soft.rs` exists. Never route no-GPU machines back through wgpu.

## Rules

1. **UI thread does UI only.** Disk, drives, PowerShell, hashing, compression: always a job
   (`HexApp::run`) or a background thread with a polled slot. One `GetVolumeInformationW` on an empty
   optical drive can take 20 ms to seconds.
2. **Repaint discipline.** egui repaints on input; your code should only call
   `request_repaint_after(..)` for timed refresh (the job modal uses 100 ms). Continuous
   `request_repaint()` is allowed only in `--screenshot` and `HEXISO_BENCH` modes. Use
   `HEXISO_DEBUG_REPAINT=1` to find stray repaints.
3. **No per-frame tree walks.** `Entry::count()/size()` and `writer::estimate()` are O(n): call them
   through cached accessors (`stats()`, `rows()`), invalidated by `touch()` / `version`.
4. **Virtualise lists** with `ScrollArea::show_rows` and fixed row heights (file table 21 px, log
   15 px, hex view 17 px). Don't create widgets for off-screen rows.
5. **Don't send viewport commands every frame** (e.g. the title is only sent when it changes).
6. **Stream I/O.** Use `Document::reader` / `ExtentReader` with ~1 MiB buffers; `BufWriter` with
   4 MiB for image output; never load whole files except boot images and floppy images.
7. **Parallel where it pays.** CSO/HXZ compress batches of blocks on all cores
   (`std::thread::scope`); checksums hash MD5/SHA-1/SHA-256 in parallel per chunk.
8. **Startup.** GPU detection enumerates adapters once (~tens of ms). Don't add network, registry or
   large file reads at startup.
9. **Binary size / deps.** Release uses thin LTO, 1 codegen unit, stripped; static CRT. Justify any
   new dependency with a measurement.

## How to measure

```powershell
$env:HEXISO_BENCH=1; $env:HEXISO_RENDERER="cpu"; .\target\release\hexiso.exe   # fps
powershell -File scripts\smoke-test.ps1 -SkipBuild                              # idle CPU + fps checks
```

Hover the frame-time readout (status bar, bottom right) to see the renderer in use.
