//! The writer: a `zjsb` state document in, status-file writes out.
//!
//! [`Writer`] holds the throttle, unchanged-text suppression and the owned
//! slots, with the clock passed in so it is testable. [`StateHandler`] and
//! [`DeckHandler`] adapt the two stream views to the shared line reader in
//! `cli`; only the parsing and the payload differ.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::Value;

use crate::cli::{self, OnGap};
use crate::config::Config;
use crate::frame::{self, ParseError};
use crate::status::{self, Status};
use crate::zjsb::{self, document::Document};

/// Which stream `follow-zellij` and `apply-zellij` read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum View {
    /// Complete documents; the default.
    #[default]
    State,
    /// Deck frames; legacy, kept for one release.
    Deck,
}

/// One `state` line: whether it carried a document, and its `seq`.
pub struct StateLine {
    pub doc: Option<Document>,
    pub seq: Option<u64>,
}

/// Parse one `view=state` line. `hello` is logged, `ping` and unknown types are
/// ignored, an `error` is logged. A higher `v` is refused; a malformed `state`
/// line is skipped.
pub fn parse_state_line(line: &str) -> Result<StateLine, ParseError> {
    let value: Value = serde_json::from_str(line).map_err(|_| ParseError::Skip)?;
    let Value::Object(mut map) = value else {
        return Err(ParseError::Skip);
    };
    let seq = map.get("seq").and_then(Value::as_u64);
    let v = map.get("v").and_then(Value::as_u64);
    if v.is_some_and(|v| v > frame::KNOWN_VERSION) {
        return Err(ParseError::TooOld);
    }
    let none = StateLine { doc: None, seq };
    match map.get("type").and_then(Value::as_str) {
        None => return Err(ParseError::Skip),
        Some("hello") => {
            log::info!(
                "state stream: plugin {} version {} session {} view {}",
                map.get("plugin").and_then(Value::as_str).unwrap_or("?"),
                map.get("version").and_then(Value::as_str).unwrap_or("?"),
                map.get("session").and_then(Value::as_str).unwrap_or("?"),
                map.get("view").and_then(Value::as_str).unwrap_or("?")
            );
            return Ok(none);
        }
        Some("ping") => return Ok(none),
        Some("error") => {
            log::error!(
                "state stream error: {}",
                map.get("message")
                    .or_else(|| map.get("error"))
                    .and_then(Value::as_str)
                    .unwrap_or("?")
            );
            return Ok(none);
        }
        Some("state") => {}
        _ => return Ok(none),
    }
    // Never guess the version or the shape of a document.
    if v.is_none() {
        return Err(ParseError::Skip);
    }
    let Some(Value::Object(doc)) = map.remove("doc") else {
        return Err(ParseError::Skip);
    };
    let doc =
        serde_json::from_value::<Document>(Value::Object(doc)).map_err(|_| ParseError::Skip)?;
    Ok(StateLine {
        doc: Some(doc),
        seq,
    })
}

/// A frame the throttle is holding back.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Text(String),
    Clear,
}

/// What the status file is known to show in the owned slots.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    /// Nothing written yet; a previous run may have left anything there.
    Unknown,
    Empty,
    Text(String),
}

/// Owned slots, throttle, unchanged suppression and `strip_mode = raw`.
pub struct Writer {
    options: zjsb::Options,
    strip: bool,
    slot: String,
    owned: Vec<String>,
    interval: Duration,
    shown: Shown,
    raw_set: bool,
    pending: Option<Pending>,
    last_write: Option<Instant>,
    writes: usize,
}

impl Writer {
    pub fn new(config: &Config) -> Self {
        let options = zjsb::Options::from_config(config);
        let strip = options.mode == zjsb::Mode::Strip;
        let (slot, owned) = if strip {
            let mut owned = vec!["strip".to_owned()];
            owned.extend((1..=4).map(|n| format!("dial{n}")));
            ("strip".to_owned(), owned)
        } else {
            let slot = options.slot.clone().unwrap_or_else(|| "dial1".to_owned());
            (slot.clone(), vec![slot])
        };
        Self {
            options,
            strip,
            slot,
            owned,
            interval: Duration::from_millis(config.zjsb_min_interval_ms),
            shown: Shown::Unknown,
            // Compact never writes the strip, so it never announces raw mode.
            raw_set: !strip,
            pending: None,
            last_write: None,
            writes: 0,
        }
    }

    pub fn owned(&self) -> &[String] {
        &self.owned
    }

    /// Status writes performed, flush included; `--once` counts these.
    pub fn applied(&self) -> usize {
        self.writes
    }

