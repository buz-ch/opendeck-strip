//! Deck frames from `zj-strip-bridge` over `zellij pipe --name zjsb.follow -a view=deck`.
//!
//! One line is one JSON object. Every object carries `v` and `type`; `seq` increases
//! by one per line of a stream. A `deck` frame is a delta unless `full` is set.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::cli;
use crate::status::{SlotValue, Status};

/// Frame versions this build understands. A higher `v` is refused, never guessed at.
pub const KNOWN_VERSION: u64 = 1;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeckFrame {
    pub v: u64,
    pub seq: Option<u64>,
    pub full: bool,
    pub set: BTreeMap<String, Value>,
    pub settings: BTreeMap<String, Value>,
    pub clear: Vec<String>,
    pub owned: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Not a JSON object: log it and read the next line.
    Skip,
    /// The producer speaks a newer protocol; stop and exit 2.
    TooOld,
}

/// A `hello`, `ping`, `state` or unknown line yields `Ok(None)`; only `deck` yields a frame.
pub fn parse_line(line: &str) -> Result<Option<DeckFrame>, ParseError> {
    Ok(parse_parts(line)?.0)
}

/// Like [`parse_line`], also returning the `seq` of a line that is not a deck
/// frame, so a consumer can notice a gap whatever the line's type.
pub fn parse_parts(line: &str) -> Result<(Option<DeckFrame>, Option<u64>), ParseError> {
    let value: Value = serde_json::from_str(line).map_err(|_| ParseError::Skip)?;
    let Value::Object(mut map) = value else {
        return Err(ParseError::Skip);
    };
    let seq = map.get("seq").and_then(Value::as_u64);
    let v = map.get("v").and_then(Value::as_u64);
    if v.is_some_and(|v| v > KNOWN_VERSION) {
        return Err(ParseError::TooOld);
    }
    let kind = map.get("type").and_then(Value::as_str);
    if kind == Some("hello") {
        log::info!(
            "deck stream: plugin {} version {} session {} view {}",
            map.get("plugin").and_then(Value::as_str).unwrap_or("?"),
            map.get("version").and_then(Value::as_str).unwrap_or("?"),
            map.get("session").and_then(Value::as_str).unwrap_or("?"),
            map.get("view").and_then(Value::as_str).unwrap_or("?")
        );
        return Ok((None, seq));
    }
    if kind != Some("deck") {
        return Ok((None, seq));
    }
    // Without a version a frame could belong to any protocol; never guess one.
    let Some(v) = v else {
        return Err(ParseError::Skip);
    };
    // An absent `full` is a delta; a mis-typed one must not turn a full frame into a delta.
    let full = match map.get("full") {
        None => false,
        Some(Value::Bool(full)) => *full,
        Some(_) => return Err(ParseError::Skip),
    };
    Ok((
        Some(DeckFrame {
            v,
            full,
            seq,
            set: take_map(map.remove("set")),
            settings: take_map(map.remove("settings")),
            clear: take_list(map.remove("clear")),
            owned: take_list(map.remove("owned")),
        }),
        seq,
    ))
}

fn take_map(value: Option<Value>) -> BTreeMap<String, Value> {
    match value {
        Some(Value::Object(map)) => map.into_iter().collect(),
        _ => BTreeMap::new(),
    }
}

fn take_list(value: Option<Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(name) => Some(name),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Where a stream's numbering stands, to notice a gap or a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Seq {
    /// No numbered line seen yet.
    #[default]
    Start,
    /// The next numbered line must carry this `seq`.
    Next(u64),
    /// A line was dropped or unparsable; the next numbered line cannot be trusted.
    Lost,
}

impl Seq {
    /// Feed one line's `seq` (`None` for lines without one); true when the stream
    /// skipped or repeated a number, so a frame in between may have been lost.
    pub fn check(&mut self, seq: Option<u64>) -> bool {
        let Some(seq) = seq else {
            return false;
        };
        let (next, gap) = match *self {
            Self::Start => (seq + 1, false),
            Self::Next(expected) => (seq + 1, seq != expected),
            Self::Lost => (seq + 1, true),
        };
        *self = Self::Next(next);
        gap
    }

    /// A line was received but could not be read; its number is unknown.
    pub fn lost(&mut self) {
        if matches!(self, Self::Next(_)) {
            *self = Self::Lost;
        }
    }
}

