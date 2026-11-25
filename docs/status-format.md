# Status file format

This is the reference for the JSON (JavaScript Object Notation) status file that `opendeck-strip` reads and writes.

## Path

The path is resolved in this order:

1. `$OPENDECK_STATUS_FILE`, when set and non-empty.
2. `$XDG_RUNTIME_DIR/opendeck-status.json`.
3. The system temporary directory plus `opendeck-status.json`.

`opendeck-strip where` prints the path for the current shell. The plugin resolves the same rules at start-up, so a path set only in the shell that starts the plugin is not visible to a desktop-launched OpenDeck.

Writes are atomic: a temporary file in the parent directory is synced and renamed over the target. The parent directory is created when missing.

## Top-level object

The file is one JSON object. Unknown top-level keys are preserved by the command-line interface (CLI) read-modify-write commands and ignored by the plugin.

| Key | Type | Meaning |
|---|---|---|
| `dial1`, `dial2`, `dial3`, `dial4` | slot value | Dial slots |
| `key<row>_<col>` | slot value | Key slots, 1-based; the N4E has rows 1 to 2 and columns 1 to 5 |
| `strip` | slot value or `@slices` string | Strip content; a non-empty value activates strip mode |
| `strip_mode` | string | `continuous`, `tiles` or `raw`; aliases `strip`, `tile` are accepted |
| `strip_gap` | integer | Gap between tiles in pixels; `set` clamps to 0–100, rendering clamps to 0–32 |
| `labels` | boolean, number or string | Show slot names on blank slots when true |

`strip_mode` and `strip_gap` override the config file. `labels` has no config equivalent.

Unknown slot names are stored by `set` with a warning and ignored by the plugin. A slot value that does not match the schema renders as empty. Only invalid JSON for the whole file is treated as an error: the plugin logs it once and keeps the previous state. `set` refuses to modify an unparseable file; `clear` with no arguments overwrites it with `{}`.

## Slot values

A slot value is either a string or an object.

A string is shorthand for an object with only `text`:

```json
{"dial1": "INBOX\n3"}
```

An object accepts these fields. All fields are optional. Content comes from `text` or `spans`; an object with neither renders empty.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | Text with `\n` line breaks and inline markup |
| `spans` | array of span objects | Explicit styled runs; when non-empty, takes precedence over `text` |
| `color` | string | Text colour; hex or named |
| `bg` | string | Background colour |
| `size` | number, numeric string or `"auto"` | Base font size in pixels, or automatic fitting |
| `font` | string | `mono` or `monospace` selects the monospaced family; other values keep the proportional family |
| `align` | string | `left`, `center`/`centre`/`middle` or `right` |
| `valign` | string | `top`, `middle`/`center`/`centre` or `bottom` |
| `bar` | number | Progress bar fraction, clamped to 0 to 1; drawn under the text |

Unset fields fall back to the config defaults. `get` serialises objects with every field, using `null` for unset values; a reader may omit them. Unknown fields inside the object are ignored.

`size` is an absolute pixel size, not a multiplier. The markup tag `[s=...]` is a multiplier of that base size.

### Colours

Hex forms: `#rgb`, `#rrggbb`, `#rrggbbaa`. The alpha byte is parsed and ignored.

Named colours: `black`, `white`, `red`, `green`, `blue`, `yellow`, `orange`, `grey`, `gray`, `darkgrey`, `darkgray`.

### Spans

A span is an explicit run, used instead of markup when a script already builds structured data. Spans carry their style across newlines; the `text` field of a span may contain `\n`.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | Run text; may contain newlines |
| `bold` | boolean | Bold face |
| `italic` | boolean | Italic face |
| `underline` | boolean | Underline |
| `mono` | boolean | Monospaced face |
| `color` | string | Text colour |
| `bg` | string | Background colour |
| `size` | number | Multiplier of the base size, clamped to 0.5 to 3.0 |

```json
{
  "dial3": {
    "spans": [
      {"text": "CPU ", "bold": true},
      {"text": "42", "color": "#ff8800", "size": 1.6},
      {"text": "%"}
    ]
  }
}
```

Spans apply to one slot value. The `|` per-segment split applies only to string values and to the `text` field of an object without spans.

## Inline markup

Markup is parsed after line splitting and after the `|` split. Tags therefore do not cross a newline or a segment boundary.

