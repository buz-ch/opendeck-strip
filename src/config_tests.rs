use super::*;

const FILE: &str = "/cfg/config.toml";
const AT: &str = "/cfg/config.toml: ";

fn from_toml(text: &str) -> Config {
    let mut config = Config::default();
    config.apply_toml(text, Path::new(FILE));
    config.normalise();
    config
}

fn env(vars: &[(&str, &str)]) -> Config {
    let vars: Vec<_> = vars
        .iter()
        .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v)))
        .collect();
    let mut config = Config::default();
    config.apply_env_vars(vars);
    config.normalise();
    config
}

fn loaded(text: &str, vars: &[(&str, &str)]) -> Config {
    let mut config = from_toml(text);
    config.warnings.clear();
    let vars: Vec<_> = vars
        .iter()
        .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v)))
        .collect();
    config.apply_env_vars(vars);
    config.normalise();
    config
}

#[test]
fn defaults_are_sane() {
    let config = Config::default();
    assert_eq!(config.strip_mode(), StripMode::Continuous);
    assert!(config.max_font_size >= config.min_font_size);
    assert!(config.redraw_hz > 0.0);
    assert!(config.warnings.is_empty());
}

#[test]
fn parses_toml() {
    let config = from_toml(
        r#"
        strip_mode = "tiles"
        strip_gap = 12
        max_jpeg_bytes = 5000
        "#,
    );
    assert_eq!(config.strip_mode(), StripMode::Tiles);
    assert_eq!(config.strip_gap, 12);
    assert_eq!(config.max_jpeg_bytes, 5000);
    assert_eq!(config.align, Config::default().align);
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
}

#[test]
fn every_key_loads_from_the_file_and_the_environment() {
    // Fields of `Config` minus `warnings`.
    assert_eq!(KEYS.len(), 37);
    let defaults = Config::default();
    let mut text = String::new();
    let mut vars: Vec<(std::ffi::OsString, std::ffi::OsString)> = Vec::new();
    for key in KEYS {
        let name = key.name;
        assert_eq!(
            env_key(&name.to_ascii_uppercase()).map(|k| k.name),
            Some(name)
        );
        let value = match key.kind {
            Kind::Path => format!("{name}-path"),
            Kind::String => format!("{name}-string"),
            Kind::Bool => "true".to_owned(),
            Kind::Num { min, max, .. } => {
                let n = match (min, max) {
                    (_, Some(high)) => high - 1.0,
                    (Limit::AtLeast(low), None) => low + 3.0,
                    (Limit::Above(low), None) => low + 3.0,
                    (Limit::Any, None) => 300.0,
                };
                trim_num(n.to_string())
            }
            Kind::Enum(..) => match name {
                "strip_mode" => "raw",
                "align" => "right",
                "valign" => "top",
                "zjsb_mode" => "compact",
                "zjsb_slot" => "key2_3",
                _ => unreachable!(),
            }
            .to_owned(),
            Kind::Template(_) => "{total}".to_owned(),
        };
        let shown = match key.kind {
            Kind::Path | Kind::String | Kind::Enum(..) | Kind::Template(_) => {
                format!("{value:?}")
            }
            _ => value.clone(),
        };
        text.push_str(&format!("{name} = {shown}\n"));
        vars.push((
            std::ffi::OsString::from(format!("OPENDECK_STRIP_{}", name.to_ascii_uppercase())),
            std::ffi::OsString::from(format!(" {value} ")),
        ));
    }
    let by_file = from_toml(&text);
    let by_env = env(&vars
        .iter()
        .map(|(k, v)| (k.to_str().unwrap(), v.to_str().unwrap()))
        .collect::<Vec<_>>());
    assert!(by_file.warnings.is_empty(), "{:?}", by_file.warnings);
    assert!(by_env.warnings.is_empty(), "{:?}", by_env.warnings);
    for key in KEYS {
        let name = key.name;
        let text = (key.get)(&by_file);
        assert_ne!(text, (key.get)(&defaults), "{name} stayed at its default");
        assert_eq!(
            text,
            (key.get)(&by_env),
            "{name} differs between file and env"
        );
    }
}

