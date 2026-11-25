use std::path::{Path, PathBuf};

use crate::render::layout::{Align, VAlign};
use crate::status::StripMode;
use crate::zjsb::MAX_ROWS;
use crate::zjsb::template::Template;

#[derive(Clone, Debug)]
pub struct Config {
    pub font_sans: Option<PathBuf>,
    pub font_sans_bold: Option<PathBuf>,
    pub font_sans_italic: Option<PathBuf>,
    pub font_sans_bold_italic: Option<PathBuf>,
    pub font_mono: Option<PathBuf>,
    pub font_mono_bold: Option<PathBuf>,
    pub font_mono_italic: Option<PathBuf>,
    pub font_mono_bold_italic: Option<PathBuf>,

    pub default_color: String,
    pub default_bg: String,
    pub min_font_size: f32,
    pub max_font_size: f32,
    pub line_height: f32,
    pub padding: f32,
    /// Spaces between the columns of a strip grid.
    pub column_gap: u32,
    pub word_wrap: bool,
    pub align: String,
    pub valign: String,
    pub bar_color: String,
    pub bar_bg: String,

    pub strip_mode: String,
    pub strip_gap: u32,
    /// Give all strip segments the smallest size automatic fitting picked.
    pub strip_uniform_size: bool,
    /// With `strip_uniform_size`: leave segment 1 (e.g. a counts panel) out of the shared size.
    pub strip_uniform_skip_first: bool,
    pub max_jpeg_bytes: usize,
    pub redraw_hz: f32,
    /// AKP05 raw strip socket for `strip_mode = raw`; default: found automatically.
    pub raw_socket: Option<PathBuf>,

    pub watch_timeout_secs: u64,

    /// Expect `opendeck-strip follow-zellij` to be running; `doctor` then checks for a live session.
    pub follow: bool,

    /// `strip`: the 4-segment strip. `compact`: one slot (`zjsb_slot`).
    pub zjsb_mode: String,
    /// `strip` only: `segments` (four segments) or `lines` (one grid).
    pub zjsb_layout: String,
    /// Compact only: `dial1`..`dial4` or `key<row>_<col>`.
    pub zjsb_slot: String,
    /// Clear the deck when nothing is active, instead of the idle text.
    pub zjsb_blank_when_idle: bool,
    /// Minimum time between two writes when following.
    pub zjsb_min_interval_ms: u64,
    pub zjsb_entry_template: String,
    pub zjsb_counts_template: String,
    pub zjsb_more_template: String,
    pub zjsb_compact_template: String,
    pub zjsb_idle_template: String,

