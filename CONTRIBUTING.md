# Contributing to HEXISO

Thanks for helping! HEXISO is created and maintained by **Nick Petropoulos**
([github.com/Altair47](https://github.com/Altair47)) and is licensed under the
**GNU Affero General Public License v3.0** (AGPL-3.0-only). By submitting a contribution you agree that
it is licensed under the same terms.

## Getting started

1. Install [Rust](https://rustup.rs) (stable, MSVC toolchain) and the Visual Studio Build Tools
   ("Desktop development with C++").
2. Fork [github.com/Altair47/hexiso](https://github.com/Altair47/hexiso), clone your fork, then
   `cargo build --release` and run `target\release\hexiso.exe`.
3. Read [AGENTS.md](AGENTS.md): the project map, the commands and the rules that keep HEXISO light.
   The detailed guides live in [`.agents/skills/`](.agents/skills):
   [architecture](.agents/skills/architecture/SKILL.md) ·
   [UI design](.agents/skills/ui-design/SKILL.md) ·
   [formats](.agents/skills/formats/SKILL.md) ·
   [command line](.agents/skills/cli/SKILL.md) ·
   [testing](.agents/skills/testing/SKILL.md) ·
   [performance](.agents/skills/performance/SKILL.md) ·
   [platform](.agents/skills/platform/SKILL.md) ·
   [release](.agents/skills/release/SKILL.md) ·
   [conventions](.agents/skills/contributing/SKILL.md).

## Before opening a pull request

```powershell
cargo clippy                                                     # no warnings
cargo run --release -- selftest                                  # ALL PASSED
powershell -ExecutionPolicy Bypass -File scripts\smoke-test.ps1  # SMOKE TEST PASSED
hexiso --screenshot shot.png some.iso                            # for UI changes: look at it
```

* Keep the UI thread free of slow work, keep idle CPU at zero, and keep per-frame work cached.
* New features should be reachable from both the window and the command line.
* Put a licence header at the top of new source files.
* Update the README, `hexiso help` text and tooltips with the change.
* Describe in the pull request what changed for users, how you tested it, and add a screenshot for
  visual changes.

## Reporting bugs

Open an issue at [github.com/Altair47/hexiso/issues](https://github.com/Altair47/hexiso/issues). Please include the HEXISO version (`hexiso version`), Windows version, what you did, what happened,
and, if an image fails to open, the output of `hexiso info <image>` and `hexiso selftest <dir> <image>`.
Don't attach copyrighted disc images.

## Good first contributions

UDF writing, Rock Ridge writing, multi-track BIN/CUE, NRG footer parsing, MP3/FLAC ripping,
7z/RAR reading, NTFS/exFAT readers, and a Linux port of `src/drives.rs`
(see the [platform guide](.agents/skills/platform/SKILL.md)).
