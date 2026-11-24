use image::RgbImage;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::{Value, json};

use crate::config::Config;
use crate::doctor;
use crate::encode;
use crate::install;
use crate::raw_client;
use crate::render::markup::escape_markup;
use crate::render::{Detail, Renderer, continuous_preview, raw_frame, tiles_preview};
use crate::sizetest;
use crate::status::{self, SlotValue, Status, StripMode, parse_bool};
use crate::striptest;
use crate::writer::{self, Handler, View};

#[derive(Parser, Debug)]
#[command(
    name = "opendeck-strip",
    version,
    about = "Status text on the N4E keys, dials and touch strip through OpenDeck",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Generate the plugin directory and copy this binary into it
    Install {
        /// OpenDeck plugins directory (default: auto-detect native or Flatpak)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Update slots. Each slot is followed by its text (an even number of arguments)
    Set {
        /// Turn inline markup off; brackets are shown literally
        #[arg(long)]
        plain: bool,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        items: Vec<String>,
    },
    /// Print the current state as JSON
    Get,
    /// Blank slots; no arguments clears everything
    Clear {
        #[arg(allow_hyphen_values = true)]
        slots: Vec<String>,
    },
    /// Apply a zj-strip-bridge stream from stdin
    #[command(name = "apply-zellij")]
    Apply {
        /// Clear the owned slots if no line arrives for this many seconds
        #[arg(long, value_parser = positive_secs)]
        idle_clear: Option<u64>,
        /// `state`: complete documents rendered here (default); `deck`: legacy frames
        #[arg(long, value_enum, default_value_t = View::State)]
        view: View,
    },
    /// Follow the zj-strip-bridge stream of a Zellij session and apply it
    #[command(name = "follow-zellij")]
    Follow {
        /// Zellij session; default: $ZELLIJ_SESSION_NAME if live, else the first live one
        #[arg(long)]
        session: Option<String>,
        /// Zellij binary (default: `zellij` on PATH)
        #[arg(long)]
        zellij: Option<PathBuf>,
        /// Reconnect when no line arrives for this many seconds
        #[arg(long, default_value_t = 15, value_parser = positive_secs)]
        stall_secs: u64,
        /// Reconnect backoff in seconds: MIN,MAX
        #[arg(long, default_value = "1,10")]
        retry_secs: String,
        /// Clear the owned slots after no session was live for this many seconds
        #[arg(long, default_value_t = 10)]
        grace_secs: u64,
        /// Clear the owned slots on SIGTERM or SIGINT
        #[arg(long)]
        clear_on_exit: bool,
        /// Read until the first end of stream, then exit (0 if a status write happened, else 1)
        #[arg(long)]
        once: bool,
        /// `state`: complete documents rendered here (default); `deck`: legacy frames
        #[arg(long, value_enum, default_value_t = View::State)]
        view: View,
    },
    /// Print the status file path this shell would use
    Where,
    /// Run a command periodically and show its first four output lines
    Watch {
        /// Slot to write (default: strip)
        #[arg(long)]
        slot: Option<String>,
        /// Seconds between runs
        #[arg(long, default_value_t = 2)]
        every: u64,
        /// Turn inline markup off
        #[arg(long)]
        plain: bool,
        #[arg(last = true, required = true, num_args = 1.., allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Drive the device strip directly
    Strip {
        #[command(subcommand)]
        command: StripCommand,
    },
    /// Render a strip spec to a PNG without touching the device
    #[command(name = "strip-preview")]
    StripPreview {
        /// Text, a style object, `test`, or `@slices:<dir>#<n>`
        spec: String,
        /// Output PNG
        file: PathBuf,
        /// Render the stock-plugin tile layout
        #[arg(long)]
        tiles: bool,
        /// Show the dead bands at their physical positions
        #[arg(long)]
        simulate_device: bool,
    },
    /// Grow a noise block until the device JPEG reaches each target size
    #[command(name = "size-test")]
    SizeTest {
        #[arg(long, default_value_t = 1000)]
        start: usize,
        #[arg(long, default_value_t = 500)]
        step: usize,
        #[arg(long, default_value_t = 1)]
        every: u64,
        #[arg(long, default_value_t = 40000)]
        max: usize,
    },
    /// Check the plugin, hidraw access, and the installed AKP05 build
    Doctor,
    /// Show an 800x128 PNG or JPEG file 1:1 on the strip via the AKP05 raw socket
    /// (needs `strip_raw = true` in AKP05)
    RawSend {
        file: PathBuf,
        /// Socket path (default: the only socket in $XDG_RUNTIME_DIR/opendeck-akp05)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Leave raw mode and show the dial images again
    RawClear {
        /// Socket path (default: the only socket in $XDG_RUNTIME_DIR/opendeck-akp05)
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Show an 800x128 test frame with a coloured background, then clear it
    RawTest {
        /// Seconds to show it
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Socket path (default: the only socket in $XDG_RUNTIME_DIR/opendeck-akp05)
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Write the frame as a PNG instead of sending it; no device needed
        #[arg(long)]
        out: Option<PathBuf>,
        /// Show an edge ruler instead: eight colour bands per edge, to measure how many
        /// outer pixels the device hides
        #[arg(long)]
        ruler: bool,
    },
    /// Render STRING as an 800x128 strip frame and show it until `raw-clear`
    ///
    /// STRING takes the same syntax as `set strip`: `|` splits up to four segments,
    /// `[b]..[/b]` and `[c=red]..[/c]` style text, `\n` is a line break, and a JSON object is a
    /// styled value. Needs `strip_raw = true` in AKP05.
    RawString {
        #[arg(allow_hyphen_values = true)]
        text: String,
        /// Socket path (default: the only socket in $XDG_RUNTIME_DIR/opendeck-akp05)
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Write the frame as a PNG instead of sending it; no device needed
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum StripCommand {
    /// Show the seam test pattern on the strip, then clear it
    Test {
        /// Seconds to show it; default is until Ctrl-C
        #[arg(long)]
        seconds: Option<u64>,
    },
}

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        Cli::command().print_help()?;
        println!();
        return Ok(0);
    };
    match command {
        Commands::Install { dir } => {
            println!("quit OpenDeck before installing, then start it again");
            let installed = install::install(dir)?;
            println!("installed {}", installed.display());
            Ok(0)
        }
        Commands::Set { plain, items } => {
            set(&items, plain)?;
            Ok(0)
        }
        Commands::Get => {
            let status = status::load(&status::status_path())?;
            println!("{}", status.to_json_pretty());
            Ok(0)
        }
        Commands::Clear { slots } => {
            clear(&slots)?;
            Ok(0)
        }
        Commands::Apply { idle_clear, view } => apply(idle_clear.map(Duration::from_secs), view),
        Commands::Follow {
            session,
            zellij,
            stall_secs,
            retry_secs,
            grace_secs,
            clear_on_exit,
            once,
            view,
        } => crate::follow::run(crate::follow::Options {
            session,
            zellij: zellij.unwrap_or_else(|| PathBuf::from("zellij")),
            stall: Duration::from_secs(stall_secs),
            retry: crate::follow::parse_retry(&retry_secs)?,
            grace: Duration::from_secs(grace_secs),
            clear_on_exit,
            once,
            view,
        }),
        Commands::Where => {
            println!("{}", status::status_path().display());
            Ok(0)
        }
        Commands::Watch {
            slot,
            every,
            plain,
            command,
        } => {
            watch(slot.as_deref().unwrap_or("strip"), every, plain, &command)?;
            Ok(0)
        }
        Commands::Strip { command } => {
            match command {
                StripCommand::Test { seconds } => striptest::run(seconds)?,
            }
            Ok(0)
        }
        Commands::StripPreview {
            spec,
            file,
            tiles,
            simulate_device,
        } => {
            strip_preview(&spec, &file, tiles, simulate_device)?;
            Ok(0)
        }
        Commands::SizeTest {
            start,
            step,
            every,
            max,
        } => {
            sizetest::run(sizetest::Options {
                start,
                step,
                every,
                max,
            })?;
            Ok(0)
        }
        Commands::Doctor => Ok(if doctor::run()? { 0 } else { 1 }),
        Commands::RawSend { file, socket } => {
            raw_send(&file, socket.as_deref())?;
            Ok(0)
        }
        Commands::RawClear { socket } => {
            raw_clear(socket.as_deref())?;
            Ok(0)
        }
        Commands::RawTest {
            seconds,
            socket,
            out,
            ruler,
        } => {
            raw_test(seconds, socket.as_deref(), out.as_deref(), ruler)?;
            Ok(0)
        }
        Commands::RawString { text, socket, out } => {
            raw_text(&text, socket.as_deref(), out.as_deref())?;
            Ok(0)
        }
    }
}

/// A raw strip frame rendered from strip text, ready to send.
struct RawTextFrame {
    frame: RgbImage,
    png: Vec<u8>,
    /// The text did not fit and was shortened.
    truncated: bool,
    /// The frame was over the socket limit, so images and bars were dropped.
    text_only: bool,
}

/// Render `text` as an 800x128 raw frame with the `set strip` syntax. A frame
/// whose PNG is over the socket limit is redrawn as text only.
fn render_raw_text(renderer: &Renderer, text: &str) -> Result<RawTextFrame> {
    let value = parse_slot_value(&unescape(text));
    let mut rendered = renderer.render_strip(&value, StripMode::Raw, 0, Detail::Full);
    let mut frame = raw_frame(&rendered.images);
    let mut png = encode::png_bytes(&frame)?;
    let mut text_only = false;
    if png.len() > raw_client::MAX_FRAME_BYTES {
        rendered = renderer.render_strip(&value, StripMode::Raw, 0, Detail::TextOnly);
        frame = raw_frame(&rendered.images);
        png = encode::png_bytes(&frame)?;
        text_only = true;
    }
    if png.len() > raw_client::MAX_FRAME_BYTES {
        bail!(
            "the frame is {} bytes as PNG, over the {} byte socket limit even as text only",
            png.len(),
            raw_client::MAX_FRAME_BYTES
        );
    }
    Ok(RawTextFrame {
        frame,
        png,
        truncated: rendered.truncated,
        text_only,
    })
}

fn raw_text(text: &str, socket: Option<&Path>, out: Option<&Path>) -> Result<()> {
    let config = Config::load();
    let renderer = Renderer::load(&config);
    let rendered = render_raw_text(&renderer, text)?;
    if rendered.truncated {
        println!("note: the text did not fit and was shortened");
    }
    if rendered.text_only {
        println!(
            "note: the frame was over the {} byte socket limit, so it was drawn as text only",
            raw_client::MAX_FRAME_BYTES
        );
    }
    let jpeg = encode::estimate_raw(&rendered.frame)?.bytes;
    if jpeg > config.max_jpeg_bytes {
        println!(
            "note: the device JPEG is ~{jpeg} bytes, over max_jpeg_bytes ({}); if the strip shows corruption, that size limit may be the cause",
            config.max_jpeg_bytes
        );
    }

    if let Some(out) = out {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, &rendered.png)
            .with_context(|| format!("cannot write {}", out.display()))?;
        println!(
            "wrote {} (800x128, {} bytes)",
            out.display(),
            rendered.png.len()
        );
        return Ok(());
    }

    let socket = raw_client::find_socket(socket)?;
    raw_client::send_frame_bytes(&socket, &rendered.png)?;
    println!(
        "showing text on {} ({} byte PNG); `opendeck-strip raw-clear` ends it",
        socket.display(),
        rendered.png.len()
    );
    Ok(())
}

fn raw_send(file: &Path, socket: Option<&Path>) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let socket = raw_client::find_socket(socket)?;
    raw_client::send_frame_bytes(&socket, &bytes)?;
    println!(
        "sent {} ({} bytes) to {}",
        file.display(),
        bytes.len(),
        socket.display()
    );
    Ok(())
}

fn raw_test(seconds: u64, socket: Option<&Path>, out: Option<&Path>, ruler: bool) -> Result<()> {
    let config = Config::load();
    let renderer = Renderer::load(&config);
    let frame = if ruler {
        renderer.raw_ruler_pattern()
    } else {
        renderer.raw_test_pattern()
    };
    let png = encode::png_bytes(&frame)?;
    let jpeg = encode::estimate_raw(&frame)?.bytes;
    println!(
        "test frame: 800x128, {} byte PNG, ~{jpeg} byte device JPEG",
        png.len()
    );
    if jpeg > config.max_jpeg_bytes {
        println!(
            "note: the device JPEG is over max_jpeg_bytes ({}); if the strip shows corruption, that size limit may be the cause",
            config.max_jpeg_bytes
        );
    }

    if let Some(out) = out {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, &png).with_context(|| format!("cannot write {}", out.display()))?;
        println!("wrote {}", out.display());
        if ruler {
            print_ruler_help();
        }
        return Ok(());
    }

    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;

    let socket = raw_client::find_socket(socket)?;
    raw_client::send_frame_bytes(&socket, &png)?;
    println!(
        "showing it on {} for {seconds}s; Ctrl-C clears it",
        socket.display()
    );
    if ruler {
        print_ruler_help();
    } else {
        println!(
            "check: coloured background to every edge (no black bands), white border, \
             magenta middle row, yellow middle column, no seam at x=200/400/600, upright text, quadrants 1-4 left to right"
        );
    }
    striptest::wait(&stop, Some(Duration::from_secs(seconds)));
    raw_client::send_clear(&socket)?;
    println!("cleared raw mode");
    Ok(())
}