#[test]
fn bad_toml_value_keeps_the_default_and_the_rest_of_the_file() {
    let config = from_toml("strip_gap = \"wide\"\nfollow = true\n");
    assert_eq!(
        config.warnings,
        [format!(
            "{AT}strip_gap = \"wide\" is not a number; using 28"
        )]
    );
    assert_eq!(config.strip_gap, 28);
    assert!(config.follow);
}

#[test]
fn wrong_toml_type_names_the_expectation() {
    let config = from_toml("follow = 1\nalign = 5\nfont_sans = 12\nstrip_gap = 1.5\n");
    assert_eq!(
        config.warnings,
        [
            format!("{AT}align = 5 is not one of left, center, right; using center"),
            format!("{AT}follow = 1 is not a boolean (1/true/yes/on, 0/false/no/off); using false"),
            format!("{AT}font_sans = 12 is not a path; using none"),
            format!("{AT}strip_gap = 1.5 is not a whole number; using 28"),
        ]
    );
}

#[test]
fn unknown_toml_key_is_ignored() {
    let config = from_toml("nope = 1\nstrip_gap = 9\n[also_nope]\nx = 1\n");
    assert_eq!(
        config.warnings,
        [
            format!("{AT}unknown key also_nope; ignored"),
            format!("{AT}unknown key nope; ignored"),
        ]
    );
    assert_eq!(config.strip_gap, 9);
}

#[test]
fn invalid_file_is_all_defaults_with_one_warning() {
    let config = from_toml("strip_gap = 9\nfollow =\n");
    assert_eq!(config.warnings.len(), 1);
    assert!(
        config.warnings[0].starts_with(&format!("Ignoring invalid config {FILE}: ")),
        "{}",
        config.warnings[0]
    );
    assert_eq!(config.strip_gap, 28);
    assert!(!config.follow);
}

#[test]
fn unreadable_file_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::create_dir(&path).unwrap();
    let mut config = Config::default();
    config.load_path(&path);
    assert_eq!(config.warnings.len(), 1);
    assert!(config.warnings[0].starts_with(&format!("Cannot read config {}: ", path.display())));
}

#[test]
fn clamps_say_what_they_did() {
    let config = from_toml(
        "min_font_size = 1\nline_height = 0.5\npadding = -1\nstrip_gap = 50\nmax_jpeg_bytes = 100\nredraw_hz = 0\nwatch_timeout_secs = 0\nzjsb_min_interval_ms = 0\n",
    );
    assert_eq!(
        config.warnings,
        [
            format!("{AT}line_height = 0.5 is below 0.8; using 0.8"),
            format!("{AT}max_jpeg_bytes = 100 is below 1000; using 1000"),
            format!("{AT}min_font_size = 1 is below 4; using 4"),
            format!("{AT}padding = -1 is below 0; using 0"),
            format!("{AT}redraw_hz = 0 must be above 0; using 2"),
            format!("{AT}strip_gap = 50 is above 32; using 32"),
            format!("{AT}watch_timeout_secs = 0 must be above 0; using 5"),
            format!("{AT}zjsb_min_interval_ms = 0 must be above 0; using 1000"),
        ]
    );
    assert_eq!(config.min_font_size, 4.0);
    assert_eq!(config.line_height, 0.8);
    assert_eq!(config.padding, 0.0);
    assert_eq!(config.strip_gap, 32);
    assert_eq!(config.max_jpeg_bytes, 1000);
    assert_eq!(config.redraw_hz, 2.0);
    assert_eq!(config.watch_timeout_secs, 5);
    assert_eq!(config.zjsb_min_interval_ms, 1000);
}

#[test]
fn max_font_size_follows_min_font_size() {
    let config = from_toml("min_font_size = 40\nmax_font_size = 20\n");
    assert_eq!(
        config.warnings,
        ["max_font_size = 20 is below min_font_size 40; using 40"]
    );
    assert_eq!(config.max_font_size, 40.0);
}

