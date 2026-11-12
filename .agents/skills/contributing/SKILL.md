---
name: hexiso-contributing
description: Code style, workflow, commit and pull-request conventions for HEXISO. Use when preparing changes for review.
---

# Contributing conventions

## Workflow

1. Read `AGENTS.md`, then the guide for your area.
2. Make the smallest change that solves the problem; keep the engine, UI and CLI layering (UI and CLI
   call `ops.rs` / engine modules, never the other way round).
3. Verify: `cargo clippy` (no warnings), `cargo run --release -- selftest`, the smoke test, and a
   `--screenshot` for UI changes.
4. Update docs in the same change: README (features / formats / commands), `--help` text, tooltips,
   and the relevant `.agents/skills/*` guide if a convention changed.

## Code style

* Rust 2024 edition, `rustfmt` defaults with long lines tolerated (the codebase uses up to ~160
  columns for UI builder chains). Run `cargo fmt` only on files you touch.
* Every source file starts with the licence header (see `AGENTS.md` rule 9).
* Module-level `//!` doc comment explaining the file's job; `///` on public items whose purpose isn't
  obvious. Comments explain *why*, not *what*.
* Names: `snake_case` functions, `CamelCase` types, constants in `SCREAMING_CASE`; UI helper functions
  are short verbs/nouns (`section`, `kvt`, `tool_btn`).
* No `unsafe` except FFI and the one documented pixel-buffer cast in `soft.rs`; wrap each `unsafe` block
  minimally and say why it's sound.
* No panics on user input (malformed images must produce errors or be skipped).
* Dependencies: pure Rust, widely used, permissive licence (compatible with AGPL-3.0), justified.

## Commits & pull requests

* Commit messages: imperative summary ≤ 72 chars (`Add NRG footer parsing`), blank line, the why.
* One topic per pull request. Describe what changed for the person using the app, how you tested
  (paste the self-test / smoke-test summary), and include a screenshot for visual changes.
* By contributing you agree your contribution is licensed under AGPL-3.0-only.