fn print_ruler_help() {
    let order: Vec<&str> = crate::render::RAW_RULER_BANDS
        .iter()
        .map(|(name, _)| *name)
        .collect();
    println!(
        "ruler bands, from the outer edge inward, on all four edges: {}",
        order.join(", ")
    );
    println!(
        "the first colour you can see on each edge tells how many pixels are hidden (white = 0)"
    );
}

fn raw_clear(socket: Option<&Path>) -> Result<()> {
    let socket = raw_client::find_socket(socket)?;
    raw_client::send_clear(&socket)?;
    println!("cleared raw mode on {}", socket.display());
    Ok(())
}

fn set(items: &[String], plain: bool) -> Result<()> {
    if items.is_empty() {
        bail!("set needs SLOT TEXT pairs");
    }
    if !items.len().is_multiple_of(2) {
        bail!("set needs an even number of arguments (SLOT TEXT [SLOT TEXT ...])");
    }
    let path = status::status_path();
    status::update(&path, |status| {
        for pair in items.chunks(2) {
            apply_set(status, &pair[0], &pair[1], plain);
        }
    })?;
    Ok(())
}

/// Apply one `SLOT TEXT` pair the way `set` does. Shared with `apply-zellij`, which passes
/// `plain = false` because the plugin escapes untrusted text itself.
pub(crate) fn apply_set(status: &mut Status, key: &str, raw: &str, plain: bool) {
    let raw = unescape(raw);
    match key {
        "labels" => match parse_bool(&raw) {
            Some(value) => status.set_raw("labels", Value::Bool(value)),
            None => log::warn!("labels expects 0/1/on/off, got {raw:?}"),
        },
        "strip_mode" => match StripMode::parse(&raw) {
            Some(mode) => status.set_raw("strip_mode", json!(mode.to_string())),
            None => log::warn!("strip_mode expects continuous, tiles or raw, got {raw:?}"),
        },
        "strip_gap" => match raw.trim().parse::<u32>() {
            Ok(gap) => status.set_raw("strip_gap", json!(gap.min(100))),
            Err(_) => log::warn!("strip_gap expects a number, got {raw:?}"),
        },
        _ => {
            let text = if plain { escape_markup(&raw) } else { raw };
            apply_slot(status, key, parse_slot_value(&text));
        }
    }
}

