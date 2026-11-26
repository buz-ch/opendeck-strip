//! The preview examples of `tests/zjse/examples.toml`: one manifest for the
//! parity test and the `zjse_preview` example. Each names a scenario document
//! and the renderer keys it overrides.
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::Config;

use super::document::Document;
use super::{Options, render};

/// The clock the scenarios were written for; keeps `{age}` stable.
pub const NOW_MS: u64 = 1_800_000_000_000;

pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/zjse")
}

#[derive(Clone, Debug)]
pub struct Example {
    pub name: String,
    pub scenario: String,
    /// `zjse_*` key to value.
    pub overrides: BTreeMap<String, toml::Value>,
}

pub fn load() -> Result<Vec<Example>, String> {
    let path = dir().join("examples.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    parsed
        .get("example")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{}: no [[example]]", path.display()))?
        .iter()
        .map(|e| {
            let table = e.as_table().ok_or("example is not a table")?;
            let get = |k: &str| {
                table
                    .get(k)
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| format!("example without a {k}"))
            };
            Ok(Example {
                name: get("name")?,
                scenario: get("scenario")?,
                overrides: table
                    .iter()
                    .filter(|(k, _)| k.as_str() != "name" && k.as_str() != "scenario")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            })
        })
        .collect()
}

impl Example {
    /// The defaults with this example's keys.
    pub fn config(&self) -> Result<Config, String> {
        let mut c = Config::default();
        for (k, v) in &self.overrides {
            apply(&mut c, k, v).map_err(|e| format!("{}: {e}", self.name))?;
        }
        Ok(c)
    }

    pub fn document(&self) -> Result<Document, String> {
        let path = dir()
            .join("scenarios")
            .join(format!("{}.json", self.scenario));
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The strip string, or `None` when the slot is cleared.
    pub fn render(&self) -> Result<Option<String>, String> {
        let o = Options::from_config(&self.config()?);
        Ok(render(&self.document()?, &o, NOW_MS))
    }

    /// The keys as TOML lines (mode and slot first), for display. A template
    /// is shown as one line, its newlines written `\\n`.
    pub fn overrides_toml(&self) -> Vec<String> {
        let rank = |k: &str| match k {
            "zjse_mode" => 0,
            "zjse_slot" => 1,
            _ => 2,
        };
        let mut keys: Vec<_> = self.overrides.iter().collect();
        keys.sort_by_key(|(k, _)| rank(k));
        keys.iter()
            .map(|(k, v)| match v.as_str() {
                Some(s) => format!(
                    "{k} = \"{}\"",
                    s.replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('\n', "\\n")
                ),
                None => format!("{k} = {v}"),
            })
            .collect()
    }
}

/// Set one renderer key; anything else is a mistake in the manifest.
fn apply(c: &mut Config, key: &str, v: &toml::Value) -> Result<(), String> {
    let text = || {
        v.as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{key} = {v} is not a non-empty string"))
    };
    match key {
        "zjse_mode" => c.zjse_mode = text()?,
        "zjse_layout" => c.zjse_layout = text()?,
        "zjse_slot" => c.zjse_slot = text()?,
        "zjse_entry_template" => c.zjse_entry_template = text()?,
        "zjse_counts_template" => c.zjse_counts_template = text()?,
        "zjse_more_template" => c.zjse_more_template = text()?,
        "zjse_compact_template" => c.zjse_compact_template = text()?,
        "zjse_idle_template" => c.zjse_idle_template = text()?,
        "zjse_blank_when_idle" => {
            c.zjse_blank_when_idle = v
                .as_bool()
                .ok_or_else(|| format!("{key} = {v} is not a boolean"))?
        }
        "zjse_min_interval_ms" => {
            c.zjse_min_interval_ms = v
                .as_integer()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{key} = {v} is not a number above 0"))?
                as u64
        }
        _ => return Err(format!("{key} is not a renderer key")),
    }
    Ok(())
}
