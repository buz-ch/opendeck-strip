//! The `zj-strip-bridge` document consumer: a version 2 document in, the
//! text of the strip or of one compact slot out.
//!
//! The plugin provides data only; this module owns the presentation: the
//! templates, the deck selection, the `+N more` segment and the idle text.
//! See `docs/status-format.md` and `zj-strip-bridge-data-only-spec.md`.

pub mod document;
pub mod examples;
pub mod template;

use crate::config::Config;
use document::Document;
use template::{Ctx, Template};

/// Default `zjsb_entry_template`.
pub const DEFAULT_ENTRY: &str =
    "[b]{app|title:14}[/b]\\n[c={color}]{state_label}[/c]\\n{progress_pct|msg:22}";
/// Default `zjsb_counts_template`.
pub const DEFAULT_COUNTS: &str = "[c={needs_you_color}]● {needs_you}[/c]\\n[c={failed_color}]✗ {failed}[/c]\\n[c={done_color}]✓ {done}[/c]\\n[c={working_color}]» {working}[/c]";
/// Default `zjsb_more_template`.
pub const DEFAULT_MORE: &str = "[b]+{hidden} more[/b]\\n[c={color}]{state_label}[/c]\\nsee inbox";
/// Default `zjsb_compact_template`.
pub const DEFAULT_COMPACT: &str = "[c={color}]{marker} {count}[/c]\\n{app|title:8}";
/// Default `zjsb_idle_template`: empty, the counts panel plus "All clear".
pub const DEFAULT_IDLE: &str = "";
/// Segments 2 to 4 of the strip when nothing is active and no idle template
/// is set; the counts panel stays in segment 1.
pub const DEFAULT_IDLE_STRIP: &str = "[c=#66cc88][b]All clear[/b][/c]\\n[c=grey]no failures[/c]\\n[c=grey]nothing running[/c]|[c=grey]waiting for agents…[/c]|";
/// The whole deck string while Zellij's attention feature is switched off.
pub const ATTENTION_OFF: &str = "[c=grey]Zellij attention is off[/c]";

/// Fields every template may use: the counts of the deck entries.
pub const COUNT_FIELDS: &[&str] = &[
    "needs_you",
    "failed",
    "done",
    "working",
    "total",
    "needs_you_color",
    "failed_color",
    "done_color",
    "working_color",
];
/// Fields of one entry (`zjsb_entry_template`, `zjsb_compact_template`).
pub const ENTRY_FIELDS: &[&str] = &[
    "app",
    "title",
    "state",
    "state_label",
    "kind",
    "msg",
    "progress_pct",
    "tasks",
    "marker",
    "color",
    "count",
    "tab",
    "pane",
    "age",
    "session",
    "tab_name",
];
/// Fields of the `+N more` segment, besides [`COUNT_FIELDS`].
pub const MORE_FIELDS: &[&str] = &["hidden", "state_label", "color", "marker"];

/// Entries shown before the rest fold into `+N more`.
const SHOWN: usize = 3;
/// A segment must stay small: lines are dropped past this many characters.
const MAX_SEGMENT: usize = 400;
/// Rows the `lines` layout shows at most; a legibility limit, not a config key.
const MAX_ROWS: usize = 5;
/// The first entry cell of the idle row in the `lines` layout.
const ALL_CLEAR: &str = "[c=#66cc88][b]All clear[/b][/c]";

/// Deck priority, lowest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Idle,
    Working,
    Done,
    Error,
    Blocked,
}

/// Why an entry is blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "permission" => Self::Permission,
            "question" => Self::Question,
            "auth" => Self::Auth,
            _ => return None,
        })
    }
}

/// One deck entry of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tab: usize,
    pub pane: u32,
    pub app: Option<String>,
    /// The pane title.
    pub title: String,
    pub state: State,
    pub kind: Option<Kind>,
    pub msg: Option<String>,
    pub progress: Option<u8>,
    /// `local.tasks`.
    pub children: usize,
    /// `since_ms`.
    pub updated_ms: u64,
    pub session: String,
    /// Empty when the tab has no name.
    pub tab_name: String,
}

