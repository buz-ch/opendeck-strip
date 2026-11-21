//! Integration tests that drive the real binary in an isolated status/config dir.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use image::Rgb;
use serde_json::Value;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_opendeck-strip");

struct Env {
    dir: TempDir,
    status: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = dir.path().join("status.json");
        let config = dir.path().join("config.toml");
        fs::write(&config, "").expect("write config");
        Self { dir, status }
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(BIN);
        command
            .env_clear()
            .env("OPENDECK_STATUS_FILE", &self.status)
            .env("OPENDECK_STRIP_CONFIG", self.dir.path().join("config.toml"))
            // Keep install/doctor away from the user's real plugin directories.
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join(".config"))
            .args(args);
        command
    }

    fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.command(args).output().expect("run binary")
    }

    fn run_ok<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "command failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8(self.run_ok(args).stdout).expect("utf-8 stdout")
    }

    fn status_json(&self) -> Value {
        let text = fs::read_to_string(&self.status).expect("status file exists");
        serde_json::from_str(&text).expect("status file is JSON")
    }

    fn status_text(&self) -> String {
        fs::read_to_string(&self.status).expect("status file exists")
    }

    fn get_json(&self) -> Value {
        serde_json::from_str(&self.stdout(&["get"])).expect("get prints JSON")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn png(path: &Path) -> image::RgbImage {
    image::open(path)
        .unwrap_or_else(|err| panic!("open {}: {err}", path.display()))
        .to_rgb8()
}

fn has_ink(image: &image::RgbImage) -> bool {
    image.pixels().any(|p| p.0 != [0, 0, 0])
}

fn wait_for_slot(env: &Env, slot: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(&env.status)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
        {
            let slot = &value[slot];
            if let Some(text) = slot.as_str() {
                return Some(text.to_owned());
            }
            if let Some(text) = slot["text"].as_str() {
                return Some(text.to_owned());
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn wait_for_exit(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn where_prints_the_status_file_path() {
    let env = Env::new();
    let output = env.stdout(&["where"]);
    assert_eq!(PathBuf::from(output.trim_end_matches('\n')), env.status);
}

#[test]
fn set_unescapes_newlines_and_get_roundtrips() {
    let env = Env::new();
    env.run_ok(["set", "key1_1", "HELLO", "dial2", r"a\nb"]);

    let status = env.status_json();
    assert_eq!(status["key1_1"], "HELLO");
    assert_eq!(status["dial2"], "a\nb");

    let got = env.get_json();
    assert_eq!(got, status);
    assert_eq!(got["dial2"].as_str(), Some("a\nb"));
}

#[test]
fn plain_mode_escapes_brackets_without_losing_them() {
    let env = Env::new();
    env.run_ok(["set", "--plain", "dial1", "a[b]c"]);

    let status = env.status_json();
    assert_eq!(status["dial1"], "a[[b]c");
    assert!(
        env.status_text().contains("[["),
        "stored JSON must contain the escaped bracket"
    );
    assert!(env.stdout(&["get"]).contains("a[[b]c"));

    // The escaped form must render, and reading it back must keep the escape.
    let out = env.path("bracket.png");
    env.run_ok(["strip-preview", "a[[b]c", out.to_str().expect("utf-8 path")]);
    assert!(out.exists());
    assert_eq!(env.status_json()["dial1"], "a[[b]c");
}

#[test]
fn odd_set_arguments_fail_without_touching_the_file() {
    let env = Env::new();
    env.run_ok(["set", "labels", "1"]);
    let before = env.status_text();

    let output = env.run(["set", "dial1"]);
    assert!(!output.status.success());
    assert_eq!(env.status_text(), before);
}

#[test]
fn typed_settings_are_stored() {
    let env = Env::new();
    env.run_ok([
        "set",
        "labels",
        "1",
        "strip_mode",
        "tiles",
        "strip_gap",
        "12",
    ]);

    let status = env.status_json();
    assert_eq!(status["labels"], Value::Bool(true));
    assert_eq!(status["strip_mode"], Value::String("tiles".to_owned()));
    assert_eq!(status["strip_gap"], Value::Number(12.into()));

    env.run_ok(["set", "labels", "0"]);
    assert_eq!(env.status_json()["labels"], Value::Bool(false));
}

#[test]
fn style_objects_roundtrip() {
    let env = Env::new();
    env.run_ok(["set", "dial1", r##"{"text":"X","color":"#ff8800"}"##]);

    let status = env.status_json();
    assert_eq!(status["dial1"]["text"], "X");
    assert_eq!(status["dial1"]["color"], "#ff8800");

    let got = env.get_json();
    assert_eq!(got["dial1"]["text"], "X");
    assert_eq!(got["dial1"]["color"], "#ff8800");
}

#[test]
fn clear_removes_the_expected_keys() {
    let env = Env::new();
    env.run_ok([
        "set", "dial1", "a", "dial2", "b", "dial3", "c", "dial4", "d", "key1_1", "K", "strip", "S",
        "labels", "1",
    ]);

    env.run_ok(["clear", "dial1"]);
    let status = env.status_json();
    assert!(status.get("dial1").is_none());
    assert!(status.get("dial2").is_some());

    env.run_ok(["clear", "strip"]);
    let status = env.status_json();
    assert!(status.get("strip").is_none());
    for index in 1..=4 {
        assert!(status.get(format!("dial{index}")).is_none());
    }
    assert_eq!(status["key1_1"], "K");
    assert_eq!(status["labels"], Value::Bool(true));

    env.run_ok(["clear"]);
    assert_eq!(env.stdout(&["get"]).trim(), "{}");
    assert_eq!(env.status_text().trim(), "{}");
}

#[test]
fn clear_without_arguments_recovers_a_corrupt_file() {
    let env = Env::new();
    fs::write(&env.status, "{not json").unwrap();
    env.run_ok(["clear"]);
    assert_eq!(env.status_text().trim(), "{}");
}

#[test]
fn strip_preview_renders_continuous_canvas() {
    let env = Env::new();
    let out = env.path("hello.png");
    env.run_ok(["strip-preview", "HELLO", out.to_str().unwrap()]);

    let image = png(&out);
    assert_eq!(image.dimensions(), (800, 128));
    assert!(has_ink(&image));
}

#[test]
fn strip_preview_tiles_and_simulated_device_geometry() {
    let env = Env::new();

    let tiles = env.path("tiles.png");
    env.run_ok([
        "strip-preview",
        "CPU|MEM|NET|DISK",
        tiles.to_str().unwrap(),
        "--tiles",
    ]);
    assert_eq!(png(&tiles).dimensions(), (788, 88));

    let simulated = env.path("simulated.png");
    env.run_ok([
        "strip-preview",
        "CPU|MEM|NET|DISK",
        simulated.to_str().unwrap(),
        "--tiles",
        "--simulate-device",
    ]);
    assert_eq!(png(&simulated).dimensions(), (800, 128));
}

#[test]
fn strip_preview_test_pattern_has_ink() {
    let env = Env::new();
    let out = env.path("test.png");
    env.run_ok(["strip-preview", "test", out.to_str().unwrap()]);

    let image = png(&out);
    assert_eq!(image.dimensions(), (800, 128));
    assert!(has_ink(&image));
}

#[test]
fn strip_preview_composes_slices() {
    let env = Env::new();
    let slices = env.path("slices");
    fs::create_dir_all(&slices).unwrap();
    let colors = [
        Rgb([255u8, 0, 0]),
        Rgb([0, 255, 0]),
        Rgb([0, 0, 255]),
        Rgb([255, 255, 0]),
    ];
    for (index, color) in colors.iter().enumerate() {
        image::RgbImage::from_pixel(200, 100, *color)
            .save(slices.join(format!("slice{index}.png")))
            .unwrap();
    }

    let out = env.path("slices.png");
    let spec = format!("@slices:{}#1", slices.display());
    env.run_ok(["strip-preview", &spec, out.to_str().unwrap()]);

    let image = png(&out);
    assert_eq!(image.dimensions(), (800, 128));
    for (index, color) in colors.iter().enumerate() {
        let x = index as u32 * 200 + 100;
        for y in [14u32, 64, 113] {
            assert_eq!(image.get_pixel(x, y), color, "slice {index} at ({x},{y})");
        }
        for y in [0u32, 127] {
            assert_eq!(
                image.get_pixel(x, y),
                &Rgb([0, 0, 0]),
                "letterbox at ({x},{y})"
            );
        }
    }
}

#[test]
fn install_writes_plugin_directory_and_manifest() {
    let env = Env::new();
    env.run_ok(["install", "--dir", env.dir.path().to_str().unwrap()]);

    let plugin = env.dir.path().join("local.statusdisplay.sdPlugin");
    assert!(plugin.join("manifest.json").exists());
    assert!(plugin.join("opendeck-strip").exists());
    for icon in ["plugin.png", "category.png", "action.png"] {
        assert!(plugin.join("icons").join(icon).exists(), "missing {icon}");
    }

    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(plugin.join("manifest.json")).unwrap()).unwrap();
    let action = &manifest["Actions"][0];
    assert_eq!(action["UUID"], "local.statusdisplay.slot");
    assert_eq!(
        action["Controllers"],
        serde_json::json!(["Keypad", "Encoder"])
    );
    assert_eq!(action["Encoder"]["layout"], "$A0");
}

#[test]
fn doctor_exits_nonzero_without_an_installation() {
    let env = Env::new();
    let output = env.run(["doctor"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("not installed"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn non_ascii_colours_render_without_panicking() {
    let env = Env::new();
    let out = env.path("nonascii.png");
    env.run_ok([
        "strip-preview",
        r##"{"text":"hi","color":"#aé","bg":"#éabcd"}"##,
        out.to_str().unwrap(),
    ]);
    assert!(has_ink(&png(&out)));
}

#[cfg(unix)]
#[test]
fn non_utf8_environment_is_skipped() {
    use std::os::unix::ffi::OsStringExt;

    let env = Env::new();
    let out = env.path("env.png");
    let output = env
        .command(["strip-preview", "test", out.to_str().unwrap()])
        .env(
            "OPENDECK_STRIP_DEFAULT_COLOR",
            std::ffi::OsString::from_vec(b"#a\xff".to_vec()),
        )
        .env(
            std::ffi::OsString::from_vec(b"OPENDECK_STRIP_\xff".to_vec()),
            "1",
        )
        .output()
        .expect("run binary");
    assert!(
        output.status.success(),
        "non-UTF-8 environment must not panic: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn size_test_refuses_tiles_from_config() {
    let env = Env::new();
    fs::write(
        env.dir.path().join("config.toml"),
        "strip_mode = \"tiles\"\n",
    )
    .unwrap();
    let output = env.run(["size-test", "--start", "4000", "--max", "4000"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("continuous"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn watch_preserves_argv_boundaries() {
    let env = Env::new();
    let mut child = env
        .command([
            "watch", "--slot", "key1_1", "--every", "60", "--", "printf", "%s", "a b",
        ])
        .spawn()
        .expect("spawn watch");
    let seen = wait_for_slot(&env, "key1_1", Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(seen.as_deref(), Some("a b"));
}

#[test]
fn watch_timeout_kills_the_process_group() {
    let env = Env::new();
    fs::write(
        env.dir.path().join("config.toml"),
        "watch_timeout_secs = 1\n",
    )
    .unwrap();
    let mut child = env
        .command([
            "watch",
            "--slot",
            "key1_1",
            "--every",
            "60",
            "--",
            "sh",
            "-c",
            "sleep 100 & sleep 100",
        ])
        .spawn()
        .expect("spawn watch");
    let seen = wait_for_slot(&env, "key1_1", Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        seen.as_deref()
            .is_some_and(|text| text.starts_with("TIMEOUT")),
        "got {seen:?}"
    );
}

#[test]
fn watch_spawn_failure_is_reported_as_an_error() {
    let env = Env::new();
    let mut child = env
        .command([
            "watch",
            "--slot",
            "key1_1",
            "--every",
            "60",
            "--",
            "definitely-not-a-command",
        ])
        .spawn()
        .expect("spawn watch");
    let seen = wait_for_slot(&env, "key1_1", Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        seen.as_deref()
            .is_some_and(|text| text.starts_with("ERROR")),
        "got {seen:?}"
    );
}

#[test]
fn watch_samples_large_output() {
    let env = Env::new();
    let mut child = env
        .command([
            "watch",
            "--slot",
            "key1_1",
            "--every",
            "60",
            "--",
            "sh",
            "-c",
            "yes | head -n 40000",
        ])
        .spawn()
        .expect("spawn watch");
    let seen = wait_for_slot(&env, "key1_1", Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(seen.as_deref(), Some("y\ny\ny\ny"));
}

#[test]
fn watch_keeps_the_exit_status_after_large_output() {
    let env = Env::new();
    let mut child = env
        .command([
            "watch",
            "--slot",
            "key1_1",
            "--every",
            "60",
            "--",
            "sh",
            "-c",
            "yes | head -c 200000; exit 7",
        ])
        .spawn()
        .expect("spawn watch");
    let seen = wait_for_slot(&env, "key1_1", Duration::from_secs(10));
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        seen.as_deref()
            .is_some_and(|text| text.starts_with("EXIT 7")),
        "got {seen:?}"
    );
}

#[test]
fn strip_test_sets_slices_and_clears() {
    let env = Env::new();
    let mut child = env
        .command(["strip", "test", "--seconds", "2"])
        .spawn()
        .expect("spawn strip test");

    let spec = wait_for_slot(&env, "strip", Duration::from_secs(5)).expect("strip is set");
    let slices = opendeck_strip::status::parse_slices(&spec).expect("@slices spec");
    for index in 0..4 {
        let image = png(&slices.dir.join(format!("slice{index}.png")));
        assert_eq!(image.dimensions(), (200, 100), "slice {index}");
        assert!(has_ink(&image), "slice {index} is blank");
    }

    let status = wait_for_exit(&mut child, Duration::from_secs(10));
    if status.is_none() {
        let _ = child.kill();
    }
    assert!(status.expect("strip test exits").success());
    assert!(env.status_json().get("strip").is_none());
    assert!(!slices.dir.exists(), "slice directory is removed");
}

#[test]
fn raw_string_out_writes_an_800x128_png() {
    let env = Env::new();
    let out = env.dir.path().join("raw.png");
    env.run_ok(["raw-string", "hello", "--out", out.to_str().unwrap()]);
    let image = png(&out);
    assert_eq!(image.dimensions(), (800, 128));
    assert!(has_ink(&image), "frame is blank");
}

#[test]
fn raw_string_out_handles_four_segments() {
    let env = Env::new();
    let out = env.dir.path().join("raw4.png");
    env.run_ok([
        "raw-string",
        "ONE|TWO|[c=red]THREE[/c]|FOUR",
        "--out",
        out.to_str().unwrap(),
    ]);
    let image = png(&out);
    assert_eq!(image.dimensions(), (800, 128));
    for quarter in 0..4u32 {
        let part = image::imageops::crop_imm(&image, quarter * 200, 0, 200, 128).to_image();
        assert!(has_ink(&part), "segment {quarter} is blank");
    }
}

#[test]
fn raw_string_accepts_words_that_are_command_names() {
    let env = Env::new();
    let out = env.dir.path().join("clear.png");
    env.run_ok(["raw-string", "raw-clear", "--out", out.to_str().unwrap()]);
    assert_eq!(png(&out).dimensions(), (800, 128));
}

#[test]
fn raw_test_out_still_works() {
    let env = Env::new();
    let out = env.dir.path().join("test.png");
    env.run_ok(["raw-test", "--out", out.to_str().unwrap()]);
    assert_eq!(png(&out).dimensions(), (800, 128));
}

#[test]
fn the_raw_command_group_is_gone() {
    let env = Env::new();
    assert!(!env.run(["raw", "hello"]).status.success());
}
