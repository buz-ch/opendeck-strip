//! `follow`/`apply` against the `view=state` stream: the real binary, the real
//! golden documents, an isolated status file.

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_opendeck-strip");

const ONE_BLOCKED: &str = include_str!("zjsb/scenarios/one-blocked.json");
const IDLE: &str = include_str!("zjsb/scenarios/idle.json");
const DEFAULT_ONE_BLOCKED: &str = include_str!("zjsb/expected/default-one-blocked.txt");

struct Env {
    dir: TempDir,
    status: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = dir.path().join("status.json");
        fs::write(dir.path().join("config.toml"), "").expect("write config");
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
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.dir.path().join(".config"))
            .args(args);
        command
    }

    fn run_stdin<I, S>(&self, args: I, input: &str) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn binary");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        if let Err(err) = stdin.write_all(input.as_bytes()) {
            assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe, "{err}");
        }
        drop(stdin);
        child.wait_with_output().expect("wait for binary")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn status_json(&self) -> Value {
        let text = fs::read_to_string(&self.status).expect("status file");
        serde_json::from_str(&text).expect("status JSON")
    }
}

fn hello(seq: u64) -> String {
    json!({
        "v": 1,
        "type": "hello",
        "plugin": "zj-strip-bridge",
        "version": "0.1.0",
        "session": "work",
        "view": "state",
        "seq": seq,
    })
    .to_string()
}

fn state(seq: u64, doc: &str) -> String {
    let doc: Value = serde_json::from_str(doc).expect("scenario JSON");
    json!({"v": 1, "type": "state", "seq": seq, "doc": doc}).to_string()
}

fn expected_default_strip() -> String {
    DEFAULT_ONE_BLOCKED.replace("\\n", "\n")
}