#[test]
fn non_finite_numbers_are_refused() {
    let config = from_toml("line_height = nan\npadding = inf\n");
    assert_eq!(
        config.warnings,
        [
            format!("{AT}line_height = nan is not a finite number; using 1.15"),
            format!("{AT}padding = inf is not a finite number; using 4"),
        ]
    );
}

#[test]
fn enums_name_the_accepted_values() {
    let config = from_toml(
        "strip_mode = \"cont\"\nalign = \"side\"\nvalign = \"up\"\nzjsb_mode = \"wide\"\nzjsb_slot = \"dial9\"\n",
    );
    assert_eq!(
        config.warnings,
        [
            format!("{AT}align = \"side\" is not one of left, center, right; using center"),
            format!(
                "{AT}strip_mode = \"cont\" is not one of continuous, tiles, raw; using continuous"
            ),
            format!("{AT}valign = \"up\" is not one of top, middle, bottom; using middle"),
            format!("{AT}zjsb_mode = \"wide\" is not one of strip, compact; using strip"),
            format!("{AT}zjsb_slot = \"dial9\" is not {SLOTS}; using dial1"),
        ]
    );
    assert_eq!(config.strip_mode(), StripMode::Continuous);
}

#[test]
fn enum_aliases_and_case_stay_accepted() {
    let config = from_toml("strip_mode = \"tile\"\nalign = \"Centre \"\nvalign = \"TOP\"\n");
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.strip_mode(), StripMode::Tiles);
}

#[test]
fn zjsb_slot_accepts_dials_and_keys() {
    for good in ["dial1", "dial4", "key1_1", "key9_9", "key3_5"] {
        let config = from_toml(&format!("zjsb_slot = \"{good}\"\n"));
        assert!(config.warnings.is_empty(), "{good}: {:?}", config.warnings);
        assert_eq!(config.zjsb_slot, good);
    }
    for bad in [
        "dial0", "dial5", "dial", "key0_1", "key1_0", "key10_1", "key1_10", "key1", "key_1", "",
    ] {
        let config = from_toml(&format!("zjsb_slot = \"{bad}\"\n"));
        assert_eq!(config.warnings.len(), 1, "{bad}");
        assert_eq!(config.zjsb_slot, "dial1", "{bad}");
    }
}

#[test]
fn zjsb_scalars() {
    let config = from_toml(
        "zjsb_mode = \"compact\"\nzjsb_slot = \"dial3\"\nzjsb_blank_when_idle = true\nzjsb_min_interval_ms = 500\n",
    );
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.zjsb_mode, "compact");
    assert_eq!(config.zjsb_slot, "dial3");
    assert!(config.zjsb_blank_when_idle);
    assert_eq!(config.zjsb_min_interval_ms, 500);

    let config = from_toml(
        "zjsb_mode = 1\nzjsb_slot = \"nope\"\nzjsb_blank_when_idle = \"yes\"\nzjsb_min_interval_ms = 2.5\n",
    );
    assert_eq!(
        config.warnings,
        [
            format!(
                "{AT}zjsb_blank_when_idle = \"yes\" is not a boolean (1/true/yes/on, 0/false/no/off); using false"
            ),
            format!("{AT}zjsb_min_interval_ms = 2.5 is not a whole number; using 1000"),
            format!("{AT}zjsb_mode = 1 is not one of strip, compact; using strip"),
            format!("{AT}zjsb_slot = \"nope\" is not {SLOTS}; using dial1"),
        ]
    );
}