pub(crate) fn apply_slot(status: &mut Status, key: &str, value: SlotValue) {
    if status::SlotName::parse(key).is_none() && key != "strip" {
        log::warn!("unknown slot {key}; storing it anyway");
    }
    status.set_slot(key, value);
}

/// Apply one typed settings value from a deck frame. Known keys go through the
/// same conversion as `set`; unknown keys are stored as-is for forward compatibility.
pub(crate) fn apply_setting(status: &mut Status, key: &str, value: &Value) {
    let raw = match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        _ => {
            status.set_raw(key, value.clone());
            return;
        }
    };
    match key {
        "labels" | "strip_mode" | "strip_gap" => apply_set(status, key, &raw, false),
        _ => status.set_raw(key, value.clone()),
    }
}

/// Remove one slot the way `clear` does: `strip` also removes the four dials.
pub(crate) fn remove_slot(status: &mut Status, slot: &str) {
    if slot == "strip" {
        status.remove("strip");
        for index in 1..=4 {
            status.remove(&format!("dial{index}"));
        }
    } else {
        status.remove(slot);
    }
}

fn parse_slot_value(text: &str) -> SlotValue {
    if text.trim_start().starts_with('{')
        && let Ok(value) = serde_json::from_str::<SlotValue>(text)
    {
        return value;
    }
    SlotValue::Text(text.to_owned())
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            Some('t') => {
                chars.next();
                out.push('\t');
            }
            Some('\\') => {
                chars.next();
                out.push('\\');
            }
            _ => out.push('\\'),
        }
    }
    out
}