/// Apply one frame in a single read-modify-write.
///
/// A full frame first removes every owned slot it does not set. `set` values are
/// the `set` syntax (markup on, `\n` unescaped); `settings` are typed top-level
/// keys; `clear` uses `clear` semantics, so `strip` also removes the four dials.
pub fn apply_frame(status: &mut Status, frame: &DeckFrame) {
    if frame.full {
        for slot in &frame.owned {
            if !frame.set.contains_key(slot) {
                cli::remove_slot(status, slot);
            }
        }
    }
    for (key, value) in &frame.set {
        match value {
            Value::String(raw) => cli::apply_set(status, key, raw, false),
            Value::Object(_) => match serde_json::from_value::<SlotValue>(value.clone()) {
                Ok(slot) => cli::apply_slot(status, key, slot),
                Err(err) => log::warn!("ignoring slot {key}: {err}"),
            },
            other => {
                log::warn!("ignoring slot {key}: expected a string or an object, got {other}")
            }
        }
    }
    for (key, value) in &frame.settings {
        cli::apply_setting(status, key, value);
    }
    for slot in &frame.clear {
        cli::remove_slot(status, slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::SlotName;

    fn status_with(json: &str) -> Status {
        Status::from_json(json).unwrap()
    }

    fn text(status: &Status, key: &str) -> Option<String> {
        status
            .get_slot_str(key)
            .unwrap()
            .and_then(|value| value.text().map(str::to_owned))
    }

    #[test]
    fn non_deck_lines_are_ignored() {
        for line in [
            r#"{"v":1,"type":"hello","plugin":"zj-strip-bridge","session":"work","view":"deck","seq":0}"#,
            r#"{"v":1,"type":"ping","seq":4}"#,
            r#"{"v":1,"type":"state","seq":3,"doc":{}}"#,
            r#"{"v":1,"type":"something_new"}"#,
            r#"{"v":1}"#,
        ] {
            assert_eq!(parse_line(line), Ok(None), "{line}");
        }
    }

    #[test]
    fn bad_json_is_skipped() {
        for line in ["", "not json", "[1,2,3]", "42", "null"] {
            assert_eq!(parse_line(line), Err(ParseError::Skip), "{line}");
        }
    }

    #[test]
    fn a_newer_version_is_refused_for_any_type() {
        assert_eq!(
            parse_line(r#"{"v":2,"type":"deck","full":true}"#),
            Err(ParseError::TooOld)
        );
        assert_eq!(
            parse_line(r#"{"v":2,"type":"hello"}"#),
            Err(ParseError::TooOld)
        );
        assert_eq!(
            parse_line(r#"{"v":1,"type":"deck","full":true}"#)
                .unwrap()
                .unwrap()
                .v,
            1
        );
    }

    #[test]
    fn a_deck_frame_carries_every_field() {
        let frame = parse_line(
            r#"{"v":1,"type":"deck","seq":7,"full":true,"set":{"strip":"A","dial1":{"text":"B"}},"settings":{"strip_mode":"tiles","labels":true,"strip_gap":20},"clear":["key1_1"],"owned":["strip","dial1","dial2"]}"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(frame.v, 1);
        assert_eq!(frame.seq, Some(7));
        assert!(frame.full);
        assert_eq!(frame.set.len(), 2);
        assert_eq!(frame.settings.len(), 3);
        assert_eq!(frame.clear, ["key1_1"]);
        assert_eq!(frame.owned, ["strip", "dial1", "dial2"]);
    }

    #[test]
    fn a_partial_frame_defaults_to_a_delta_with_no_owned_slots() {
        let frame = parse_line(r#"{"v":1,"type":"deck","set":{"strip":"A"}}"#)
            .unwrap()
            .unwrap();
        assert!(!frame.full);
        assert_eq!(frame.seq, None);
        assert!(frame.owned.is_empty());
        assert!(frame.clear.is_empty());
        assert!(frame.settings.is_empty());
    }

    #[test]
    fn a_deck_line_without_a_version_is_skipped() {
        for line in [
            r#"{"type":"deck","full":true,"set":{"strip":"A"}}"#,
            r#"{"v":"1","type":"deck","full":true,"set":{"strip":"A"}}"#,
            r#"{"v":1.5,"type":"deck","full":true,"set":{"strip":"A"}}"#,
            r#"{"v":-1,"type":"deck","full":true,"set":{"strip":"A"}}"#,
        ] {
            assert_eq!(parse_line(line), Err(ParseError::Skip), "{line}");
        }
    }

    #[test]
    fn a_deck_line_with_a_non_boolean_full_is_skipped() {
        for full in [r#""true""#, "1", "null"] {
            let line = format!(r#"{{"v":1,"type":"deck","full":{full},"set":{{"strip":"A"}}}}"#);
            assert_eq!(parse_line(&line), Err(ParseError::Skip), "{line}");
        }
    }

    #[test]
    fn seq_notices_a_gap_only_after_a_numbered_line() {
        let mut seq = Seq::default();
        assert!(!seq.check(Some(0)));
        assert!(!seq.check(Some(1)));
        assert!(
            !seq.check(None),
            "a line without seq leaves the numbering alone"
        );
        assert!(seq.check(Some(3)), "a gap");
        assert!(!seq.check(Some(4)), "tracking resumes after the gap");
        assert!(seq.check(Some(4)), "a repeated number");
        seq.lost();
        assert!(
            seq.check(Some(5)),
            "an unreadable line hides the next number"
        );
        assert!(!seq.check(Some(6)));
        let mut fresh = Seq::default();
        fresh.lost();
        assert!(!fresh.check(Some(0)), "junk before hello is not a gap");
    }

    #[test]
    fn a_full_frame_removes_unset_owned_slots() {
        let frame = parse_line(
            r#"{"v":1,"type":"deck","full":true,"set":{"strip":"S"},"owned":["strip","dial1","dial2","dial3","dial4"]}"#,
        )
        .unwrap()
        .unwrap();
        let mut status = status_with(r#"{"strip":"old","dial1":"a","dial2":"b","key1_1":"K"}"#);
        apply_frame(&mut status, &frame);
        assert_eq!(text(&status, "strip").as_deref(), Some("S"));
        assert!(status.get_slot_str("dial1").unwrap().is_none());
        assert!(status.get_slot_str("dial2").unwrap().is_none());
        assert_eq!(text(&status, "key1_1").as_deref(), Some("K"));
    }

    #[test]
    fn removing_an_owned_strip_also_removes_the_dials() {
        let frame = parse_line(
            r#"{"v":1,"type":"deck","full":true,"set":{},"owned":["strip","dial1","dial2","dial3","dial4"]}"#,
        )
        .unwrap()
        .unwrap();
        let mut status =
            status_with(r#"{"strip":"old","dial1":"a","dial2":"b","dial3":"c","dial4":"d"}"#);
        apply_frame(&mut status, &frame);
        assert_eq!(status.to_json_pretty().trim(), "{}");
    }

    #[test]
    fn an_incremental_frame_only_touches_its_keys() {
        let frame = parse_line(
            r#"{"v":1,"type":"deck","set":{"dial2":"C"},"settings":{"strip_gap":12},"clear":["dial1"]}"#,
        )
        .unwrap()
        .unwrap();
        let mut status = status_with(r#"{"dial1":"a","dial2":"b","dial3":"c"}"#);
        apply_frame(&mut status, &frame);
        assert!(status.get_slot_str("dial1").unwrap().is_none());
        assert_eq!(text(&status, "dial2").as_deref(), Some("C"));
        assert_eq!(status.strip_gap(), Some(12));
        assert_eq!(text(&status, "dial3").as_deref(), Some("c"));
    }

    #[test]
    fn set_values_use_the_set_syntax() {
        let frame = parse_line(
            r##"{"v":1,"type":"deck","set":{"strip":"a\\nb","dial1":"[b]x[/b]","dial2":{"text":"obj","color":"#ff0000"},"dial3":"{\"text\":\"json\"}"}}"##,
        )
        .unwrap()
        .unwrap();
        let mut status = Status::empty();
        apply_frame(&mut status, &frame);
        assert_eq!(text(&status, "strip").as_deref(), Some("a\nb"));
        assert_eq!(text(&status, "dial1").as_deref(), Some("[b]x[/b]"));
        let dial2 = status.slot(&SlotName::Dial(2)).unwrap().unwrap();
        assert_eq!(dial2.text(), Some("obj"));
        assert_eq!(
            status.slot(&SlotName::Dial(3)).unwrap().unwrap().text(),
            Some("json")
        );
    }

    #[test]
    fn clear_uses_clear_semantics() {
        let frame = parse_line(r#"{"v":1,"type":"deck","clear":["strip"]}"#)
            .unwrap()
            .unwrap();
        let mut status = status_with(r#"{"strip":"S","dial1":"a","key1_1":"K","labels":true}"#);
        apply_frame(&mut status, &frame);
        assert!(status.get_slot_str("strip").unwrap().is_none());
        assert!(status.get_slot_str("dial1").unwrap().is_none());
        assert_eq!(text(&status, "key1_1").as_deref(), Some("K"));
        assert!(status.labels());
    }

    #[test]
    fn settings_are_written_as_typed_top_level_keys() {
        let frame = parse_line(
            r#"{"v":1,"type":"deck","settings":{"strip_mode":"raw","strip_gap":7,"labels":true,"future":{"x":1}}}"#,
        )
        .unwrap()
        .unwrap();
        let mut status = Status::empty();
        apply_frame(&mut status, &frame);
        assert_eq!(status.strip_mode(), Some(crate::status::StripMode::Raw));
        assert_eq!(status.strip_gap(), Some(7));
        assert!(status.labels());
        assert_eq!(
            status.raw_value("future"),
            Some(&serde_json::json!({"x": 1}))
        );
    }
}