#[test]
fn templates_check_their_fields() {
    let config = from_toml(
        "zjsb_entry_template = \"{app}: {total}\"\nzjsb_counts_template = \"{total}\"\nzjsb_more_template = \"{hidden} {total}\"\nzjsb_compact_template = \"{marker}\"\nzjsb_idle_template = \"\"\n",
    );
    assert!(config.warnings.is_empty(), "{:?}", config.warnings);
    assert_eq!(config.zjsb_idle_template, "");
    assert_eq!(config.zjsb_more_template, "{hidden} {total}");

    let defaults = Config::default();
    let config = from_toml(
        "zjsb_entry_template = \"{bogus}\"\nzjsb_counts_template = \"{app}\"\nzjsb_more_template = \"{app}\"\nzjsb_compact_template = \"{hidden}\"\nzjsb_idle_template = \"{app}\"",
    );
    let known = |fields: &[&str]| fields.join(", ");
    assert_eq!(
        config.warnings,
        [
            format!(
                "{AT}zjsb_compact_template = \"{{hidden}}\" is not a valid template (unknown field `hidden` (known: {})); using {}",
                known(ENTRY_AND_COUNTS),
                defaults.zjsb_compact_template
            ),
            format!(
                "{AT}zjsb_counts_template = \"{{app}}\" is not a valid template (unknown field `app` (known: {})); using {}",
                known(crate::zjsb::COUNT_FIELDS),
                defaults.zjsb_counts_template
            ),
            format!(
                "{AT}zjsb_entry_template = \"{{bogus}}\" is not a valid template (unknown field `bogus` (known: {})); using {}",
                known(ENTRY_AND_COUNTS),
                defaults.zjsb_entry_template
            ),
            format!(
                "{AT}zjsb_idle_template = \"{{app}}\" is not a valid template (unknown field `app` (known: {})); using ",
                known(crate::zjsb::COUNT_FIELDS)
            ),
            format!(
                "{AT}zjsb_more_template = \"{{app}}\" is not a valid template (unknown field `app` (known: {})); using {}",
                known(MORE_AND_COUNTS),
                defaults.zjsb_more_template
            ),
        ]
    );
    assert_eq!(config.zjsb_entry_template, defaults.zjsb_entry_template);
    assert_eq!(config.zjsb_idle_template, "");

    let config = from_toml("zjsb_entry_template = \"{title\"\n");
    assert_eq!(
        config.warnings,
        [format!(
            "{AT}zjsb_entry_template = \"{{title\" is not a valid template (unclosed `{{` in `{{title`); using {}",
            defaults.zjsb_entry_template
        )]
    );
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
        let config = env(&[
            ("OPENDECK_STRIP_FOLLOW", value),
            ("OPENDECK_STRIP_WORD_WRAP", value),
        ]);
        assert!(config.warnings.is_empty(), "{value}: {:?}", config.warnings);
        assert_eq!(config.follow, expected, "FOLLOW={value}");
        assert_eq!(config.word_wrap, expected, "WORD_WRAP={value}");
    }
}

#[test]
fn env_bad_number() {
    let config = env(&[
        ("OPENDECK_STRIP_STRIP_GAP", "abc"),
        ("OPENDECK_STRIP_FOLLOW", "true"),
    ]);
    assert_eq!(
        config.warnings,
        ["OPENDECK_STRIP_STRIP_GAP=\"abc\" is not a number; using 28"]
    );
    assert_eq!(config.strip_gap, 28);
    assert!(config.follow);
}

#[test]
fn env_bad_boolean() {
    let config = env(&[("OPENDECK_STRIP_FOLLOW", "maybe")]);
    assert_eq!(
        config.warnings,
        [
            "OPENDECK_STRIP_FOLLOW=\"maybe\" is not a boolean (1/true/yes/on, 0/false/no/off); using false"
        ]
    );
    assert!(!config.follow);
}

#[test]
fn env_unknown_key() {
    let config = env(&[
        ("OPENDECK_STRIP_NOPE", "1"),
        ("OPENDECK_STRIP_follow", "true"),
        ("OPENDECK_STRIP_CONFIG", "/elsewhere.toml"),
    ]);
    assert_eq!(
        config.warnings,
        [
            "OPENDECK_STRIP_NOPE is not a known key; ignored",
            "OPENDECK_STRIP_follow is not a known key; ignored",
        ]
    );
}