    /// When the held frame must be written, if one is held.
    pub fn deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .and_then(|_| self.last_write.map(|last| last + self.interval))
    }

    /// Feed one document: write it now, hold it, or drop it as unchanged.
    pub fn document(&mut self, status: &mut Status, doc: &Document, now_ms: u64, now: Instant) {
        self.stage(doc, now_ms);
        self.flush_due(status, now);
    }

    /// Decide what the document should show without touching the status file.
    /// A due frame stays in `pending` for [`Writer::flush_due`]; an unchanged
    /// one is dropped, and clears a held frame.
    pub fn stage(&mut self, doc: &Document, now_ms: u64) {
        let next = match zjsb::render(doc, &self.options, now_ms) {
            Some(text)
                if matches!(&self.shown, Shown::Text(shown) if *shown == text) && self.raw_set =>
            {
                // The newest state is already shown; a held frame would only repeat it.
                self.pending = None;
                return;
            }
            Some(text) => Pending::Text(text),
            None if self.shown == Shown::Empty => {
                self.pending = None;
                return;
            }
            // Unknown: the owned slots may still hold a previous run's frame.
            None => Pending::Clear,
        };
        self.pending = Some(next);
    }

    fn due(&self, now: Instant) -> bool {
        self.last_write
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval)
    }

    /// The deadline has passed and a frame is held.
    pub fn pending_due(&self, now: Instant) -> bool {
        self.pending.is_some() && self.due(now)
    }

    /// Write the held frame when the throttle allows it; true if it wrote.
    pub fn flush_due(&mut self, status: &mut Status, now: Instant) -> bool {
        if !self.pending_due(now) {
            return false;
        }
        if let Some(frame) = self.pending.take() {
            self.write(status, frame, now);
        }
        true
    }

    /// Write any held frame, ignoring the throttle; true if it wrote.
    pub fn flush(&mut self, status: &mut Status, now: Instant) -> bool {
        match self.pending.take() {
            Some(frame) => {
                self.write(status, frame, now);
                true
            }
            None => false,
        }
    }

    /// The owned slots were cleared by the idle, stall or grace machinery.
    pub fn cleared(&mut self) {
        self.shown = Shown::Empty;
    }

    fn write(&mut self, status: &mut Status, frame: Pending, now: Instant) {
        match frame {
            Pending::Clear => {
                for slot in &self.owned {
                    cli::remove_slot(status, slot);
                }
                self.shown = Shown::Empty;
            }
            Pending::Text(text) => {
                if self.strip && !self.raw_set {
                    cli::apply_set(status, "strip_mode", "raw", false);
                    self.raw_set = true;
                }
                cli::apply_set(status, &self.slot, &text, false);
                self.shown = Shown::Text(text);
            }
        }
        self.last_write = Some(now);
        self.writes += 1;
    }
}

/// What one stream line made the reader do.
pub(crate) enum LineOutcome {
    Continue,
    /// But the line was refused; the reader logs and stops.
    TooOld,
    /// A deck `seq` gap the caller must resync on.
    SeqGap,
}

/// One view's line handling, shared by `run_lines`.
pub(crate) trait Handler {
    fn owned(&self) -> &[String];
    fn line(&mut self, line: &str, now: Instant, now_ms: u64) -> Result<LineOutcome>;
    /// Called on every reader timeout tick, after an idle clear.
    fn tick(&mut self, now: Instant) -> Result<()>;
    /// Write any held frame; called when a stream ends.
    fn flush(&mut self, now: Instant) -> Result<()>;
    /// Status writes for `--once`.
    fn applied(&self) -> usize;
    fn deadline(&self) -> Option<Instant> {
        None
    }
    /// The owned slots were cleared outside the handler.
    fn cleared(&mut self) {}
    /// A new connection starts numbering its lines at `hello` again.
    fn reconnected(&mut self) {}
}

/// The `view=state` handler: each line is a complete document.
pub(crate) struct StateHandler {
    path: PathBuf,
    writer: Writer,
    seq: frame::Seq,
}

impl StateHandler {
    pub fn new(config: &Config, path: PathBuf) -> Self {
        Self {
            path,
            writer: Writer::new(config),
            seq: frame::Seq::default(),
        }
    }
}

impl Handler for StateHandler {
    fn owned(&self) -> &[String] {
        self.writer.owned()
    }

