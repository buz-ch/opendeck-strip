# On-device checklist

Manual checks for an N4E running `opendeck-strip` through OpenDeck. Run them after a fresh install and after any change to the renderer, the encoding path or the AKP05 plugin.

## Before starting

Record the environment for every run. Results without these details cannot be compared later.

- Device firmware version, from the device label or OpenDeck's device page.
- OpenDeck version.
- AKP05 plugin version; it must be v0.12.0 or newer for continuous mode.
- N4E firmware; continuous mode needs 02.014 or newer. `opendeck-strip doctor` prints the firmware from the AKP05 log line.
- `opendeck-strip --version`, and the output of `opendeck-strip doctor`.
- Strip mode: `opendeck-strip get` or `opendeck-strip set strip_mode continuous`.

Useful setup commands:

```sh
opendeck-strip where
opendeck-strip doctor
opendeck-strip set labels 1
```

Keep a copy of the status file before a destructive check. `clear` without arguments removes the settings too.

Known unverified behaviour:

- AKP05 v0.12 enables the N4E full-width slot from firmware 02.014. Other units and firmware versions may differ; `doctor` reports the line the plugin logged.
- The reported cap of about 10 KB on the device image is unconfirmed on the N4E. The default budget is 9000 bytes.

## Checks

### 1. Test-pattern seams

Goal: confirm that the four images form one picture without missing, duplicated or shifted columns, and that tile joins land in the physical dead bands.

Local reference:

```sh
opendeck-strip strip-preview test seam-continuous.png
opendeck-strip strip-preview test seam-tiles.png --tiles --simulate-device
```

The preview contains a gradient, a diagonal line and red edge markers at 0, 200, 400, 600 and 799 in continuous mode, and at every tile boundary in tiles mode.

Continuous mode is an 800x100 picture centred in the 800x128 strip, so there are 14 px black bands above and below. The preview shows them too. On firmware that supports the full-width slot, the device plugin uses it for every encoder image; tiles mode then loses its physical dead bands.

Device check: show the same pattern on the device, then clear it:

```sh
opendeck-strip strip test --seconds 30
```

The diagonal should cross the 800x100 picture without a break at 200, 400 or 600, and the red markers should sit exactly on the slice boundaries, 14 px in from the physical top and bottom. The black bands above and below are expected. Add `--tiles` to `strip-preview` for the tiles reference; `strip test` follows `strip_mode` from the status file and config.

For a text ruler, use `set strip "$(printf '0123456789%.0s' $(seq 8))"`. In tiles mode, a single unsegmented string may be hidden by the physical dead bands where a glyph crosses a join; that is expected. Use `set strip 'L|C1|C2|R'` to keep each part inside one tile, and check that each part is centred in its tile.

Record: mode, whether a seam is visible, at which quarter or tile join, whether columns are duplicated or lost, and whether the line stays at one height across joins. Attach a photo where possible.

### 2. Legibility at small sizes

Goal: find the smallest size that is still readable at the 1:1 continuous rendering.

Dial checks:

```sh
opendeck-strip clear strip
opendeck-strip set dial1 'ABCDEF 012345'
opendeck-strip set dial2 '{"text":"MIN 8","size":8}'
opendeck-strip set dial3 '{"text":"MIN 10","size":10}'
opendeck-strip set dial4 '{"text":"MIN 12","size":12}'
```

Strip checks:

```sh
opendeck-strip set strip '{"text":"Legibility 8","size":8}'
opendeck-strip set strip '{"text":"Legibility 10","size":10}'
```

Repeat in continuous and tiles mode. Also check bold, italic, underline, the monospaced face and a coloured run on a dark background, since those use different faces and blending:

```sh
opendeck-strip set strip '[b]BOLD[/b] [i]italic[/i] [u]under[/u] [mono]mono[/mono] [c=orange]COLOUR[/c]'
```

Key titles are drawn by OpenDeck, not by this tool, so check them separately with a short and a long title.

Record: smallest readable dial size, smallest readable strip size per mode, whether anti-aliasing looks soft or broken at the limit, and whether bold or italic degrades earlier than regular. Note any glyph that disappears or falls back to a placeholder.

### 3. Image size limit

Goal: find the largest final JPEG (Joint Photographic Experts Group image) the unit displays correctly, and compare it with the default budget.

Continuous mode only:

```sh
opendeck-strip set strip_mode continuous
opendeck-strip size-test --start 4000 --step 1000 --max 20000
```

The command prints the blank floor and then one line per target. Watch the strip as the noise block grows. Corruption, missing bands, colour block artefacts or a frozen image mark the limit. Ctrl-C stops the run and clears the strip.

Record: blank floor in bytes, largest target that displayed cleanly, first target that showed corruption, and the exact symptom. Then set `max_jpeg_bytes` below the first corrupt target and repeat a styled strip render to confirm the degradation path.

Do not assume the ~10 KB figure applies. Record the measured value with the firmware version, because the cap may be firmware-specific.

### 4. Flicker and update rate

Goal: see whether a redraw burst is visible and whether the rate limit is adequate.

```sh
opendeck-strip watch --slot strip --every 1 -- date +%S
```

