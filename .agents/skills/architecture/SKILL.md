---
name: hexiso-architecture
description: Core architecture and conventions of HEXISO (sector sources, file-system readers, the editable Document tree, the ISO writer, background jobs, caching, the two renderers). Use before changing data flow or adding a subsystem.
---

# HEXISO architecture

```text
 window (app.rs, dialogs.rs) ─┐                      ┌─ cli.rs
                              ├──── ops.rs ──────────┤
 renderers: eframe/wgpu | soft.rs (CPU)              │
                                                     ▼
 source.rs / hxz.rs  ──►  fs_iso.rs / fs_udf.rs / fs_fat.rs  ──►  model.rs (Document)  ──►  writer.rs / hxz.rs / ops::save_zip / fs_fat::make_floppy
 (bytes → sectors)        (sectors → Entry tree)                  (edit in memory)          (tree → new image)
                                                     drives.rs (OS integration, Windows-only parts)
```

## 1. Sector sources (`source.rs`)

```rust
pub trait SectorSource: Send + Sync {
    fn sector_count(&self) -> u64;                                   // 2048-byte sectors
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> io::Result<()>; // buf.len() % 2048 == 0
    fn format_name(&self) -> String;
    fn details(&self) -> Vec<(String, String)> { .. }                // extra rows for the UI
}
pub type Src = Arc<dyn SectorSource>;
```

* Every container (ISO, raw BIN 2352/2336/2448, NRG, MDF, VHD fixed, CSO, HXZ, split `.001`, physical
  drives) becomes "a run of 2048-byte sectors". File systems never know the container.
* Reads are **positional** (`read_at` = `seek_read` / `FileExt::read_at`), never `seek` + `read`, so a
  source is shared across threads without locks. Compressed sources keep a one-block cache behind a
  `Mutex`.
* `read_range(src, byte_offset, buf)` handles unaligned reads.
* Detection lives in `open_image`: by magic (`CISO`, `HXZ1`, `PK`), then by probing the volume
  descriptor / UDF anchor at candidate (header, sector size, data offset) layouts.

## 2. File systems → tree (`fs_*.rs`)

Readers parse bytes straight into `model::Entry` trees. **Nothing is extracted on open**: files point
at extents in the image:

```rust
pub enum FileSrc { Image(Vec<(u64 /*byte offset*/, u64 /*len*/)>), Disk(PathBuf), Mem(Arc<Vec<u8>>), Zip(usize) }
pub const SPARSE: u64 = u64::MAX;   // extent offset meaning "zeros"
```

`ops::doc_from_source(src, view)` tries the available views (UDF, Rock Ridge, Joliet, ISO 9660, FAT,
Partitions) and keeps the richest; `view` forces one ("names from" in the UI, `--fs` on the CLI).
Guard every parser against hostile input: depth limits, visited-set for loops, size caps, bounds
checks (`get`, `min`) instead of panics.

## 3. Documents (`model.rs`)

`Document` = path + format + current view + `src` + `root: Entry` + label + boot specs + `modified` +
media. Editing (add / delete / rename / new folder / boot) only changes the tree. `Document::reader`
returns a streaming `Read` for any `FileSrc`.

In the app, call **`HexApp::touch()`** after every edit (bumps `version`, sets `modified`); caches
(`rows()`, `stats()`) are keyed on `version`. `set_doc` replaces the whole document.

## 4. Writing (`writer.rs`, `hxz.rs`, `ops.rs`)

`write_iso(doc, opts, impl Write, &Progress)` lays out volume descriptors, path tables, ISO + Joliet
directories, boot catalog, then streams file data in one pass. Output wrappers implement `Write`
(`CsoWriter`, `HxzWriter`) so any writer can target any container. `ops::save_doc` always writes a
temp file beside the target and renames it at the end, so the source image can be the target; after
saving the app **re-opens** the result (the old extents are no longer valid).

## 5. Background jobs (`jobs.rs`) — the only way to do slow work

```rust
self.run("Doing thing", move |prog: &Progress| {
    prog.set_total(n); prog.add(k); prog.say("current file"); if prog.cancelled() { .. }
    let result = /* slow work on a worker thread */;
    Ok(Box::new(move |app: &mut HexApp| { /* apply on the UI thread */ }) as Apply)
});
```

* Capture clones (`self.doc.clone()` is cheap: sources are `Arc`s), never `&mut self`.
* Errors are `String`s; `"cancelled"` is treated as a cancel, everything else opens an error dialog.
* One job at a time (`self.job`); the progress modal blocks other input meanwhile.
* Fire-and-forget background refreshes (drive list) use their own thread + `Arc<Mutex<Option<T>>>`
  slot polled each frame (`refresh_drives` / `poll_drives`) and `ctx.request_repaint()` when done.

## 6. UI frame (`HexApp::draw`)

`draw(ui, cpu_seconds)` is the single entry point for one frame and is called by both renderers:
`eframe::App::ui` (GPU) and `soft.rs` (CPU). Keep it free of eframe-specific APIs. Order: diagnostics
→ poll job → poll drives → drops → shortcuts → panels → dialogs → job modal → screenshot.

## 7. Renderers (`main.rs`, `soft.rs`)

* `soft::has_hardware_gpu()` enumerates wgpu adapters; CPU / "Basic Render" / llvmpipe / WARP count
  as no GPU. `HEXISO_RENDERER=cpu|gpu` overrides.
* GPU: eframe + wgpu, `PresentMode::AutoNoVsync`, frame latency 1 (smooth window drags).
* CPU: winit event loop, `egui_winit::State`, `hdrshot-egui-backend` tile-cached rasterizer into a
  `softbuffer` surface (BGRA). Repaints are driven by the egui repaint callback through an
  `EventLoopProxy`. **Window moves never repaint.** `TexturesDelta` must be `clear()`-ed after use.

## 8. Command line (`cli.rs`) and start-up (`main.rs`)

The exe is a console program. `main` routes: no args / an existing file / `--window-options` → window;
anything else → `cli::run` (exit codes 0/1/2). For the window, `detach_console` frees the console (and
relaunches detached when started from a terminal, so the terminal isn't blocked).

## Conventions

* Errors: `io::Result` in the engine; `String` at job/CLI boundaries (`es()` converts).
* No panics on user data. `unwrap()` only on invariants you just established.
* Sizes in bytes as `u64`; sectors as `u64` LBAs; times as unix seconds (`i64`).
* Keep modules focused; put shared helpers where both users can reach them (e.g. `model::human`).
