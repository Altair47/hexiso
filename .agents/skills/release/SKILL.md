---
name: hexiso-release
description: How to version, package and publish HEXISO (package.ps1, portable zip, Scoop manifest, GitHub release checklist, licence requirements). Use when preparing a release.
---

# Releasing HEXISO

## Checklist

1. Bump `version` in `Cargo.toml` (SemVer). The About box, `hexiso version` and the exe's version
   resource read it automatically.
2. `cargo clippy` clean; `cargo run --release -- selftest` passes;
   `powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1 -Mount` passes.
3. If the UI changed, refresh `docs/screenshots/` (`hexiso --screenshot docs\screenshots\files.png
   some.iso`, `welcome.png` with no image, `sectors.png` with `--view sectors --lba 16`). Use a demo
   image without personal paths.
4. Package:
   ```powershell
   powershell -ExecutionPolicy Bypass -File package.ps1
   # defaults: -Url https://github.com/Altair47/hexiso/releases/download/v<version>/hexiso-<version>-win64.zip
   #           -Homepage https://github.com/Altair47/hexiso
   ```
   Produces `dist\hexiso\` (exe + README.txt + LICENSE), `dist\hexiso-<version>-win64.zip` and
   `dist\hexiso.json` (Scoop manifest with the zip's SHA-256, license `AGPL-3.0-only`).
5. Tag `v<version>` and push the tag: `.github/workflows/ci.yml` builds, tests, packages and creates a
   **draft** release with the zip and `hexiso.json` attached. Review the draft, paste the SHA-256 in
   the notes, publish. (Manual alternative: run step 4 locally and upload the zip yourself.)
6. Scoop: the manifest has `checkver: github` + `autoupdate` (URL pattern + `$url.sha256` hash), so a
   bucket only needs it **once**: `bucket/hexiso.json` in github.com/Altair47/scoop (Excavator action
   keeps it updated) and/or a pull request to ScoopInstaller/Extras. Every release must keep the asset
   names `hexiso-<version>-win64.zip` and `hexiso-<version>-win64.zip.sha256` and the tag `v<version>`,
   or autoupdate breaks. Users can also install from
   `https://github.com/Altair47/hexiso/releases/latest/download/hexiso.json`.

Repository: https://github.com/Altair47/hexiso

## Build properties to keep

* Static CRT (`.cargo/config.toml`): no Visual C++ redistributable needed. Check with any import
  viewer that the exe only imports Windows system DLLs.
* `build.rs` embeds the icon and version info; needs the Windows SDK's `rc.exe` (a missing `rc.exe`
  only drops the icon, with a warning).
* Release profile: thin LTO, `codegen-units = 1`, `strip = true`.

## Licence (AGPL-3.0-only)

* `LICENSE` must ship in the repo and in the zip (package.ps1 copies it).
* Every source file starts with the two-line copyright/SPDX header.
* The About box and `hexiso help` credit Nick Petropoulos (https://github.com/Altair47) and name the licence.
* Third-party crates keep their own licences (MIT / Apache-2.0 / BSD / Zlib families), all
  compatible with AGPL-3.0.
