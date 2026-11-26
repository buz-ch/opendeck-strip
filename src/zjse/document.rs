//! Serde types of the version 2 document (`zj-status-export.get`, or the `doc` of a
//! `state` line). Unknown fields are ignored, so a newer plugin can add some.
use serde::Deserialize;

pub mod de {
    pub fn yes() -> bool {
        true
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub floating: bool,
}

/// What only the pane's own session knows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Local {
    #[serde(default)]
    pub tasks: usize,
    #[serde(default)]
    pub layout: Layout,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Tab {
    #[serde(default)]
    pub id: usize,
    /// `null` when Zellij has not reported a name.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Pane {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: String,
}

/// One attention item. `app` is the program, `title` its own title; the pane
/// title is `pane.title`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub tab: Tab,
    #[serde(default)]
    pub pane: Pane,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub since_ms: u64,
    #[serde(default)]
    pub seen: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub on_deck: bool,
    /// Absent for panes of other sessions.
    #[serde(default)]
    pub local: Option<Local>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub panes: Vec<Record>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub generated_ms: u64,
    /// `false` while the plugin has no attention state, so its empty document
    /// means "not the merged view" rather than "nothing is on deck". A plugin
    /// that does not send the field is assumed to have it.
    #[serde(default = "de::yes")]
    pub synced: bool,
    /// Without the flag the document does not claim that attention is off.
    #[serde(default = "de::yes")]
    pub attention_enabled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub sessions: Vec<Session>,
}
