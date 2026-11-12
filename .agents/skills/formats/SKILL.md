---
name: hexiso-formats
description: How to add or fix image containers, file-system readers and writers in HEXISO (SectorSource implementations, detection, tree building, writer layout, HXZ/CSO), with the safety and test requirements. Use when touching source.rs, fs_*.rs, writer.rs or hxz.rs.
---

# Adding and fixing formats

## What exists

| Layer | Supported | File |
|---|---|---|
| Containers (read) | ISO 2048, BIN 2352 Mode 1/2, 2336, 2448 (subchannel), NRG (fixed header), MDF, 32 KB header, CUE (first FILE / data track), VHD fixed, CSO v1, HXZ, split `.001`, ZIP (as archive), physical drives | `source.rs`, `hxz.rs`, `ops::open_zip` |
| File systems (read) | ISO 9660, Joliet, Rock Ridge (NM + CE), UDF 1.02–2.60 (physical + metadata partitions), El Torito, FAT12/16/32 + LFN, MBR, GPT | `fs_iso.rs`, `fs_udf.rs`, `fs_fat.rs` |
| Writers | ISO 9660 + Joliet (+El Torito BIOS/UEFI, multi-extent > 4 GB, boot-info table), CSO, HXZ, ZIP, FAT12 floppy, split | `writer.rs`, `hxz.rs`, `ops.rs`, `fs_fat.rs` |

Known gaps (good first projects): UDF writing, Rock Ridge writing, multi-track BIN/CUE, NRG footer
parsing, dynamic VHD/VHDX, VMDK, VDI, DMG, 7z/RAR, NTFS/exFAT/ext/HFS+ readers, MP3/FLAC ripping.

## Add a container

1. Implement `SectorSource` in `source.rs` (or its own module): positional reads only, `Send + Sync`,
   zero-fill reads past the end instead of erroring on truncated images.
2. Add detection to `open_image`: a magic check before the layout probe, or a new
   `(header, sector_size, data_offset, name)` candidate if it is a raw layout.
3. Put useful facts in `details()`; they appear in the properties panel and `info --json`.
4. Add the extension to the open-file filter in `app.rs` (`open_dialog`) and the convert dialog.
5. Extend `selftest.rs` with a round trip (write or synthesise a sample, read it back, compare bytes).

## Add a file-system reader

1. Parse into `Entry` trees with `FileSrc::Image` extents (byte offsets in sector space); never read
   whole files on open.
2. Treat input as hostile: cap recursion depth (64), keep a visited set of directory locations, cap
   allocation sizes, use checked indexing, and skip broken entries rather than failing the image.
3. De-duplicate sibling names (the tree is addressed by name).
4. Register it in `ops::fs_views` (detection) and `ops::doc_from_source` (the `match w.as_str()`
   list), with a plain-language explanation in `app::fs_help`.

## Change the ISO writer

* Layout is computed first (sizes don't depend on LBAs), then LBAs are assigned in this order:
  VDs (16..), path tables (L/M for ISO and Joliet), ISO dirs (BFS), Joliet dirs (BFS), boot catalog,
  hidden boot images, file data (boot files first). Writing is a single forward pass (`Pos::seek_to`
  refuses to go backwards, which catches layout bugs).
* Name rules: ISO 9660 `[A-Z0-9_]`, ≤ 30 chars + `;1`, `~n` uniquifier; Joliet UCS-2BE ≤ 103 chars.
* After any change: run `selftest` (it writes, re-reads, edits in place, converts and byte-compares)
  and, on Windows, mount the result (`smoke-test.ps1 -Mount`): Windows' own ISO driver is the
  independent check.

## HXZ format (stable, keep backward compatible)

96-byte header: `HXZ1`, version u8 = 1, flags u8 (bit0 = encrypted), block size u32 (65536), total
bytes u64, block count u32, index offset u64, salt[16], password check[32]; then blocks; then index
entries `(offset u64, stored_len u32, flags u32 bit0 = deflated)`. Encryption: PBKDF2-HMAC-SHA256
(200,000 iterations) → ChaCha20-Poly1305, nonce = block index (LE, 12 bytes), check = encryption of 16
zero bytes with nonce `FF..FF`. Bump the version byte for any incompatible change and keep reading v1.
