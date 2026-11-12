# AGENTS.md

Guidance for AI coding agents (and humans) working on **HEXISO**, a fast, portable disc image
toolkit for Windows written in Rust. Created by **Nick Petropoulos** ([github.com/Altair47](https://github.com/Altair47)).
Repository: [github.com/Altair47/hexiso](https://github.com/Altair47/hexiso). Licensed **AGPL-3.0-only**.

Read this file first, then the guide in [`.agents/skills/`](.agents/skills) that matches your task.

## What the project is

One crate, one binary (`hexiso.exe`) with two front ends over one engine:

* **Window** (egui): dark terminal-style UI. Two renderers: GPU (`eframe` + `wgpu`) and a built-in CPU
  renderer (`soft.rs`) chosen automatically when there is no real GPU.
* **Command line** (`cli.rs`): every main feature as `hexiso <command>`, `--json` output, exit codes.
* **Engine** (pure Rust, no C libraries): image containers → 2048-byte sectors → file systems →
  editable tree → ISO writer / converters. Windows-specific code lives **only** in `drives.rs`.

## Commands

All commands run from the crate root (the folder holding this file).

| Task | Command |
|---|---|
| Build (debug) | `cargo build` |
| Build (release, what users get) | `cargo build --release` |
| Lint (must be clean) | `cargo clippy` |
| Engine self-test (fast, no GUI) | `cargo run --release -- selftest` |
| Full smoke test (CLI, both renderers, performance) | `powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1` (add `-Mount` to test virtual drives) |
| Run the window | `cargo run --release` |
| Picture of the window | `hexiso --screenshot shot.png [image.iso] [--view sectors --lba 16]` |
| Redraw benchmark | `$env:HEXISO_BENCH=1; hexiso` (prints frames per second, then exits) |
| Release package + Scoop manifest | `powershell -ExecutionPolicy Bypass -File package.ps1 -Url <zip URL>` |

Before you finish any change: `cargo clippy` is clean, `selftest` passes, and for UI changes you have
looked at a `--screenshot` of the result.

## Map

```text
src/main.rs      start-up: CLI vs window, console handling, renderer choice, viewport setup
src/app.rs       HexApp: state, actions, toolbar, panels, file table, sector view, log, status bar
src/dialogs.rs   Dialog enum + every modal dialog
src/theme.rs     colours, fonts, widget visuals (the only place colours are defined)
src/soft.rs      CPU renderer window loop (winit + egui-winit + softbuffer + rasterizer)
src/jobs.rs      background jobs: Progress (atomics) + Apply closures run on the UI thread
src/cli.rs       command line
src/ops.rs       open / save / convert / extract / test / checksum / split / search
src/model.rs     Document, Entry tree, FileSrc, readers, Media, time helpers
src/source.rs    SectorSource trait + containers (ISO/BIN/NRG/MDF/VHD/CSO/split/drives) + detection
src/fs_iso.rs    ISO 9660 / Joliet / Rock Ridge / El Torito reader
src/fs_udf.rs    UDF reader
src/fs_fat.rs    FAT reader, MBR/GPT, FAT12 floppy writer
src/writer.rs    ISO 9660 + Joliet writer (El Torito BIOS/UEFI, multi-extent)
src/hxz.rs       HXZ container (reader + writer) and CSO writer, multi-core
src/drives.rs    OS layer: drive list, raw drive reads, mount, burn, format, eject, audio ripping
src/selftest.rs  end-to-end engine self-test
src/icon_art.rs  pixel-art icon (also include!()-ed by build.rs)
build.rs         icon + Windows version resource
scripts/         smoke-test.ps1
.github/         CI workflow (clippy, build, self-test; draft release on v* tags)
docs/screenshots README pictures
```

## Rules that keep HEXISO fast and correct

1. **Never block the UI thread.** Anything that touches disks, drives, PowerShell or more than a few
   KB of data goes through `HexApp::run(title, |prog| ... Ok(Box::new(|app| ...)))` (see
   `.agents/skills/architecture`). Drive scans already run on a background thread.
2. **Idle means 0% CPU.** Don't call `ctx.request_repaint()` every frame except in documented
   diagnostic modes. Use `request_repaint_after` for timed updates. Verify with the smoke test's
   "idle CPU" check.
3. **Per-frame work must be cheap.** Cache anything that walks the tree (`rows()`, `stats()` are keyed
   on `self.version`; call `touch()` after every edit). Use `ScrollArea::show_rows` for long lists.
4. **Both renderers must work.** Don't use eframe-only APIs inside `HexApp::draw`; it is shared by
   `eframe::App::ui` and `soft.rs`. Test with `HEXISO_RENDERER=cpu` and `=gpu`.
5. **Windows-only code stays in `drives.rs`** behind `#[cfg(windows)]` with a non-Windows fallback,
   so a Linux port only has to rewrite that file.
6. **Every GUI feature should also exist on the command line**, and vice versa where it makes sense.
7. **Plain language for people.** Labels, tooltips and log lines are written for non-experts; say what
   happened and what to do next. Every toolbar button and property has a tooltip.
8. **Stay lightweight.** No new heavy dependencies without a measured reason; pure-Rust crates only;
   no network access; no settings files or registry writes (portable app).
9. **Licence header** at the top of every source file:
   `// HEXISO - Copyright (C) Nick Petropoulos (https://github.com/Altair47)` /
   `// SPDX-License-Identifier: AGPL-3.0-only`.

## Guides (`.agents/skills/`)

| Guide | Use it when |
|---|---|
| [`architecture`](.agents/skills/architecture/SKILL.md) | changing how data flows: sources, file systems, documents, jobs, renderers |
| [`ui-design`](.agents/skills/ui-design/SKILL.md) | touching anything visible: colours, widgets, dialogs, tooltips, wording |
| [`formats`](.agents/skills/formats/SKILL.md) | adding or fixing an image container, file system or writer |
| [`cli`](.agents/skills/cli/SKILL.md) | adding or changing a command-line command |
| [`testing`](.agents/skills/testing/SKILL.md) | verifying a change; extending the self-test or smoke test |
| [`performance`](.agents/skills/performance/SKILL.md) | anything that could cost CPU, memory or frame time |
| [`platform`](.agents/skills/platform/SKILL.md) | drives, mounting, burning, PowerShell calls, a Linux port |
| [`release`](.agents/skills/release/SKILL.md) | versioning, packaging, Scoop, GitHub releases |
| [`contributing`](.agents/skills/contributing/SKILL.md) | code style, commits, pull requests |

Human contributors: see [CONTRIBUTING.md](CONTRIBUTING.md).
