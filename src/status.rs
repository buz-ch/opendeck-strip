use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::color::Rgb;

pub const STATUS_FILE_NAME: &str = "opendeck-status.json";
/// A slot value: the prototype's plain string, or a style object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SlotValue {
    Text(String),
    Styled(Styles),
}

impl SlotValue {
    pub fn from_text(text: impl Into<String>) -> Self {
        SlotValue::Text(text.into())
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            SlotValue::Text(t) => Some(t),
            SlotValue::Styled(s) => s.text.as_deref(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            SlotValue::Text(t) => t.trim().is_empty(),
            SlotValue::Styled(s) => {
                s.text.as_deref().is_none_or(|text| text.trim().is_empty())
                    && s.spans.as_ref().is_none_or(|v| v.is_empty())
                    && s.bar.is_none()
                    && s.bg.is_none()
            }
        }
    }
}

/// Everything except `text` is optional. Unknown fields are ignored so a newer
/// writer does not break an older plugin.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Styles {
    pub text: Option<String>,
    pub spans: Option<Vec<SpanSpec>>,
    pub color: Option<String>,
    pub bg: Option<String>,
    pub size: Option<Value>,
    pub font: Option<String>,
    pub align: Option<String>,
    pub valign: Option<String>,
    pub bar: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpanSpec {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Option<String>,
    pub bg: Option<String>,
    pub size: Option<f32>,
    pub mono: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripMode {
    Continuous,
    Tiles,
    /// One 800x128 picture sent straight to the AKP05 raw strip socket.
    Raw,
}

impl StripMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "continuous" | "strip" => Some(Self::Continuous),
            "tiles" | "tile" => Some(Self::Tiles),
            "raw" => Some(Self::Raw),
            _ => None,
        }
    }
}

impl fmt::Display for StripMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Continuous => f.write_str("continuous"),
            Self::Tiles => f.write_str("tiles"),
            Self::Raw => f.write_str("raw"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SlotName {
    Dial(u8),
    Key { row: u8, col: u8 },
}

impl SlotName {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("dial") {
            let n: u8 = rest.parse().ok()?;
            return (1..=4).contains(&n).then_some(Self::Dial(n));
        }
        if let Some(rest) = s.strip_prefix("key") {
            let (row, col) = rest.split_once('_')?;
            let row: u8 = row.parse().ok()?;
            let col: u8 = col.parse().ok()?;
            return (row >= 1 && col >= 1).then_some(Self::Key { row, col });
        }
        None
    }

    pub fn from_event(controller: &str, row: u8, col: u8) -> Option<Self> {
        match controller {
            "Encoder" => (col < 4).then_some(Self::Dial(col + 1)),
            "Keypad" => Some(Self::Key {
                row: row + 1,
                col: col + 1,
            }),
            _ => None,
        }
    }

    pub fn as_wire_name(&self) -> String {
        match self {
            Self::Dial(n) => format!("dial{n}"),
            Self::Key { row, col } => format!("key{row}_{col}"),
        }
    }
}

impl fmt::Display for SlotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_wire_name())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicesSpec {
    pub dir: PathBuf,
    pub generation: u64,
}

/// Parse `@slices:<dir>#<generation>`.
pub fn parse_slices(s: &str) -> Option<SlicesSpec> {
    let rest = s.trim().strip_prefix("@slices:")?;
    let (dir, generation) = rest.rsplit_once('#')?;
    let generation = generation.parse().ok()?;
    Some(SlicesSpec {
        dir: PathBuf::from(dir),
        generation,
    })
}

/// The status file as a JSON object. Unknown keys are preserved on read-modify-write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    map: Map<String, Value>,
}

