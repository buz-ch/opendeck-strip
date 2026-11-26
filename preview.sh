#!/bin/sh
# Render the preview images and preview/index.html with opendeck-strip (no device).
#   ./preview.sh          the 29 examples of tests/zjse/examples.toml into preview/
#   ./preview.sh NAME     one of them, as preview/adhoc.png
# Each example names a scenario document (tests/zjse/scenarios/) and its
# `zjse_*` keys; `cargo run --example zjse_preview -- NAME` renders the strip
# string with this repository's renderer. Images are the 800x128 raw frame
# (opendeck-strip raw-string --out). Compact output is one slot, so it is shown
# as the first 200 px segment of an otherwise empty frame.
BIN=${OPENDECK_STRIP:-target/release/opendeck-strip}
cd "$(dirname "$0")" && mkdir -p preview
DEF=preview/default.html.tmp
CUS=preview/custom.html.tmp

esc() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

preview() { # example-name: the strip string on stdout
  cargo run -q --example zjse_preview -- "$1"
}

render() { # example-name
  name=$1
  s=$(preview "$name") || exit 1
  # The rendered string is complete; the raw-string command must not read the
  # user's own config (word_wrap would change every example).
  OPENDECK_STRIP_CONFIG=/dev/null "$BIN" raw-string "$s" --out "preview/$name.png" >/dev/null || exit 1
  echo "preview/$name.png"
}

# example default|custom NAME "Title" "Description" SCENARIO
# The keys come from the manifest, not from this script.
example() {
  kind=$1; name=$2; title=$3; desc=$4; scen=$5
  render "$name"
  if [ "$kind" = default ]; then
    out=$DEF; badge='<span class="badge def">Default</span>'
  else
    out=$CUS; badge='<span class="badge cus">Custom</span>'
  fi
  {
    printf '<section id="%s">\n<h3>%s %s</h3>\n<p>%s</p>\n' "$name" "$badge" "$title" "$desc"
    printf '<p class="meta">Example <code>%s</code>, scenario <code>%s</code>' "$name" "$scen"
    keys=$(cargo run -q --example zjse_preview -- --config "$name") || exit 1
    if [ "$kind" = default ]; then
      [ -z "$keys" ] && printf ', default templates, no configuration' || printf ', default templates; only the mode and the slot are set'
    fi
    printf '</p>\n'
    printf '%s\n' "$keys" | while IFS= read -r kv; do
      [ -n "$kv" ] && printf '<pre>%s</pre>\n' "$(printf '%s' "$kv" | esc)"
    done
    printf '<img src="%s.png" width="800" height="128" alt="%s">\n</section>\n' "$name" "$title"
  } >> "$out"
}

subhead() { # default|custom "Heading" "text"
  if [ "$1" = default ]; then out=$DEF; else out=$CUS; fi
  printf '<h4>%s</h4>\n<p class="lead">%s</p>\n' "$2" "$3" >> "$out"
}