#[test]
fn a_state_line_renders_the_default_strip_and_announces_raw() {
    let env = Env::new();
    let input = format!("{}\n{}\n", hello(0), state(1, ONE_BLOCKED));
    let output = env.run_stdin(["apply"], &input);
    assert!(
        output.status.success(),
        "apply failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let status = env.status_json();
    assert_eq!(status["strip"], expected_default_strip());
    assert_eq!(status["strip_mode"], "raw");
    for dial in ["dial1", "dial2", "dial3", "dial4"] {
        assert!(status.get(dial).is_none(), "{dial} is not written");
    }
}

fn wait_for_status(env: &Env, timeout: Duration, want: impl Fn(&Value) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(&env.status)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && want(&value)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn write_stdin(child: &mut Child, text: &str) {
    let stdin = child.stdin.as_mut().expect("stdin is piped");
    stdin.write_all(text.as_bytes()).expect("write stdin");
    stdin.flush().expect("flush stdin");
}

#[test]
fn the_same_state_twice_is_not_rewritten() {
    let env = Env::new();
    let mut child = env
        .command(["apply"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn apply");
    write_stdin(
        &mut child,
        &format!("{}\n{}\n", hello(0), state(1, ONE_BLOCKED)),
    );
    assert!(
        wait_for_status(&env, Duration::from_secs(10), |status| {
            status.get("strip").is_some()
        }),
        "the first state is written"
    );

    // A hand edit proves the second, identical state leaves the slot alone.
    let mut status = env.status_json();
    status["strip"] = Value::String("manual".to_owned());
    fs::write(&env.status, serde_json::to_string_pretty(&status).unwrap()).unwrap();

    write_stdin(&mut child, &format!("{}\n", state(2, ONE_BLOCKED)));
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("apply exits at EOF");
    assert!(output.status.success());
    assert_eq!(env.status_json()["strip"], "manual");
}

#[test]
fn blank_when_idle_removes_the_strip_and_the_dials() {
    let env = Env::new();
    fs::write(&env.status, r#"{"key1_1":"K"}"#).unwrap();
    let input = format!(
        "{}\n{}\n{}\n",
        hello(0),
        state(1, ONE_BLOCKED),
        state(2, IDLE)
    );
    let output = env
        .command(["apply"])
        .env("OPENDECK_STRIP_ZJSB_BLANK_WHEN_IDLE", "1")
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .expect("stdin is piped")
                .write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run apply");
    assert!(output.status.success());

    let status = env.status_json();
    for slot in ["strip", "dial1", "dial2", "dial3", "dial4"] {
        assert!(status.get(slot).is_none(), "{slot} is cleared");
    }
    assert_eq!(status["key1_1"], "K", "a foreign slot survives");
}

#[test]
fn compact_mode_writes_only_the_configured_slot() {
    let env = Env::new();
    let input = format!("{}\n{}\n", hello(0), state(1, ONE_BLOCKED));
    let output = env
        .command(["apply"])
        .env("OPENDECK_STRIP_ZJSB_MODE", "compact")
        .env("OPENDECK_STRIP_ZJSB_SLOT", "key1_2")
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .expect("stdin is piped")
                .write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run apply");
    assert!(output.status.success());

    let status = env.status_json();
    assert!(status.get("key1_2").is_some(), "the compact slot is set");
    assert!(status.get("strip").is_none());
    assert!(status.get("strip_mode").is_none());
}

#[test]
fn a_newer_state_version_exits_two_and_leaves_the_file_alone() {
    let env = Env::new();
    let before = r#"{"dial1":"keep"}"#;
    fs::write(&env.status, before).unwrap();
    let output = env.run_stdin(
        ["apply"],
        "{\"v\":2,\"type\":\"state\",\"seq\":0,\"doc\":{}}\n",
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(&env.status).unwrap(), before);
}

#[test]
fn a_state_gap_is_no_reason_to_reconnect_and_keeps_rendering() {
    let env = Env::new();
    let input = format!(
        "{}\n{}\n{}\n",
        hello(0),
        state(1, IDLE),
        state(9, ONE_BLOCKED)
    );
    let output = env.run_stdin(["apply"], &input);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(env.status_json()["strip"], expected_default_strip());
}

#[test]
fn the_deck_view_still_applies_a_deck_frame() {
    let env = Env::new();
    let input = concat!(
        r#"{"v":1,"type":"hello","plugin":"zj-strip-bridge","session":"work","view":"deck","seq":0}"#,
        "\n",
        r#"{"v":1,"type":"deck","seq":1,"full":true,"set":{"strip":"S","dial1":"D1"},"settings":{"strip_mode":"tiles"},"owned":["strip","dial1","dial2","dial3","dial4"]}"#,
        "\n"
    );
    let output = env.run_stdin(["apply", "--view", "deck"], input);
    assert!(output.status.success());
    let status = env.status_json();
    assert_eq!(status["strip"], "S");
    assert_eq!(status["dial1"], "D1");
    assert_eq!(status["strip_mode"], "tiles");
}

#[cfg(unix)]
fn fake_zellij(env: &Env, stream: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let stream_path = env.path("stream.ndjson");
    fs::write(&stream_path, stream).unwrap();
    let path = env.path("zellij");
    fs::write(
        &path,
        r#"#!/bin/sh
case "$1" in
  list-sessions)
    echo "spike [Created 1m ago]"
    exit 0
    ;;
  --session)
    ok=
    for arg in "$@"; do
      [ "$arg" = "view=deck" ] && { echo "deck view requested" >&2; exit 3; }
      [ "$arg" = "view=state" ] && ok=1
    done
    [ -n "$ok" ] || { echo "missing view=state" >&2; exit 3; }
    cat "$FAKE_STREAM"
    exit 0
    ;;
esac
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    stream_path
}

#[cfg(unix)]
#[test]
fn follow_reads_state_by_default_and_once_reports_a_write() {
    let env = Env::new();
    let stream = format!("{}\n{}\n", hello(0), state(1, ONE_BLOCKED));
    let stream_path = fake_zellij(&env, &stream);
    let fake = env.path("zellij");
    let output = env
        .command([
            "follow",
            "--zellij",
            fake.to_str().unwrap(),
            "--session",
            "spike",
            "--once",
        ])
        .env("FAKE_STREAM", &stream_path)
        .output()
        .expect("run follow");
    assert!(
        output.status.success(),
        "follow failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let status = env.status_json();
    assert_eq!(status["strip"], expected_default_strip());
    assert_eq!(status["strip_mode"], "raw");
}

#[cfg(unix)]
#[test]
fn follow_once_exits_one_when_no_state_is_written() {
    let env = Env::new();
    let stream_path = fake_zellij(&env, &(hello(0) + "\n"));
    let fake = env.path("zellij");
    let output = env
        .command([
            "follow",
            "--zellij",
            fake.to_str().unwrap(),
            "--session",
            "spike",
            "--once",
        ])
        .env("FAKE_STREAM", &stream_path)
        .output()
        .expect("run follow");
    assert_eq!(output.status.code(), Some(1));
}
