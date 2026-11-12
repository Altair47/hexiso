---
name: hexiso-ui-design
description: HEXISO's visual language and UI patterns (dark terminal theme, colours, monospace type, toolbar buttons, panels, dialogs, tooltips, log lines, wording). Use when adding or changing anything visible in the window.
---

# HEXISO UI design & patterns

The look is a **dark "hacker terminal"**: near-black panels, phosphor-green text and borders, cyan
for interactive / folder items, amber for warnings, red for failures, **monospace everywhere**,
square-ish corners (radius 1-2), thin 1 px strokes. It must stay simple: a person who has never used
a disc tool should understand every screen from its labels and tooltips.

## Colours: `src/theme.rs` only

Never write `Color32::from_rgb` for a semantic colour in UI code; use the constants:

| Constant | Use |
|---|---|
| `BG` | window background, central panel, status bar, log |
| `PANEL`, `PANEL2` | side panels; raised surfaces (toolbar buttons, header box, table header) |
| `ROW_ALT`, `SEL` | zebra rows; selected row background |
| `LINE` | thin separators and idle borders |
| `GREEN` | primary text accent, headings, active borders, OK |
| `GREEN_DIM` | secondary accent (timestamps, LBA column, tick marks) |
| `CYAN` | folders, links, values you can click, hover highlights |
| `AMBER` | warnings, "UNSAVED", items not yet written ("new") |
| `RED` | errors, destructive confirmations, "won't fit" |
| `TEXT`, `DIM` | normal text; labels / hints / disabled |

Fonts: `theme::apply` maps every text style to monospace (Body/Button 13 px, Small 10.5 px,
Heading 17 px). Use `RichText::small()` for secondary info rather than custom sizes.

## Layout (top to bottom)

menu bar (`menu_bar`) → toolbar (`toolbar`) → [left: folder tree | centre: header + tabs + file
list or sector view | right: properties] → activity log (`log_panel`) → status bar (`status_bar`).
Panels are `egui::Panel::{top,bottom,left,right}` with explicit `egui::Frame` fills. The centre
always starts with `doc_header` ("OPEN IMAGE / NEW IMAGE / READING DRIVE", the name and location,
unsaved badge) so people know what they are working on.

## Responsive layout (window can shrink to 480 x 340)

`draw` stores the window size in `self.win_w` / `self.win_h` every frame; layout decisions read
those (or `ui.available_width()` for the area at hand). Current breakpoints:

| What | Rule |
|---|---|
| Toolbar (`toolbar_mode`) | full tiles ≥ 1280 · compact tiles ≥ 1110 · icons only below (name stays in the tooltip); wraps to a 2nd row only below ~600 |
| Folder tree (`tree_visible`) | auto-shown ≥ 820 wide |
| Info panel (`info_visible`) | auto-shown ≥ 1150 wide |
| Activity log (`log_visible`) | hidden when the window is < 460 tall |
| Header (`doc_header`) | one line (badge, `show`, name; location in tooltip) when < 520 tall |
| File table columns | LBA ≥ 840 · MODIFIED ≥ 720 · TYPE ≥ 600 (list width); name + size always |
| Status bar | "disc fill" label ≥ 1000 · meter ≥ 600 · otherwise just "N% DVD 4.7GB"; document name truncates |
| Start screen | tiles 3 / 2 / 1 columns (> 900 / > 470 / below), logo scales, shorter tiles on short windows |
| Dialogs / progress | widths clamp to the window (`win_w - 60`) |

The `[=]` / `[i]` toggles above the file list (and View menu) override the automatic panel choice
(`tree_pref` / `info_pref`); **View > Automatic layout** resets them. Rules for new UI: never assume a
fixed width; use `horizontal_wrapped` for rows of controls, `Label::truncate()` (with the full text as
tooltip) for long names, and clip custom painting (`painter().with_clip_rect`). Check at
`HEXISO_SIZE=480x340`, `640x460`, `900x560` and full size.

## Building blocks (in `app.rs` unless noted)

* **`tool_btn(ui, glyph, label, tip, enabled)`**: 66x48 toolbar tile, ASCII glyph (`</`, `(*)`,
  `<=>` ...) + uppercase label, corner ticks. `tip` is `"Title|What it does|Why it is disabled"`
  (split by `split_tip`); the third part is shown in amber only when disabled.
* **`section(ui, "TITLE")`**: `// TITLE ───` heading for panel sections.
* **`kvt(ui, key, value, tooltip)`** inside an `egui::Grid` with 2 columns: dim key, normal
  value, explanation on hover. Use `kv` only when the key needs no explanation.
* **`meter(ui, fraction, height)`**: thin fill bar (drive usage); the status bar draws the
  disc-fill meter itself with 10% ticks.
* **File table** (`file_table`): custom-painted rows inside `ScrollArea::show_rows` (21 px). Folders
  are cyan with a trailing `/`, hidden items dim, `-`/`d` type marker, LBA shown as `new` (amber)
  for items not yet in the image. Column headers sort and carry tooltips.
* **Tree** (`tree_node`): `+`/`-` toggles, `/ name` root, current folder highlighted.
* **Log** (`log`, `log_panel`, `log_row`): `Lvl::{Info, Ok, Warn, Err}` → tags `..`, `OK`,
  `WARN`, `FAIL`; single-line rows (15 px) with the full text in a tooltip; a cyan live line shows the
  running job.
* **Progress** (`draw_job`): modal with an ASCII bar `[#####.....] 42.0%`, bytes, speed, ETA, status
  and `[ cancel ]`.
* **Empty states** are never blank: start screen tiles (`welcome`), the dashed "THIS NEW IMAGE IS
  EMPTY" drop zone (`new_image_hint`), `-- empty folder --` messages.

## Dialogs (`src/dialogs.rs`)

* Add a variant to `enum Dialog` holding only the dialog's own state; open it with
  `self.dialog = Some(Dialog::X { .. })`.
* Render it in `dialog_body` and return `true` to keep it open, `false` to close. Start with
  `title_bar(ui, "lowercase title")` (shown as `>> UPPERCASE`), buttons via `btn(ui, "label")`
  (rendered `[ label ]`). Put long-running work in `self.run(...)` and close the dialog.
* Destructive actions need an explicit confirmation (typed `ERASE` for formatting, `Discard`
  dialog for unsaved changes). One dialog at a time; dialogs are `egui::Modal`s.

## Wording rules

* Say what happened and what happens next, in plain words: "started a new, empty image in memory
  (nothing saved yet)" plus "next: add files ..., then press SAVE".
* Name things by what people see: "disc name" (not "volume identifier"), "names from" (not "file
  system view"), "inside image: / (top)".
* Every toolbar button, panel key, column header and non-obvious control gets a tooltip; disabled
  buttons explain why.
* Log lines are lowercase sentences; paths in full; sizes via `model::human`.

## Checklist for a UI change

1. Colours from `theme.rs`, text monospace, sizes via `small()` / defaults.
2. Tooltips on new controls; disabled state explained.
3. No per-frame tree walks or allocations proportional to image size (see the `performance` guide).
4. Works in both renderers: `HEXISO_RENDERER=cpu` and `=gpu`.
5. Look at it: `hexiso --screenshot shot.png some.iso` and open the PNG. Check that nothing overflows
   at 1360x820 and down to the minimum size 480x340 (`HEXISO_SIZE=480x340`).
