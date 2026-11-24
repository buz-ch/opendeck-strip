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

/// When `slot` left the status file within `timeout`.
fn wait_for_slot_gone(env: &Env, slot: &str, timeout: Duration) -> Option<Instant> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(&env.status)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && value.get(slot).is_none()
        {
            return Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// True when the status file satisfies `check` for the whole of `hold`.
fn status_holds(env: &Env, hold: Duration, check: impl Fn(&Value) -> bool) -> bool {
    let deadline = Instant::now() + hold;
    loop {
        let current = fs::read_to_string(&env.status)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        if !current.is_some_and(|value| check(&value)) {
            return false;
        }
        if Instant::now() >= deadline {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// True once the status file satisfies `check`; polls until `timeout`.
fn wait_for_status(env: &Env, timeout: Duration, check: impl Fn(&Value) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(&env.status)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && check(&value)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn slot_gone(value: &Value, slot: &str) -> bool {
    value.get(slot).is_none()
}

/// Start `apply-zellij --idle-clear 1`, feed it `input` and wait until the idle
/// timeout has cleared the owned slots. The child's stdin is still open.
fn apply_until_idle_clear(env: &Env, input: &str) -> std::process::Child {
    let mut child = env
        .command(["apply-zellij", "--idle-clear", "1", "--view", "deck"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn apply");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        stdin.write_all(input.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    assert_eq!(
        wait_for_slot(env, "strip", Duration::from_secs(5)).as_deref(),
        Some("S"),
        "the frame is applied before the idle timeout"
    );
    let cleared = wait_for_status(env, Duration::from_secs(8), |status| {
        slot_gone(status, "strip") && slot_gone(status, "dial1")
    });
    if !cleared {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(cleared, "idle-clear must remove strip and the dials");
    child
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

fn run_stdin<I, S>(env: &Env, args: I, input: &str) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use std::io::Write;
    let mut child = env
        .command(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("spawn binary");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // A command that refuses a frame may exit before the whole input is written.
    if let Err(err) = stdin.write_all(input.as_bytes()) {
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::BrokenPipe,
            "writing the test input failed: {err}"
        );
    }
    drop(stdin);
    child.wait_with_output().expect("wait for binary")
}

const FULL_FRAME: &str = concat!(
    r#"{"v":1,"type":"hello","plugin":"zj-strip-bridge","version":"0.1.0","session":"work","view":"deck","seq":0}"#,
    "\n",
    r#"{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"D1"},"settings":{"strip_mode":"tiles","labels":true,"strip_gap":12},"clear":[],"owned":["strip","dial1","dial2","dial3","dial4"]}"#,
    "\n",
    r#"{"v":1,"type":"deck","seq":2,"full":false,"set":{"dial2":"D2"},"settings":{},"clear":[]}"#,
    "\n"
);

#[test]
fn apply_applies_a_full_then_incremental_frame() {
    let env = Env::new();
    fs::write(&env.status, r#"{"key1_1":"K"}"#).unwrap();
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], FULL_FRAME);
    assert!(output.status.success());
    let status = env.status_json();
    assert_eq!(status["strip"], "S");
    assert_eq!(status["dial1"], "D1");
    assert_eq!(status["dial2"], "D2");
    assert_eq!(status["key1_1"], "K");
    assert_eq!(status["strip_mode"], "tiles");
    assert_eq!(status["labels"], Value::Bool(true));
    assert_eq!(status["strip_gap"], Value::Number(12.into()));
}

#[test]
fn apply_clear_names_follow_clear_semantics() {
    let env = Env::new();
    fs::write(
        &env.status,
        r#"{"strip":"S","dial1":"a","dial2":"b","key1_1":"K"}"#,
    )
    .unwrap();

    let frame = r#"{"v":1,"type":"deck","set":{},"clear":["strip"]}"#;
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], frame);
    assert!(output.status.success());

    let status = env.status_json();
    for key in ["strip", "dial1", "dial2"] {
        assert!(status.get(key).is_none(), "{key} must be removed");
    }
    assert_eq!(status["key1_1"], "K");
}

#[test]
fn apply_refuses_a_newer_frame_and_leaves_the_file_alone() {
    let env = Env::new();
    let before = r#"{"dial1":"keep"}"#;
    fs::write(&env.status, before).unwrap();

    let output = run_stdin(
        &env,
        ["apply-zellij", "--view", "deck"],
        "{\"v\":2,\"type\":\"deck\",\"full\":true,\"set\":{\"dial1\":\"changed\"}}\n",
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(env.status_text(), before);
}

#[test]
fn apply_skips_garbage_lines_and_keeps_going() {
    let env = Env::new();
    let input = concat!(
        "not json\n",
        r#"{"v":1,"type":"deck","set":{"dial1":"one"}}"#,
        "\n",
        "[1,2,3]\n",
        r#"{"v":1,"type":"deck","set":{"dial2":"two"}}"#,
        "\n"
    );
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], input);
    assert!(output.status.success());
    let status = env.status_json();
    assert_eq!(status["dial1"], "one");
    assert_eq!(status["dial2"], "two");
}

#[test]
fn apply_ignores_known_non_deck_types_and_unknown_types() {
    let env = Env::new();
    let before = r#"{"dial1":"keep"}"#;
    fs::write(&env.status, before).unwrap();
    let input = concat!(
        r#"{"v":1,"type":"hello"}"#,
        "\n",
        r#"{"v":1,"type":"ping"}"#,
        "\n",
        r#"{"v":1,"type":"state","doc":{}}"#,
        "\n",
        r#"{"v":1,"type":"something_new"}"#,
        "\n"
    );
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], input);
    assert!(output.status.success());
    assert_eq!(env.status_text(), before);
}

#[test]
fn apply_idle_clear_removes_the_owned_slots() {
    let env = Env::new();
    let mut child = apply_until_idle_clear(&env, FULL_FRAME);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn zero_second_durations_are_usage_errors() {
    let env = Env::new();
    assert_eq!(
        run_stdin(
            &env,
            ["apply-zellij", "--idle-clear", "0", "--view", "deck"],
            ""
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        env.run(["follow-zellij", "--stall-secs", "0"])
            .status
            .code(),
        Some(2)
    );
}

/// What `apply-zellij --idle-clear` must leave alone: a slot nobody owns, a setting the
/// frame did not carry and the settings it did.
fn assert_unowned_state_survives(env: &Env) {
    let status = env.status_json();
    assert_eq!(status["key1_1"], "K", "a foreign slot survives");
    assert_eq!(status["brightness"], 7, "a foreign setting survives");
    assert_eq!(status["strip_mode"], "tiles", "a frame setting survives");
    assert_eq!(status["strip_gap"], 12);
    assert_eq!(status["labels"], Value::Bool(true));
    for slot in ["strip", "dial1", "dial2", "dial3"] {
        assert!(slot_gone(&status, slot), "{slot} is owned and cleared");
    }
}

const SEEDED_STATUS: &str = r#"{"key1_1":"K","brightness":7}"#;

#[test]
fn apply_idle_clear_removes_only_the_owned_slots() {
    let env = Env::new();
    fs::write(&env.status, SEEDED_STATUS).unwrap();

    let mut child = apply_until_idle_clear(&env, FULL_FRAME);
    assert_unowned_state_survives(&env);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn apply_idle_clear_clears_once_and_keeps_reading() {
    let env = Env::new();
    fs::write(&env.status, SEEDED_STATUS).unwrap();

    let mut child = apply_until_idle_clear(&env, FULL_FRAME);

    // Cleared once per silent stretch: the next interval must not clear again.
    env.run_ok(["set", "dial1", "manual"]);
    assert!(
        status_holds(&env, Duration::from_millis(2500), |status| status["dial1"]
            == "manual"),
        "nothing is cleared again while no frame arrives"
    );

    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        writeln!(
            stdin,
            r#"{{"v":1,"type":"deck","seq":2,"full":false,"set":{{"dial2":"after"}},"settings":{{}},"clear":[]}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }
    assert!(
        wait_for_status(&env, Duration::from_secs(5), |status| status["dial2"]
            == "after"),
        "a frame after the clear is still applied"
    );
    assert_eq!(
        env.status_json()["dial1"],
        "manual",
        "the delta sets only dial2"
    );

    drop(child.stdin.take());
    let status = wait_for_exit(&mut child, Duration::from_secs(10)).expect("apply exits at EOF");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn apply_applies_a_delta_after_a_seq_gap_and_exits_zero() {
    let env = Env::new();
    let input = concat!(
        r#"{"v":1,"type":"hello","seq":0}"#,
        "\n",
        r#"{"v":1,"type":"deck","seq":1,"full":true,"set":{"dial1":"one"},"owned":["dial1"]}"#,
        "\n",
        r#"{"v":1,"type":"deck","seq":3,"full":false,"set":{"dial2":"two"}}"#,
        "\n"
    );
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], input);
    assert_eq!(output.status.code(), Some(0));
    let status = env.status_json();
    assert_eq!(status["dial1"], "one");
    assert_eq!(
        status["dial2"], "two",
        "the delta is applied, never dropped or taken as full"
    );
}

#[test]
fn apply_skips_a_deck_line_with_a_non_boolean_full() {
    let env = Env::new();
    let input = concat!(
        r#"{"v":1,"type":"deck","seq":1,"full":"true","set":{"dial1":"one"},"owned":["dial1"]}"#,
        "\n",
        r#"{"v":1,"type":"deck","seq":2,"full":false,"set":{"dial2":"two"}}"#,
        "\n"
    );
    let output = run_stdin(&env, ["apply-zellij", "--view", "deck"], input);
    assert_eq!(output.status.code(), Some(0));
    let status = env.status_json();
    assert!(
        status.get("dial1").is_none(),
        "the mis-typed frame is skipped"
    );
    assert_eq!(status["dial2"], "two");
}

#[cfg(unix)]
fn write_fake_zellij(dir: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("zellij");
    fs::write(&path, script).expect("write fake zellij");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake zellij");
    path
}

#[cfg(unix)]
fn fake_path(bin: &Path) -> std::ffi::OsString {
    std::env::join_paths([
        bin.to_path_buf(),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .expect("join PATH")
}

#[cfg(unix)]
const FAKE_ZELLIJ: &str = r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "old [Created 2m 10s ago] (EXITED - attach to resurrect)"
    echo "work [Created 3m 10s ago]"
    exit 0
    ;;
  --session)
    printf '%s\n' "$@" > "${FAKE_ARGV:-/dev/null}"
    if [ "$2" != "work" ]; then
      echo "wrong session $2" >&2
      exit 1
    fi
    printf '%s\n' \
      '{"v":1,"type":"hello","plugin":"zj-strip-bridge","version":"0.1.0","session":"work","view":"deck","seq":0}' \
      '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"D1"},"settings":{"strip_mode":"tiles"},"clear":[],"owned":["strip","dial1","dial2","dial3","dial4"]}' \
      '{"v":1,"type":"ping","seq":2}'
    exit 0
    ;;
esac
exit 1
"#;

#[cfg(unix)]
#[test]
fn follow_once_applies_frames_from_a_fake_zellij() {
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir_all(&bin).unwrap();
    write_fake_zellij(&bin, FAKE_ZELLIJ);

    let output = env
        .command(["follow-zellij", "--once", "--view", "deck"])
        .env("PATH", fake_path(&bin))
        .output()
        .expect("run follow");
    assert!(
        output.status.success(),
        "follow failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let status = env.status_json();
    assert_eq!(status["strip"], "S");
    assert_eq!(status["dial1"], "D1");
    assert_eq!(status["strip_mode"], "tiles");
}

/// The one command `follow-zellij` starts: session, pipe name, deck view and the
/// payload that makes a held-open stdin invoke the plugin at all.
#[cfg(unix)]
fn assert_follow_argv(env: &Env) {
    assert_eq!(
        fs::read_to_string(env.path("argv")).unwrap(),
        "--session\nwork\npipe\n--name\nzjsb.follow\n-a\nview=deck\nstart\n"
    );
}

#[cfg(unix)]
fn stop_follow(child: &mut std::process::Child) -> std::process::ExitStatus {
    // Safety: the pid is live (try_wait has not reported it dead).
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = wait_for_exit(child, Duration::from_secs(10));
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    status.expect("follow exits after SIGTERM")
}

#[cfg(unix)]
#[test]
fn follow_pinned_live_session_is_followed_with_the_documented_argv() {
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir_all(&bin).unwrap();
    write_fake_zellij(&bin, FAKE_ZELLIJ);

    let output = env
        .command([
            "follow-zellij",
            "--once",
            "--session",
            "work",
            "--view",
            "deck",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_ARGV", env.path("argv"))
        .output()
        .expect("run follow");
    assert!(
        output.status.success(),
        "follow failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_follow_argv(&env);
    assert_eq!(env.status_json()["strip"], "S");
}

/// A fake whose pipe prints nothing and fails at once; it counts its calls.
#[cfg(unix)]
fn write_silent_zellij(bin: &Path) {
    write_fake_zellij(
        bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    n=$(cat "$FAKE_CALLS" 2>/dev/null || echo 0)
    echo $((n + 1)) > "$FAKE_CALLS"
    exit 1
    ;;
esac
exit 1
"#,
    );
}

#[cfg(unix)]
fn fake_bin(env: &Env) -> PathBuf {
    let bin = env.path("bin");
    fs::create_dir_all(&bin).unwrap();
    bin
}

#[cfg(unix)]
#[test]
fn follow_backs_off_while_no_line_arrives() {
    let env = Env::new();
    let bin = fake_bin(&env);
    write_silent_zellij(&bin);
    let calls = env.path("calls");
    let mut child = env
        .command([
            "follow-zellij",
            "--view",
            "deck",
            "--retry-secs",
            "1,3",
            "--stall-secs",
            "60",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_CALLS", &calls)
        .spawn()
        .expect("spawn follow");
    std::thread::sleep(Duration::from_millis(5200));
    let _ = child.kill();
    let _ = child.wait();
    let attempts = fs::read_to_string(&calls).unwrap_or_default();
    let attempts = attempts.trim().parse::<usize>().unwrap_or(0);
    assert!(
        (2..=4).contains(&attempts),
        "backoff 1,2,3 gives about 4 attempts in 5s, got {attempts}"
    );
}

#[cfg(unix)]
#[test]
fn follow_stall_clears_across_reconnects() {
    let env = Env::new();
    let bin = fake_bin(&env);
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    n=$(cat "$FAKE_CALLS" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo $n > "$FAKE_CALLS"
    if [ "$n" = 1 ]; then
      printf '%s\n' \
        '{"v":1,"type":"hello","seq":0}' \
        '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S"},"owned":["strip","dial1","dial2","dial3","dial4"]}'
    fi
    exit 1
    ;;
esac
exit 1
"#,
    );
    let calls = env.path("calls");

    let mut child = env
        .command([
            "follow-zellij",
            "--view",
            "deck",
            "--stall-secs",
            "1",
            "--retry-secs",
            "1,2",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_CALLS", &calls)
        .spawn()
        .expect("spawn follow");
    assert_eq!(
        wait_for_slot(&env, "strip", Duration::from_secs(5)).as_deref(),
        Some("S"),
        "the first connection applies its frame"
    );
    let cleared = wait_for_status(&env, Duration::from_secs(8), |status| {
        slot_gone(status, "strip")
    });
    if cleared {
        let _ = stop_follow(&mut child);
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        cleared,
        "a producer silent for --stall-secs is cleared although every connection ends at once"
    );
}

/// The stall clock counts from the last line, not from the last connection:
/// after a frame and a long backoff, a hung connection is cleared as soon as
/// `--stall-secs` have passed since that frame, not `--stall-secs` after it began.
#[cfg(unix)]
#[test]
fn follow_stall_clock_spans_connections() {
    let env = Env::new();
    let bin = fake_bin(&env);
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    n=$(cat "$FAKE_CALLS" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo $n > "$FAKE_CALLS"
    if [ "$n" = 1 ]; then
      printf '%s\n' \
        '{"v":1,"type":"hello","seq":0}' \
        '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S"},"owned":["strip","dial1","dial2","dial3","dial4"]}'
      exit 0
    fi
    exec sleep 100
    ;;
esac
exit 1
"#,
    );
    let calls = env.path("calls");

    let mut child = env
        .command([
            "follow-zellij",
            "--view",
            "deck",
            "--stall-secs",
            "2",
            "--retry-secs",
            "3,3",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_CALLS", &calls)
        .spawn()
        .expect("spawn follow");
    let first = wait_for_slot(&env, "strip", Duration::from_secs(5));
    let started = Instant::now();
    let cleared_at = first
        .as_ref()
        .and_then(|_| wait_for_slot_gone(&env, "strip", Duration::from_secs(8)));
    let exit = if cleared_at.is_some() {
        Some(stop_follow(&mut child))
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    };
    assert!(first.is_some(), "the first connection applies its frame");
    let since_frame =
        cleared_at.expect("a hung connection is cleared after --stall-secs") - started;
    assert!(
        since_frame < Duration::from_secs(3),
        "cleared {since_frame:?} after the frame; the backoff must not delay the stall clear"
    );
    assert!(exit.expect("stopped").success());
}

#[cfg(unix)]
#[test]
fn follow_seq_gap_reconnects_without_applying_the_delta() {
    let env = Env::new();
    let bin = fake_bin(&env);
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    n=$(cat "$FAKE_CALLS" 2>/dev/null || echo 0)
    n=$((n + 1))
    echo $n > "$FAKE_CALLS"
    if [ "$n" = 1 ]; then
      printf '%s\n' \
        '{"v":1,"type":"hello","seq":0}' \
        '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"D1"},"owned":["strip","dial1"]}' \
        '{"v":1,"type":"deck","seq":5,"full":false,"set":{"dial2":"LOST"}}'
    else
      printf '%s\n' \
        '{"v":1,"type":"hello","seq":0}' \
        '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"again"},"owned":["strip","dial1"]}'
    fi
    exec sleep 100
    ;;
esac
exit 1
"#,
    );
    let calls = env.path("calls");

    let mut child = env
        .command([
            "follow-zellij",
            "--view",
            "deck",
            "--retry-secs",
            "1,1",
            "--stall-secs",
            "60",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_CALLS", &calls)
        .spawn()
        .expect("spawn follow");
    let reconnected = wait_for_status(&env, Duration::from_secs(10), |status| {
        status["dial1"] == "again"
    });
    let exit = if reconnected {
        Some(stop_follow(&mut child))
    } else {
        let _ = child.kill();
        let _ = child.wait();
        None
    };
    assert!(reconnected, "a seq gap ends the connection and reconnects");
    assert!(exit.expect("stopped").success());
    let status = env.status_json();
    assert!(
        slot_gone(&status, "dial2"),
        "the delta after the gap is not applied"
    );
}

#[cfg(unix)]
#[test]
fn follow_pinned_dead_session_clears_after_the_grace_period() {
    let env = Env::new();
    let bin = fake_bin(&env);
    let gone = env.path("gone");
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    [ -e "$FAKE_GONE" ] || echo "work [Created 1m ago]"
    echo "other [Created 1m ago]"
    exit 0
    ;;
  --session)
    printf '%s\n' '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"D1"},"owned":["strip","dial1","dial2","dial3","dial4"]}'
    while [ ! -e "$FAKE_GONE" ]; do sleep 0.1; done
    exit 0
    ;;
esac
exit 1
"#,
    );
    fs::write(&env.status, r#"{"key1_1":"K"}"#).unwrap();

    let mut child = env
        .command([
            "follow-zellij",
            "--view",
            "deck",
            "--session",
            "work",
            "--grace-secs",
            "1",
            "--retry-secs",
            "1,5",
        ])
        .env("PATH", fake_path(&bin))
        .env("FAKE_GONE", &gone)
        .spawn()
        .expect("spawn follow");
    assert_eq!(
        wait_for_slot(&env, "strip", Duration::from_secs(5)).as_deref(),
        Some("S"),
        "the pinned session is followed while it is live"
    );
    // The session goes away: its pipe ends and it is no longer listed.
    fs::write(&gone, "").unwrap();
    let started = Instant::now();
    let cleared_at = wait_for_slot_gone(&env, "strip", Duration::from_secs(10));
    if let Some(at) = cleared_at {
        // Nothing is cleared before the grace period has passed.
        assert!(at - started >= Duration::from_millis(900));
    }
    if child.try_wait().expect("try_wait").is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        cleared_at.is_some(),
        "a pinned session that is not live is cleared after --grace-secs"
    );
    let status = env.status_json();
    assert!(slot_gone(&status, "dial1"));
    assert_eq!(status["key1_1"], "K");
}

#[cfg(unix)]
#[test]
fn follow_keeps_retrying_when_sessions_cannot_be_listed() {
    let env = Env::new();
    let bin = fake_bin(&env);
    let ready = env.path("ready");
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    [ -e "$FAKE_READY" ] || exit 2
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    printf '%s\n' '{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S"},"owned":["strip"]}'
    exec sleep 100
    ;;
esac
exit 1
"#,
    );

    let mut child = env
        .command(["follow-zellij", "--view", "deck", "--retry-secs", "1,1"])
        .env("PATH", fake_path(&bin))
        .env("FAKE_READY", &ready)
        .spawn()
        .expect("spawn follow");
    std::thread::sleep(Duration::from_millis(1500));
    let alive = child.try_wait().expect("try_wait").is_none();
    fs::write(&ready, "").unwrap();
    let followed = alive && wait_for_slot(&env, "strip", Duration::from_secs(8)).is_some();
    if followed {
        let _ = stop_follow(&mut child);
    } else {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(alive, "a failed list-sessions is retried, not fatal");
    assert!(
        followed,
        "follow connects once sessions can be listed again"
    );
}

#[cfg(unix)]
#[test]
fn follow_once_exits_one_when_no_frame_lands() {
    let env = Env::new();
    let bin = fake_bin(&env);
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    exit 0
    ;;
esac
exit 1
"#,
    );
    let output = env
        .command(["follow-zellij", "--once", "--view", "deck"])
        .env("PATH", fake_path(&bin))
        .output()
        .expect("run follow");
    assert_eq!(
        output.status.code(),
        Some(1),
        "a stream that ends without a frame exits 1"
    );

    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "No active zellij sessions found." >&2
    exit 1
    ;;
esac
exit 1
"#,
    );
    let output = env
        .command(["follow-zellij", "--once", "--view", "deck"])
        .env("PATH", fake_path(&bin))
        .output()
        .expect("run follow");
    assert_eq!(output.status.code(), Some(1), "no live session exits 1");
}

#[cfg(unix)]
#[test]
fn follow_prefers_the_live_session_name_from_the_environment() {
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir_all(&bin).unwrap();
    let chosen = env.path("chosen");
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 3m 10s ago]"
    echo "other [Created 1m ago]"
    exit 0
    ;;
  --session)
    echo "$2" > "$FAKE_SESSION_FILE"
    printf '%s\n' '{"v":1,"type":"deck","full":true,"set":{"dial1":"picked"},"owned":["dial1"]}'
    exit 0
    ;;
esac
exit 1
"#,
    );

    // The live `$ZELLIJ_SESSION_NAME` wins over the first live line ("work").
    let output = env
        .command(["follow-zellij", "--once", "--view", "deck"])
        .env("PATH", fake_path(&bin))
        .env("ZELLIJ_SESSION_NAME", "other")
        .env("FAKE_SESSION_FILE", &chosen)
        .output()
        .expect("run follow");
    assert!(
        output.status.success(),
        "follow failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&chosen).unwrap().trim(), "other");
    assert_eq!(env.status_json()["dial1"], "picked");
}

#[cfg(unix)]
#[test]
fn follow_clears_the_owned_slots_on_sigterm() {
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(&env.status, r#"{"key1_1":"K","brightness":7}"#).unwrap();
    write_fake_zellij(
        &bin,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "work [Created 1m ago]"
    exit 0
    ;;
  --session)
    printf '%s\n' '{"v":1,"type":"deck","full":true,"set":{"strip":"S","dial1":"D1"},"owned":["strip","dial1","dial2","dial3","dial4"]}'
    exec sleep 100
    ;;
esac
exit 1
"#,
    );

    let mut child = env
        .command(["follow-zellij", "--view", "deck", "--clear-on-exit"])
        .env("PATH", fake_path(&bin))
        .spawn()
        .expect("spawn follow");
    assert_eq!(
        wait_for_slot(&env, "strip", Duration::from_secs(5)).as_deref(),
        Some("S"),
        "the frame is applied before the signal"
    );

    // Safety: the pid is live (try_wait has not reported it dead).
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let status = wait_for_exit(&mut child, Duration::from_secs(10));
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(status.expect("follow exits after SIGTERM").success());
    let status = env.status_json();
    assert!(status.get("strip").is_none(), "strip is cleared on exit");
    assert!(status.get("dial1").is_none(), "dial1 is cleared on exit");
    assert_eq!(status["key1_1"], "K", "a foreign slot survives");
    assert_eq!(status["brightness"], 7, "a foreign setting survives");
}