impl Status {
    pub fn from_json(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text).context("status file is not valid JSON")?;
        let Value::Object(map) = value else {
            bail!("status file must be a JSON object");
        };
        Ok(Self { map })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(&Value::Object(self.map.clone()))
            .unwrap_or_else(|_| "{}".to_owned())
    }

    pub fn raw(&self) -> &Map<String, Value> {
        &self.map
    }

    pub fn raw_value(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    pub fn set_raw(&mut self, key: &str, value: Value) {
        self.map.insert(key.to_owned(), value);
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.map.remove(key).is_some()
    }

    pub fn clear_all(&mut self) {
        self.map.clear();
    }

    pub fn slot(&self, name: &SlotName) -> Result<Option<SlotValue>> {
        let Some(value) = self.map.get(&name.as_wire_name()) else {
            return Ok(None);
        };
        slot_value_from_json(value).map(Some)
    }

    pub fn get_slot_str(&self, key: &str) -> Result<Option<SlotValue>> {
        let Some(value) = self.map.get(key) else {
            return Ok(None);
        };
        slot_value_from_json(value).map(Some)
    }

    pub fn set_slot(&mut self, key: &str, value: SlotValue) {
        let json = serde_json::to_value(value).unwrap_or(Value::Null);
        self.map.insert(key.to_owned(), json);
    }

    pub fn strip(&self) -> Option<&Value> {
        self.map.get("strip")
    }

    pub fn strip_text(&self) -> Option<&str> {
        self.strip().and_then(Value::as_str)
    }

    pub fn strip_active(&self) -> bool {
        match self.strip() {
            Some(Value::String(s)) => !s.trim().is_empty(),
            Some(value) => slot_value_from_json(value).is_ok_and(|slot| !slot.is_empty()),
            _ => false,
        }
    }

    pub fn strip_value(&self) -> Result<Option<SlotValue>> {
        match self.strip() {
            Some(v) => slot_value_from_json(v).map(Some),
            None => Ok(None),
        }
    }

    pub fn strip_mode(&self) -> Option<StripMode> {
        self.map
            .get("strip_mode")
            .and_then(Value::as_str)
            .and_then(StripMode::parse)
    }

    pub fn strip_gap(&self) -> Option<u32> {
        parse_u32(self.map.get("strip_gap"))
    }

    pub fn labels(&self) -> bool {
        match self.map.get("labels") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64().is_some_and(|v| v != 0),
            Some(Value::String(s)) => parse_bool(s).unwrap_or(false),
            _ => false,
        }
    }

    /// All slot-looking entries, in stable order. Used by the renderer/diagnostics.
    pub fn named_slots(&self) -> BTreeMap<SlotName, SlotValue> {
        let mut out = BTreeMap::new();
        for (key, value) in &self.map {
            let Some(name) = SlotName::parse(key) else {
                continue;
            };
            if let Ok(slot) = slot_value_from_json(value) {
                out.insert(name, slot);
            }
        }
        out
    }
}