fn clear(slots: &[String]) -> Result<()> {
    let path = status::status_path();
    if slots.is_empty() && status::load(&path).is_err() {
        log::warn!("overwriting unparseable status file {}", path.display());
        status::write_atomic(&path, &Status::empty())?;
        return Ok(());
    }
    status::update(&path, |status| {
        if slots.is_empty() {
            status.clear_all();
            return;
        }
        for slot in slots {
            remove_slot(status, slot);
        }
    })?;
    Ok(())
}

/// Whole seconds, 1 or more; rejected by clap as a usage error.
fn positive_secs(text: &str) -> Result<u64, String> {
    match text.trim().parse::<u64>() {
        Ok(0) => Err("must be at least 1 second".to_owned()),
        Ok(secs) => Ok(secs),
        Err(_) => Err(format!("expected whole seconds, got {text:?}")),
    }
}

fn apply(idle_clear: Option<Duration>, view: View) -> Result<i32> {
    let path = status::status_path();
    let config = Config::load();
    let mut handler = writer::handler(view, &config, &path, OnGap::Warn);
    let end = run_stream(
        io::stdin(),
        &mut *handler,
        idle_clear,
        OnIdle::Continue,
        None,
        &mut Stream::new(),
    )?;
    Ok(match end {
        StreamEnd::TooOld => 2,
        StreamEnd::Eof | StreamEnd::Stall | StreamEnd::Stopped | StreamEnd::SeqGap => 0,
    })
}