| Tag | Effect |
|---|---|
| `[b]...[/b]` | Bold |
| `[i]...[/i]` | Italic |
| `[u]...[/u]` | Underline |
| `[c=<colour>]...[/c]` | Text colour |
| `[bg=<colour>]...[/bg]` | Background |
| `[s=<number>]...[/s]` | Size multiplier, clamped to 0.5 to 3.0 |
| `[mono]...[/mono]` | Monospaced face |
| `[[` | Literal `[` |

Rules:

- Tags nest. A close tag must match the innermost open tag; a mismatched close is shown literally.
- An unclosed tag ends at the end of its line.
- An unknown tag is shown literally.
- Markup is on by default for `set` and `watch`. `set --plain` and `watch --plain` escape `[` as `[[`, so brackets appear literally.
- Underline and background are honoured in images. Titles sent to OpenDeck are plain text, so key styling and dial title styling are flattened.

Examples:

```text
INBOX [c=#ff8800]3[/c]
[b]BUILD[/b] [c=green]OK[/c]
CPU [s=1.6]42[/s]%
```

## Settings

The CLI has typed handling for three keys:

```sh
opendeck-strip set labels 1
opendeck-strip set strip_mode tiles
opendeck-strip set strip_gap 20
```

`labels` accepts `1`, `0`, `true`, `false`, `yes`, `no`, `on`, `off`. In the file it may also be a number or a string; zero and false-like strings mean false.

`strip_mode` stores the normalised `continuous` or `tiles`. Reading accepts `strip` and `tile` as aliases. The default comes from `strip_mode` in the config file.

`strip_gap` is stored as an integer clamped to 0–100. Rendering clamps it to 0–32 so four 176 px tiles fit inside the 800 px strip. A value edited directly in the file is not clamped on read.

Setting any of these does not itself activate the strip. Setting `strip` does.

## The `strip` key

A non-blank string or an object with text or non-empty `spans` activates strip mode. An empty object, an empty or whitespace-only string, a missing key, or a value of another type leaves strip mode off. This keeps a writer that emits `{}` or `{"text":""}` from blanking the dials.

While strip mode is active, `dial1` to `dial4` are ignored and the dials show the four rendered slices. Entering strip mode first sends each dial an empty title, then the images. Leaving strip mode, by clearing or emptying `strip`, empties the dial titles, restores the dial layout, and returns the dials to their own slot values.

The string `test` is literal text. Use `opendeck-strip strip test` to show the test pattern on the device, or `opendeck-strip strip-preview test <file>` for a local PNG.

### Per-segment form

A `|` in a strip string splits it into independent segments:

```sh
opendeck-strip set strip 'CPU|MEM|NET|DISK'
```

| Segment count | Layout |
|---|---|
| 1 | Fills the whole strip |
| 2 to 4 | Segment N maps to slice N; missing segments stay blank |
| More than 4 | Extra segments are dropped |

In continuous mode each segment occupies 200 logical pixels of the 800-pixel canvas. In tiles mode each occupies one 176-pixel tile. The markup in each part is parsed separately, so a tag opened in one part does not close in another.

### Tab columns

A tab in a single-segment strip value (one with no `|`) lays the value out as a grid of columns. This applies when the strip mode is `raw` or `continuous` (including the continuous fallback from `raw`) and `word_wrap` is off, and when at least one line holds a tab. It does not apply to keys, dials, multi-segment strips, `tiles` mode or wrapped text; a tab is drawn as four spaces there. In `tiles` mode a tab also logs a warning, because the value was probably meant as a grid. The continuous fallback joins the four dial images, so the grid still spans the strip.

This covers `set strip`, `raw-string`, `strip-preview`, `watch` writing to the strip, a status file edited directly and the zj-strip-bridge writer. Both `text` and `spans` values work.

Cells:

- Each line is split into cells on tabs. A run or span crossing a tab keeps its style in both cells. Markup is parsed per line before the split, so a tag may span cells.
- The number of columns is the maximum cell count of any line. Missing cells are empty; leading and trailing tabs create empty cells.
- A line without a tab is a single cell in column 1 and counts toward that column's width; it does not span the grid.
- An empty line still occupies a row, and each column is as wide as its widest cell.
- The gap between columns is `column_gap` spaces (config, default 2, 0 to 16), measured once for the whole grid in the regular face of the value's base family (sans, or mono with `font = "mono"`) at the base size. Backgrounds and underlines are not painted in the gap.
- Cells are left-aligned in their columns. The grid as a whole is left-aligned unless the strip value sets `align`; the global `align` key does not apply to grids. `valign`, `bg` and `bar` work as elsewhere.

Fitting and truncation:

