# Implementation plan: entries on lines

Implements [lines-layout-design.md](lines-layout-design.md). The design is the
specification; this plan fixes the interfaces, the split between subagents and
the checks. Where this plan is more precise than the design, see "Decisions".

Running this plan creates a branch `LineLayout` and commits on it. Nothing is
pushed, installed or sent to the device without a separate request.

## Baseline (2026-11-25, `ab0bdd3`)

`cargo test`: 274 pass, 0 fail. `cargo clippy --all-targets` and
`cargo fmt --check`: clean.

## Decisions added by this plan

| # | Decision |
|---|---|
| D1 | When columns are dropped, each row that lost non-empty content gets `…` appended to its cell in the last kept column (even an empty cell), then narrowed to fit. |
| D2 | In compact mode with `lines`, only the "layout ignored" warning is logged; the other `lines` warnings are skipped. |
| D3 | A `lines` row is `[gutter cell] + entry cells`. With a gutter, a row without an entry is `gutter\t`. Without a gutter there is no leading cell. |
| D4 | `SlotSpec` records whether the value set `align`; a grid uses it, else `Left`. |
| D5 | Two preview examples are added, `lines-agents` and `lines-busy` (27 → 29). |
| D6 | Row height, ascent and descent come from all runs of the row together, as for an unsplit line. Cell truncation never changes row height. |

## Orchestration

Subagents are used only to keep each context small. They run one at a time, in
this checkout, on `LineLayout`; there are no worktrees and no merges. The main
session is the orchestrator: it starts each agent, checks the result and
reports. It does not write feature code.

| Order | Task | Agent | Files owned |
|---|---|---|---|
| 0 | T0 branch and docs | orchestrator | `docs/lines-layout-*.md` |
| 1 | T1 config and template cells | `general-purpose` | `src/config.rs`, `src/config_tests.rs`, `src/zjsb/template.rs` |
| 2 | T2 grid layout | `general-purpose` | `src/render/layout.rs`, one line of `src/render/mod.rs` |
| 3 | T3 renderer wiring | `general-purpose` | `src/render/mod.rs`, `tests/render_edge.rs`, `tests/golden.rs`, `tests/golden/strip_grid.png` |
| 4 | T4 zjsb `lines` | `general-purpose` | `src/zjsb/{mod,examples,tests}.rs`, `tests/zjsb/`, `tests/zjsb_golden.rs`, `tests/cli.rs` |
| 5 | T5 docs and previews | `general-purpose` | `README.md`, `docs/status-format.md`, `docs/on-device-checklist.md`, `examples/config.toml`, `preview.sh`, `preview/` |
| 6 | T6 review | `gsd-code-reviewer` | `REVIEW.md` in the scratchpad |
| 7 | T7 fixes and final check | `general-purpose`, then orchestrator | as needed |

### Preamble for every implementation prompt

```text
Repository: /home/buz/code/n4/opendeck-strip (Rust, edition 2024). Work in this
checkout on branch LineLayout; check that first and stop if it differs. Do not
switch branches.
Read docs/lines-layout-design.md (the specification) and your task section in
docs/lines-layout-plan.md, including "Decisions". Change only the files your
task owns; if another file must change, make the smallest change and say so.
Write tests first for new behaviour. Do not change existing expected outputs or
golden images; if one fails, your change is wrong or the design is, so report.
Before finishing: cargo fmt --check, cargo clippy --all-targets -- -D warnings,
cargo test. Commit only your files (never `git add -A`;
preview/stats-block-entries-right-mono.png stays untracked), ending the message
with "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>". Do not push,
do not install, do not run anything that talks to the device.
Report: commits, files changed, test counts, deviations, open questions.
```

Each prompt is the preamble plus: `Your task is T<n>.`

### After each task (orchestrator)

1. Read the report and resolve deviations.
2. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
3. `git diff --stat main -- tests/golden tests/zjsb/expected` shows only new
   files.
4. A failure goes back to the same agent (`SendMessage`). Do not start the next
   task on a red tree.

## T0: branch and docs

`git switch -c LineLayout`, then commit the design and this plan. Leave
`preview/stats-block-entries-right-mono.png` untracked.

## T1: config and template cells

### `src/config.rs`

