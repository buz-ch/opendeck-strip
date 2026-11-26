# Design: entries on lines

Status: implemented (`LineLayout`).

Goal: show the deck entries as rows across the whole touch strip instead of
the four 200 px segments, with the counts block as a left gutter. The renderer
gets tab columns for the full-width strip, the layout is selected in
`config.toml`, and the bridge is only a data source and is not changed.

## Non-goals

- No columns on keys, dials or `|` segments.
- No new data model outside the existing version 2 document.
- No new `zjse_*_template` keys: the existing templates do the formatting.
- No bridge change and no new bridge command or view.
- `zjse_mode = "compact"` and `zjse_layout = "segments"` produce the same
  strings as today.

## 1. Renderer: tab cells on the full-width strip

### When it applies

A strip value is drawn as a grid only when all of these hold:

- it is the `strip` value and renders as one full-width segment (no `|`);
- the strip mode is `raw` or `continuous` (including the continuous fallback
  from `raw`);
- `word_wrap` is off;
- at least one line contains a tab.

In every other case (keys, dials, `|` segments, `tiles`, wrapping, no tab) a
value renders exactly as today, and a tab drawn by the renderer stays four
spaces. Key and dial titles are not touched. A tab-containing single-segment
strip in `tiles` mode logs a warning, because it was probably meant as a grid.

This covers `set strip`, `raw-string`, `strip-preview`, `watch` writing to the
strip, a status file edited directly, and the zjse writer. `text` and `spans`
values both work.

### Cells and columns

- Each line is split into cells on `\t`. A run or span crossing a tab keeps its
  style in both cells. Markup is parsed per line before the split, so a tag may
  span cells.
- The number of columns is the maximum cell count of any line. Missing cells
  are empty; leading and trailing tabs create empty cells.
- A line without a tab is a single cell in column 1 and counts toward that
  column's width. It does not span the grid.
- An empty line still occupies a row.
- Column *n* is as wide as its widest cell.
- The gap between columns is `column_gap` spaces. One space is measured once
  for the whole grid, in the regular face of the value's base family (sans, or
  mono with `font = "mono"`) at the base size, ignoring per-run styles and
  `[s=]`. Backgrounds and underlines are not painted in the gap.
- Cells are left-aligned in their columns. The grid as a whole is left-aligned
  unless the strip value sets `align`; the global `align` key does not apply to
  grids. `valign`, `bg` and `bar` work as today.

### Fitting and truncation

- Auto-fit measures the grid: the width is the sum of the column widths plus
  the gaps, and the height is the line count. The existing size bisection is
  reused. An explicit `size` skips auto-fit, and the rules below still apply.
- If the grid is too wide at the final size, the rightmost column is narrowed
  to the remaining width and its cells that overflow end in `…`.
- If not even `…` fits in that column, the column is dropped from every row and
  the same rule applies to the next column to the left. Cells that lost content
  because of a dropped column end in `…`.
- Column 1 is never dropped.
- If the rows do not fit vertically at the final size, the top rows are kept,
  as today.
- Any narrowing or dropping sets `truncated`.

### Examples

| Input (full-width strip) | Result |
|---|---|
| 4 lines, no tabs | not a grid; current behaviour |
| 4 lines with 2–4 cells | grid with that many columns |
| Rows with different cell counts | max columns; short rows leave trailing cells blank |
| One title line without tabs above tabbed rows | title is a column 1 cell and may widen it |
| Empty line in the middle | blank row |
| Last column slightly too wide at minimum size | last column ellipsized, not dropped |
| Column 1 alone wider than the strip | other columns dropped, column 1 ellipsized |

## 2. zjse: `lines` layout

`zjse_layout = "segments" | "lines"`, default `segments`. It only applies with
`zjse_mode = "strip"`.

In `lines` the output is one full-width segment: rows joined by `\n`, cells by
`\t` (the same literal escapes `set` accepts), never `|`.

### Cells

- **Counts template:** its lines form the left gutter, one line per row.
- **Entry template:** its lines are the cells of one entry row. The default
  three-line template gives `name | state | message` after the gutter.
- **More template:** its lines are the cells of the `+N more` row.

For these three templates:

- A line whose placeholders are all empty yields an empty cell, including its
  literal text. The cell keeps its position (in `segments` the line is still
  dropped).
- A blank template line is still removed when the config is loaded. A line
  holding one space is a spacer.
- One template line is one cell: a tab in template text becomes a space. Values
  cannot inject cells, because sanitising turns control characters into spaces
  and escapes backslashes.
- Markup is parsed per output row, so a tag left unclosed in one template line
  carries into the later cells of that row.

### Rows

- At most **five** rows. This is a fixed limit, chosen for legibility, not a
  config key: `rows = min(5, max(counts_lines, entry_rows))`.
- Up to five entries are all shown. With six or more, the first four are shown
  and row 5 is the more template; `hidden`, `state_label`, `color` and
  `marker` are computed from the rest as in `segments`.
- A counts template with more than five lines shows the first five.
- The gutter is independent of the entry rows: a count next to an entry says
  nothing about that entry.
- A row without a counts line still starts with an empty gutter cell, so the
  entry columns stay in place. A row without an entry has blank cells after the
  gutter.