    /// Everything that was wrong in the file or the environment, one line each;
    /// the key named in the line kept the value the line ends with.
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_sans: None,
            font_sans_bold: None,
            font_sans_italic: None,
            font_sans_bold_italic: None,
            font_mono: None,
            font_mono_bold: None,
            font_mono_italic: None,
            font_mono_bold_italic: None,
            default_color: "#ffffff".to_owned(),
            default_bg: "#000000".to_owned(),
            min_font_size: 8.0,
            max_font_size: 96.0,
            line_height: 1.15,
            padding: 4.0,
            column_gap: 2,
            word_wrap: false,
            align: "center".to_owned(),
            valign: "middle".to_owned(),
            bar_color: "#55ccff".to_owned(),
            bar_bg: "#202830".to_owned(),
            strip_mode: "continuous".to_owned(),
            strip_gap: 28,
            strip_uniform_size: false,
            strip_uniform_skip_first: false,
            max_jpeg_bytes: 9000,
            redraw_hz: 2.0,
            raw_socket: None,
            watch_timeout_secs: 5,
            follow: false,
            zjsb_mode: "strip".to_owned(),
            zjsb_layout: "segments".to_owned(),
            zjsb_slot: "dial1".to_owned(),
            zjsb_blank_when_idle: false,
            zjsb_min_interval_ms: 1000,
            zjsb_entry_template: crate::zjsb::DEFAULT_ENTRY.to_owned(),
            zjsb_counts_template: crate::zjsb::DEFAULT_COUNTS.to_owned(),
            zjsb_more_template: crate::zjsb::DEFAULT_MORE.to_owned(),
            zjsb_compact_template: crate::zjsb::DEFAULT_COMPACT.to_owned(),
            zjsb_idle_template: crate::zjsb::DEFAULT_IDLE.to_owned(),
            warnings: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let mut config = Self::default();
        config.load_path(&config_path());
        config.apply_env();
        config.normalise();
        config
    }

    /// A missing file is the defaults; an unreadable or unparsable one is reported.
    fn load_path(&mut self, path: &Path) {
        match std::fs::read_to_string(path) {
            Ok(text) => self.apply_toml(&text, path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => self
                .warnings
                .push(format!("Cannot read config {}: {err}", path.display())),
        }
    }

    pub fn strip_mode(&self) -> StripMode {
        StripMode::parse(&self.strip_mode).unwrap_or(StripMode::Continuous)
    }

    /// The file, key by key: a bad key keeps its value, the rest stays in effect.
    fn apply_toml(&mut self, text: &str, path: &Path) {
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(err) => {
                self.warnings
                    .push(format!("Ignoring invalid config {}: {err}", path.display()));
                return;
            }
        };
        let at = format!("{}: ", path.display());
        for (name, value) in &table {
            let shown = value.to_string();
            let from = Origin::File {
                at: &at,
                name,
                shown: &shown,
            };
            let Some(key) = file_key(name) else {
                self.warnings
                    .push(format!("{at}unknown key {name}; ignored"));
                continue;
            };
            match key.kind.toml_text(value) {
                Ok(text) => self.set_key(key, &text, from),
                Err(reason) => {
                    self.warnings.push(from.line(&reason, &(key.get)(self)));
                }
            }
        }
    }

    fn apply_env(&mut self) {
        self.apply_env_vars(std::env::vars_os());
    }

    fn apply_env_vars(
        &mut self,
        vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    ) {
        for (var, value) in vars {
            let (Some(var), Some(value)) = (var.to_str(), value.to_str()) else {
                continue;
            };
            let Some(name) = var.strip_prefix("OPENDECK_STRIP_") else {
                continue;
            };
            // Names the file, not a key.
            if name == "CONFIG" {
                continue;
            }
            let value = value.trim();
            let shown = format!("{value:?}");
            let Some(key) = env_key(name) else {
                self.warnings
                    .push(format!("{var} is not a known key; ignored"));
                continue;
            };
            self.set_key(key, value, Origin::Env { var, shown: &shown });
        }
    }

    /// Check `text` for `key` and store it; anything else is reported through `warnings`.
    fn set_key(&mut self, key: &Key, text: &str, from: Origin) {
        let kept = (key.get)(self);
        match key.kind.convert(text) {
            Ok(text) => (key.set)(self, &text),
            Err(Fault::Keep(reason)) => self.warnings.push(from.line(&reason, &kept)),
            Err(Fault::Clamp(reason, value)) => {
                self.warnings.push(from.line(&reason, &value));
                (key.set)(self, &value);
            }
        }
    }

    /// The checks that need more than one key; every single-key clamp lives in [`KEYS`].
    fn normalise(&mut self) {
        if self.max_font_size < self.min_font_size {
            let kept = trim_num(self.min_font_size.to_string());
            self.warnings.push(format!(
                "max_font_size = {} is below min_font_size {kept}; using {kept}",
                trim_num(self.max_font_size.to_string())
            ));
            self.max_font_size = self.min_font_size;
        }
        self.lines_warnings();
    }

    /// `zjsb_layout = "lines"` works with some settings better than others; no
    /// value changes, each line names the key, the value and the consequence.
    fn lines_warnings(&mut self) {
        if self.zjsb_layout != "lines" {
            return;
        }
        if self.zjsb_mode == "compact" {
            self.warnings.push(
                "zjsb_layout = \"lines\" with zjsb_mode = \"compact\"; the layout is ignored"
                    .to_owned(),
            );
            return;
        }
        if self.word_wrap {
            self.warnings
                .push("zjsb_layout = \"lines\" with word_wrap = true; no columns".to_owned());
        }
        // A template that does not parse has its own warning; no line count.
        let count = |text: &str, fields: &[&str]| {
            Template::parse(text, fields).ok().map(|t| t.line_count())
        };
        let counts = count(&self.zjsb_counts_template, crate::zjsb::COUNT_FIELDS);
        if let Some(counts) = counts.filter(|n| *n > MAX_ROWS) {
            self.warnings.push(format!(
                "zjsb_layout = \"lines\" with a zjsb_counts_template of {counts} lines; the first {MAX_ROWS} are used"
            ));
        }
        if let (Some(entry), Some(more)) = (
            count(&self.zjsb_entry_template, ENTRY_AND_COUNTS),
            count(&self.zjsb_more_template, MORE_AND_COUNTS),
        ) && entry != more
        {
            self.warnings.push(format!(
                "zjsb_layout = \"lines\" with {more} zjsb_more_template lines and {entry} zjsb_entry_template lines; the more row will not align"
            ));
        }
        if self.min_font_size > LINES_MAX_MIN_FONT_SIZE {
            self.warnings.push(format!(
                "zjsb_layout = \"lines\" with min_font_size {}; five rows may not fit and the +N more row is lost first",
                trim_num(self.min_font_size.to_string())
            ));
        }
    }
}

