use std::time::{Duration, Instant};

use serde_json::json;

use super::*;
use crate::status::StripMode;

const INTERVAL: Duration = Duration::from_millis(1000);

fn config() -> Config {
    Config::default()
}

fn doc(app: &str, state: &str, on_deck: bool) -> Document {
    let value = json!({
        "version": 2,
        "generated_ms": 1_800_000_000_000u64,
        "attention_enabled": true,
        "sessions": [{
            "name": "work",
            "is_current": true,
            "panes": [{
                "source": "program_status",
                "tab": {"id": 0, "name": null},
                "pane": {"id": 1, "title": app},
                "state": state,
                "kind": "permission",
                "app": app,
                "msg": "write src/auth.rs",
                "since_ms": 1_799_999_905_000u64,
                "on_deck": on_deck
            }]
        }]
    });
    serde_json::from_value::<Document>(value).expect("document")
}

fn idle() -> Document {
    serde_json::from_value(json!({
        "version": 2,
        "attention_enabled": true,
        "sessions": [{"name": "work", "is_current": true, "panes": []}]
    }))
    .expect("document")
}

fn text(status: &Status, key: &str) -> Option<String> {
    status
        .get_slot_str(key)
        .unwrap()
        .and_then(|value| value.text().map(str::to_owned))
}

#[test]
fn the_first_frame_is_written_immediately_with_raw_mode() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();

    writer.document(&mut status, &doc("claude", "blocked", true), 0, t0);

    assert_eq!(
        writer.owned(),
        ["strip", "dial1", "dial2", "dial3", "dial4"]
    );
    assert!(text(&status, "strip").is_some());
    assert_eq!(status.strip_mode(), Some(StripMode::Raw));
    assert_eq!(writer.applied(), 1);
    assert_eq!(writer.deadline(), None);
}

#[test]
fn a_change_inside_the_interval_is_held_until_the_deadline() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();
    writer.document(&mut status, &doc("a", "blocked", true), 0, t0);
    let first = text(&status, "strip").expect("strip written");

    writer.document(
        &mut status,
        &doc("b", "working", true),
        0,
        t0 + INTERVAL / 2,
    );

    assert_eq!(text(&status, "strip").as_deref(), Some(first.as_str()));
    assert_eq!(writer.applied(), 1);
    assert_eq!(writer.deadline(), Some(t0 + INTERVAL));
    assert!(!writer.flush_due(&mut status, t0 + INTERVAL - Duration::from_millis(1)));
    assert!(writer.flush_due(&mut status, t0 + INTERVAL));
    assert_ne!(text(&status, "strip").as_deref(), Some(first.as_str()));
    assert_eq!(writer.applied(), 2);
}

#[test]
fn the_latest_held_frame_wins() {
    let config = config();
    let mut writer = Writer::new(&config);
    let mut status = Status::empty();
    let t0 = Instant::now();
    writer.document(&mut status, &doc("a", "blocked", true), 0, t0);
    let want_b = zjsb::render(
        &doc("b", "working", true),
        &zjsb::Options::from_config(&config),
        0,
    );
    let want_c = zjsb::render(
        &doc("c", "done", true),
        &zjsb::Options::from_config(&config),
        0,
    );
    writer.document(
        &mut status,
        &doc("b", "working", true),
        0,
        t0 + INTERVAL / 4,
    );
    writer.document(&mut status, &doc("c", "done", true), 0, t0 + INTERVAL / 2);

    assert!(writer.flush_due(&mut status, t0 + INTERVAL));
    assert_ne!(text(&status, "strip"), want_b);
    assert_eq!(
        text(&status, "strip"),
        want_c.map(|text| text.replace("\\n", "\n"))
    );
}

#[test]
fn an_unchanged_frame_is_dropped_and_clears_a_held_one() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();
    let same = doc("a", "blocked", true);
    writer.document(&mut status, &same, 0, t0);
    assert_eq!(writer.applied(), 1);

    writer.document(&mut status, &same, 0, t0 + INTERVAL / 2);
    assert_eq!(writer.applied(), 1, "no second write");
    assert_eq!(writer.deadline(), None, "no frame held");

    // A held change is dropped when the state returns to what is shown.
    writer.document(
        &mut status,
        &doc("b", "working", true),
        0,
        t0 + INTERVAL / 4,
    );
    assert_eq!(writer.deadline(), Some(t0 + INTERVAL));
    writer.document(&mut status, &same, 0, t0 + INTERVAL / 2);
    assert_eq!(writer.deadline(), None, "the held change is dropped");
    assert!(!writer.flush_due(&mut status, t0 + INTERVAL * 10));
    assert_eq!(writer.applied(), 1);
    assert!(text(&status, "strip").is_some());
}

#[test]
fn raw_mode_is_announced_once() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();
    writer.document(&mut status, &doc("a", "blocked", true), 0, t0);
    assert_eq!(status.strip_mode(), Some(StripMode::Raw));

    status.remove("strip_mode");
    writer.document(&mut status, &doc("b", "working", true), 0, t0 + INTERVAL);
    assert!(
        status.raw_value("strip_mode").is_none(),
        "raw mode was already announced"
    );
    assert!(text(&status, "strip").is_some());
}