if [ $# -gt 0 ]; then render "$1"; exit 0; fi

: > "$DEF"
: > "$CUS"

# ---- Default templates: no template settings ----
subhead default "Strip" "The whole touch strip, the default mode."
example default default-mixed "Mixed: two blocked, one failed, one working, one done" "Five agents. Segment 1 counts them; segments 2 and 3 are the two that wait longest, segment 4 summarises the other three." mixed
example default default-agents "Three agents" "Claude waits for approval, Codex works (60%), OpenCode has finished." agents
example default default-busy "Many panes" "More than three active panes: two are shown, the rest fold into +N more, headed by the most urgent hidden state." busy
example default default-one-blocked "One pane" "A single blocked pane." one-blocked
example default default-tasks "One agent with background tasks" "By default the message is shown, not the task count." tasks
example default default-idle "Nothing active" "The counts panel stays, all grey, and the other segments say that all is clear." idle
example default safe-text "Untrusted text" "A pane whose name and message contain markup, a segment separator and an escape code. They are shown as plain text and cannot add segments." hostile

subhead default "One slot (compact mode)" "<code>zjse_mode</code> and <code>zjse_slot</code> are set; the template is the default. The slot is drawn as the first 200 px of the frame; on the device it is the one dial or key you chose. It shows the most urgent state with its count, and the top pane."
example default default-compact-mixed "Mixed" "Two panes wait for you: the slot shows the orange marker, 2, and the pane that has waited longest." mixed
example default default-compact-agents "Three agents" "Claude waits, so the slot shows it." agents
example default default-compact-busy "Many panes" "Only the most urgent pane is named; the count covers all panes in that state." busy
example default default-compact-one-blocked "One pane" "A single blocked pane." one-blocked
example default default-compact-tasks "Working pane" "No waiting pane, so the top state is working." tasks
example default default-compact-idle "Nothing active" "The slot stays blank (empty idle template). With <code>zjse_blank_when_idle</code> it is cleared instead." idle

# ---- Custom templates ----
subhead custom "Strip" "Templates for the touch strip."
example custom idle-text "Idle text" "<code>zjse_idle_template</code> replaces the whole output when nothing is active." idle
example custom tasks "Background tasks" "Child records show up as <code>{tasks}</code>." tasks
example custom age "Age" "<code>{age}</code> is the time since the pane last reported." agents
example custom custom-layout "Counts and entries re-templated" "A one-line summary on the left, a marker before each name." mixed
example custom message-first "Message first" "The request is the headline, the agent a grey caption." agents
example custom big-marker "Big marker" "A large coloured marker and the name. <code>[s=N]</code> scales text between 0.5 and 3." mixed
example custom progress "Progress first" "Working panes show their percentage, others their state." agents
example custom kind "State and kind" "The raw protocol values, <code>{state}</code> and <code>{kind}</code>, for people who want them." mixed
example custom where "Where is it?" "Tab and pane ids, to find the pane that needs you." mixed
example custom more-segment "The +N more segment" "<code>zjse_more_template</code>, here with the marker and the number of hidden panes." busy
subhead custom "The lines layout" "<code>zjse_layout = \"lines\"</code>: one full-width segment whose rows are tab-separated cells. The counts template is the left gutter, and each entry template line is one column."
example custom lines-agents "Three agents on three rows" "The default templates as a grid: marker and count, name, state, message." agents
example custom lines-busy "Six agents on five rows" "Five is the limit; the last row folds the hidden panes into the more template." busy
subhead custom "One slot (compact mode)" "<code>zjse_compact_template</code> and <code>zjse_idle_template</code>, with the mode and the slot set."
example custom compact-custom "Compact with age" "<code>zjse_compact_template</code> with the age." agents
example custom compact-ratio "Compact: ratio" "Waiting panes out of all active ones." mixed
example custom compact-message "Compact: message" "The request itself in the slot." agents
example custom compact-idle "Compact: idle text" "What the slot shows when nothing is active." idle

ndef=$(grep -c '<section' "$DEF")
ncus=$(grep -c '<section' "$CUS")

{
cat <<'HEAD'
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>opendeck-strip previews: the zj-status-export strip</title>
<style>
  :root { --bg: #fff; --fg: #1d2127; --muted: #5b6573; --card: #f4f6f8; --line: #d8dde3; --link: #0b62c4; }
  @media (prefers-color-scheme: dark) {
    :root { --bg: #14171b; --fg: #e6e9ed; --muted: #9aa4b1; --card: #1d2228; --line: #2e353d; --link: #6cb2ff; }
  }
  body { margin: 0; padding: 24px 16px 64px; background: var(--bg); color: var(--fg);
         font: 16px/1.5 system-ui, sans-serif; }
  main { max-width: 860px; margin: 0 auto; }
  h1 { margin: 0 0 4px; font-size: 1.6rem; }
  h2 { margin: 40px 0 4px; padding-top: 8px; border-top: 2px solid var(--line); font-size: 1.3rem; }
  h2 small { color: var(--muted); font-weight: 400; font-size: 0.8em; }
  h3 { margin: 0 0 4px; font-size: 1.05rem; }
  h4 { margin: 28px 0 2px; font-size: 1.1rem; }
  p { margin: 0 0 8px; }
  nav { margin: 12px 0; }
  nav a { color: var(--link); }
  .lead, .meta { color: var(--muted); }
  .legend { padding: 8px 14px; background: var(--card); border: 1px solid var(--line); border-radius: 8px; margin: 12px 0; }
  .legend p { margin: 4px 0; }
  .badge { display: inline-block; padding: 1px 8px; border-radius: 10px; font-size: 0.75rem; font-weight: 600; vertical-align: 2px; color: #fff; }
  .badge.def { background: #1f7a3f; }
  .badge.cus { background: #b25b00; }
  section { margin-top: 16px; padding: 16px; background: var(--card); border: 1px solid var(--line); border-radius: 8px; }
  pre { margin: 6px 0; padding: 8px 10px; overflow-x: auto; background: var(--bg); border: 1px solid var(--line);
        border-radius: 6px; font-size: 0.85rem; }
  code { font-size: 0.9em; }
  img { display: block; width: 100%; max-width: 800px; height: auto; margin-top: 10px; background: #000; border-radius: 4px; }
</style>
</head>
<body>
<main>
<h1>opendeck-strip previews: the zj-status-export strip</h1>
<p class="lead">What the N4E touch strip shows for each example, rendered by opendeck-strip from the
zj-status-export status document, without a device (<code>./preview.sh</code>). Each image is the 800x128 raw frame.</p>
<nav><a href="#default">Default templates</a> &middot; <a href="#custom">Custom templates</a></nav>
<div class="legend">
  <p><span class="badge def">Default</span> built-in templates, <strong>no template settings</strong>. Compact mode only sets <code>zjse_mode</code> and <code>zjse_slot</code>.</p>
  <p><span class="badge cus">Custom</span> one or more <code>zjse_*_template</code> keys of <code>config.toml</code>, shown in the boxes under the title.</p>
</div>
HEAD
printf '<h2 id="default">Default templates <small>%s examples</small></h2>\n' "$ndef"
printf '<p class="lead">What you get without any template setting: first the strip, then the one-slot compact mode.</p>\n'
cat "$DEF"
printf '<h2 id="custom">Custom templates <small>%s examples</small></h2>\n' "$ncus"
printf '<p class="lead">The same documents laid out with <code>zjse_entry_template</code>, <code>zjse_counts_template</code>, <code>zjse_more_template</code>, <code>zjse_compact_template</code> and <code>zjse_idle_template</code>.</p>\n'
cat "$CUS"
printf '</main>\n</body>\n</html>\n'
} > preview/index.html
rm -f "$DEF" "$CUS"
echo preview/index.html