/// How a stream ends. `Stall` means the idle timeout fired; the caller decides
/// whether that is an error (`follow-zellij` reconnects) or expected
/// (`apply-zellij --idle-clear`).
/// `SeqGap` means lines were lost, so only `follow-zellij` sees it: it reconnects, because
/// the first frame of a connection is full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamEnd {
    Eof,
    Stall,
    SeqGap,
    TooOld,
    Stopped,
}

/// What a gap in `seq` means to a caller. A deck delta cannot rebuild what was
/// lost, and treating one as full would delete owned slots, so neither caller
/// does that. A state line is a whole document, so a gap loses nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnGap {
    /// One stream only, or complete documents: warn and go on reading.
    Warn,
    /// `follow-zellij` ends the connection; the reconnect's first frame is full.
    Resync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnIdle {
    /// Clear the owned slots and keep reading.
    Continue,
    /// Clear the owned slots and return `StreamEnd::Stall`.
    Stop,
}

/// What a stream remembers across the connections `follow-zellij` makes: the idle
/// clock, which only a line restarts. The `seq` numbering lives in the handler,
/// since each connection starts at `hello`.
pub(crate) struct Stream {
    started: Instant,
    /// When the last line of any connection arrived; `None` until the first one.
    last_line: Option<Instant>,
    /// The owned slots were already cleared in the current silent stretch.
    idle_cleared: bool,
    /// Lines received since the stream started.
    pub(crate) lines: usize,
}

impl Stream {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            last_line: None,
            idle_cleared: false,
            lines: 0,
        }
    }

    fn line_arrived(&mut self) {
        self.last_line = Some(Instant::now());
        self.idle_cleared = false;
        self.lines += 1;
    }

    /// How long no line arrived, counted from the stream's start before the first.
    fn idle_for(&self) -> Duration {
        self.last_line.unwrap_or(self.started).elapsed()
    }

    /// Clear the owned slots once per silent stretch of `idle` or more. A
    /// reconnect does not restart the clock; only a line does. True when this
    /// call cleared, so the writer can consider the slots empty.
    pub(crate) fn clear_if_idle(
        &mut self,
        path: &Path,
        idle: Duration,
        owned: &[String],
    ) -> Result<bool> {
        if self.idle_cleared || self.idle_for() < idle {
            return Ok(false);
        }
        self.idle_cleared = true;
        if !owned.is_empty() {
            clear_owned(path, owned)?;
            log::info!(
                "no line for {}s; cleared {} owned slot(s)",
                idle.as_secs(),
                owned.len()
            );
        }
        Ok(true)
    }
}

/// Apply lines from `reader` until end of input, a stall, a gap in `seq`, a
/// newer protocol version or `stop`, handing each line to `handler`.
///
/// The reader runs in its own thread so an idle timeout, a throttle deadline
/// and the stop flag can be observed while a pipe or terminal is quiet.
/// `stream` is kept across calls by `follow-zellij`, so its idle clock spans
/// connections. With `stop` set the reader is polled, so a signal is seen
/// without waiting out a long stall.
pub(crate) fn run_stream<R: Read + Send + 'static>(
    reader: R,
    handler: &mut dyn Handler,
    idle: Option<Duration>,
    on_idle: OnIdle,
    stop: Option<&AtomicBool>,
    stream: &mut Stream,
) -> Result<StreamEnd> {
    let (tx, rx) = mpsc::channel::<io::Result<String>>();
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx.send(Ok(std::mem::take(&mut line))).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err));
                    break;
                }
            }
        }
    });
    run_lines(rx, handler, idle, on_idle, stop, stream)
}