- If the counts template is empty there is no gutter column.
- The limit applies to zjse output only. A manual `set strip` with more lines
  shrinks to fit as today.

### Length limit

The 400-character `MAX_SEGMENT` total does not apply to `lines`: five rows with
markup can exceed it, and rows would be lost. Instead each row has its own
400-character budget, and trailing cells that exceed it are dropped whole. A
row is never dropped by the budget.

### Idle and attention off

- A non-empty `zjse_idle_template` still replaces the whole output. Its lines
  are rows, cut to five.
- With an empty idle template and nothing active, the gutter stays and row 1
  gets `[c=#66cc88][b]All clear[/b][/c]` as its first entry cell. The other
  idle texts of `segments` ("no failures", "nothing running", "waiting for
  agents…") are not shown.
- With an empty counts template as well, the output is that `All clear` line
  alone.
- `zjse_blank_when_idle` behaves as today.
- `attention_enabled: false` still shows the fixed attention-off text.

An output without any tab (idle replacement, attention off, or one column only)
is not a grid and follows the global `align`.

### Writer

The writer keeps the strip-owned slots and still sets `strip_mode = "raw"`. The
continuous fallback joins the four dial images, so the grid still spans the
strip. If the strip mode is later changed to `tiles`, tabs render as spaces
(with the renderer warning above).

### Default shape

With `zjse_layout = "lines"`, default templates and the `busy` scenario (six
entries):

```text
● 2  claude    needs login     login expired
✗ 1  codex     needs approval  apply patch
✓ 1  opencode  failed          rate limited
» 2  claude    done
     +2 more   working         see inbox
```

## 3. Row matrix

| Counts lines | Entries | Rows | Right side |
|---|---|---|---|
| 4 | 0 | 4 | `All clear` in row 1 |
| 4 | 1–3 | 4 | entries on top, empty below |
| 4 | 4 | 4 | fully filled |
| 4 | 5 | 5 | all entries |
| 4 | 6 or more | 5 | 4 entries + more |
| 1 | 4 | 4 | gutter on row 1 only |
| 6 or more | 4 | 5 | entries in rows 1–4, first 5 counts lines |
| 0 | 0 | 1 | `All clear` only |
| 0 | 5 | 5 | no gutter, 5 entry rows |
| 0 | 6 or more | 5 | 4 entries + more |

An entry template with *n* lines gives *n* entry columns.

## 4. Configuration

```toml
zjse_layout = "lines"
# column_gap = 2        # spaces between grid columns, whole number, 0 to 16
```

`column_gap` is a renderer key and applies to every grid. Values outside the
range are clamped, like other numeric keys. No `align` setting is needed. The
columns work with the proportional face; use `[mono]` in the templates for a
monospaced look.

Warnings at config load:

- `zjse_layout = "lines"` with `word_wrap = true`: no columns.
- `zjse_layout = "lines"` with `zjse_mode = "compact"`: layout ignored.
- `lines`: the counts template has more than five lines.
- `lines`: the more and entry templates have different line counts, so the
  more row will not align.
- `lines`: `min_font_size` above 16, so five rows may not fit and the `+N more`
  row is the first to be lost.

Five rows come out at roughly 21 px text in `raw` and 16 px in `continuous`
with the default padding and line height (estimates, to be checked on the
device).

## 5. Changes by file

| File | Change |
|---|---|
| `src/render/layout.rs` | cell split, column measurement, gap, grid auto-fit, grid truncation |
| `src/render/mod.rs` | decide when a strip is a grid, pass `column_gap` and grid alignment, tiles warning |
| `src/config.rs` | `column_gap`, `zjse_layout`, the warnings |
| `src/zjse/template.rs` | render a template as cells, keeping empty positions |
| `src/zjse/mod.rs` | `lines` output, five-row limit, more row, per-row budget, idle |
| `src/zjse/examples.rs` | accept `zjse_layout` in the example manifest |
| `docs/status-format.md` | tab columns and the layout key |
| `README.md` | configuration table and template semantics |
| `examples/config.toml` | lines layout example |
| `tests/` | as below |

## 6. Tests

Renderer:

- Grid conditions: each one failing (key, dial, `|`, `tiles`, `word_wrap`, no
  tab) renders as today.
- Cells: styles across a tab, spans with tabs, empty leading and trailing
  cells, ragged rows, tab-less line in a grid, empty lines.
- Layout: default left alignment and `align` override, `column_gap`,
  proportional and mono faces, auto-fit with 1–8 columns, narrowing before
  dropping, column 1 survival.
- A PNG golden of a grid in `tests/golden/`.

zjse:

- Rows: counts 0/1/4/6 lines; entries 0/1/4/5/6.
- Columns: entry template 1/2/3/5 lines; more template line-count mismatch.
- All-empty placeholder lines keep their cell; tabs in template text become
  spaces.
- Five rows with full-width values are not cut by the length limit.
- Idle with and without a gutter.
- String goldens for `lines` with the `agents` and `busy` scenarios.

## 7. Compatibility

- Tabs in an unsegmented strip value in `raw` or `continuous` mode used to
  render as four spaces and now make columns. This is a deliberate break and is
  documented. This includes `watch` commands whose output contains tabs.
- `zjse_layout` and `column_gap` are additive; `segments` remains the default,
  so existing zjse decks are unchanged.
- The status-file schema does not change.