- The grid is laid out inside the slot's inner box: `padding_horizontal` insets the left and right edges, `padding_vertical` the top and bottom.
- Auto-fit measures the grid: its width is the sum of the column widths plus the gaps, its height the line count. An explicit `size` skips auto-fit; the rules below still apply.
- If the grid is too wide at the final size, the rightmost column is narrowed to the remaining width and its overflowing cells end in `…`.
- If not even `…` fits in that column, the column is dropped from every row and the same rule applies to the next column to the left. Cells that lost content because of a dropped column end in `…`. Column 1 is never dropped.
- If the rows do not fit vertically, the top rows are kept, as for other strip values.
- Any narrowing or dropping sets the truncated flag: the plugin logs `strip text was truncated to fit` and `raw-string` prints a note.

A tab in an unsegmented strip value used to render as four spaces. In `raw` and `continuous` mode it now makes columns; this includes `watch` commands whose output contains tabs.

### Internal slice form

`strip` may hold a slice spec instead of text:

```text
@slices:<dir>#<generation>
```

This is an internal form written by `size-test`. It is documented here because it appears in the status file and in `strip-preview`.

- `<dir>` is a directory containing exactly four Portable Network Graphics (PNG) files named `slice0.png` to `slice3.png`.
- Expected dimensions are 200x100 in continuous mode and 176x88 in tiles mode. Continuous slices are displayed 1:1 and centred, so the device shows 14 px black bands above and below. A missing or mismatched file is logged and retried with a backoff that grows to 60 seconds. If a strip was already showing, its last image stays on the dials until the new files load; a first activation stays blank until a load succeeds. `strip-preview` refuses a mismatched file immediately.
- `<generation>` is a decimal counter. It is not used to find files. It is part of the status value and therefore part of the plugin's cache key; increment it after rewriting the PNG files so the plugin reloads them.
- Relative paths are resolved against the process working directory. `size-test` writes an absolute path.
- The value is a string, so it cannot be combined with object styling or spans.

`size-test` creates `opendeck-strip-sizetest-<pid>` next to the status file, writes four PNG slices, sets `strip` to the spec, and increments the generation for each target. On completion, or on Ctrl-C, it removes `strip` only if it still holds the last spec this run wrote, then deletes the directory. Dial values are left alone. It requires continuous mode; if either the status file or the config selects tiles, it exits with an error.

`strip test` uses the same form with a directory named `opendeck-strip-test-<pid>`. It removes `strip` and the directory when it stops and leaves the dial slot values alone.

`strip-preview` accepts the same spec and renders the files to a preview PNG:

```sh
opendeck-strip strip-preview '@slices:/run/user/1000/opendeck-strip-sizetest-1234#3' out.png
```

## Transport and rendering effects

The file is the only input. These details affect what a value looks like on the device.

- Keys use OpenDeck titles. Any styling, spans, bars or layout fields on a key are flattened to plain text.
- Dials use a title for plain text. A dial value that needs colour, a background, a bar, an explicit size, spans or inline styling is rendered as a 200x100 image and sent through `setFeedback` on `full-canvas`. The title item is disabled so OpenDeck does not show the action name.
- In continuous mode the renderer works in an 800x100 coordinate space. AKP05 v0.12 centres that picture in the 800x128 strip, so 14 px black bands sit above and below the content. Glyphs are rasterised 1:1.
- In tiles mode the renderer works at 176x88 per tile with a configurable gap of blank canvas. The gap is not displayed; the physical dead bands are.
- The plugin caches each rendered strip until the `strip` value, `strip_mode` or `strip_gap` changes. Editing only `dial1` while the strip is active does not redraw the strip.

## Examples

Minimal:

```json
{"key1_1": "OK", "dial1": "INBOX\n3"}
```

Styled dial and progress bar:

```json
{
  "dial2": {
    "text": "DISK",
    "bg": "#101820",
    "color": "#55ccff",
    "bar": 0.72,
    "size": 34
  }
}
```

Strip with segments and a non-default mode:

```json
{
  "strip": "CPU|MEM|NET|DISK",
  "strip_mode": "tiles",
  "strip_gap": 20,
  "labels": true
}
```

## Following zj-strip-bridge

`zj-strip-bridge` serves its data over `zellij pipe`. `opendeck-strip apply-zellij` reads that stream and renders it into the status file; `opendeck-strip follow-zellij` starts it, renders the same way and reconnects.