fn run_lines(
    rx: mpsc::Receiver<io::Result<String>>,
    handler: &mut dyn Handler,
    idle: Option<Duration>,
    on_idle: OnIdle,
    stop: Option<&AtomicBool>,
    stream: &mut Stream,
) -> Result<StreamEnd> {
    let path = status::status_path();
    // A short poll tick makes a signal visible without waiting out a long stall.
    let poll = stop.map(|_| Duration::from_millis(200));
    loop {
        if stop.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            handler.flush(Instant::now())?;
            return Ok(StreamEnd::Stopped);
        }
        let mut wait = match (idle, poll) {
            (Some(idle), Some(poll)) => Some(idle.min(poll)),
            (Some(idle), None) => Some(idle),
            (None, Some(poll)) => Some(poll),
            (None, None) => None,
        };
        // Wake at the throttle deadline even while no line arrives.
        if let Some(deadline) = handler.deadline() {
            let left = deadline.saturating_duration_since(Instant::now());
            wait = Some(wait.map_or(left, |wait| wait.min(left)));
        }
        let received = match wait {
            Some(wait) => match rx.recv_timeout(wait) {
                Ok(item) => Some(item),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    handler.flush(Instant::now())?;
                    return Ok(StreamEnd::Eof);
                }
            },
            None => match rx.recv() {
                Ok(item) => Some(item),
                Err(_) => {
                    handler.flush(Instant::now())?;
                    return Ok(StreamEnd::Eof);
                }
            },
        };
        let Some(item) = received else {
            let now = Instant::now();
            handler.tick(now)?;
            if let Some(idle) = idle
                && stream.clear_if_idle(&path, idle, handler.owned())?
            {
                handler.cleared();
                if on_idle == OnIdle::Stop {
                    return Ok(StreamEnd::Stall);
                }
            }
            continue;
        };
        let line = item?;
        stream.line_arrived();
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            continue;
        }
        let now = Instant::now();
        match handler.line(line, now, writer::epoch_ms())? {
            writer::LineOutcome::Continue => {}
            writer::LineOutcome::TooOld => return Ok(StreamEnd::TooOld),
            writer::LineOutcome::SeqGap => {
                handler.flush(now)?;
                return Ok(StreamEnd::SeqGap);
            }
        }
    }
}

pub(crate) fn clear_owned(path: &Path, owned: &[String]) -> Result<()> {
    status::update(path, |status| {
        for slot in owned {
            remove_slot(status, slot);
        }
    })?;
    Ok(())
}

fn watch(slot: &str, every: u64, plain: bool, command: &[String]) -> Result<()> {
    let config = Config::load();
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;
    let path = status::status_path();

    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let value = match run_with_timeout(command, config.watch_timeout_secs, &stop) {
            Ok(output) if output.success => SlotValue::Text(first_lines(&output.stdout, plain)),
            Ok(output) => {
                let label = if output.timed_out {
                    "TIMEOUT".to_owned()
                } else {
                    match output.status {
                        Some(code) => format!("EXIT {code}"),
                        None => "SIGNAL".to_owned(),
                    }
                };
                error_value(&label, &output.stderr)
            }
            Err(err) => error_value("ERROR", &err.to_string()),
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        status::update(&path, |status| {
            status.set_slot(slot, value.clone());
        })?;
        let elapsed = started.elapsed();
        let deadline = Duration::from_secs(every);
        while elapsed < deadline && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
            if started.elapsed() >= deadline {
                break;
            }
        }
    }
    Ok(())
}