impl Entry {
    fn from_record(r: &document::Record, session: &str) -> Self {
        Self {
            tab: r.tab.id,
            pane: r.pane.id,
            app: r.app.clone(),
            title: r.pane.title.clone(),
            state: parse_state(&r.state),
            kind: r.kind.as_deref().and_then(Kind::parse),
            msg: r.msg.clone(),
            progress: r.progress,
            children: r.local.as_ref().map_or(0, |l| l.tasks),
            updated_ms: r.since_ms,
            session: session.to_owned(),
            tab_name: r.tab.name.clone().unwrap_or_default(),
        }
    }
}

/// An unknown state counts as idle.
fn parse_state(s: &str) -> State {
    match s {
        "working" => State::Working,
        "done" => State::Done,
        "error" => State::Error,
        "blocked" => State::Blocked,
        _ => State::Idle,
    }
}

/// The ordering behind [`entries`]: state, then blocked oldest first and every
/// other state newest first, then tab, pane, title and session.
pub fn cmp_entries(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.state
        .cmp(&a.state)
        .then_with(|| {
            if a.state == State::Blocked {
                a.updated_ms.cmp(&b.updated_ms)
            } else {
                b.updated_ms.cmp(&a.updated_ms)
            }
        })
        .then(a.tab.cmp(&b.tab))
        .then(a.pane.cmp(&b.pane))
        .then_with(|| a.title.cmp(&b.title))
        .then_with(|| a.session.cmp(&b.session))
}

/// The deck entries in priority order.
///
/// The document is session-major: each session lists its on-deck records
/// first, and sessions follow event order. Collecting those in document order
/// and stable-sorting is the plugin's own order, ties included.
pub fn entries(doc: &Document) -> Vec<Entry> {
    let mut v: Vec<Entry> = doc
        .sessions
        .iter()
        .flat_map(|s| {
            s.panes
                .iter()
                .filter(|r| r.on_deck)
                .map(move |r| Entry::from_record(r, &s.name))
        })
        .collect();
    v.sort_by(cmp_entries);
    v
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub needs_you: usize,
    pub failed: usize,
    pub done: usize,
    pub working: usize,
}

pub fn counts(entries: &[Entry]) -> Counts {
    let mut c = Counts::default();
    for e in entries {
        match e.state {
            State::Blocked => c.needs_you += 1,
            State::Error => c.failed += 1,
            State::Done => c.done += 1,
            State::Working => c.working += 1,
            State::Idle => {}
        }
    }
    c
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Four segments: counts, entries, `+N more`.
    Strip,
    /// One slot.
    Compact,
}

/// How the strip output is written: `segments` (the default) or tab-separated
/// `lines`; see `docs/lines-layout-design.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    Segments,
    Lines,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    pub entry: Template,
    pub counts: Template,
    pub more: Template,
    pub compact: Template,
    /// Shown instead of the output when nothing is active.
    pub idle: Template,
    pub mode: Mode,
    pub layout: Layout,
    /// Compact only: the slot the text goes to.
    pub slot: Option<String>,
    /// Clear the deck when nothing is active and `idle` is empty.
    pub blank_when_idle: bool,
}