- Fields: `column_gap: u32` (default 2), `zjsb_layout: String` (default
  `"segments"`).
- `KEYS` rows: `column_gap` is `Kind::Num { real: false, min:
  Limit::AtLeast(0.0), max: Some(16.0) }`; `zjsb_layout` is
  `Kind::Enum("one of segments, lines", is_zjsb_layout)`.
- Cross-key warnings in `normalise`, only when `zjsb_layout == "lines"`:
  1. `zjsb_mode == "compact"`: layout ignored; stop here (D2).
  2. `word_wrap`: no columns.
  3. counts template has more than five lines: first five are shown.
  4. more and entry templates differ in line count: more row will not align.
  5. `min_font_size > 16`: five rows may not fit.
- Warnings change no value. Follow the wording style of the existing ones:
  name the key, the value, the consequence.

### `src/zjsb/template.rs`

```rust
/// Number of template lines kept by `parse`.
pub fn line_count(&self) -> usize;
/// One string per kept line. A line whose placeholders are all empty is "".
/// A tab in template text, literal or the two characters `\t`, is a space.
pub fn render_cells(&self, ctx: &Ctx) -> Vec<String>;
```

- Share the per-line rendering with `render`, which keeps its behaviour.
- Replace tabs only in `Part::Lit`, scanning escapes in pairs so `\\t` (an
  escaped backslash, then `t`) is left alone.

### Tests

- `render_cells`: empty-placeholder line with literal text gives `""` in place;
  literal-only line kept; space line kept; real and literal tab become spaces;
  `\\t` untouched; a value holding a tab or `\t` makes no cell.
- Config: both keys from file and env; `column_gap = 40` clamps to 16 with a
  warning; bad `zjsb_layout` keeps `segments`; each warning fires alone and not
  for `segments`; 16 does not warn, 17 does.
- `every_key_loads_from_the_file_and_the_environment`: key count 37 → 39, add
  the `"zjsb_layout" => "lines"` arm. That test also sets `zjsb_mode =
  "compact"`, so replace its two `warnings.is_empty()` asserts with an assert
  that the only warning is "layout ignored".

## T2: grid layout (`src/render/layout.rs`)

### Interface

```rust
#[derive(Clone, Copy, Debug)]
pub struct Grid { pub gap_spaces: u32, pub mono: bool }

// new field of DrawRequest
pub grid: Option<Grid>,

/// Whether any run of any line holds a tab.
pub fn has_tab(lines: &[Vec<Run>]) -> bool;
```

`grid: None` must be byte-for-byte today's behaviour, including `\t` drawn as
four spaces. In `src/render/mod.rs` add only `grid: None` so the crate builds.
Add it to the three `DrawRequest` literals in the layout tests too.

### Algorithm

1. Split each line's runs at `\t` into cells, keeping each run's style. A line
   without a tab is one cell; an empty line is one empty cell.
2. At a size: column width = widest cell; gap = `gap_spaces` × advance of one
   space in the regular face (mono if `grid.mono`) at the base size; grid width
   = widths + gaps; row metrics per D6.
3. Auto size: the existing bisection, with "fits" meaning grid width and total
   height fit. `SizeSpec::Px` is clamped as today.
4. At the final size: first drop bottom rows that do not fit
   (`keep_visible_lines` rule), then compute widths from the kept rows.
5. While the grid is too wide, with `last` the rightmost kept column:
   - `last == 0`: truncate its cells to the inner width; stop.
   - `avail` = inner width − widths before `last` − gaps. If `avail` holds `…`,
     truncate the overflowing cells of `last` to `avail`; stop.
   - Otherwise drop `last` and remember which rows had content there; apply D1;
     repeat.
6. Draw cells at `x_start + column offset`, where `x_start` aligns the whole
   grid by `request.align`. Backgrounds and underlines cover runs only.
7. `truncated` is set by any dropped row, narrowed cell or dropped column.

Keep grid code in its own functions (or a `render/grid.rs` submodule if
`layout.rs` would grow past about 1100 lines). Reuse `measure` and
`truncate_line`; `truncate_line` returns early when the line fits, so D1 needs a
forced-ellipsis variant of it.

### Tests (assert positions through coloured runs, not glyph shapes)