/// Above this the five rows of the `lines` layout may not fit the strip.
const LINES_MAX_MIN_FONT_SIZE: f32 = 16.0;

/// Where a value came from, and how a warning names it: the key and the value as
/// written, then `<reason>; using <kept>`.
#[derive(Clone, Copy)]
enum Origin<'a> {
    File {
        at: &'a str,
        name: &'a str,
        shown: &'a str,
    },
    Env {
        var: &'a str,
        shown: &'a str,
    },
}

impl Origin<'_> {
    fn line(&self, reason: &str, kept: &str) -> String {
        match self {
            Self::File { at, name, shown } => {
                format!("{at}{name} = {shown} {reason}; using {kept}")
            }
            Self::Env { var, shown } => format!("{var}={shown} {reason}; using {kept}"),
        }
    }
}

/// The stored form of a config field: text to report it, and back.
trait Field: Sized {
    fn to_text(&self) -> String;
    fn from_text(text: &str) -> Option<Self>;
}

impl Field for String {
    fn to_text(&self) -> String {
        self.clone()
    }
    fn from_text(text: &str) -> Option<Self> {
        Some(text.to_owned())
    }
}

impl Field for Option<PathBuf> {
    fn to_text(&self) -> String {
        self.as_deref()
            .map_or_else(|| "none".to_owned(), |p| p.to_string_lossy().into_owned())
    }
    fn from_text(text: &str) -> Option<Self> {
        Some(Some(PathBuf::from(text)))
    }
}

impl Field for bool {
    fn to_text(&self) -> String {
        self.to_string()
    }
    fn from_text(text: &str) -> Option<Self> {
        crate::status::parse_bool(text)
    }
}

macro_rules! int_field {
    ($($ty:ty),*) => {$(
        impl Field for $ty {
            fn to_text(&self) -> String {
                self.to_string()
            }
            fn from_text(text: &str) -> Option<Self> {
                text.parse().ok()
            }
        }
    )*};
}
int_field!(u32, u64, usize);

impl Field for f32 {
    fn to_text(&self) -> String {
        trim_num(self.to_string())
    }
    fn from_text(text: &str) -> Option<Self> {
        text.parse::<f32>().ok().filter(|v| v.is_finite())
    }
}