/// A template with the count fields.
fn with_counts(fields: &[&'static str]) -> Vec<&'static str> {
    fields.iter().chain(COUNT_FIELDS).copied().collect()
}

impl Default for Options {
    fn default() -> Self {
        let must = |text: &str, allowed: &[&str]| {
            Template::parse(text, allowed).expect("built-in template is valid")
        };
        Self {
            entry: must(DEFAULT_ENTRY, &with_counts(ENTRY_FIELDS)),
            counts: must(DEFAULT_COUNTS, COUNT_FIELDS),
            more: must(DEFAULT_MORE, &with_counts(MORE_FIELDS)),
            compact: must(DEFAULT_COMPACT, &with_counts(ENTRY_FIELDS)),
            idle: Template::default(),
            mode: Mode::Strip,
            layout: Layout::Segments,
            slot: None,
            blank_when_idle: false,
        }
    }
}

impl Options {
    /// The renderer keys of the config. The loader already reported a template
    /// that does not parse; this one keeps the default.
    pub fn from_config(c: &Config) -> Self {
        let d = Self::default();
        let keep = |text: &str, allowed: &[&str], fallback: &Template| {
            Template::parse(text, allowed).unwrap_or_else(|_| fallback.clone())
        };
        Self {
            entry: keep(&c.zjsb_entry_template, &with_counts(ENTRY_FIELDS), &d.entry),
            counts: keep(&c.zjsb_counts_template, COUNT_FIELDS, &d.counts),
            more: keep(&c.zjsb_more_template, &with_counts(MORE_FIELDS), &d.more),
            compact: keep(
                &c.zjsb_compact_template,
                &with_counts(ENTRY_FIELDS),
                &d.compact,
            ),
            idle: keep(&c.zjsb_idle_template, COUNT_FIELDS, &d.idle),
            mode: if c.zjsb_mode == "compact" {
                Mode::Compact
            } else {
                Mode::Strip
            },
            layout: if c.zjsb_layout == "lines" {
                Layout::Lines
            } else {
                Layout::Segments
            },
            slot: (c.zjsb_mode == "compact").then(|| c.zjsb_slot.clone()),
            blank_when_idle: c.zjsb_blank_when_idle,
        }
    }
}

fn colour(s: State) -> &'static str {
    match s {
        State::Blocked => "orange",
        State::Error => "red",
        State::Done => "green",
        State::Working => "#55ccff",
        State::Idle => "grey",
    }
}

fn marker(s: State) -> &'static str {
    match s {
        State::Blocked => "●",
        State::Error => "✗",
        State::Done => "✓",
        _ => "»",
    }
}

fn state_label(s: State, kind: Option<Kind>) -> &'static str {
    match (s, kind) {
        (State::Blocked, Some(Kind::Permission)) => "needs approval",
        (State::Blocked, Some(Kind::Question)) => "has a question",
        (State::Blocked, Some(Kind::Auth)) => "needs login",
        (State::Blocked, None) => "needs you",
        (State::Error, _) => "failed",
        (State::Done, _) => "done",
        (State::Working, _) => "working",
        (State::Idle, _) => "",
    }
}

fn state_name(s: State) -> &'static str {
    match s {
        State::Blocked => "blocked",
        State::Error => "error",
        State::Done => "done",
        State::Working => "working",
        State::Idle => "idle",
    }
}

fn count_of(c: &Counts, s: State) -> usize {
    match s {
        State::Blocked => c.needs_you,
        State::Error => c.failed,
        State::Done => c.done,
        _ => c.working,
    }
}

/// `45s`, `3m`, `2h`, `4d`; empty when either time is unknown.
pub fn age(now_ms: u64, then_ms: u64) -> String {
    if now_ms == 0 || then_ms == 0 || then_ms > now_ms {
        return String::new();
    }
    let s = (now_ms - then_ms) / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

pub fn counts_ctx(c: &Counts) -> Ctx {
    let mut m = Ctx::new();
    let mut put = |name: &'static str, cname: &'static str, n: usize, col: &'static str| {
        m.insert(name, n.to_string());
        m.insert(cname, (if n == 0 { "grey" } else { col }).to_string());
    };
    put("needs_you", "needs_you_color", c.needs_you, "orange");
    put("failed", "failed_color", c.failed, "red");
    put("done", "done_color", c.done, "green");
    put("working", "working_color", c.working, "#55ccff");
    m.insert(
        "total",
        (c.needs_you + c.failed + c.done + c.working).to_string(),
    );
    m
}