    fn line(&mut self, line: &str, now: Instant, now_ms: u64) -> Result<LineOutcome> {
        let parsed = match parse_state_line(line) {
            Ok(parsed) => parsed,
            Err(ParseError::TooOld) => return Ok(LineOutcome::TooOld),
            Err(ParseError::Skip) => {
                log::warn!("skipping unparsable state line: {line}");
                self.seq.lost();
                return Ok(LineOutcome::Continue);
            }
        };
        if self.seq.check(parsed.seq) {
            // A state line is a complete document, so a gap loses nothing.
            log::warn!(
                "state stream skipped or repeated a seq (got {:?}); the next state is complete",
                parsed.seq
            );
        }
        let Some(doc) = parsed.doc else {
            return Ok(LineOutcome::Continue);
        };
        self.writer.stage(&doc, now_ms);
        if self.writer.pending_due(now) {
            let (path, writer) = (&self.path, &mut self.writer);
            status::update(path, |status| {
                writer.flush_due(status, now);
            })?;
        }
        Ok(LineOutcome::Continue)
    }

    fn tick(&mut self, now: Instant) -> Result<()> {
        if !self.writer.pending_due(now) {
            return Ok(());
        }
        let (path, writer) = (&self.path, &mut self.writer);
        status::update(path, |status| {
            writer.flush_due(status, now);
        })?;
        Ok(())
    }

    fn flush(&mut self, now: Instant) -> Result<()> {
        if self.writer.deadline().is_none() {
            return Ok(());
        }
        let (path, writer) = (&self.path, &mut self.writer);
        status::update(path, |status| {
            writer.flush(status, now);
        })?;
        Ok(())
    }

    fn applied(&self) -> usize {
        self.writer.applied()
    }

    fn deadline(&self) -> Option<Instant> {
        self.writer.deadline()
    }

    fn cleared(&mut self) {
        self.writer.cleared();
    }

    fn reconnected(&mut self) {
        // Every connection numbers its lines from `hello` again.
        self.seq = frame::Seq::default();
    }
}

/// The `view=deck` handler: kept as today, including reconnect-on-gap.
pub(crate) struct DeckHandler {
    path: PathBuf,
    owned: Vec<String>,
    seq: frame::Seq,
    frames: usize,
    on_gap: OnGap,
}

impl DeckHandler {
    pub fn new(on_gap: OnGap, path: PathBuf) -> Self {
        Self {
            path,
            owned: Vec::new(),
            seq: frame::Seq::default(),
            frames: 0,
            on_gap,
        }
    }
}

impl Handler for DeckHandler {
    fn owned(&self) -> &[String] {
        &self.owned
    }

    fn line(&mut self, line: &str, _now: Instant, _now_ms: u64) -> Result<LineOutcome> {
        let (frame, seq) = match frame::parse_parts(line) {
            Ok(parts) => parts,
            Err(ParseError::TooOld) => {
                log::error!(
                    "deck frame is newer than version {}; update opendeck-strip",
                    frame::KNOWN_VERSION
                );
                return Ok(LineOutcome::TooOld);
            }
            Err(ParseError::Skip) => {
                log::warn!("skipping unparsable deck line: {line}");
                self.seq.lost();
                return Ok(LineOutcome::Continue);
            }
        };
        if self.seq.check(seq) {
            match self.on_gap {
                OnGap::Warn => log::warn!(
                    "deck stream skipped or repeated a seq (got {seq:?}); a frame may be lost until the next full one"
                ),
                OnGap::Resync => {
                    log::warn!("deck stream skipped or repeated a seq (got {seq:?})");
                    return Ok(LineOutcome::SeqGap);
                }
            }
        }
        let Some(frame) = frame else {
            return Ok(LineOutcome::Continue);
        };
        if frame.full {
            self.owned = frame.owned.clone();
        }
        status::update(&self.path, |status| frame::apply_frame(status, &frame))?;
        self.frames += 1;
        Ok(LineOutcome::Continue)
    }

    fn tick(&mut self, _now: Instant) -> Result<()> {
        Ok(())
    }

    fn flush(&mut self, _now: Instant) -> Result<()> {
        Ok(())
    }

    fn applied(&self) -> usize {
        self.frames
    }

    fn reconnected(&mut self) {
        self.seq = frame::Seq::default();
    }
}

/// System epoch milliseconds, the clock the renderer's `age` uses.
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Build the handler for a view.
pub(crate) fn handler(view: View, config: &Config, path: &Path, on_gap: OnGap) -> Box<dyn Handler> {
    match view {
        View::State => Box::new(StateHandler::new(config, path.to_owned())),
        View::Deck => Box::new(DeckHandler::new(on_gap, path.to_owned())),
    }
}

#[cfg(test)]
mod tests;
