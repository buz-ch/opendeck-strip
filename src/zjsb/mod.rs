//! The `zj-strip-bridge` document consumer: a version 2 document in, the
//! text of the strip or of one compact slot out.
//!
//! The plugin provides data only; this module owns the presentation: the
//! templates, the deck selection, the `+N more` segment and the idle text.
//! See `docs/status-format.md` and `zj-strip-bridge-data-only-spec.md`.

pub mod template;

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