pub fn entry_ctx(e: &Entry, c: &Counts, now_ms: u64) -> Ctx {
    let mut m = counts_ctx(c);
    m.insert("app", e.app.clone().unwrap_or_default());
    m.insert("title", e.title.clone());
    m.insert("state", state_name(e.state).into());
    m.insert("state_label", state_label(e.state, e.kind).into());
    m.insert(
        "kind",
        e.kind.map(|k| k.as_str().to_string()).unwrap_or_default(),
    );
    m.insert("msg", e.msg.clone().unwrap_or_default());
    m.insert(
        "progress_pct",
        match (e.state, e.progress) {
            (State::Working, Some(p)) => format!("{p}%"),
            _ => String::new(),
        },
    );
    m.insert(
        "tasks",
        match e.children {
            0 => String::new(),
            1 => "1 task".into(),
            n => format!("{n} tasks"),
        },
    );
    m.insert("marker", marker(e.state).into());
    m.insert("color", colour(e.state).into());
    m.insert("count", count_of(c, e.state).to_string());
    m.insert("tab", e.tab.to_string());
    m.insert("pane", e.pane.to_string());
    m.insert("age", age(now_ms, e.updated_ms));
    m.insert("session", e.session.clone());
    m.insert("tab_name", e.tab_name.clone());
    m
}