`follow-zellij` and `apply-zellij` read `zellij … pipe --name zjsb.follow` with `view=state` by default. `--view state|deck` selects the reader: `state` reads the complete version 2 documents of the current plugin; `deck` is the legacy reader for deck frames from an older `pipe`-branch plugin (see "Frame format" below). The `data-only` plugin refuses `view=deck` with `unknown view`.

### Rendering the document

1. **Stream.** Each `state` line carries a complete version 2 document under `doc`. `follow-zellij` starts `zellij --session S pipe --name zjsb.follow -a view=state start`; `apply-zellij` reads the same lines on standard input. `hello` is logged, `ping` and unknown types are ignored, and `error` is logged. A line without a numeric `v`, or a malformed `state` line, is logged and skipped. A version above this build's is exit code 2. A `seq` gap is only a warning, because every state line replaces the whole deck.

2. **Selection.** The renderer takes the records with `on_deck: true` from the document, collects them in document order (sessions, then panes), and stable-sorts them by priority: blocked oldest first, then error, done and working newest first; tab id, pane id, pane title and session name break ties. Counts come from those records (`blocked` → needs you, `error` → failed, `done` → done, `working` → working). In `segments` (the default layout) the strip is the counts segment plus up to three entries. With more than three, it shows the first two and a `+N more` segment whose `hidden` is the rest and whose colour, marker and `state_label` come from the highest hidden state (blocked reads "need you"). With nothing active the `zjsb_idle_template` is the whole strip; without one, the counts panel is followed by the default `All clear` text; `zjsb_blank_when_idle` clears the slots instead. `attention_enabled: false` shows the fixed `Zellij attention is off` text. Compact mode is the first entry (or the idle template, or a clear) in `zjsb_slot`. The `lines` layout is described below.

3. **Templates.** The five templates live in `config.toml`: `zjsb_entry_template` (default `[b]{app|title:14}[/b]\n[c={color}]{state_label}[/c]\n{progress_pct|msg:22}`), `zjsb_counts_template` (default the four `● ✗ ✓ »` count lines), `zjsb_more_template` (default `[b]+{hidden} more[/b]\n[c={color}]{state_label}[/c]\nsee inbox`), `zjsb_compact_template` (default `[c={color}]{marker} {count}[/c]\n{app|title:8}`) and `zjsb_idle_template` (default empty).

   Syntax: `{field}` inserts a value, `{field:24}` truncates to 24 characters with `…`, `{a|b|c}` takes the first non-empty value, and `{{`/`}}` are literal braces. A literal `\n` and a real newline both separate lines. In `segments` a line whose placeholders are all empty is dropped; `lines` keeps it as an empty cell. A template is limited to 8 lines. Values are sanitised: control characters and `|` become spaces, `[` is escaped as `[[`, whitespace is collapsed, and long values are truncated. `|` in the template text itself is refused, because it would add a strip segment.

   Fields. Counts: `needs_you`, `failed`, `done`, `working`, `total`, and `needs_you_color`, `failed_color`, `done_color`, `working_color` (grey at zero). Entry: `app`, `title`, `state`, `state_label`, `kind`, `msg`, `progress_pct`, `tasks`, `marker`, `color`, `count`, `tab`, `pane`, `age`, `session`, `tab_name`, plus the counts fields. `+N more`: `hidden`, `state_label`, `color`, `marker`, plus the counts fields. `age` is `45s`, `3m`, `2h` or `4d`; `tasks` is `1 task` or `2 tasks`; `progress_pct` only for working records.

   In TOML the defaults are multi-line basic strings (`"""…"""`), so `\n` is a real newline and quotes need no escaping.

4. **Raw mode.** When the renderer writes a strip it sets `strip_mode = "raw"` in the status file once; compact mode does not. `labels` and `strip_gap` stay writer and status-file keys.

5. **Throttle.** `zjsb_min_interval_ms` (default 1000) is the minimum time between two writes; the latest state wins. Text that is already shown is not rewritten. A held state is written when the interval elapses, at the end of the stream and before a reconnect. An idle, stall or grace clear resets the "shown" state, so the next identical state is written again. `--clear-on-exit`, `--grace-secs`, `--stall-secs` and `--once` keep their meanings; a state write counts as an applied frame.

### The `lines` layout

`zjsb_layout = "segments" | "lines"` (default `segments`) selects how strip mode lays the deck out; it has no effect in compact mode, which logs that the layout is ignored. `lines` writes one full-width segment whose rows are separated by a newline and whose cells are separated by a tab. The status file then holds real tabs and newlines, like a `set strip` value with `\t` and `\n`. A `|` is never written.

