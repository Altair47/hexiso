---
name: hexiso-cli
description: Conventions for HEXISO's command line (cli.rs): adding commands, argument parsing, exit codes, --json output, progress on stderr, editing images in place. Use when adding or changing a hexiso <command>.
---

# Command-line conventions

## Routing

`main.rs` sends everything that isn't "no arguments", "an existing file" or a `--window-option` to
`cli::run(args) -> exit code`. Unknown words are treated as commands, so typos fail with exit code 2
instead of opening a window. New commands must be added to `cli::COMMANDS`.

## Contract (scripts depend on it; don't break it)

| | |
|---|---|
| Exit codes | `0` success · `1` the operation failed · `2` wrong usage (return an error string starting with `usage:`) |
| stdout | results only: plain text, or **one** JSON document with `--json` |
| stderr | progress (only when stderr is a terminal, see `progress()`), warnings, errors as `hexiso <cmd>: message` |
| Paths inside images | `/`- or `\`-separated from the top folder (`split_path`) |
| Edits | saved over the original when it is `.iso/.cso/.hxz/.zip`, otherwise require `--out` (`save_edit`) |
| Destructive drive actions | require `--yes`; non-removable targets additionally need `--force` |

## Adding a command

1. Add the name to `COMMANDS` and a line to the `HELP` text (keep the sections).
2. Add a `match` arm in `command()`. Use `a.need(i, "usage: ...")` for positionals, `a.opt("name")`
   for valued options (add the name to `VALUE_OPTS`), `a.flag("name")` for switches.
3. Reuse `open_doc` (handles `--password` and `--fs`), `find` for inner paths, `save_edit` for writes,
   `progress(quiet, "label", |p| ...)` around anything slow, and `ops::*` for the real work. Never
   duplicate engine logic in the CLI.
4. For `--json`, build output with `js()` (escapes strings); keep field names `snake_case` and stable.
5. Document it in `README.md` (command table) and test it in `scripts/smoke-test.ps1`.

## Examples to copy

* Read-only + JSON: `info`, `ls`.
* Edit + save: `rename`, `mkdir`, `label`.
* Long job with progress: `convert`, `extract`.
* OS integration: `mount` (checks for an existing mount first), `usb` (confirmation flags).