#[test]
fn compact_mode_owns_only_its_slot() {
    let mut config = config();
    config.zjsb_mode = "compact".to_owned();
    config.zjsb_slot = "key1_2".to_owned();
    let mut writer = Writer::new(&config);
    let mut status = Status::empty();

    writer.document(&mut status, &doc("a", "blocked", true), 0, Instant::now());

    assert_eq!(writer.owned(), ["key1_2"]);
    assert!(text(&status, "key1_2").is_some());
    assert!(status.get_slot_str("strip").unwrap().is_none());
    assert!(status.raw_value("strip_mode").is_none());
}

#[test]
fn blank_when_idle_removes_the_owned_slots() {
    let mut config = config();
    config.zjsb_blank_when_idle = true;
    let mut writer = Writer::new(&config);
    let mut status = Status::empty();
    status.set_slot("dial2", crate::status::SlotValue::Text("stale".into()));
    let t0 = Instant::now();
    writer.document(&mut status, &doc("a", "blocked", true), 0, t0);
    assert!(text(&status, "strip").is_some());

    writer.document(&mut status, &idle(), 0, t0 + INTERVAL);

    assert!(status.get_slot_str("strip").unwrap().is_none());
    for slot in ["dial1", "dial2", "dial3", "dial4"] {
        assert!(
            status.get_slot_str(slot).unwrap().is_none(),
            "{slot} is removed"
        );
    }
    assert_eq!(writer.applied(), 2);
}

#[test]
fn an_idle_document_writes_nothing_while_the_slot_is_known_clear() {
    let mut config = config();
    config.zjsb_blank_when_idle = true;
    let mut writer = Writer::new(&config);
    let mut status = Status::empty();

    writer.document(&mut status, &idle(), 0, Instant::now());

    assert_eq!(writer.applied(), 0);
    assert_eq!(writer.deadline(), None);
    assert!(status.raw().is_empty());
}

#[test]
fn an_external_clear_resets_what_is_shown() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();
    let same = doc("a", "blocked", true);
    writer.document(&mut status, &same, 0, t0);

    for slot in writer.owned() {
        cli::remove_slot(&mut status, slot);
    }
    writer.cleared();

    writer.document(&mut status, &same, 0, t0 + INTERVAL);
    assert_eq!(writer.applied(), 2);
    assert!(
        text(&status, "strip").is_some(),
        "the same state is written again"
    );
}

#[test]
fn a_newer_state_version_is_refused() {
    assert!(matches!(
        parse_state_line(r#"{"v":2,"type":"state","doc":{}}"#),
        Err(ParseError::TooOld)
    ));
}

#[test]
fn state_lines_without_a_document_are_skipped() {
    for line in [
        r#"{"v":1,"type":"state"}"#,
        r#"{"v":1,"type":"state","doc":7}"#,
        r#"{"v":1,"type":"state","doc":{"sessions":"no"}}"#,
        r#"{"type":"state","doc":{}}"#,
        r#"{"v":"1","type":"state","doc":{}}"#,
        "not json",
    ] {
        assert_eq!(
            parse_state_line(line).err(),
            Some(ParseError::Skip),
            "{line}"
        );
    }
}

#[test]
fn non_state_lines_are_ignored() {
    for line in [
        r#"{"v":1,"type":"hello","plugin":"zj-strip-bridge","view":"state","seq":0}"#,
        r#"{"v":1,"type":"ping","seq":3}"#,
        r#"{"v":1,"type":"error","message":"boom"}"#,
        r#"{"v":1,"type":"something_new"}"#,
    ] {
        let parsed = parse_state_line(line).expect("ignored");
        assert!(parsed.doc.is_none(), "{line}");
    }
}

#[test]
fn a_state_line_carries_its_document_and_seq() {
    let line = json!({
        "v": 1,
        "type": "state",
        "seq": 4,
        "doc": {"version": 2, "attention_enabled": true, "sessions": []}
    })
    .to_string();
    let parsed = parse_state_line(&line).expect("parses");
    assert_eq!(parsed.seq, Some(4));
    assert_eq!(parsed.doc.unwrap().version, 2);
}

#[test]
fn epoch_ms_is_a_plausible_epoch() {
    assert!(epoch_ms() > 1_600_000_000_000);
}

#[test]
fn an_unchanged_compact_frame_is_dropped_too() {
    let mut config = config();
    config.zjsb_mode = "compact".to_owned();
    config.zjsb_slot = "key1_2".to_owned();
    let mut writer = Writer::new(&config);
    let mut status = Status::empty();
    let t0 = Instant::now();
    let same = doc("a", "blocked", true);
    writer.document(&mut status, &same, 0, t0);
    assert_eq!(writer.applied(), 1);
    writer.document(&mut status, &same, 0, t0 + INTERVAL * 10);
    assert_eq!(
        writer.applied(),
        1,
        "unchanged compact text must not be rewritten"
    );
}

#[test]
fn staging_a_document_never_touches_the_status_file() {
    let mut writer = Writer::new(&config());
    let mut status = Status::empty();
    let t0 = Instant::now();
    let same = doc("a", "blocked", true);
    writer.document(&mut status, &same, 0, t0);

    writer.stage(&same, 0);
    assert!(
        !writer.pending_due(t0 + INTERVAL * 10),
        "unchanged: nothing staged"
    );

    writer.stage(&doc("b", "working", true), 0);
    assert!(
        writer.pending_due(t0 + INTERVAL * 10),
        "changed: staged for the next write"
    );
}