Cells come from the templates:

- The lines of `zjsb_counts_template` are the left gutter, one line per row.
- The lines of `zjsb_entry_template` are the cells of one entry row, so the default three-line template gives `name | state | message` after the gutter.
- The lines of `zjsb_more_template` are the cells of the `+N more` row.

For these three templates, a line whose placeholders are all empty yields an empty cell, including its literal text, and keeps its position; in `segments` such a line is dropped. A blank template line is still removed when the config is loaded, while a line holding one space is a spacer. One template line is one cell: a tab in template text, literal or the two characters `\t`, becomes a space, so values cannot inject cells. Markup is parsed per output row, so a tag left unclosed in one template line carries into the later cells of that row.

Rows:

- At most five rows. The count is `min(5, max(counts lines, entry rows))`; five is a fixed legibility limit, not a config key.
- Up to five entries are all shown. With six or more, the first four are shown and row 5 is the more template; `hidden`, `state_label`, `color` and `marker` are computed from the rest as in `segments`.
- A counts template with more than five lines shows the first five.
- The gutter is independent of the entries: a count next to an entry says nothing about that entry.
- A row without a counts line still starts with an empty gutter cell, so the entry columns stay in place. A row without an entry has blank cells after the gutter.
- A template with no lines gives no gutter column at all.

The 400-character `MAX_SEGMENT` total does not apply to `lines`: each row has its own 400-character budget instead, and trailing cells past it are dropped whole. A row is never dropped by the budget. A manual `set strip` is not limited to five rows; it shrinks to fit as usual.

Idle and attention:

- A non-empty `zjsb_idle_template` replaces the whole output; its lines are rows, cut to five. One template line is one cell, and a tab in its text, literal or the two characters `\t`, is a space too.
- With an empty idle template and nothing active, the gutter stays and row 1 gets the `All clear` text as its first entry cell. With an empty counts template too, the output is that line alone.
- `zjsb_blank_when_idle` behaves as in `segments`, and `attention_enabled: false` still shows the fixed attention-off text.

An output without any tab (an idle replacement, the attention-off text, or a single column) is not a grid and follows the global `align`.

The writer keeps the strip-owned slots and still sets `strip_mode = "raw"`. If the strip mode is later changed to `tiles`, the tabs render as spaces with the renderer warning above.

With default templates and six entries, the strip shows:

```text
● 2  claude    needs login     login expired
✗ 1  codex     needs approval  apply patch
✓ 1  opencode  failed          rate limited
» 2  claude    done
     +2 more   working         see inbox
```

### `apply-zellij`

Reads a `view=state` stream on standard input, renders each document and applies it to the status file in one atomic read-modify-write.

```sh
(printf 'start\n'; cat) | zellij --session work pipe --name zjsb.follow | opendeck-strip apply-zellij
opendeck-strip apply-zellij --view deck < recorded-deck-frames.ndjson
```

- `--view state` (default) reads documents; `--view deck` reads the legacy deck frames described below.
- `hello` is logged; `ping` and unknown types are ignored on either stream, and a `state` line in a deck stream is ignored. An `error` line is logged.
- A line that is not valid JSON, a state line without a numeric `v` or a valid `doc`, and a deck line without a numeric `v` are logged and skipped. A `v` above the version this build knows stops with exit 2.
- `seq` is checked from `hello` (`seq` 0). A gap, a repeated number or an unparsable line is a warning; the lines after it are applied as what they declare. A state line is complete, so a gap loses nothing; a deck delta cannot rebuild lost state. Restart the stream to get a fresh full frame.
- `--idle-clear SECS` removes the owned slots when no line, `ping` included, arrived for that long. It clears once per silent stretch and keeps reading; the next line re-arms it. It is off by default for `apply-zellij`.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | End of input, with or without `--idle-clear` having fired |
| 1 | The status file cannot be read or updated |
| 2 | A line has a higher `v` than this build knows |

### `follow-zellij`

Supervises the pipe and renders and applies the same stream.

```sh
opendeck-strip follow-zellij [--session NAME] [--zellij BIN] [--stall-secs N] [--retry-secs MIN,MAX]
                      [--grace-secs N] [--clear-on-exit] [--once] [--view state|deck]
```