/// Split on the literal two-character `\n` line separator, skipping an
/// escaped backslash pair so a value's own `\\n` is not a break.
fn split_lines(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let (mut lines, mut start, mut i) = (Vec::new(), 0, 0);
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'n' {
                lines.push(&s[start..i]);
                start = i + 2;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    lines.push(&s[start..]);
    lines
}

/// Drop trailing lines past the segment budget.
fn cap(s: String) -> String {
    let mut out = String::new();
    for (i, line) in split_lines(&s).into_iter().enumerate() {
        if i > 0 && out.chars().count() + line.chars().count() + 2 > MAX_SEGMENT {
            break;
        }
        if i > 0 {
            out.push_str("\\n");
        }
        out.push_str(line);
    }
    out
}

/// The context of the `+N more` row: the hidden entries and the counts.
fn more_ctx(hidden: &[Entry], counts: &Counts) -> Ctx {
    let top = hidden.iter().map(|e| e.state).max().unwrap_or(State::Idle);
    let mut m = counts_ctx(counts);
    m.insert("hidden", hidden.len().to_string());
    m.insert(
        "state_label",
        state_label(top, None).replace("needs you", "need you"),
    );
    m.insert("color", colour(top).into());
    m.insert("marker", marker(top).into());
    m
}

/// The strip string: counts, up to three entries, or two plus `+N more`.
fn strip(entries: &[Entry], counts: &Counts, o: &Options, now_ms: u64) -> String {
    if entries.is_empty() && !o.idle.is_empty() {
        return cap(o.idle.render(&counts_ctx(counts)));
    }
    let mut segs = vec![cap(o.counts.render(&counts_ctx(counts)))];
    if entries.is_empty() {
        return format!("{}|{}", segs[0], DEFAULT_IDLE_STRIP);
    }
    let show = |e: &Entry| cap(o.entry.render(&entry_ctx(e, counts, now_ms)));
    if entries.len() <= SHOWN {
        segs.extend(entries.iter().map(show));
    } else {
        segs.extend(entries[..SHOWN - 1].iter().map(show));
        let m = more_ctx(&entries[SHOWN - 1..], counts);
        segs.push(cap(o.more.render(&m)));
    }
    segs.join("|")
}

/// One row of the `lines` layout: cells joined by a literal `\t`. Trailing
/// cells past the row budget are dropped whole; the first cell always stays.
fn join_row(cells: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut chars = 0;
    for cell in cells {
        let sep = usize::from(!kept.is_empty());
        let len = cell.chars().count();
        if !kept.is_empty() && chars + sep + len > MAX_SEGMENT {
            break;
        }
        chars += sep + len;
        kept.push(cell);
    }
    kept.join("\\t")
}

/// The counter gutter: one cell per counts line, at most [`MAX_ROWS`]. Empty
/// when the template has no lines, so there is no gutter column.
fn gutter(counts: &Counts, o: &Options) -> Vec<String> {
    if o.counts.is_empty() {
        return Vec::new();
    }
    o.counts
        .render_cells(&counts_ctx(counts))
        .into_iter()
        .take(MAX_ROWS)
        .collect()
}

/// The entry side of the `lines` layout: one row of cells per entry, or the
/// `All clear` row when nothing is active, or four rows plus `+N more`.
fn entry_rows(entries: &[Entry], counts: &Counts, o: &Options, now_ms: u64) -> Vec<Vec<String>> {
    let cells = |e: &Entry| o.entry.render_cells(&entry_ctx(e, counts, now_ms));
    if entries.is_empty() {
        return vec![vec![ALL_CLEAR.to_owned()]];
    }
    if entries.len() <= MAX_ROWS {
        return entries.iter().map(cells).collect();
    }
    let mut rows: Vec<Vec<String>> = entries[..MAX_ROWS - 1].iter().map(cells).collect();
    rows.push(
        o.more
            .render_cells(&more_ctx(&entries[MAX_ROWS - 1..], counts)),
    );
    rows
}

/// The `lines` layout: a grid of `[gutter cell] + entry cells` rows, joined by
/// a literal `\n`.
fn lines(entries: &[Entry], counts: &Counts, o: &Options, now_ms: u64) -> String {
    if entries.is_empty() && !o.idle.is_empty() {
        // As in `segments`, a line whose placeholders are all empty is not a row.
        return split_lines(&o.idle.render(&counts_ctx(counts)))
            .into_iter()
            .take(MAX_ROWS)
            .collect::<Vec<_>>()
            .join("\\n");
    }
    let gutter_cells = gutter(counts, o);
    let entry_rows = entry_rows(entries, counts, o, now_ms);
    let has_gutter = !gutter_cells.is_empty();
    let rows = MAX_ROWS.min(gutter_cells.len().max(entry_rows.len()));
    (0..rows)
        .map(|i| {
            let mut cells = Vec::new();
            if has_gutter {
                cells.push(gutter_cells.get(i).cloned().unwrap_or_default());
            }
            match entry_rows.get(i) {
                Some(entry) => cells.extend(entry.iter().cloned()),
                // A row without an entry keeps the gutter column and one blank
                // cell, so the entry columns stay in place.
                None if has_gutter => cells.push(String::new()),
                None => {}
            }
            join_row(&cells)
        })
        .collect::<Vec<_>>()
        .join("\\n")
}

/// The value of one compact slot: the first entry, else the idle template.
fn compact(entries: &[Entry], counts: &Counts, o: &Options, now_ms: u64) -> String {
    match entries.first() {
        Some(top) => cap(o.compact.render(&entry_ctx(top, counts, now_ms))),
        None => cap(o.idle.render(&counts_ctx(counts))),
    }
}

/// The text for the strip (`Mode::Strip`) or for `Options::slot` (`Mode::Compact`).
/// `None` clears it: nothing is active, there is no idle template and
/// `blank_when_idle` is set.
pub fn render(doc: &Document, o: &Options, now_ms: u64) -> Option<String> {
    if !doc.attention_enabled {
        return Some(ATTENTION_OFF.to_owned());
    }
    let entries = entries(doc);
    if entries.is_empty() && o.blank_when_idle && o.idle.is_empty() {
        return None;
    }
    let c = counts(&entries);
    Some(match (o.mode, o.layout) {
        (Mode::Strip, Layout::Lines) => lines(&entries, &c, o, now_ms),
        (Mode::Strip, Layout::Segments) => strip(&entries, &c, o, now_ms),
        (Mode::Compact, _) => compact(&entries, &c, o, now_ms),
    })
}

#[cfg(test)]
mod tests;