/// `4.0` reads better as `4`.
fn trim_num(text: String) -> String {
    if let Some(head) = text.strip_suffix(".0") {
        return head.to_owned();
    }
    text
}

/// What a value must be; shared by the file and the environment.
#[derive(Clone, Copy)]
enum Kind {
    Path,
    String,
    Bool,
    Num {
        real: bool,
        min: Limit,
        max: Option<f64>,
    },
    /// The accepted names, and the parser that consumes the value.
    Enum(&'static str, fn(&str) -> bool),
    /// Fields the template may use.
    Template(&'static [&'static str]),
}

#[derive(Clone, Copy)]
enum Limit {
    /// No lower bound of its own.
    Any,
    /// Below it the value is raised to it.
    AtLeast(f64),
    /// At or below it the value is refused.
    Above(f64),
}

impl Kind {
    fn expected(&self) -> &'static str {
        match self {
            Self::Path => "a path",
            Self::String => "a non-empty string",
            Self::Bool => "a boolean (1/true/yes/on, 0/false/no/off)",
            Self::Num { .. } => "a number",
            Self::Enum(names, _) => names,
            Self::Template(_) => "a template",
        }
    }

    /// A TOML value as the text the environment would carry; `Err` when the type is wrong.
    fn toml_text(&self, value: &toml::Value) -> Result<String, String> {
        let wrong = || Err(format!("is not {}", self.expected()));
        let text = match (self, value) {
            (Self::Bool, toml::Value::Boolean(b)) => b.to_string(),
            (Self::Num { .. }, toml::Value::Integer(i)) => i.to_string(),
            (Self::Num { real, .. }, toml::Value::Float(f)) if *real => f.to_string(),
            (Self::Num { .. }, toml::Value::Float(_)) => {
                return Err("is not a whole number".to_owned());
            }
            (_, toml::Value::String(s)) if !matches!(self, Self::Bool | Self::Num { .. }) => {
                s.clone()
            }
            _ => return wrong(),
        };
        Ok(text)
    }

    /// The text to store, or what is wrong with it.
    fn convert(&self, text: &str) -> Result<String, Fault> {
        if matches!(self, Self::Path | Self::String | Self::Enum(..)) && text.is_empty() {
            return Err(Fault::Keep("is empty".to_owned()));
        }
        match self {
            Self::Path | Self::String => Ok(text.to_owned()),
            Self::Bool => match crate::status::parse_bool(text) {
                Some(b) => Ok(b.to_string()),
                None => Err(Fault::Keep(format!("is not {}", self.expected()))),
            },
            Self::Num { real, min, max } => convert_num(text, *real, *min, *max),
            Self::Enum(_, accepts) if accepts(text) => Ok(text.to_owned()),
            Self::Enum(..) => Err(Fault::Keep(format!("is not {}", self.expected()))),
            Self::Template(fields) => match Template::parse(text, fields) {
                Ok(_) => Ok(text.to_owned()),
                Err(err) => Err(Fault::Keep(format!("is not a valid template ({err})"))),
            },
        }
    }
}

/// What is wrong with a value: keep the current one, or clamp to the second one.
enum Fault {
    Keep(String),
    Clamp(String, String),
}

