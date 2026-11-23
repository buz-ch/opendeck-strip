use std::path::PathBuf;

use serde::Deserialize;

use crate::status::StripMode;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
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

    /// Expect `opendeck-strip follow` to be running; `doctor` then checks for a live session.
    pub follow: bool,

    /// `strip`: the 4-segment strip. `compact`: one slot (`zjsb_slot`).
    pub zjsb_mode: String,
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
            zjsb_slot: "dial1".to_owned(),
            zjsb_blank_when_idle: false,
            zjsb_min_interval_ms: 1000,
            zjsb_entry_template: crate::zjsb::DEFAULT_ENTRY.to_owned(),
            zjsb_counts_template: crate::zjsb::DEFAULT_COUNTS.to_owned(),
            zjsb_more_template: crate::zjsb::DEFAULT_MORE.to_owned(),
            zjsb_compact_template: crate::zjsb::DEFAULT_COMPACT.to_owned(),
            zjsb_idle_template: crate::zjsb::DEFAULT_IDLE.to_owned(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(config) => config,
                Err(err) => {
                    log::warn!("Ignoring invalid config {}: {err}", path.display());
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                log::warn!("Cannot read config {}: {err}", path.display());
                Self::default()
            }
        };
        config.apply_env();
        config.normalise();
        config
    }

    pub fn strip_mode(&self) -> StripMode {
        StripMode::parse(&self.strip_mode).unwrap_or(StripMode::Continuous)
    }

    fn apply_env(&mut self) {
        self.apply_env_vars(std::env::vars_os());
    }

    fn apply_env_vars(
        &mut self,
        vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    ) {
        for (key, value) in vars {
            let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
                continue;
            };
            let Some(name) = key.strip_prefix("OPENDECK_STRIP_") else {
                continue;
            };
            let value = value.trim();
            match name {
                "FONT_SANS" => self.font_sans = Some(value.into()),
                "FONT_SANS_BOLD" => self.font_sans_bold = Some(value.into()),
                "FONT_SANS_ITALIC" => self.font_sans_italic = Some(value.into()),
                "FONT_SANS_BOLD_ITALIC" => self.font_sans_bold_italic = Some(value.into()),
                "FONT_MONO" => self.font_mono = Some(value.into()),
                "FONT_MONO_BOLD" => self.font_mono_bold = Some(value.into()),
                "FONT_MONO_ITALIC" => self.font_mono_italic = Some(value.into()),
                "FONT_MONO_BOLD_ITALIC" => self.font_mono_bold_italic = Some(value.into()),
                "DEFAULT_COLOR" => set(&mut self.default_color, value),
                "DEFAULT_BG" => set(&mut self.default_bg, value),
                "MIN_FONT_SIZE" => set_parse(&mut self.min_font_size, value),
                "MAX_FONT_SIZE" => set_parse(&mut self.max_font_size, value),
                "LINE_HEIGHT" => set_parse(&mut self.line_height, value),
                "PADDING" => set_parse(&mut self.padding, value),
                "WORD_WRAP" => set_bool(&mut self.word_wrap, value),
                "ALIGN" => set(&mut self.align, value),
                "VALIGN" => set(&mut self.valign, value),
                "BAR_COLOR" => set(&mut self.bar_color, value),
                "BAR_BG" => set(&mut self.bar_bg, value),
                "STRIP_MODE" => set(&mut self.strip_mode, value),
                "STRIP_GAP" => set_parse(&mut self.strip_gap, value),
                "STRIP_UNIFORM_SIZE" => set_bool(&mut self.strip_uniform_size, value),
                "STRIP_UNIFORM_SKIP_FIRST" => set_bool(&mut self.strip_uniform_skip_first, value),
                "MAX_JPEG_BYTES" => set_parse(&mut self.max_jpeg_bytes, value),
                "REDRAW_HZ" => set_parse(&mut self.redraw_hz, value),
                "RAW_SOCKET" => self.raw_socket = Some(value.into()),
                "WATCH_TIMEOUT_SECS" => set_parse(&mut self.watch_timeout_secs, value),
                "FOLLOW" => set_bool(&mut self.follow, value),
                _ => {}
            }
        }
    }

    fn normalise(&mut self) {
        if self.min_font_size < 4.0 {
            self.min_font_size = 4.0;
        }
        if self.max_font_size < self.min_font_size {
            self.max_font_size = self.min_font_size;
        }
        if !self.line_height.is_finite() || self.line_height < 0.8 {
            self.line_height = 1.15;
        }
        if !self.padding.is_finite() || self.padding < 0.0 {
            self.padding = 0.0;
        }
        if !self.redraw_hz.is_finite() || self.redraw_hz <= 0.0 {
            self.redraw_hz = 2.0;
        }
        if self.strip_gap > 32 {
            self.strip_gap = 32;
        }
        if self.max_jpeg_bytes < 1000 {
            self.max_jpeg_bytes = 1000;
        }
        if self.watch_timeout_secs == 0 {
            self.watch_timeout_secs = 5;
        }
    }
}

fn set(target: &mut String, value: &str) {
    if !value.is_empty() {
        *target = value.to_owned();
    }
}

/// Booleans take the same words as the status file's `labels`: 1/true/yes/on, 0/false/no/off.
fn set_bool(target: &mut bool, value: &str) {
    if let Some(parsed) = crate::status::parse_bool(value) {
        *target = parsed;
    }
}

fn set_parse<T: std::str::FromStr>(target: &mut T, value: &str) {
    if let Ok(parsed) = value.parse() {
        *target = parsed;
    }
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
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.strip_mode(), StripMode::Continuous);
        assert!(config.max_font_size >= config.min_font_size);
        assert!(config.redraw_hz > 0.0);
    }

    #[test]
    fn parses_toml() {
        let config: Config = toml::from_str(
            r#"
            strip_mode = "tiles"
            strip_gap = 12
            max_jpeg_bytes = 5000
            "#,
        )
        .unwrap();
        assert_eq!(config.strip_mode(), StripMode::Tiles);
        assert_eq!(config.strip_gap, 12);
        assert_eq!(config.max_jpeg_bytes, 5000);
        assert_eq!(config.align, Config::default().align);
    }

    #[test]
    fn boolean_environment_variables_take_words_and_digits() {
        for (value, expected) in [
            ("1", true),
            ("on", true),
            ("TRUE", true),
            ("0", false),
            ("no", false),
        ] {
            let mut config = Config::default();
            config.apply_env_vars([(
                std::ffi::OsString::from("OPENDECK_STRIP_FOLLOW"),
                std::ffi::OsString::from(value),
            )]);
            assert_eq!(config.follow, expected, "FOLLOW={value}");
            let mut config = Config {
                follow: true,
                ..Config::default()
            };
            config.apply_env_vars([(
                std::ffi::OsString::from("OPENDECK_STRIP_WORD_WRAP"),
                std::ffi::OsString::from(value),
            )]);
            assert_eq!(config.word_wrap, expected, "WORD_WRAP={value}");
        }
    }

    #[test]
    fn follow_defaults_off_and_parses() {
        assert!(!Config::default().follow);
        let config: Config = toml::from_str("follow = true\n").unwrap();
        assert!(config.follow);
    }
}
