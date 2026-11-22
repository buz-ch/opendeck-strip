# opendeck-strip

Live status text for the keys, dials and touch strip of a Mirabox N4E on Linux, drawn through [OpenDeck](https://github.com/nekename/OpenDeck).

## What it is

`opendeck-strip` is a small Rust program that turns text into pictures for the Mirabox N4E's ten keys, four dials and touch strip. It is both an OpenDeck plugin and a command-line tool. You put a value in a slot, and the tool lays it out, fits it, and puts it on the device.

It reads everything from one JSON status file, so anything that can write a file or call the CLI can drive the display: a shell script, a cron job, a systemd timer or a status daemon.

## What it does

- **Renders text.** Embedded anti-aliased fonts, automatic size fitting, multi-line layout, inline styling (colour, size, bold, italic) and progress bars.
- **Fills the whole strip.** Continuous mode draws one picture across the four dials with no gaps. Strip text can be split into up to four independent segments with `|`.
- **Sends full-height frames.** In `raw` mode it renders an 800x128 frame and sends it straight to the AKP05 plugin's socket, so the strip is used edge to edge with no letterbox bands. It falls back to the standard dial-image path when the socket isn't available. `opendeck-strip raw-string "text"` renders a string (same syntax as `set strip`) as such a frame and shows it directly.
- **Runs from scripts.** `set`, `get`, `clear`, `watch` (re-run a command every few seconds and show its output) and `where` cover everyday use.
- **Helps you test.** `strip-preview` renders a PNG without a device, `size-test` measures the image size the device will receive, `raw-test` shows a coloured test frame, `raw-test --ruler` shows an edge ruler, and `doctor` checks the plugin, hidraw access and the installed AKP05 build.

## Quick example

```sh
opendeck-strip set dial1 'INBOX\n3' dial2 '{"text":"DISK","bar":0.72}'
opendeck-strip watch --slot dial3 --every 2 -- date +%H:%M
opendeck-strip clear
opendeck-strip raw-string '[b]BUILD[/b] [c=green]OK[/c]|CPU 42%|MEM 61%|DISK 72%'   # full 800x128 strip via AKP05 raw mode
```

## Requirements

- Mirabox N4E (USB ID `6603:1007`), Linux only.
- OpenDeck with the [AKP05 device plugin](https://github.com/ambiso/opendeck-akp05) v0.12.0 or newer. Continuous mode also needs N4E firmware 02.014 or newer.
- For `raw` mode: an AKP05 build with raw strip mode and `strip_raw = true` in its config.
- Rust 1.88 or newer to build.

Installation, usage and configuration are described below. Supported device: Mirabox N4E.

## Strip modes and requirements

Continuous mode needs the AKP05 device plugin **v0.12.0 or newer** and N4E firmware **02.014 or newer**. The plugin reads the firmware version and enables the full-width image slot itself; there is no patch to apply. To force the experimental BGPIC renderer on a deck the plugin does not enable automatically (untested; report results to the AKP05 project), set `bgpic = true` in `${XDG_CONFIG_HOME:-~/.config}/opendeck-akp05/config.toml` (Flatpak: `~/.var/app/me.amankhanna.opendeck/config/opendeck-akp05/config.toml`). Create the file and directory if they do not exist, put the key at the top of the file before any `[table]` header, and use a boolean (`bgpic = true`, not a string). A parse error is only logged as a warning in the AKP05 plugin log and the default is used. The override applies to every recognised deck and bypasses the firmware check, so it can leave the strip blank on unsupported firmware. Restart OpenDeck or reconnect the deck afterwards.

| Mode | What you see | Requirement |
|---|---|---|
| `continuous` (default) | One picture across the whole strip, no gaps | AKP05 v0.12.0+, N4E firmware 02.014+ |
| `tiles` | Four 176x88 tiles with dead bands between them | Any AKP05 version, but only older firmware or non-full-strip devices still show the physical dead bands |
| `raw` | One 800x128 picture across the whole strip, no bands, one device write per redraw | An AKP05 build with the raw strip socket and `strip_raw = true`, plus a full-strip transport |

OpenDeck renders every dial image on a fixed 200x100 canvas. AKP05 v0.12 preserves the aspect ratio when it builds the 800x128 strip, so each 200x100 picture is centred in its quarter with 14 px black bands above and below. The renderer therefore lays continuous mode out in 800x100 at 1:1; it does not compensate for a device-side stretch.

On firmware that supports the full-width slot, the device plugin uses it for every encoder image. `tiles` then no longer lands on the physical dead bands, so use `continuous`. `tiles` remains useful on older firmware and non-full-strip devices. `strip_gap` only affects tiles mode.

A strip redraw in continuous mode writes four dial images, which the device plugin joins into one 800x128 JPEG (Joint Photographic Experts Group image). Redraws are rate limited to `redraw_hz` bursts per second, default 2. This is the mitigation for visible flicker.

### Raw mode

OpenDeck renders every dial image on a fixed 200x100 canvas, so `continuous` can never use the 14 px bands above and below the picture. `raw` skips OpenDeck: it draws a full 800x128 frame and sends it to AKP05 over a local Unix socket, `$XDG_RUNTIME_DIR/opendeck-akp05/<device-id>.sock`. AKP05 shows it 1:1. It needs an AKP05 build that has this socket (the `raw-image` branch) and `strip_raw = true` in AKP05's `config.toml`, next to `bgpic`.

```sh
opendeck-strip set strip_mode raw
opendeck-strip set strip 'LEFT|MID|RIGHT'
```

While the raw frame is shown, the plugin sends no dial images, because AKP05 only stores them. If the socket is missing or rejects a frame, the plugin logs one warning and draws the strip as `continuous` dial images, then retries the socket after 10 seconds (doubling up to 60). Removing `strip`, or changing `strip_mode`, clears raw mode and redraws the dials. After the deck reconnects, OpenDeck re-announces the dials and the frame is sent again.

Raw mode uses the same device JPEG budget as `continuous` (`max_jpeg_bytes`) and AKP05's 512 KiB socket limit; an over-budget frame degrades to text only. The socket is found from the OpenDeck device id, then as the only socket in the directory. `raw_socket` in the config names one explicitly. `@slices` sources in raw mode are four 200x128 images.

Commands for the socket:

```sh
opendeck-strip raw-test                 # coloured 800x128 test frame for 10 s, then clear
opendeck-strip raw-test --seconds 30
opendeck-strip raw-test --out test.png  # write the frame; no device needed
opendeck-strip raw-test --ruler         # edge ruler: how many outer pixels does the device hide?
opendeck-strip raw-send frame.png       # show an 800x128 PNG or JPEG
opendeck-strip raw-string 'HELLO'       # render a string as an 800x128 frame and show it
opendeck-strip raw-string 'A|B|C|D' --out s.png  # write the frame; no device needed
opendeck-strip raw-clear                # leave raw mode
```

`raw-string STRING` takes the same strip syntax as `set strip`: `|` splits the strip into up to four 200 px segments, `[b]..[/b]` and `[c=red]..[/c]` style the text, `\n` is a line break, and a JSON object is a styled value (`{"text":"DISK","bar":0.72}`). The frame is 800x128 with no bands, sent as one PNG, and stays until `raw-clear`. It is rendered with the same fitting as the plugin's raw mode. If the PNG is over AKP05's 512 KiB socket limit it is redrawn as text only, and the command prints a note when the device JPEG is over `max_jpeg_bytes`. `--socket PATH` picks a socket and `--out FILE.png` writes the frame instead of sending it.

`raw-test` is generated in the binary, so it needs no files. Its background is not black: four different quadrant colours, a white one-pixel border on every edge, a magenta middle row, a yellow middle column, orange ticks at x = 200 and 600, upright text and the numbers 1 to 4. On the device you should see colour to every edge with no black bands, all four border edges, no seam or shift at x = 200, 400 or 600, and upright text. The command prints the frame's PNG size and its estimated device JPEG size; the test frame is larger than the default `max_jpeg_bytes`, so corruption on the device may be that size limit and not raw mode. `--socket PATH` picks a socket when several decks are connected.

`raw-test --ruler` shows a mid-grey frame with eight one-pixel bands on every edge, from the outer edge inward: white, red, green, blue, yellow, cyan, magenta, orange. The first colour you can see on each edge tells how many pixels the device hides there (white = 0, red = 1, green = 2, and so on). Use it when `raw test` shows a cut-off border. Look straight at the strip first: the cover glass sits above the panel, so from an angle (parallax) the outermost row can look cut off even though it is displayed. It works with `--seconds`, `--socket` and `--out`.

## Build

Requirements: a Rust toolchain, version 1.88 or newer. No system libraries are needed.

```sh
cargo build --release
```

On openSUSE Tumbleweed, install the toolchain with rustup if it is missing.

## Install

Quit OpenDeck first.

```sh
./target/release/opendeck-strip install
```

`install` writes `local.statusdisplay.sdPlugin` into the native OpenDeck plugin directory, or the Flatpak one when only that exists. It copies the binary that is currently running, so install from the release build you want to keep. `--dir` selects another plugins directory; a path ending in `.sdPlugin` is used as the plugin directory itself.

You can also put the binary on `PATH` and run the command by name:

```sh
cargo install --path .
opendeck-strip install
```

Start OpenDeck again and place the "Status Slot" action on keys or dials. The action uses OpenDeck's `$A0` encoder layout. Re-run `install` after building a new binary; the plugin directory holds a copy.

## Quick start

`where` prints the status file this shell would use.

```sh
opendeck-strip where
```

`set` takes one or more `SLOT TEXT` pairs. `\n` is a line break.

```sh
opendeck-strip set key1_1 OK
opendeck-strip set dial1 'INBOX\n3'
opendeck-strip set strip '[b]BUILD[/b] [c=green]OK[/c]'
```

A text argument that starts with `{` and parses as a slot object is stored as an object. Everything else is stored as a string.

```sh
opendeck-strip set dial2 '{"text":"DISK","bar":0.72,"size":34}'
```

`set --plain` escapes brackets so they appear literally. This is for text that contains markup-like sequences.

`get` prints the current status file as JSON.

```sh
opendeck-strip get
```

`clear` blanks slots. With no arguments it clears the whole file, including settings. `clear strip` removes the strip and the four dial entries, returning the dials to their normal layout.

```sh
opendeck-strip clear dial1
opendeck-strip clear strip
```

`strip-preview` renders a strip spec to a Portable Network Graphics (PNG) file without touching the device. The spec is text, a slot object, `test`, or an internal slice reference.

```sh
opendeck-strip strip-preview 'LEFT|MID|RIGHT' strip.png
opendeck-strip strip-preview test test-continuous.png
opendeck-strip strip-preview test test-tiles.png --tiles --simulate-device
```

Without `--tiles`, the preview uses `strip_mode` from the status file, falling back to the config file. The gap likewise comes from the status file first. In tiles mode, `--simulate-device` shows the physical dead bands and the 800x128 geometry; without it, tiles are composed tightly. The continuous preview includes the 14 px bands the device adds above and below the picture.

`size-test` grows a block of noise until the estimated device JPEG reaches each target size. It needs `continuous` mode and clears the strip when it stops.

```sh
opendeck-strip size-test
opendeck-strip size-test --start 4000 --step 1000 --max 20000
```

`strip test` shows the seam test pattern on the device (in raw mode it goes through the plugin as four 200x128 slices): a gradient, a diagonal line and red markers at the slice boundaries. It writes four slices next to the status file, points `strip` at them, waits for Ctrl-C (or `--seconds`), then clears the strip.

```sh
opendeck-strip strip test
opendeck-strip strip test --seconds 30
```

`doctor` checks the status file path and readability, the plugin directories, hidraw access to `6603:1007`, and the installed AKP05 build.

```sh
opendeck-strip doctor
```

`watch` runs a command on an interval and shows its first four output lines. Failures and timeouts use a red error style. The command is executed directly, not through a shell, so arguments with spaces stay intact; use `sh -c` for pipes and redirection.

```sh
opendeck-strip watch --slot dial1 --every 2 -- date +%H:%M
opendeck-strip watch --slot strip --every 5 -- sh -c 'df -h / | tail -1'
```

All commands are listed by `opendeck-strip --help`, and each subcommand has its own `--help`.

## Status file

Location, in order of precedence:

1. `$OPENDECK_STATUS_FILE`, when set and non-empty.
2. `$XDG_RUNTIME_DIR/opendeck-status.json`.
3. The system temporary directory, usually `/tmp/opendeck-status.json`.

The plugin and the CLI must resolve the same path. `opendeck-strip where` shows the result for the current shell. Writes are atomic: a temporary file is renamed over the old one.

Top-level keys:

| Key | Meaning |
|---|---|
| `dial1` to `dial4` | Dial slot values |
| `key<row>_<col>` | Key slot values, 1-based; `key1_1` to `key2_5` on the N4E |
| `strip` | Strip value; setting it activates strip mode |
| `strip_mode` | `continuous`, `tiles` or `raw`, overrides the config for this file |
| `strip_gap` | Gap between tiles in pixels, 0 to 32 at render time, overrides the config |
| `labels` | When true, blank slots show their own names |

A slot value is either a string or an object. The string form is shorthand for `{"text": ...}`.

```json
{
  "key1_1": "OK",
  "dial1": {"text": "INBOX\n3", "color": "#ff8800", "size": 30},
  "dial2": {"text": "DISK", "bar": 0.72},
  "dial3": {
    "spans": [
      {"text": "CPU ", "bold": true},
      {"text": "42", "color": "#ff8800", "size": 1.6},
      {"text": "%"}
    ]
  },
  "strip": "CPU|MEM|NET|DISK",
  "strip_mode": "tiles",
  "labels": true
}
```

Object fields: `text`, `spans`, `color`, `bg`, `size` (pixels or `"auto"`), `font` (`mono` or `monospace`; the default family is proportional), `align`, `valign`, `bar` (0 to 1). All fields except the content are optional. Unknown fields are ignored, and unknown top-level keys are preserved by `set`, `clear` and `get`.

Inline markup is enabled for `set` and `watch`, and can be turned off with `--plain`. It applies to string values and to the `text` field of objects.

| Markup | Effect |
|---|---|
| `[b]...[/b]`, `[i]...[/i]`, `[u]...[/u]` | Bold, italic, underline |
| `[c=#ff8800]...[/c]`, `[c=red]...[/c]` | Text colour |
| `[bg=#202020]...[/bg]` | Background behind a run |
| `[s=1.5]...[/s]` | Size multiplier, 0.5 to 3 |
| `[mono]...[/mono]` | Monospaced face |
| `[[` | A literal `[` |

Tags nest. An unclosed tag ends at the line end. Unknown tags are shown literally. A closing tag must match the innermost open tag. Named colours are `black`, `white`, `red`, `green`, `blue`, `yellow`, `orange`, `grey`/`gray` and `darkgrey`/`darkgray`; hex colours accept `#rgb`, `#rrggbb` and `#rrggbbaa`.

`|` splits a strip string into independent segments, at most four. Text never crosses a segment boundary, and tags do not carry over a split.

```sh
opendeck-strip set strip 'cpu|mem|net|disk'
```

The full schema, the span object fields and the internal `@slices:<dir>#<n>` form are documented in [docs/status-format.md](docs/status-format.md).

## Configuration

The config file is `~/.config/opendeck-strip/config.toml`. `OPENDECK_STRIP_CONFIG` selects another file. Every key can be overridden with an environment variable named `OPENDECK_STRIP_` plus the upper-case key, for example `OPENDECK_STRIP_STRIP_MODE=tiles`.

| Key | Default | Meaning |
|---|---|---|
| `strip_mode` | `continuous` | Initial strip mode: `continuous`, `tiles` or `raw` |
| `strip_gap` | 28 | Tile gap in pixels, clamped to 0–32 at render time |
| `max_jpeg_bytes` | 9000 | Size budget for the final device JPEG |
| `redraw_hz` | 2.0 | Redraw bursts per second; raw mode redraws at least 10 per second |
| `raw_socket` | none | AKP05 raw socket path for `strip_mode = raw`; default is found automatically |
| `default_color` | `#ffffff` | Default text colour |
| `default_bg` | `#000000` | Default background |
| `min_font_size` | 8.0 | Lower auto-fit limit, clamped to at least 4 |
| `max_font_size` | 96.0 | Upper auto-fit limit |
| `line_height` | 1.15 | Line height factor, at least 0.8 |
| `padding` | 4.0 | Inner padding in pixels |
| `word_wrap` | false | Wrap at spaces |
| `strip_uniform_size` | false | Strip segments that auto-fit all use the smallest size any of them needs, instead of one size each |
| `strip_uniform_skip_first` | false | With `strip_uniform_size`: segment 1 keeps its own size and is left out of the shared one |
| `align` | `center` | `left`, `center` or `right` |
| `valign` | `middle` | `top`, `middle` or `bottom` |
| `bar_color` | `#55ccff` | Progress bar fill |
| `bar_bg` | `#202830` | Progress bar track |
| `watch_timeout_secs` | 5 | Timeout for `watch` commands |
| `font_sans`, `font_sans_bold`, `font_sans_italic`, `font_sans_bold_italic` | embedded | Proportional font paths |
| `font_mono`, `font_mono_bold`, `font_mono_italic`, `font_mono_bold_italic` | embedded | Monospaced font paths |

The plugin reads the config once at start-up; restart OpenDeck after editing it. The status file keys `strip_mode` and `strip_gap` take precedence and apply without a restart. `labels` exists only in the status file.

Values set with `set`:

```sh
opendeck-strip set labels 1 strip_mode tiles strip_gap 20
opendeck-strip set labels 0
```

`labels` accepts `1`, `0`, `true`, `false`, `yes`, `no`, `on` or `off`.

### Segment font sizes

Each `|` segment of a strip value auto-fits on its own, so a segment with more or longer lines gets a smaller font than its neighbours. To give the segments a common size, set `strip_uniform_size = true`: every auto-sized segment then uses the smallest size any of them needs. A segment that sets an absolute `size` is left alone.

If the first segment is a different kind of panel (a counts panel with more lines, say), add `strip_uniform_skip_first = true`. Segment 1 then keeps its own size and does not pull the others down.

## Fonts

The binary embeds Source Sans 3 (proportional) and Source Code Pro (monospaced), each in Regular, Bold, Italic and Bold Italic. Both families are under the SIL Open Font License. The license texts are `assets/fonts/LICENSE-SourceSans3.md` and `assets/fonts/LICENSE-SourceCodePro.md`.

Missing glyphs fall back to the other embedded family, then to a visible placeholder. Custom font paths replace individual faces; missing faces fall back to synthetic bold or italic.

Keys receive OpenDeck titles, so inline styling on a key is flattened to plain text. Dials use an image when a value needs colour, a background, a bar, an explicit size, spans or other style fields; otherwise they use a title.

## Troubleshooting

Logs go to standard error and to `<status file>.log`. The default level is `info`; use `RUST_LOG=debug` for more detail. Every CLI command also appends to that log.

`opendeck-strip doctor` is the first check. It reports the status path and its source, whether the file parses, whether a running OpenDeck process resolves the same status path, which plugin directory is installed, whether the plugin binary is executable, hidraw access, and the AKP05 version. It also reports whether an AKP05 raw socket exists (a warning when `strip_mode` is `raw` and none does). It also prints the last full-strip line from the AKP05 log, which carries the device firmware and the chosen renderer (`ImageSlot(n)`, or `Bgpic` when the override is set). AKP05 versions below 0.12.0 are a failure for continuous mode and only a warning when `strip_mode` is `tiles`. The command exits non-zero on failures; warnings alone leave the exit code at zero.

Nothing appears on the device:

- Run `opendeck-strip install` and restart OpenDeck after every binary update.
- Make sure the action is placed on the key or dial you are writing to.
- Check `where`. OpenDeck started from a desktop launcher does not see variables from your shell. If you use `OPENDECK_STATUS_FILE`, set it for OpenDeck as well, for example with `flatpak override --user --env=OPENDECK_STATUS_FILE=/run/user/1000/opendeck-status.json me.amankhanna.opendeck`.
- Run `doctor` and look at the log.

hidraw is not readable: the device plugin needs the udev rules shipped as `40-opendeck-akp05.rules` by the AKP05 plugin. Copy them to `/etc/udev/rules.d/`, run `sudo udevadm control --reload-rules`, then unplug and replug the device. `opendeck-strip` itself only talks to OpenDeck over the local WebSocket.

Flatpak: the plugin directory is under `~/.var/app/me.amankhanna.opendeck/config/opendeck/plugins`. `XDG_RUNTIME_DIR` is shared with the sandbox, so the default status path works. A custom `OPENDECK_STATUS_FILE` must be passed into the sandbox as described above.

The strip shows corruption above a certain image size: run `size-test` to find the limit on your unit, then lower `max_jpeg_bytes`. In continuous mode an over-budget render degrades to text only: run backgrounds and bars are dropped. The reported ~10 KB cap has not been confirmed on the N4E.

A malformed status file is kept: the plugin logs a warning and keeps the last good state, while `set` refuses to overwrite it. `clear` without arguments is the exception; it resets the file to `{}` so a bad hand edit can be recovered.

Continuous mode shows seams or gaps: the AKP05 plugin is older than 0.12.0 or the N4E firmware is below 02.014, so the full-width slot is off. `doctor` prints the AKP05 log line with the firmware. Update the plugin or firmware, or use `tiles`. As a last resort on an untested deck, see the `bgpic` override in [Strip modes and requirements](#strip-modes-and-requirements); it bypasses the firmware check.

## Safety

Never write the boot logo or any ROM data. The device SDK warns that this can brick the device. This tool has no such command; it only sends image and title events through OpenDeck.

## Development

The crate is split into `cli`, `status`, `plugin`, `render` (`canvas`, `fonts`, `layout`, `markup`), `encode`, `sizetest`, `install`, `doctor`, `config`, `color` and `logging`.

Tests:

```sh
cargo test
UPDATE_GOLDEN=1 cargo test
```

Golden images live in `tests/golden/`. The mock OpenDeck server is in `tests/mock_server.rs`. Manual device checks are in [docs/on-device-checklist.md](docs/on-device-checklist.md). The status format reference is in [docs/status-format.md](docs/status-format.md).

The plugin installs as `local.statusdisplay.sdPlugin`; its manifest plugin UUID is `local.statusdisplay` and its action UUID is `local.statusdisplay.slot`. OpenDeck starts it with `-port`, `-pluginUUID`, `-registerEvent` and `-info`; the binary connects to `ws://127.0.0.1:<port>` and registers. It handles `willAppear`, `willDisappear`, `keyDown`, `dialDown` and `touchTap`, and sends `setTitle` and `setFeedback` events.

## License

MIT, see [LICENSE](LICENSE). The embedded fonts are under the SIL Open Font License; see the Fonts section.