pub fn slot_value_from_json(value: &Value) -> Result<SlotValue> {
    serde_json::from_value(value.clone()).context("invalid slot value")
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_u32(value: Option<&Value>) -> Option<u32> {
    match value? {
        Value::Number(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn color_or(value: Option<&str>, default: Rgb) -> Rgb {
    value.and_then(Rgb::parse).unwrap_or(default)
}

/// Where this process reads the status file from.
pub fn status_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OPENDECK_STATUS_FILE")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
        && !runtime.is_empty()
    {
        return PathBuf::from(runtime).join(STATUS_FILE_NAME);
    }
    std::env::temp_dir().join(STATUS_FILE_NAME)
}

pub fn path_source() -> &'static str {
    if std::env::var_os("OPENDECK_STATUS_FILE").is_some_and(|p| !p.is_empty()) {
        "OPENDECK_STATUS_FILE"
    } else if std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|p| !p.is_empty()) {
        "XDG_RUNTIME_DIR"
    } else {
        "temporary directory"
    }
}

pub fn log_path(status: &Path) -> PathBuf {
    PathBuf::from(format!("{}.log", status.display()))
}

/// Read a status file. A missing file is an empty status, unparseable content is an error.
pub fn load(path: &Path) -> Result<Status> {
    match fs::read_to_string(path) {
        Ok(text) => Status::from_json(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Status::empty()),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// Load, modify, write. Missing files start empty; invalid files are an error so a
/// malformed hand edit is never silently clobbered.
pub fn update(path: &Path, f: impl FnOnce(&mut Status)) -> Result<Status> {
    let mut status = load(path)?;
    f(&mut status);
    write_atomic(path, &status)?;
    Ok(status)
}

pub fn write_atomic(path: &Path, status: &Status) -> Result<()> {
    write_bytes_atomic(path, status.to_json_pretty().as_bytes())
}

pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temporary file in {}", parent.display()))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

/// A per-run directory next to the status file, so a sandboxed plugin can read it.
pub fn work_dir(status: &Path, prefix: &str) -> Result<PathBuf> {
    let parent = status
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = parent.join(format!("{prefix}-{}", std::process::id()));
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    Ok(dir)
}

/// A value that differs between runs; used in `@slices` cache keys.
pub fn generation() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(1)
}

/// Append one line to the plugin log next to the status file.
pub fn append_log_line(status: &Path, line: &str) {
    let path = log_path(status);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_and_object_slots() {
        let status =
            Status::from_json(r##"{"dial1":"a","dial2":{"text":"b","color":"#fff"},"labels":1}"##)
                .unwrap();
        assert_eq!(
            status.slot(&SlotName::Dial(1)).unwrap(),
            Some(SlotValue::Text("a".into()))
        );
        let dial2 = status.slot(&SlotName::Dial(2)).unwrap().unwrap();
        assert_eq!(dial2.text(), Some("b"));
        assert!(status.labels());
    }

    #[test]
    fn slot_name_roundtrip() {
        assert_eq!(SlotName::parse("dial4"), Some(SlotName::Dial(4)));
        assert_eq!(
            SlotName::parse("key2_5"),
            Some(SlotName::Key { row: 2, col: 5 })
        );
        assert_eq!(SlotName::parse("key0_1"), None);
        assert_eq!(SlotName::parse("dial5"), None);
        assert_eq!(SlotName::Dial(2).as_wire_name(), "dial2");
        assert_eq!(SlotName::Key { row: 1, col: 3 }.as_wire_name(), "key1_3");
    }

    #[test]
    fn parses_slices_spec() {
        let spec = parse_slices("@slices:/tmp/foo#7").unwrap();
        assert_eq!(spec.dir, PathBuf::from("/tmp/foo"));
        assert_eq!(spec.generation, 7);
        assert!(parse_slices("hello").is_none());
        assert!(parse_slices("@slices:/tmp/foo").is_none());
    }

    #[test]
    fn empty_strip_objects_are_inactive() {
        let active = |json: &str| Status::from_json(json).unwrap().strip_active();
        assert!(!active("{}"));
        assert!(!active(r#"{"strip":{}}"#));
        assert!(!active(r#"{"strip":{"text":""}}"#));
        assert!(!active(r#"{"strip":{"text":"   "}}"#));
        assert!(!active(r#"{"strip":{"spans":[]}}"#));
        assert!(!active(r##"{"strip":{"color":"#fff"}}"##));
        assert!(!active(r#"{"strip":"   "}"#));
        assert!(active(r#"{"strip":{"text":"x"}}"#));
        assert!(active(r#"{"strip":{"spans":[{"text":"x"}]}}"#));
        assert!(active(r#"{"strip":{"bar":0.5}}"#));
        assert!(active(r##"{"strip":{"bg":"#000000"}}"##));
        assert!(active(r#"{"strip":"x"}"#));
    }

    #[test]
    fn strip_modes() {
        assert_eq!(StripMode::parse("TILES"), Some(StripMode::Tiles));
        assert_eq!(StripMode::parse("continuous"), Some(StripMode::Continuous));
        assert_eq!(StripMode::parse(" Raw "), Some(StripMode::Raw));
        assert_eq!(StripMode::Raw.to_string(), "raw");
        assert_eq!(StripMode::parse("streak"), None);
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let mut status = Status::empty();
        status.set_slot("dial1", SlotValue::from_text("one"));
        write_atomic(&path, &status).unwrap();
        let mut next = load(&path).unwrap();
        next.set_slot("dial1", SlotValue::from_text("two"));
        write_atomic(&path, &next).unwrap();
        assert_eq!(
            load(&path).unwrap().slot(&SlotName::Dial(1)).unwrap(),
            Some(SlotValue::Text("two".into()))
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn unparseable_is_an_error() {
        assert!(Status::from_json("not json").is_err());
        assert!(Status::from_json("[]").is_err());
    }
}