struct Output {
    success: bool,
    status: Option<i32>,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

const WATCH_CAPTURE_LIMIT: usize = 64 * 1024;

fn run_with_timeout(command: &[String], timeout_secs: u64, stop: &AtomicBool) -> Result<Output> {
    let Some(program) = command.first() else {
        bail!("watch needs a command");
    };
    let mut process = Command::new(program);
    process.args(&command[1..]).stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
        process.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    #[cfg(not(unix))]
    {
        process.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = process
        .spawn()
        .with_context(|| format!("cannot run {program:?}"))?;
    let pid = child.id();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();

    let started = Instant::now();
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let mut timed_out = false;
    let status = loop {
        drain_pipe(&mut stdout, &mut out);
        drain_pipe(&mut stderr, &mut err);
        if stop.load(Ordering::Relaxed) {
            kill_process_group(pid);
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(err) => {
                kill_process_group(pid);
                let _ = child.kill();
                let _ = child.wait();
                return Err(err).with_context(|| format!("cannot wait for {program:?}"));
            }
        }
        if started.elapsed() >= timeout {
            kill_process_group(pid);
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    drain_pipe(&mut stdout, &mut out);
    drain_pipe(&mut stderr, &mut err);
    // The group id stays valid while any member survives, including after the
    // leader is reaped. This stops background children from outliving watch.
    kill_process_group(pid);
    Ok(Output {
        success: status.is_some_and(|s| s.success()),
        status: status.and_then(|s| s.code()),
        timed_out,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
    })
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    // Negative pid addresses the group created by `process_group(0)`.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: u32) {}

/// Read what is immediately available; keep draining after the capture cap so a
/// chatty command can still finish and report its real exit status.
#[cfg(unix)]
fn drain_pipe<P: Read + std::os::unix::io::AsRawFd>(pipe: &mut Option<P>, buffer: &mut Vec<u8>) {
    use std::io::ErrorKind;

    let Some(active) = pipe.as_mut() else {
        return;
    };
    let mut chunk = [0u8; 8192];
    loop {
        let mut pollfd = libc::pollfd {
            fd: active.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Safety: one valid pollfd; poll does not touch the pipe itself.
        let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
        let readable =
            ready > 0 && (pollfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0;
        if !readable {
            return;
        }
        match active.read(&mut chunk) {
            Ok(0) => {
                *pipe = None;
                return;
            }
            Ok(n) => {
                if buffer.len() < WATCH_CAPTURE_LIMIT {
                    let room = WATCH_CAPTURE_LIMIT - buffer.len();
                    buffer.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock => return,
            Err(_) => {
                *pipe = None;
                return;
            }
        }
    }
}

#[cfg(not(unix))]
fn drain_pipe<P: Read>(_pipe: &mut Option<P>, _buffer: &mut Vec<u8>) {}

fn first_lines(text: &str, plain: bool) -> String {
    let joined = text.lines().take(4).collect::<Vec<_>>().join("\n");
    if plain {
        escape_markup(&joined)
    } else {
        joined
    }
}

fn error_value(label: &str, detail: &str) -> SlotValue {
    let detail = detail
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("command failed");
    let text = format!("{label}\n{detail}");
    SlotValue::Styled(status::Styles {
        text: Some(text),
        color: Some("#ff8080".to_owned()),
        bg: Some("#3a0000".to_owned()),
        ..Default::default()
    })
}

fn strip_preview(spec: &str, file: &Path, tiles: bool, simulate: bool) -> Result<()> {
    let config = Config::load();
    let renderer = Renderer::load(&config);
    let status = status::load(&status::status_path()).unwrap_or_default();
    let mode = if tiles {
        StripMode::Tiles
    } else {
        status.strip_mode().unwrap_or_else(|| config.strip_mode())
    };
    let gap = status.strip_gap().unwrap_or(config.strip_gap).min(32);

    let preview = if spec.trim() == "test" {
        renderer
            .test_pattern(mode, gap, Detail::Full)
            .images
            .into_iter()
            .collect::<Vec<_>>()
    } else if let Some(slices) = status::parse_slices(spec) {
        load_preview_slices(&slices.dir, mode)?
    } else {
        let value = if spec.trim_start().starts_with('{') {
            serde_json::from_str::<SlotValue>(spec)
                .unwrap_or_else(|_| SlotValue::Text(spec.to_owned()))
        } else {
            SlotValue::Text(unescape(spec))
        };
        renderer
            .render_strip(&value, mode, gap, Detail::Full)
            .images
    };

    let image = match mode {
        StripMode::Continuous => continuous_preview(&preview),
        StripMode::Tiles => tiles_preview(&preview, gap, simulate),
        StripMode::Raw => raw_frame(&preview),
    };
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    image
        .save(file)
        .with_context(|| format!("cannot write {}", file.display()))?;
    println!(
        "wrote {} ({}x{}, {} bytes)",
        file.display(),
        image.width(),
        image.height(),
        std::fs::metadata(file)?.len()
    );
    Ok(())
}

fn load_preview_slices(dir: &Path, mode: StripMode) -> Result<Vec<RgbImage>> {
    let expected = match mode {
        StripMode::Continuous => (200u32, 100u32),
        StripMode::Tiles => (176u32, 88u32),
        StripMode::Raw => (200u32, 128u32),
    };
    let mut out = Vec::with_capacity(4);
    for index in 0..4 {
        let path = dir.join(format!("slice{index}.png"));
        let bytes =
            std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let image = encode::decode_png(&bytes)?;
        if image.dimensions() != expected {
            bail!(
                "{} is {}x{}, expected {}x{}",
                path.display(),
                image.width(),
                image.height(),
                expected.0,
                expected.1
            );
        }
        out.push(image);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("opendeck-strip").chain(args.iter().copied()))
    }

    fn command(args: &[&str]) -> Commands {
        parse(args).expect("parses").command.expect("a command")
    }

    #[test]
    fn apply_and_follow_parse_their_options() {
        assert!(matches!(
            command(&["apply-zellij", "--idle-clear", "5"]),
            Commands::Apply {
                idle_clear: Some(5),
                view: View::State,
            }
        ));
        assert!(matches!(
            command(&["apply-zellij"]),
            Commands::Apply {
                idle_clear: None,
                view: View::State,
            }
        ));
        assert!(matches!(
            command(&["apply-zellij", "--view", "deck"]),
            Commands::Apply {
                idle_clear: None,
                view: View::Deck,
            }
        ));
        match command(&["follow-zellij", "--session", "work", "--clear-on-exit"]) {
            Commands::Follow {
                session,
                zellij,
                stall_secs,
                retry_secs,
                grace_secs,
                clear_on_exit,
                once,
                view,
            } => {
                assert_eq!(session.as_deref(), Some("work"));
                assert_eq!(zellij, None);
                assert_eq!(stall_secs, 15);
                assert_eq!(retry_secs, "1,10");
                assert_eq!(grace_secs, 10);
                assert!(clear_on_exit);
                assert!(!once);
                assert_eq!(view, View::State);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn zero_second_options_are_usage_errors() {
        for args in [
            ["apply-zellij", "--idle-clear", "0"],
            ["follow-zellij", "--stall-secs", "0"],
            ["follow-zellij", "--stall-secs", "x"],
        ] {
            let err = parse(&args).expect_err("rejected");
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{args:?}"
            );
            assert_eq!(err.exit_code(), 2, "{args:?}");
        }
        assert!(matches!(
            command(&["apply-zellij", "--idle-clear", "1"]),
            Commands::Apply {
                idle_clear: Some(1),
                view: View::State,
            }
        ));
    }

    #[test]
    fn raw_string_parses_text_and_options() {
        match command(&[
            "raw-string",
            "A|B",
            "--out",
            "x.png",
            "--socket",
            "/tmp/s.sock",
        ]) {
            Commands::RawString { text, socket, out } => {
                assert_eq!(text, "A|B");
                assert_eq!(out, Some(PathBuf::from("x.png")));
                assert_eq!(socket, Some(PathBuf::from("/tmp/s.sock")));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn raw_string_can_be_any_word() {
        for word in ["clear", "send", "test", "raw-clear", "text"] {
            match command(&["raw-string", word]) {
                Commands::RawString { text, .. } => assert_eq!(text, word),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn flat_raw_commands_parse() {
        assert!(matches!(
            command(&["raw-send", "f.png"]),
            Commands::RawSend { .. }
        ));
        assert!(matches!(command(&["raw-clear"]), Commands::RawClear { .. }));
        assert!(matches!(
            command(&["raw-test", "--ruler", "--seconds", "3"]),
            Commands::RawTest {
                ruler: true,
                seconds: 3,
                ..
            }
        ));
    }

    #[test]
    fn the_raw_command_group_is_gone() {
        assert!(parse(&["raw", "hello"]).is_err());
        assert!(parse(&["raw", "send", "f.png"]).is_err());
    }

    #[test]
    fn raw_text_renders_an_800x128_frame() {
        let renderer = Renderer::embedded();
        let one = render_raw_text(&renderer, "hello").unwrap();
        assert_eq!(one.frame.dimensions(), (800, 128));
        let four = render_raw_text(&renderer, "A|B\\nC|[c=red]D[/c]|E").unwrap();
        assert_eq!(four.frame.dimensions(), (800, 128));
        assert!(four.png.len() <= raw_client::MAX_FRAME_BYTES);
    }
}