- Second column starts at the same x in rows with very different first cells.
- A line without tabs widens column 1.
- Ragged rows, leading and trailing tabs, empty lines, a style spanning a tab.
- `gap_spaces` 0, 2 and 16 move column 2 by the expected amount; mono differs.
- Left, center and right move the grid as a block.
- Auto size with 1 to 8 columns shrinks monotonically and stays in bounds.
- Slightly too wide: last column ellipsized, not dropped.
- Wide column 1: others dropped, column 1 ends in `…`.
- D1 in a row whose last kept cell is empty.
- Too many rows at `Px`: bottom rows gone, `truncated` set.
- Add a small pure helper returning the placed cell texts, so truncation tests
  can compare strings instead of pixels.

## T3: renderer wiring (`src/render/mod.rs`)

- `RenderConfig.column_gap` from `Config`.
- `SlotSpec.explicit_align: Option<Align>`, set in `spec()` (D4).
- In `render_strip`, grid applies when `spec_segments` returned one spec, mode
  is `Raw` or `Continuous`, `word_wrap` is off and `has_tab`. Then pass
  `Grid { gap_spaces: column_gap, mono: <value's font is mono> }` and
  `explicit_align.unwrap_or(Align::Left)`.
- One segment in `Tiles` with a tab and `word_wrap` off: `log::warn!`, draw as
  today.
- `render_slot`, `draw_segment` for dials, the ruler and multi-segment strips
  pass `None`. `slot_plain_text`, `needs_image` and `src/plugin.rs` are not
  touched.
- `Detail::TextOnly` keeps the grid.

Tests: for each failing condition (dial, `|`, tiles, `word_wrap`) a tab renders
pixel-identical to the same text with four spaces; `text` and `spans` values
make grids; default left, `align: "right"` honoured, global `align` ignored for
grids and still honoured without a tab; `TextOnly` still has columns.

Golden `strip_grid`: raw mode, the five `busy` rows from T4. Create it with
`UPDATE_GOLDEN=1 cargo test --test golden golden_strip_grid` (only that test),
then open the PNG and check the columns before committing.

## T4: zjsb `lines` layout

### `src/zjsb/mod.rs`

```rust
pub enum Layout { Segments, Lines }   // Options.layout, default Segments
const MAX_ROWS: usize = 5;
const ALL_CLEAR: &str = "[c=#66cc88][b]All clear[/b][/c]";
```

- `render`: attention off and `blank_when_idle` checks stay first; then
  `Mode::Strip` + `Layout::Lines` calls `lines()`.
- Extract the `+N more` context from `strip()` into `more_ctx(hidden, counts)`
  and use it in both.
- `lines()`:
  1. No entries and a non-empty idle template: its rendered lines, first five.
  2. Gutter = first five `counts.render_cells`; none if the template is empty.
  3. Entry rows: no entries → one row `[ALL_CLEAR]`; up to five entries → one
     row each; more → four entry rows plus `more.render_cells` for
     `entries[4..]`.
  4. `rows = min(5, max(gutter, entry rows))`; build each row per D3.
  5. Per row, 400 characters including separators: drop whole trailing cells,
     never the first.
  6. Join cells with the two characters `\t`, rows with `\n`. Never `|`.

`src/writer.rs` needs no change.

### Other files

- `Options::from_config` maps `zjsb_layout`.
- `src/zjsb/examples.rs`: `apply` accepts `zjsb_layout`.
- `tests/zjsb/examples.toml`: add `lines-agents` and `lines-busy`; update the
  count in the header, in `tests/zjsb_golden.rs` and in `src/zjsb/tests.rs`.

### Expected strings

Write these by hand, without a trailing newline. Do not generate them from the
code under test.

`tests/zjsb/expected/lines-agents.txt`, one line in the file:

```text
[c=orange]● 1[/c]\t[b]claude[/b]\t[c=orange]needs approval[/c]\trun cargo test\n[c=grey]✗ 0[/c]\t[b]opencode[/b]\t[c=green]done[/c]\tPR #42 ready\n[c=green]✓ 1[/c]\t[b]codex[/b]\t[c=#55ccff]working[/c]\t60%\n[c=#55ccff]» 1[/c]\t
```

