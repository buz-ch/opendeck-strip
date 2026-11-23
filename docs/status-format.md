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

`zj-strip-bridge` can serve its deck over `zellij pipe` instead of running `opendeck-strip set` for every change. The deck arrives as newline-delimited JSON (NDJSON) frames. `opendeck-strip apply` applies that stream; `opendeck-strip follow` starts it and reconnects.

### `apply`

Reads frames on standard input and applies every `deck` frame to the status file in one atomic read-modify-write.

```sh
zellij --session work pipe --name zjsb.follow -a view=deck | opendeck-strip apply
```

- Slot strings use the `set` syntax: markup is on (the plugin escapes untrusted text), `\n` is a line break, and a string that starts with `{` and parses as a slot object is stored as an object. An object value is stored as a slot value.
- `settings` keys are written as top-level keys (`strip_mode`, `strip_gap`, `labels`). Unknown settings are stored unchanged.
- `clear` names follow `clear` semantics: `strip` also removes `dial1` to `dial4`.
- `full: true` first removes every slot in the frame's `owned` list that the frame does not set. This makes the deck equal to the frame.
- `hello`, `ping` and `state` lines are accepted and ignored; `hello` is logged.
- Unknown `type` values and unknown fields are ignored. A line that is not valid JSON is logged and skipped. A `deck` line without a numeric `v`, or with a `full` that is not a boolean, is skipped the same way: a producer bug must not turn a full frame into a delta.
- `seq` is checked from `hello` (`seq` 0). A gap, a repeated number or an unparsable line is logged as a warning; the frames after it are applied as what they declare. This differs from "treat the next frame as full" in the frame format below: a delta cannot rebuild the lost state, and taking one as full would delete every owned slot it does not set. Restart the stream to get a full frame; an idle `apply` catches up at the next full frame, which every new connection starts with.
- `--idle-clear SECS` removes the owned slots when no line, including `ping`, arrived for that long. It clears once per silent stretch and keeps reading; the next line re-arms it. It is off by default for `apply`.

Exit codes:

| Code | Meaning |
|---|---|
| 0 | End of input, with or without `--idle-clear` having fired |
| 1 | The status file cannot be read or updated |
| 2 | A frame has a higher `v` than this build knows |

### `follow`

Supervises the pipe and applies the same frames.

```sh
opendeck-strip follow [--session NAME] [--zellij BIN] [--stall-secs N] [--retry-secs MIN,MAX]
                      [--grace-secs N] [--clear-on-exit] [--once]
```

| Option | Default | Meaning |
|---|---|---|
| `--session` | first live session | Zellij session to follow; it must still be live, see below |
| `--zellij` | `zellij` on `PATH` | Zellij binary |
| `--stall-secs` | 15 | Reconnect when no line arrived for this long; clears the owned slots first |
| `--retry-secs` | `1,10` | Reconnect backoff range in seconds; doubles from MIN to MAX and returns to MIN only after a connection delivered a line |
| `--grace-secs` | 10 | Clear the owned slots after no session was live for this long |
| `--clear-on-exit` | off | Clear the owned slots on SIGTERM or SIGINT |
| `--once` | off | Read until the first end of stream, then exit: 0 if a `deck` frame was applied, 1 if none was |

Without `--session` the session is resolved on every connection attempt: `$ZELLIJ_SESSION_NAME` when it appears in `zellij list-sessions --no-formatting`, otherwise the first live line. Lines containing `(EXITED - attach to resurrect)` are dead sessions. A session given with `--session` is connected to by that name but is still checked against the same list on every attempt: when it is not live, `follow` waits for it and clears the owned slots after `--grace-secs`, as for any other session. When the sessions cannot be listed at all, it logs a warning and retries; it neither connects a session it would have had to guess nor clears. A session given with `--session` is still connected to by name then.

`follow` runs `zellij --session S pipe --name zjsb.follow -a view=deck start` with the child's standard input held open and never written to. It reconnects when the process exits, stdout reaches end of file, no line arrives for `--stall-secs`, or `seq` skips or repeats a number. The first frame after a reconnect is full, so no state is lost; a frame that follows a `seq` gap is not applied. The stall clock counts from the last line of any connection, not from the last connection, and is also checked between reconnects, so a producer that never sends a line is cleared after `--stall-secs` however quickly each connection ends. The owned slots from the last full frame are cleared once per silent stretch, before `follow` reconnects; `--clear-on-exit` does the same on SIGTERM or SIGINT. Only slots listed in the last full frame's `owned` are ever removed. A hard kill leaves the last frame on the deck.

`follow` exits with 0 on SIGTERM or SIGINT, with 1 when the stream could not be read and, under `--once`, when no frame was applied or no session was live, and with 2 on a frame with a higher `v`. `--idle-clear` and `--stall-secs` must be at least 1; 0 is a usage error (exit 2).

`follow` logs to the same file as the plugin and prints a one-line state to standard error on connect, disconnect and stall.

A systemd user unit is in [`examples/opendeck-strip-follow.service`](../examples/opendeck-strip-follow.service):

```ini
[Service]
ExecStart=%h/.cargo/bin/opendeck-strip follow --clear-on-exit
Restart=on-failure
```

### Frame format

Every line is one JSON object with `"v": 1` and `"type"`. `seq` starts at 0 for `hello` and increases by one per line of a connection; a reconnect starts at 0 again. A skipped or repeated number means a line was lost. A delta cannot rebuild what was lost, and a delta played as full would delete every owned slot it does not set, so `follow` ends the connection on a gap without applying that frame and reconnects, where the first frame is full; `apply` warns and applies the numbered lines as what they declare.

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

A `state` line carries the version 2 document of the plugin under `doc`; `opendeck-strip` ignores it.