For a faster stream, run a shell loop:

```sh
for i in $(seq 1 60); do opendeck-strip set strip "$i"; done
```

Observe the strip, not the terminal. In continuous mode one update rewrites all four dial images. Check for whole-strip flicker, tearing at the quarter points, a blank frame before each update, and whether the dials show stale content.

Then lower the rate and compare. The plugin is started by OpenDeck, so either export `OPENDECK_STRIP_REDRAW_HZ=1` for the OpenDeck process or put `redraw_hz = 1.0` in the config and restart OpenDeck.

Record: configured `redraw_hz`, observed update cadence, severity and location of flicker, and the lowest rate that removes it. Note whether the effect depends on image content.

### 5. Key and dial titles

Goal: verify slot mapping, title fallback, labels and image-versus-title behaviour.

```sh
opendeck-strip clear strip
opendeck-strip set labels 0
opendeck-strip set key1_1 K11 key1_2 K12 key1_3 K13 key1_4 K14 key1_5 K15
opendeck-strip set key2_1 K21 key2_2 K22 key2_3 K23 key2_4 K24 key2_5 K25
opendeck-strip set dial1 'INBOX\n3'
opendeck-strip set dial2 '{"text":"STYLED","color":"#ff8800","bg":"#101820"}'
opendeck-strip set dial3 '{"text":"BAR","bar":0.6}'
```

Check that every value lands on the intended physical slot and that row and column are not swapped. A styled dial should render as an image with no action name visible. A plain dial should use OpenDeck's title. An empty key or dial should be blank; record it if the action name appears, because an empty title can fall back to the action name.

Then check labels:

```sh
opendeck-strip set labels 1
opendeck-strip clear key1_1 dial1
```

Blank slots should show `key1_1` and `dial1` while labels are on, and become blank again with `set labels 0`.

Finally check flattening and markup:

```sh
opendeck-strip set key1_1 '[b]BUILD[/b] [c=green]OK[/c]'
opendeck-strip set dial4 'CPU [s=1.6]42[/s]%'
```

Keys cannot show inline styling; they should show `BUILD OK`. The styled dial should show the size change. Multi-line titles on keys are a known weak spot; record whether the second line appears.

Record: slot-to-position map, any swapped or duplicated slot, title fallback behaviour, label behaviour, and whether the dial image is centred without letterboxing.

### 6. Unplug and replug

Goal: confirm recovery after a cable disconnect and after an OpenDeck restart.

1. Put a known strip and at least two dial values on screen.
2. Unplug the N4E from Universal Serial Bus (USB), wait a few seconds, and plug it back in.
3. Observe OpenDeck, the strip and the dials.
4. Quit OpenDeck, start it again, and observe again.
5. Read `<status file>.log` for errors around the disconnect.

Record: whether the device reappears without a restart, whether the strip and dials restore automatically, whether a stale or blank frame remains, and whether OpenDeck or the plugin needed a manual restart. Note whether the status file survived and whether the plugin logged a clean close.

### 7. Five-row lines layout

Goal: check that five tab-separated rows stay legible in `raw` and `continuous` mode, that long messages are truncated rather than dropping a column, and that the idle row keeps its gutter.

Local references:

```sh
./preview.sh
```

Compare the device with `preview/lines-busy.png` (six panes, four entries plus the more row) and `preview/lines-agents.png` (three panes).

With a running `zj-strip-bridge` deck of six active panes:

```sh
OPENDECK_STRIP_ZJSB_LAYOUT=lines opendeck-strip follow-zellij
```

The writer sets `strip_mode = "raw"`, so this is the five-row raw case. Check the gutter, the name, state and message columns, the `+N more` row, and whether the smallest row height is readable. Then force the continuous case by hand, because the writer resets the mode to `raw` on every write:

```sh
opendeck-strip set strip_mode continuous
opendeck-strip set strip '[c=orange]● 2[/c]\t[b]claude[/b]\t[c=orange]needs login[/c]\tlogin expired\n[c=red]✗ 1[/c]\t[b]codex[/b]\t[c=orange]needs approval[/c]\tapply patch\n[c=green]✓ 1[/c]\t[b]opencode[/b]\t[c=red]failed[/c]\trate limited\n[c=#55ccff]» 2[/c]\t[b]claude[/b]\t[c=green]done[/c]\t\n\t[b]+2 more[/b]\t[c=#55ccff]working[/c]\tsee inbox'
```

For a long-message check, put a long value in the last cell of one row. The last column should narrow and end in `…`; if it cannot hold `…`, it is dropped from every row and the cells that lost content end in `…`. Column 1 (the gutter) must stay.

For idle, stop all activity: the four grey counts lines stay as the gutter and row 1 shows `All clear`. With an empty `zjsb_counts_template` the output is `All clear` alone. Check that the idle row is left-aligned as a whole, because an output with no tab is not a grid.

Record: which mode is readable at five rows, whether a row height clips a glyph, whether truncation is obvious, and the idle layout with and without a counts template.

## Results

| Date | Firmware | OpenDeck | AKP05 build | Check | Mode | Result | Notes |
|---|---|---|---|---|---|---|---|
| | | | | | | | |