| Option | Default | Meaning |
|---|---|---|
| `--session` | first live session | Zellij session to follow; it must still be live, see below |
| `--zellij` | `zellij` on `PATH` | Zellij binary |
| `--stall-secs` | 15 | Reconnect when no line arrived for this long; clears the owned slots first |
| `--retry-secs` | `1,10` | Reconnect backoff range in seconds; doubles from MIN to MAX and returns to MIN only after a connection delivered a line |
| `--grace-secs` | 10 | Clear the owned slots after no session was live for this long |
| `--clear-on-exit` | off | Clear the owned slots on SIGTERM or SIGINT |
| `--once` | off | Read until the first end of stream, then exit: 0 if a write happened, 1 if none was |
| `--view` | `state` | `state`: complete documents rendered here; `deck`: legacy frames |

Without `--session` the session is resolved on every connection attempt: `$ZELLIJ_SESSION_NAME` when it appears in `zellij list-sessions --no-formatting`, otherwise the first live line. Lines containing `(EXITED - attach to resurrect)` are dead sessions. A session given with `--session` is connected to by that name but is still checked against the same list on every attempt: when it is not live, `follow-zellij` waits for it and clears the owned slots after `--grace-secs`, as for any other session. When the sessions cannot be listed at all, it logs a warning and retries; it neither connects a session it would have had to guess nor clears.

`follow-zellij` runs `zellij --session S pipe --name zjsb.follow -a view=state start` (`view=deck` under `--view deck`) with the child's standard input held open and never written to. It reconnects when the process exits, stdout reaches end of file, or no line arrives for `--stall-secs`. On a state stream a `seq` gap is only a warning: each line is a complete document. The stall clock counts from the last line of any connection, not from the last connection, and is also checked between reconnects, so a producer that never sends a line is cleared after `--stall-secs` however quickly each connection ends. The owned slots (`strip` and `dial1` to `dial4`, or the compact slot from `zjsb_mode`/`zjsb_slot`) are cleared once per silent stretch, before `follow-zellij` reconnects; `--clear-on-exit` does the same on SIGTERM or SIGINT. A hard kill leaves the last render on the deck.

`follow-zellij` exits with 0 on SIGTERM or SIGINT, with 1 when the stream could not be read and, under `--once`, when nothing was written or no session was live, and with 2 on a line with a higher `v`. `--idle-clear` and `--stall-secs` must be at least 1; 0 is a usage error (exit 2).

`follow-zellij` logs to the same file as the plugin and prints a one-line state to standard error on connect, disconnect and stall.

A systemd user unit is in [`examples/opendeck-strip-follow-zellij.service`](../examples/opendeck-strip-follow-zellij.service):

```ini
[Service]
ExecStart=%h/.cargo/bin/opendeck-strip follow-zellij --clear-on-exit
Restart=on-failure
```

### Frame format (legacy)

The deck frames described here belong to the older `pipe`-branch plugin and are only parsed with `--view deck`. A current `zj-strip-bridge` (`data-only`) answers `view=deck` with `unknown view`.

Every line is one JSON object with `"v": 1` and `"type"`. `seq` starts at 0 for `hello` and increases by one per line of a connection; a reconnect starts at 0 again. A skipped or repeated number means a line was lost. A delta cannot rebuild what was lost, and a delta played as full would delete every owned slot it does not set, so `follow-zellij --view deck` ends the connection on a gap without applying that frame and reconnects, where the first frame is full; `apply-zellij --view deck` warns and applies the numbered lines as what they declare.

```json
{"v":1,"type":"hello","plugin":"zj-strip-bridge","version":"0.0.1","session":"work","view":"deck","seq":0}
{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"<markup>"},"settings":{"strip_mode":"raw"},"clear":[],"owned":["strip","dial1","dial2","dial3","dial4"]}
{"v":1,"type":"deck","seq":2,"full":false,"set":{"strip":"<markup>"},"settings":{},"clear":[]}
{"v":1,"type":"state","seq":3,"doc":{ ...version 2 document... }}
{"v":1,"type":"ping","seq":4}
{"v":1,"type":"error","error":"unknown pipe"}
```

Deck fields:

| Field | Type | Meaning |
|---|---|---|
| `full` | boolean | The deck must equal this frame; remove owned slots not in `set`. Absent means `false`; any other type makes the line skipped |
| `set` | object | Slot values in the `set` syntax |
| `settings` | object | Typed top-level keys: `strip_mode` string, `strip_gap` number, `labels` boolean |
| `clear` | array of strings | Slots to remove with `clear` semantics |
| `owned` | array of strings | Slots the plugin owns: `strip` and `dial1` to `dial4`, or the compact slot. Only sent with a full frame |

In a deck stream a `state` line is ignored. In a state stream `deck` is unknown and ignored.