fn convert_num(text: &str, real: bool, min: Limit, max: Option<f64>) -> Result<String, Fault> {
    let n: f64 = text
        .parse()
        .map_err(|_| Fault::Keep("is not a number".to_owned()))?;
    if !n.is_finite() {
        return Err(Fault::Keep("is not a finite number".to_owned()));
    }
    if !real && n.fract() != 0.0 {
        return Err(Fault::Keep("is not a whole number".to_owned()));
    }
    // Integers come out canonical; a real number stays as written.
    let stored = if real {
        text.to_owned()
    } else {
        trim_num(n.to_string())
    };
    match min {
        Limit::Any => {}
        Limit::AtLeast(low) if n < low => {
            return Err(Fault::Clamp(
                format!("is below {}", trim_num(low.to_string())),
                trim_num(low.to_string()),
            ));
        }
        Limit::Above(low) if n <= low => {
            return Err(Fault::Keep(format!(
                "must be above {}",
                trim_num(low.to_string())
            )));
        }
        _ => {}
    }
    if let Some(high) = max
        && n > high
    {
        return Err(Fault::Clamp(
            format!("is above {}", trim_num(high.to_string())),
            trim_num(high.to_string()),
        ));
    }
    // The fields are `f32` or at most `u32` wide in practice; refuse what they cannot hold.
    if real && n.abs() > f64::from(f32::MAX) {
        return Err(Fault::Keep("is too large".to_owned()));
    }
    if !real && !(0.0..=f64::from(u32::MAX)).contains(&n) {
        return Err(Fault::Keep("is out of range".to_owned()));
    }
    Ok(stored)
}

/// One row per key: the name, what the value must be, and the field it lives in.
/// Adding a key is a field plus a row here; the default comes from `Config::default`.
struct Key {
    name: &'static str,
    kind: Kind,
    get: fn(&Config) -> String,
    set: fn(&mut Config, &str),
}

macro_rules! key_table {
    ($($name:literal => $field:ident : $kind:expr),* $(,)?) => {
        static KEYS: &[Key] = &[
            $( Key {
                name: $name,
                kind: $kind,
                get: |c| c.$field.to_text(),
                set: |c, text| {
                    if let Some(value) = Field::from_text(text) {
                        c.$field = value;
                    }
                },
            } ),*
        ];
    };
}

const SLOTS: &str = "one of dial1, dial2, dial3, dial4, key<row>_<col> with row and col 1 to 9";

key_table! {
    "font_sans" => font_sans: Kind::Path,
    "font_sans_bold" => font_sans_bold: Kind::Path,
    "font_sans_italic" => font_sans_italic: Kind::Path,
    "font_sans_bold_italic" => font_sans_bold_italic: Kind::Path,
    "font_mono" => font_mono: Kind::Path,
    "font_mono_bold" => font_mono_bold: Kind::Path,
    "font_mono_italic" => font_mono_italic: Kind::Path,
    "font_mono_bold_italic" => font_mono_bold_italic: Kind::Path,
    "default_color" => default_color: Kind::String,
    "default_bg" => default_bg: Kind::String,
    "min_font_size" => min_font_size: Kind::Num { real: true, min: Limit::AtLeast(4.0), max: None },
    // Raised to `min_font_size` by `normalise` when below it.
    "max_font_size" => max_font_size: Kind::Num { real: true, min: Limit::Any, max: None },
    "line_height" => line_height: Kind::Num { real: true, min: Limit::AtLeast(0.8), max: None },
    "padding" => padding: Kind::Num { real: true, min: Limit::AtLeast(0.0), max: None },
    "column_gap" => column_gap: Kind::Num { real: false, min: Limit::AtLeast(0.0), max: Some(16.0) },
    "word_wrap" => word_wrap: Kind::Bool,
    "align" => align: Kind::Enum("one of left, center, right", |s| Align::parse(s).is_some()),
    "valign" => valign: Kind::Enum("one of top, middle, bottom", |s| VAlign::parse(s).is_some()),
    "bar_color" => bar_color: Kind::String,
    "bar_bg" => bar_bg: Kind::String,
    "strip_mode" => strip_mode: Kind::Enum("one of continuous, tiles, raw", |s| StripMode::parse(s).is_some()),
    "strip_gap" => strip_gap: Kind::Num { real: false, min: Limit::AtLeast(0.0), max: Some(32.0) },
    "strip_uniform_size" => strip_uniform_size: Kind::Bool,
    "strip_uniform_skip_first" => strip_uniform_skip_first: Kind::Bool,
    "max_jpeg_bytes" => max_jpeg_bytes: Kind::Num { real: false, min: Limit::AtLeast(1000.0), max: None },
    "redraw_hz" => redraw_hz: Kind::Num { real: true, min: Limit::Above(0.0), max: None },
    "raw_socket" => raw_socket: Kind::Path,
    "watch_timeout_secs" => watch_timeout_secs: Kind::Num { real: false, min: Limit::Above(0.0), max: None },
    "follow" => follow: Kind::Bool,
    "zjsb_mode" => zjsb_mode: Kind::Enum("one of strip, compact", is_zjsb_mode),
    "zjsb_layout" => zjsb_layout: Kind::Enum("one of segments, lines", is_zjsb_layout),
    "zjsb_slot" => zjsb_slot: Kind::Enum(SLOTS, is_zjsb_slot),
    "zjsb_blank_when_idle" => zjsb_blank_when_idle: Kind::Bool,
    "zjsb_min_interval_ms" => zjsb_min_interval_ms: Kind::Num { real: false, min: Limit::Above(0.0), max: None },
    "zjsb_entry_template" => zjsb_entry_template: Kind::Template(ENTRY_AND_COUNTS),
    "zjsb_counts_template" => zjsb_counts_template: Kind::Template(crate::zjsb::COUNT_FIELDS),
    "zjsb_more_template" => zjsb_more_template: Kind::Template(MORE_AND_COUNTS),
    "zjsb_compact_template" => zjsb_compact_template: Kind::Template(ENTRY_AND_COUNTS),
    "zjsb_idle_template" => zjsb_idle_template: Kind::Template(crate::zjsb::COUNT_FIELDS),
}