`tests/zjsb/expected/lines-busy.txt`, one line in the file:

```text
[c=orange]● 2[/c]\t[b]claude[/b]\t[c=orange]needs login[/c]\tlogin expired\n[c=red]✗ 1[/c]\t[b]codex[/b]\t[c=orange]needs approval[/c]\tapply patch\n[c=green]✓ 1[/c]\t[b]opencode[/b]\t[c=red]failed[/c]\trate limited\n[c=#55ccff]» 2[/c]\t[b]claude[/b]\t[c=green]done[/c]\t\n\t[b]+2 more[/b]\t[c=#55ccff]working[/c]\tsee inbox
```

### Tests

- Counts template with 0, 1, 4, 6 lines against 0, 1, 4, 5, 6 entries; check
  every row of the design's matrix.
- Entry template with 1, 2, 3, 5 lines; more template with fewer lines.
- All-empty placeholder lines keep their position in gutter and entries.
- Five rows with full-width values exceed 400 characters and keep all rows.
- A single row over 400 characters drops trailing cells only.
- Idle: with gutter, without gutter, idle template of seven lines, blank.
- Values with tab, `|`, `\t`, markup add no cell and no row.
- `segments` and compact output unchanged (existing goldens).
- `tests/cli.rs`: `apply-zellij` with `OPENDECK_STRIP_ZJSB_LAYOUT=lines` writes
  a `strip` holding real tabs and newlines and `strip_mode = "raw"`.

## T5: docs and previews

- `docs/status-format.md`: tab columns (when they apply, cells, alignment,
  truncation), `zjsb_layout` under "Following zj-strip-bridge", the row limit
  and the per-row budget.
- `README.md`: `column_gap` and `zjsb_layout` in the table and the commented
  config block, template meaning in `lines`, example count 29, and the break:
  tabs in an unsegmented strip now make columns, `watch` output included.
- `examples/config.toml`: both keys, commented out.
- `docs/on-device-checklist.md`: five-row readability in `raw` and
  `continuous`, long messages, idle.
- `preview.sh`: add the two examples. Make its `raw-string` call ignore the
  user's config (`OPENDECK_STRIP_CONFIG=/dev/null`). Run
  `cargo build --release && ./preview.sh`, open the two new images, and commit
  them with `preview/index.html`. Existing images must not change; if they do,
  report it and do not commit them.

Verify every documented default and warning against the code, not the design.

## T6: review

Prompt: review `git diff main...LineLayout` against
`docs/lines-layout-design.md` and this plan; do not edit source. Look in
particular for: any change in output when no grid applies; panics or
non-termination in the truncation loop (zero width, `column_gap = 16`, many
empty cells, huge cells, non-Latin placeholders); `\\t` and backslash handling
from template to status file to renderer; row and cell limits; warnings that
fire in default configurations. Write findings by severity to the scratchpad,
each with a reproducing input.

## T7: fixes and final check

One agent fixes confirmed findings, each with a failing test first. Then the
orchestrator:

1. Runs fmt, clippy and tests on `LineLayout`.
2. Renders and looks at the frames itself:
   `OPENDECK_STRIP_CONFIG=/dev/null target/release/opendeck-strip raw-string
   "$(cargo run -q --example zjsb_preview -- lines-busy)" --out
   <scratchpad>/lines-busy.png`.
3. Reports to the user: commits, test count, the rendered image, deviations from
   the design, and what is unverified.

Left for the user: merging, pushing, `opendeck-strip install`, and the on-device
checklist. Legibility at five rows is only an estimate until seen on the deck.

## Done when

- [ ] All checks pass; no existing golden or expected string changed.
- [ ] `lines-agents` and `lines-busy` match the hand-written strings.
- [ ] A tab on a key, dial, segment or tile renders as before.
- [ ] Review findings are fixed or listed with a reason.
- [ ] Docs and previews match the behaviour.

## Risks

| Risk | Handling |
|---|---|
| The golden PNG locks in a wrong picture | it is inspected before commit, and again in T7 |
| A later agent drifts from an interface | signatures are fixed above; a change needs the orchestrator |
| Five rows are hard to read on the device | on-device checklist; `MAX_ROWS` is one constant |