#[test]
fn env_empty_string() {
    let config = env(&[
        ("OPENDECK_STRIP_ALIGN", ""),
        ("OPENDECK_STRIP_FONT_SANS", "  "),
        ("OPENDECK_STRIP_STRIP_GAP", ""),
        ("OPENDECK_STRIP_ZJSB_IDLE_TEMPLATE", ""),
    ]);
    assert_eq!(
        config.warnings,
        [
            "OPENDECK_STRIP_ALIGN=\"\" is empty; using center",
            "OPENDECK_STRIP_FONT_SANS=\"\" is empty; using none",
            "OPENDECK_STRIP_STRIP_GAP=\"\" is not a number; using 28",
        ]
    );
    assert_eq!(config.zjsb_idle_template, "");
}

#[test]
fn env_clamps_and_enums_name_the_variable() {
    let config = loaded(
        "follow = true\n",
        &[
            ("OPENDECK_STRIP_STRIP_GAP", "50"),
            ("OPENDECK_STRIP_MIN_FONT_SIZE", "1"),
            ("OPENDECK_STRIP_REDRAW_HZ", "0"),
            ("OPENDECK_STRIP_STRIP_MODE", "cont"),
            ("OPENDECK_STRIP_ZJSB_MODE", "wide"),
            ("OPENDECK_STRIP_ZJSB_SLOT", "dial9"),
            ("OPENDECK_STRIP_ZJSB_MIN_INTERVAL_MS", "-5"),
            ("OPENDECK_STRIP_ZJSB_ENTRY_TEMPLATE", "{bogus}"),
        ],
    );
    assert_eq!(
        config.warnings,
        [
            "OPENDECK_STRIP_STRIP_GAP=\"50\" is above 32; using 32",
            "OPENDECK_STRIP_MIN_FONT_SIZE=\"1\" is below 4; using 4",
            "OPENDECK_STRIP_REDRAW_HZ=\"0\" must be above 0; using 2",
            "OPENDECK_STRIP_STRIP_MODE=\"cont\" is not one of continuous, tiles, raw; using continuous",
            "OPENDECK_STRIP_ZJSB_MODE=\"wide\" is not one of strip, compact; using strip",
            "OPENDECK_STRIP_ZJSB_SLOT=\"dial9\" is not one of dial1, dial2, dial3, dial4, key<row>_<col> with row and col 1 to 9; using dial1",
            "OPENDECK_STRIP_ZJSB_MIN_INTERVAL_MS=\"-5\" must be above 0; using 1000",
            "OPENDECK_STRIP_ZJSB_ENTRY_TEMPLATE=\"{bogus}\" is not a valid template (unknown field `bogus` (known: app, title, state, state_label, kind, msg, progress_pct, tasks, marker, color, count, tab, pane, age, session, tab_name, needs_you, failed, done, working, total, needs_you_color, failed_color, done_color, working_color)); using [b]{app|title:14}[/b]\\n[c={color}]{state_label}[/c]\\n{progress_pct|msg:22}",
        ]
    );
    assert_eq!(config.strip_gap, 32);
    assert!(config.follow, "the file survives bad variables");
}

#[test]
fn env_max_font_size_follows_min_font_size() {
    let config = loaded(
        "min_font_size = 40\n",
        &[("OPENDECK_STRIP_MAX_FONT_SIZE", "20")],
    );
    assert_eq!(
        config.warnings,
        ["max_font_size = 20 is below min_font_size 40; using 40"]
    );
    assert_eq!(config.max_font_size, 40.0);
}

#[test]
fn a_bad_variable_keeps_the_file_value() {
    let config = loaded("strip_gap = 9\n", &[("OPENDECK_STRIP_STRIP_GAP", "abc")]);
    assert_eq!(
        config.warnings,
        ["OPENDECK_STRIP_STRIP_GAP=\"abc\" is not a number; using 9"]
    );
    assert_eq!(config.strip_gap, 9);
}

#[test]
fn follow_defaults_off_and_parses() {
    assert!(!Config::default().follow);
    assert!(from_toml("follow = true\n").follow);
    assert!(env(&[("OPENDECK_STRIP_FOLLOW", "yes")]).follow);
}