static ENTRY_AND_COUNTS: &[&str] = &concat_fields::<
    { crate::zjsb::ENTRY_FIELDS.len() + crate::zjsb::COUNT_FIELDS.len() },
>(crate::zjsb::ENTRY_FIELDS, crate::zjsb::COUNT_FIELDS);
static MORE_AND_COUNTS: &[&str] = &concat_fields::<
    { crate::zjsb::MORE_FIELDS.len() + crate::zjsb::COUNT_FIELDS.len() },
>(crate::zjsb::MORE_FIELDS, crate::zjsb::COUNT_FIELDS);

/// Two field lists as one.
const fn concat_fields<const N: usize>(
    a: &[&'static str],
    b: &[&'static str],
) -> [&'static str; N] {
    let mut out = [""; N];
    let mut i = 0;
    while i < a.len() {
        out[i] = a[i];
        i += 1;
    }
    while i < N {
        out[i] = b[i - a.len()];
        i += 1;
    }
    out
}

fn file_key(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|key| key.name == name)
}

/// `OPENDECK_STRIP_<NAME>`; only the upper-case spelling is a known key.
fn env_key(name: &str) -> Option<&'static Key> {
    KEYS.iter()
        .find(|key| key.name.to_ascii_uppercase() == name)
}

fn is_zjsb_mode(s: &str) -> bool {
    matches!(s, "strip" | "compact")
}

fn is_zjsb_layout(s: &str) -> bool {
    matches!(s, "segments" | "lines")
}

/// The plugin's `valid_slot`: `dial1`..`dial4` or `key<row>_<col>`, row and col 1..=9.
fn is_zjsb_slot(s: &str) -> bool {
    if let Some(n) = s.strip_prefix("dial") {
        return matches!(n, "1" | "2" | "3" | "4");
    }
    s.strip_prefix("key")
        .and_then(|rest| rest.split_once('_'))
        .is_some_and(|(row, col)| {
            [row, col]
                .iter()
                .all(|x| matches!(x.as_bytes(), [b'1'..=b'9']))
        })
}

pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OPENDECK_STRIP_CONFIG")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("opendeck-strip").join("config.toml")
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
