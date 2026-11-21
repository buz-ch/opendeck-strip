use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use image::RgbImage;
use notify::Watcher;
use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::WebSocket;
use tungstenite::error::Error as WsError;

use crate::config::Config;
use crate::encode;
use crate::raw_client::{self, RawError};
use crate::render::{Detail, Renderer, SlotSpec, needs_image, raw_frame, slot_plain_text};
use crate::status::{self, SlotName, SlotValue, Status, StripMode};

pub const ACTION_UUID: &str = "local.statusdisplay.slot";
pub const PLUGIN_DIR_NAME: &str = "local.statusdisplay.sdPlugin";
/// Retry interval after an `@slices` load fails; the files may still be arriving.
const STRIP_RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// A deterministic failure backs off to this interval instead of spinning.
const STRIP_RETRY_MAX: Duration = Duration::from_secs(60);
/// First retry of the AKP05 raw socket after it failed, doubling up to `RAW_RETRY_MAX`.
const RAW_RETRY_INTERVAL: Duration = Duration::from_secs(10);
const RAW_RETRY_MAX: Duration = Duration::from_secs(60);
/// A raw strip is one atomic device write, so it may redraw faster than the
/// four-image continuous path that `redraw_hz` throttles.
const RAW_MIN_HZ: f32 = 10.0;

#[derive(Clone, Debug)]
pub struct PluginArgs {
    pub port: u16,
    pub uuid: String,
    pub register_event: String,
}

/// OpenDeck starts plugins with `-port N -pluginUUID ID -registerEvent E -info JSON`.
pub fn args_from_argv<I: IntoIterator<Item = OsString>>(argv: I) -> Option<PluginArgs> {
    let args: Vec<String> = argv
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut port = None;
    let mut uuid = None;
    let mut register_event = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-port" => port = iter.next().and_then(|v| v.parse().ok()),
            "-pluginUUID" => uuid = iter.next().cloned(),
            "-registerEvent" => register_event = iter.next().cloned(),
            "-info" => {
                let _ = iter.next();
            }
            _ => {}
        }
    }
    Some(PluginArgs {
        port: port?,
        uuid: uuid?,
        register_event: register_event.unwrap_or_else(|| "registerPlugin".to_owned()),
    })
}

pub fn run(args: PluginArgs) -> Result<()> {
    let path = status::status_path();
    let config = Config::load();
    let mut app = App::new(path, config);
    app.reload();

    let stream = TcpStream::connect(("127.0.0.1", args.port))
        .with_context(|| format!("cannot connect to OpenDeck on port {}", args.port))?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    let url = format!("ws://127.0.0.1:{}", args.port);
    let (mut socket, _response) =
        tungstenite::client(url, stream).map_err(|e| anyhow!("WebSocket handshake failed: {e}"))?;

    let register = json!({"event": args.register_event, "uuid": args.uuid});
    send_json(&mut socket, &register)?;
    log::info!("registered as {}", args.uuid);

    let (wake_tx, wake_rx) = mpsc::channel();
    spawn_status_watcher(app.path.clone(), wake_tx);

    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                if let Err(err) = app.handle_message(text.as_str(), &mut socket) {
                    log::warn!("failed to handle event: {err:#}");
                }
            }
            Ok(Message::Close(_)) => {
                log::info!("OpenDeck closed the connection");
                break;
            }
            Ok(Message::Ping(payload)) => {
                let _ = socket.send(Message::Pong(payload));
            }
            Ok(_) => {}
            Err(WsError::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(err) if is_clean_close(&err) => {
                log::info!("OpenDeck closed the connection: {err}");
                break;
            }
            Err(err) => {
                log::warn!("websocket read failed: {err}");
                break;
            }
        }

        while wake_rx.try_recv().is_ok() {
            app.reload();
        }
        if let Err(err) = app.flush(&mut socket) {
            if is_clean_close_anyhow(&err) {
                log::info!("OpenDeck closed the connection");
                break;
            }
            return Err(err);
        }
    }

    Ok(())
}

fn is_clean_close(err: &WsError) -> bool {
    matches!(
        err,
        WsError::ConnectionClosed
            | WsError::AlreadyClosed
            | WsError::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake)
    )
}

fn is_clean_close_anyhow(err: &anyhow::Error) -> bool {
    err.downcast_ref::<WsError>().is_some_and(is_clean_close)
}

fn send_json(socket: &mut WebSocket<TcpStream>, value: &Value) -> Result<()> {
    socket
        .send(Message::text(serde_json::to_string(value)?))
        .map_err(Into::into)
}

fn spawn_status_watcher(path: PathBuf, wake: mpsc::Sender<()>) {
    std::thread::spawn(move || {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_owned();
        let watched_name = path.file_name().map(|name| name.to_owned());
        let (event_tx, event_rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result
                    && event
                        .paths
                        .iter()
                        .any(|changed| changed.file_name() == watched_name.as_deref())
                {
                    let _ = event_tx.send(());
                }
            })
            .ok();
        let watching = match watcher.as_mut() {
            Some(watcher) => match watcher.watch(&parent, notify::RecursiveMode::NonRecursive) {
                Ok(()) => true,
                Err(err) => {
                    log::warn!(
                        "cannot watch {} ({err}); polling every 500 ms",
                        parent.display()
                    );
                    false
                }
            },
            None => {
                log::warn!("cannot create a file watcher; polling every 500 ms");
                false
            }
        };
        loop {
            if watching {
                if event_rx.recv().is_err() {
                    return;
                }
            } else {
                let _ = event_rx.recv_timeout(Duration::from_millis(500));
            }
            if wake.send(()).is_err() {
                return;
            }
        }
    });
}

#[derive(Clone, Debug)]
struct BoundSlot {
    context: String,
    name: SlotName,
    /// OpenDeck device id from `willAppear`; names the AKP05 raw socket.
    device: Option<String>,
    last: Option<String>,
}

/// State of the AKP05 raw socket link (`strip_mode = raw`).
struct RawLink {
    /// Digest of the frame AKP05 shows, when raw mode is active there.
    shown: Option<u64>,
    /// Delivery failed, so the strip is drawn as continuous dial images.
    fallback: bool,
    retry_at: Option<Instant>,
    retry_delay: Duration,
    /// Last failure message, so a repeating failure is logged once.
    warning: Option<String>,
}

impl RawLink {
    fn new() -> Self {
        Self {
            shown: None,
            fallback: false,
            retry_at: None,
            retry_delay: RAW_RETRY_INTERVAL,
            warning: None,
        }
    }
}

struct StripCache {
    key: CacheKey,
    pngs: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq)]
struct CacheKey {
    mode: StripMode,
    gap: u32,
    value: Value,
}

pub struct App {
    path: PathBuf,
    status: Status,
    slots: Vec<BoundSlot>,
    dirty: BTreeSet<String>,
    force: BTreeSet<String>,
    entering_strip: bool,
    leaving_strip: bool,
    strip_cache: Option<StripCache>,
    strip_retry_at: Option<Instant>,
    strip_retry_delay: Duration,
    next_burst: Instant,
    config: Config,
    renderer: Renderer,
    warned_slots: HashSet<String>,
    status_warning: Option<String>,
    slices_warning: Option<String>,
    raw: RawLink,
}

impl App {
    pub fn new(path: PathBuf, config: Config) -> Self {
        let renderer = Renderer::load(&config);
        Self {
            path,
            status: Status::empty(),
            slots: Vec::new(),
            dirty: BTreeSet::new(),
            force: BTreeSet::new(),
            entering_strip: false,
            leaving_strip: false,
            strip_cache: None,
            strip_retry_at: None,
            strip_retry_delay: STRIP_RETRY_INTERVAL,
            next_burst: Instant::now(),
            config,
            renderer,
            warned_slots: HashSet::new(),
            status_warning: None,
            slices_warning: None,
            raw: RawLink::new(),
        }
    }

    /// Re-read the status file. Invalid content keeps the previous state.
    pub fn reload(&mut self) {
        let new = match status::load(&self.path) {
            Ok(status) => status,
            Err(err) => {
                let message = format!("{err:#}");
                if self.status_warning.as_deref() != Some(message.as_str()) {
                    log::warn!("ignoring invalid status file: {message}");
                    self.status_warning = Some(message);
                }
                return;
            }
        };
        self.status_warning = None;
        if new.raw() == self.status.raw() {
            return;
        }

        let was_strip = self.status.strip_active();
        let is_strip = new.strip_active();
        let strip_changed = self.status.strip() != new.strip()
            || self.status.strip_mode() != new.strip_mode()
            || self.status.strip_gap() != new.strip_gap();
        if !was_strip && is_strip {
            self.entering_strip = true;
        }
        if was_strip && !is_strip {
            self.leaving_strip = true;
        }
        if was_strip != is_strip {
            self.strip_cache = None;
        }
        if strip_changed {
            self.strip_retry_at = None;
            self.strip_retry_delay = STRIP_RETRY_INTERVAL;
        }
        if self.status.strip_mode() != new.strip_mode() {
            // A new mode deserves a fresh try at the raw socket.
            self.raw.fallback = false;
            self.raw.retry_at = None;
            self.raw.retry_delay = RAW_RETRY_INTERVAL;
        }
        self.status = new;
        for slot in &self.slots {
            self.dirty.insert(slot.context.clone());
        }
    }

    fn handle_message(&mut self, text: &str, socket: &mut WebSocket<TcpStream>) -> Result<()> {
        let value: Value = serde_json::from_str(text)?;
        let Some(event) = value.get("event").and_then(Value::as_str) else {
            return Ok(());
        };
        match event {
            "willAppear" => {
                let Some(context) = value.get("context").and_then(Value::as_str) else {
                    return Ok(());
                };
                let payload = value.get("payload");
                let controller = payload
                    .and_then(|p| p.get("controller"))
                    .and_then(Value::as_str)
                    .unwrap_or("Keypad");
                let coordinates = payload.and_then(|p| p.get("coordinates"));
                let row = coordinates
                    .and_then(|c| c.get("row"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u8;
                let column = coordinates
                    .and_then(|c| c.get("column"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u8;
                let Some(name) = SlotName::from_event(controller, row, column) else {
                    let key = format!("{controller}:{row}:{column}");
                    if self.warned_slots.len() < 64 && self.warned_slots.insert(key.clone()) {
                        log::warn!("ignoring unsupported slot {key}");
                    }
                    return Ok(());
                };
                self.slots.retain(|slot| slot.context != context);
                if matches!(name, SlotName::Dial(_)) {
                    // OpenDeck re-announces slots after a reconnect, which
                    // ends raw mode in AKP05; send the frame again.
                    self.raw.shown = None;
                }
                self.slots.push(BoundSlot {
                    context: context.to_owned(),
                    name,
                    device: value
                        .get("device")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    last: None,
                });
                self.dirty.insert(context.to_owned());
                self.force.insert(context.to_owned());
                self.flush(socket)?;
            }
            "willDisappear" => {
                if let Some(context) = value.get("context").and_then(Value::as_str) {
                    self.slots.retain(|slot| slot.context != context);
                    self.dirty.remove(context);
                    self.force.remove(context);
                }
            }
            "keyDown" | "dialDown" | "touchTap" => {
                if let Some(context) = value.get("context").and_then(Value::as_str) {
                    self.force.insert(context.to_owned());
                    self.dirty.insert(context.to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn flush(&mut self, socket: &mut WebSocket<TcpStream>) -> Result<()> {
        let now = Instant::now();
        let strip_retry_due = self.strip_retry_at.is_some_and(|at| now >= at);
        let raw_retry_due = self.raw.fallback && self.raw.retry_at.is_some_and(|at| now >= at);
        if self.dirty.is_empty()
            && !self.entering_strip
            && !self.leaving_strip
            && !strip_retry_due
            && !raw_retry_due
        {
            return Ok(());
        }
        if now < self.next_burst {
            return Ok(());
        }
        let hz = if self.raw_wanted() && !self.raw.fallback {
            self.config.redraw_hz.max(RAW_MIN_HZ)
        } else {
            self.config.redraw_hz
        };
        self.next_burst = now + Duration::from_secs_f32(1.0 / hz.max(0.1));

        let dials: Vec<BoundSlot> = self
            .slots
            .iter()
            .filter(|slot| matches!(slot.name, SlotName::Dial(_)))
            .cloned()
            .collect();

        self.raw_prepare();

        if self.entering_strip {
            for slot in &dials {
                send_title(socket, &slot.context, "", &slot.name)?;
                if let Some(bound) = self.slot_mut(&slot.context) {
                    bound.last = None;
                }
            }
            self.entering_strip = false;
        }
        if self.leaving_strip {
            for slot in &dials {
                send_title(socket, &slot.context, "", &slot.name)?;
                send_feedback(socket, &slot.context, &blank_feedback(), &slot.name)?;
                if let Some(bound) = self.slot_mut(&slot.context) {
                    bound.last = None;
                }
            }
            self.leaving_strip = false;
        }

        self.ensure_strip();
        self.raw_deliver();

        let strip_active = self.status.strip_active();
        let mut work: Vec<(u8, u8, u8, String)> = self
            .dirty
            .iter()
            .filter_map(|context| {
                let slot = self.slots.iter().find(|s| &s.context == context)?;
                let order = match slot.name {
                    SlotName::Dial(n) => (0, n, 0),
                    SlotName::Key { row, col } => (1, row, col),
                };
                Some((order.0, order.1, order.2, context.clone()))
            })
            .collect();
        work.sort();
        self.dirty.clear();

        for (_, _, _, context) in work {
            let Some(bound) = self.slots.iter().find(|s| s.context == context).cloned() else {
                continue;
            };
            let force = self.force.remove(&context);
            if strip_active && self.raw.shown.is_some() && matches!(bound.name, SlotName::Dial(_)) {
                // AKP05 only stores dial images while a raw frame is shown.
                continue;
            }
            let outgoing = self.outgoing(&bound.name, strip_active);
            let (digest, messages) = match outgoing {
                Outgoing::Title { text, dial } => {
                    let mut messages = Vec::new();
                    if dial {
                        messages.push(feedback_message(&context, &title_feedback(&text)));
                    }
                    messages.push(title_message(&context, &text));
                    (format!("title:{text}"), messages)
                }
                Outgoing::Blank { dial } => {
                    let mut messages = Vec::new();
                    if dial {
                        messages.push(feedback_message(&context, &blank_feedback()));
                    }
                    messages.push(title_message(&context, ""));
                    ("blank".to_owned(), messages)
                }
                Outgoing::Image(png) => (
                    format!("image:{}", hash_bytes(&png)),
                    vec![feedback_message(&context, &image_feedback(&png))],
                ),
            };
            if !force && bound.last.as_deref() == Some(digest.as_str()) {
                continue;
            }
            let bytes: usize = messages
                .iter()
                .filter_map(|message| serde_json::to_string(message).ok())
                .map(|text| text.len())
                .sum();
            let kind = if digest.starts_with("image:") {
                "image"
            } else {
                "title"
            };
            for message in messages {
                send_json(socket, &message)?;
            }
            log::info!("send {} {kind} ({bytes} bytes)", bound.name);
            if let Some(stored) = self.slot_mut(&context) {
                stored.last = Some(digest);
            }
        }
        self.force.clear();
        Ok(())
    }

    fn slot_mut(&mut self, context: &str) -> Option<&mut BoundSlot> {
        self.slots.iter_mut().find(|slot| slot.context == context)
    }

    /// The mode the status file or config asks for.
    fn configured_mode(&self) -> StripMode {
        self.status
            .strip_mode()
            .unwrap_or_else(|| self.config.strip_mode())
    }

    /// The mode used to render: raw falls back to continuous while the AKP05
    /// socket is unavailable.
    fn mode(&self) -> StripMode {
        match self.configured_mode() {
            StripMode::Raw if self.raw.fallback => StripMode::Continuous,
            mode => mode,
        }
    }

    /// Whether the strip should be shown through the raw socket.
    fn raw_wanted(&self) -> bool {
        self.status.strip_active() && self.configured_mode() == StripMode::Raw
    }

    fn gap(&self) -> u32 {
        self.status
            .strip_gap()
            .unwrap_or(self.config.strip_gap)
            .min(32)
    }

    fn ensure_strip(&mut self) {
        if !self.status.strip_active() {
            self.strip_cache = None;
            self.strip_retry_at = None;
            self.strip_retry_delay = STRIP_RETRY_INTERVAL;
            return;
        }
        let mode = self.mode();
        let gap = self.gap();
        let Some(value) = self.status.strip().cloned() else {
            return;
        };
        let key = CacheKey {
            mode,
            gap,
            value: value.clone(),
        };
        if self
            .strip_cache
            .as_ref()
            .is_some_and(|cache| cache.key == key)
        {
            self.strip_retry_at = None;
            return;
        }
        // Do not hammer a deterministic failure just because other slots are busy.
        if self.strip_retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }

        let images = if let Some(spec) = value.as_str().and_then(status::parse_slices) {
            match load_slices(&spec.dir, mode) {
                Ok(images) => images,
                Err(err) => {
                    let message = format!("{err:#}");
                    if self.slices_warning.as_deref() != Some(message.as_str()) {
                        log::warn!("cannot load @slices images: {message}; will retry");
                        self.slices_warning = Some(message);
                    }
                    self.retain_compatible_strip_cache(mode, gap);
                    self.schedule_strip_retry();
                    return;
                }
            }
        } else {
            let slot = self
                .status
                .strip_value()
                .ok()
                .flatten()
                .unwrap_or_else(|| SlotValue::Text(String::new()));
            self.render_strip_with_budget(&slot, mode, gap)
        };

        let pngs: Vec<Vec<u8>> = if mode == StripMode::Raw {
            match encode::png_bytes(&raw_frame(&images)) {
                Ok(bytes) => vec![bytes],
                Err(err) => {
                    log::warn!("cannot encode raw strip frame: {err:#}");
                    Vec::new()
                }
            }
        } else {
            images
                .iter()
                .filter_map(|image| match encode::png_bytes(image) {
                    Ok(bytes) => Some(bytes),
                    Err(err) => {
                        log::warn!("cannot encode strip slice: {err:#}");
                        None
                    }
                })
                .collect()
        };
        let expected = if mode == StripMode::Raw { 1 } else { 4 };
        if pngs.len() != expected {
            self.retain_compatible_strip_cache(mode, gap);
            self.schedule_strip_retry();
            return;
        }
        self.strip_cache = Some(StripCache { key, pngs });
        self.strip_retry_at = None;
        self.strip_retry_delay = STRIP_RETRY_INTERVAL;
        self.slices_warning = None;
        self.dirty_dials();
    }

    /// Why `frame` is too big for the raw path, if it is: the socket's payload
    /// limit or the same device JPEG budget the continuous path keeps.
    fn raw_over_budget(&self, frame: &RgbImage) -> Option<String> {
        match encode::png_bytes(frame) {
            Ok(png) if png.len() > raw_client::MAX_FRAME_BYTES => {
                return Some(format!(
                    "{} bytes as PNG (socket limit {})",
                    png.len(),
                    raw_client::MAX_FRAME_BYTES
                ));
            }
            Ok(_) => {}
            Err(err) => log::warn!("cannot measure raw frame: {err:#}"),
        }
        match encode::estimate_raw(frame) {
            Ok(estimate) if estimate.bytes > self.config.max_jpeg_bytes => Some(format!(
                "{} bytes as device JPEG (budget {})",
                estimate.bytes, self.config.max_jpeg_bytes
            )),
            Ok(_) => None,
            Err(err) => {
                log::warn!("cannot estimate raw frame size: {err:#}");
                None
            }
        }
    }

    /// The AKP05 socket to use: the configured one, the one named after this
    /// deck's OpenDeck device id, or the only socket there is.
    fn raw_socket_path(&self) -> Result<PathBuf, RawError> {
        if let Some(path) = &self.config.raw_socket {
            return Ok(path.clone());
        }
        let device = self
            .slots
            .iter()
            .filter(|slot| matches!(slot.name, SlotName::Dial(_)))
            .find_map(|slot| slot.device.as_deref());
        if let Some(path) = device.and_then(raw_client::socket_path_for_device)
            && path.exists()
        {
            return Ok(path);
        }
        raw_client::find_socket(None)
    }

    /// Before drawing: retry a failed raw socket when due, and leave raw mode
    /// when the strip no longer wants it.
    fn raw_prepare(&mut self) {
        if self.raw_wanted() {
            if self.raw.fallback && self.raw.retry_at.is_some_and(|at| Instant::now() >= at) {
                self.raw.fallback = false;
                self.raw.retry_at = None;
                self.strip_cache = None;
            }
        } else {
            self.raw.fallback = false;
            self.raw.retry_at = None;
            self.raw.retry_delay = RAW_RETRY_INTERVAL;
            self.raw_leave(true);
        }
    }

    /// Sends the strip frame to AKP05 when it changed. A failure falls back to
    /// continuous dial images and schedules a retry.
    fn raw_deliver(&mut self) {
        if !self.raw_wanted() || self.raw.fallback {
            return;
        }
        let Some(png) = self
            .strip_cache
            .as_ref()
            .filter(|cache| cache.key.mode == StripMode::Raw)
            .and_then(|cache| cache.pngs.first())
            .cloned()
        else {
            return;
        };
        let digest = hash_bytes(&png);
        if self.raw.shown == Some(digest) {
            return;
        }
        let result = self
            .raw_socket_path()
            .and_then(|path| raw_client::send_frame_bytes(&path, &png).map(|()| path));
        match result {
            Ok(path) => {
                if self.raw.shown.is_none() {
                    log::info!("raw strip active via {}", path.display());
                }
                log::info!("send raw strip frame ({} bytes)", png.len());
                self.raw.shown = Some(digest);
                self.raw.warning = None;
                self.raw.retry_delay = RAW_RETRY_INTERVAL;
            }
            Err(err) => self.raw_failed(&err),
        }
    }

    fn raw_failed(&mut self, err: &RawError) {
        let message = err.to_string();
        if self.raw.warning.as_deref() != Some(message.as_str()) {
            log::warn!(
                "raw strip unavailable ({message}); drawing continuous dial images, retrying in {}s",
                self.raw.retry_delay.as_secs()
            );
            self.raw.warning = Some(message);
        }
        // The link is not trustworthy, so do not wait on another clear.
        self.raw_leave(false);
        self.raw.fallback = true;
        self.raw.retry_at = Some(Instant::now() + self.raw.retry_delay);
        self.raw.retry_delay = self.raw.retry_delay.saturating_mul(2).min(RAW_RETRY_MAX);
        self.strip_cache = None;
        self.ensure_strip();
    }

    /// Ends raw mode on our side, and in AKP05 when `clear` is set. The dials
    /// are redrawn afterwards, since AKP05 stored no updates while raw was on.
    fn raw_leave(&mut self, clear: bool) {
        if self.raw.shown.take().is_none() {
            return;
        }
        if clear {
            let result = self
                .raw_socket_path()
                .and_then(|path| raw_client::send_clear(&path));
            if let Err(err) = result {
                log::warn!("cannot clear AKP05 raw mode: {err}");
            }
        }
        for slot in &mut self.slots {
            if matches!(slot.name, SlotName::Dial(_)) {
                slot.last = None;
            }
        }
        self.dirty_dials();
    }

    fn schedule_strip_retry(&mut self) {
        self.strip_retry_at = Some(Instant::now() + self.strip_retry_delay);
        self.strip_retry_delay = self
            .strip_retry_delay
            .saturating_mul(2)
            .min(STRIP_RETRY_MAX);
    }

    /// A failed reload keeps the last image only while the geometry still fits.
    fn retain_compatible_strip_cache(&mut self, mode: StripMode, gap: u32) {
        if self
            .strip_cache
            .as_ref()
            .is_some_and(|cache| cache.key.mode != mode || cache.key.gap != gap)
        {
            self.strip_cache = None;
        }
    }

    fn dirty_dials(&mut self) {
        for slot in &self.slots {
            if matches!(slot.name, SlotName::Dial(_)) {
                self.dirty.insert(slot.context.clone());
            }
        }
    }

    fn render_strip_with_budget(
        &self,
        value: &SlotValue,
        mode: StripMode,
        gap: u32,
    ) -> Vec<RgbImage> {
        let mut rendered = self.renderer.render_strip(value, mode, gap, Detail::Full);
        if rendered.truncated {
            log::warn!("strip text was truncated to fit");
        }
        if mode == StripMode::Raw {
            if let Some(reason) = self.raw_over_budget(&rendered.images[0]) {
                log::warn!("raw strip frame is {reason}; degrading to text only");
                rendered = self
                    .renderer
                    .render_strip(value, mode, gap, Detail::TextOnly);
            }
            return rendered.images;
        }
        if mode != StripMode::Continuous {
            return rendered.images;
        }
        match encode::estimate_strip(&rendered.images) {
            Ok(estimate) if estimate.bytes <= self.config.max_jpeg_bytes => {
                return rendered.images;
            }
            Ok(estimate) => {
                log::warn!(
                    "strip image is {} bytes (budget {}); degrading to text only",
                    estimate.bytes,
                    self.config.max_jpeg_bytes
                );
                rendered = self
                    .renderer
                    .render_strip(value, mode, gap, Detail::TextOnly);
            }
            Err(err) => {
                log::warn!("cannot estimate strip size: {err:#}");
                return rendered.images;
            }
        }
        if let Ok(estimate) = encode::estimate_strip(&rendered.images) {
            if estimate.bytes > self.config.max_jpeg_bytes {
                log::warn!(
                    "strip image is still {} bytes, over the {} byte budget",
                    estimate.bytes,
                    self.config.max_jpeg_bytes
                );
            } else {
                log::info!("degraded strip image is {} bytes", estimate.bytes);
            }
        }
        rendered.images
    }

    fn outgoing(&self, name: &SlotName, strip_active: bool) -> Outgoing {
        if let SlotName::Dial(index) = name
            && strip_active
        {
            let index = (*index - 1) as usize;
            if let Some(cache) = &self.strip_cache
                && let Some(png) = cache.pngs.get(index)
            {
                return Outgoing::Image(png.clone());
            }
            return Outgoing::Blank { dial: true };
        }

        let value = self
            .status
            .get_slot_str(&name.as_wire_name())
            .ok()
            .flatten();
        let dial = matches!(name, SlotName::Dial(_));
        let empty = value.as_ref().is_none_or(SlotValue::is_empty);
        if empty {
            if self.status.labels() {
                return Outgoing::Title {
                    text: name.as_wire_name(),
                    dial,
                };
            }
            return Outgoing::Blank { dial };
        }
        let value = value.expect("checked above");
        match name {
            SlotName::Key { .. } => Outgoing::Title {
                text: slot_plain_text(&value),
                dial: false,
            },
            SlotName::Dial(_) => {
                if needs_image(&value) {
                    let spec: SlotSpec = self.renderer.spec(&value);
                    let rendered = self.renderer.render_slot(&spec, 200, 100, Detail::Full);
                    match encode::png_bytes(&rendered.images[0]) {
                        Ok(png) => Outgoing::Image(png),
                        Err(err) => {
                            log::warn!("cannot render dial image: {err:#}");
                            Outgoing::Title {
                                text: slot_plain_text(&value),
                                dial: true,
                            }
                        }
                    }
                } else {
                    Outgoing::Title {
                        text: slot_plain_text(&value),
                        dial: true,
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Outgoing {
    Title { text: String, dial: bool },
    Blank { dial: bool },
    Image(Vec<u8>),
}

fn title_message(context: &str, text: &str) -> Value {
    json!({
        "event": "setTitle",
        "context": context,
        "payload": {"title": text, "target": 0},
    })
}

fn feedback_message(context: &str, feedback: &Value) -> Value {
    json!({
        "event": "setFeedback",
        "context": context,
        "payload": feedback,
    })
}

fn title_feedback(text: &str) -> Value {
    json!({
        "full-canvas": {"enabled": false},
        "title": {"enabled": true, "value": text},
        "canvas": {"enabled": false},
    })
}

fn blank_feedback() -> Value {
    json!({
        "full-canvas": {"enabled": false},
        "title": {"enabled": false, "value": ""},
        "canvas": {"enabled": false},
    })
}

fn image_feedback(png: &[u8]) -> Value {
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(png)
    );
    json!({
        "full-canvas": {"value": data_url, "enabled": true},
        "title": {"enabled": false, "value": ""},
        "canvas": {"enabled": false},
    })
}

fn send_title(
    socket: &mut WebSocket<TcpStream>,
    context: &str,
    text: &str,
    name: &SlotName,
) -> Result<()> {
    send_json(socket, &title_message(context, text))?;
    log::info!("send {name} title ({} chars)", text.chars().count());
    Ok(())
}

fn send_feedback(
    socket: &mut WebSocket<TcpStream>,
    context: &str,
    feedback: &Value,
    name: &SlotName,
) -> Result<()> {
    send_json(socket, &feedback_message(context, feedback))?;
    log::info!("send {name} feedback");
    Ok(())
}

fn load_slices(dir: &Path, mode: StripMode) -> Result<Vec<RgbImage>> {
    let expected = match mode {
        StripMode::Continuous => (200u32, 100u32),
        StripMode::Tiles => (176u32, 88u32),
        StripMode::Raw => (200u32, 128u32),
    };
    let mut out = Vec::with_capacity(4);
    for index in 0..4 {
        let path = dir.join(format!("slice{index}.png"));
        let image = fs::read(&path)
            .with_context(|| format!("cannot read {}", path.display()))
            .and_then(|bytes| encode::decode_png(&bytes))?;
        if image.dimensions() != expected {
            anyhow::bail!(
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

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plugin_arguments() {
        let args = vec![
            "plugin".into(),
            "-port".into(),
            "57116".into(),
            "-pluginUUID".into(),
            "local.statusdisplay.sdPlugin".into(),
            "-registerEvent".into(),
            "registerPlugin".into(),
            "-info".into(),
            "{}".into(),
        ];
        let parsed = args_from_argv(args).unwrap();
        assert_eq!(parsed.port, 57116);
        assert_eq!(parsed.uuid, "local.statusdisplay.sdPlugin");
        assert_eq!(parsed.register_event, "registerPlugin");
        assert!(args_from_argv(["plugin".into()]).is_none());
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
        assert_ne!(hash_bytes(b"abc"), hash_bytes(b"abd"));
    }

    #[test]
    fn slice_dimensions_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..4 {
            let bytes = encode::png_bytes(&RgbImage::new(800, 128)).unwrap();
            fs::write(dir.path().join(format!("slice{index}.png")), bytes).unwrap();
        }
        let err = load_slices(dir.path(), StripMode::Continuous).unwrap_err();
        assert!(err.to_string().contains("expected 200x100"), "{err:#}");
    }

    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    /// A fake AKP05 that records every message and answers `0x00`.
    fn fake_akp05(path: &Path) -> Arc<Mutex<Vec<Vec<u8>>>> {
        let listener = UnixListener::bind(path).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut header = [0u8; 5];
                if stream.read_exact(&mut header).is_err() {
                    continue;
                }
                let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
                let mut payload = vec![0u8; len];
                stream.read_exact(&mut payload).unwrap();
                seen.lock()
                    .unwrap()
                    .push([header.to_vec(), payload].concat());
                stream.write_all(&[0]).unwrap();
            }
        });
        log
    }

    fn raw_app(dir: &Path, socket: &Path) -> App {
        let config = Config {
            raw_socket: Some(socket.to_owned()),
            ..Config::default()
        };
        let path = dir.join("status.json");
        fs::write(
            &path,
            r#"{"strip":"RAW|A|B|C","strip_mode":"raw","dial1":"x"}"#,
        )
        .unwrap();
        let mut app = App::new(path, config);
        app.reload();
        app.slots.push(BoundSlot {
            context: "dial-1".to_owned(),
            name: SlotName::Dial(1),
            device: Some("n4-TEST".to_owned()),
            last: Some("old".to_owned()),
        });
        app
    }

    fn deliver(app: &mut App) {
        app.raw_prepare();
        app.ensure_strip();
        app.raw_deliver();
    }

    #[test]
    fn raw_frame_is_sent_once_as_one_800x128_png() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("akp05.sock");
        let log = fake_akp05(&socket);
        let mut app = raw_app(dir.path(), &socket);

        deliver(&mut app);
        deliver(&mut app);
        let messages = log.lock().unwrap().clone();
        assert_eq!(messages.len(), 1, "an unchanged frame is not resent");
        assert_eq!(messages[0][0], 0x01);
        let frame = encode::decode_png(&messages[0][5..]).unwrap();
        assert_eq!(frame.dimensions(), (800, 128));
        assert!(app.raw.shown.is_some());
        assert!(!app.raw.fallback);
        assert_eq!(app.mode(), StripMode::Raw);
    }

    #[test]
    fn a_missing_socket_falls_back_to_continuous_dial_images() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = raw_app(dir.path(), &dir.path().join("nothing.sock"));

        deliver(&mut app);
        assert!(app.raw.fallback);
        assert!(app.raw.shown.is_none());
        assert!(app.raw.retry_at.is_some());
        assert_eq!(app.mode(), StripMode::Continuous);
        let cache = app.strip_cache.as_ref().expect("fallback strip is drawn");
        assert_eq!(cache.key.mode, StripMode::Continuous);
        assert_eq!(cache.pngs.len(), 4);
    }

    #[test]
    fn the_retry_returns_to_raw_once_the_socket_exists() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("akp05.sock");
        let mut app = raw_app(dir.path(), &socket);
        deliver(&mut app);
        assert!(app.raw.fallback);

        let log = fake_akp05(&socket);
        app.raw.retry_at = Some(Instant::now());
        deliver(&mut app);
        assert!(!app.raw.fallback);
        assert!(app.raw.shown.is_some());
        assert_eq!(log.lock().unwrap().len(), 1);
        assert_eq!(app.mode(), StripMode::Raw);
    }

    #[test]
    fn leaving_raw_clears_akp05_and_redraws_the_dials() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("akp05.sock");
        let log = fake_akp05(&socket);
        let mut app = raw_app(dir.path(), &socket);
        deliver(&mut app);
        assert!(app.raw.shown.is_some());
        app.dirty.clear();

        fs::write(&app.path, r#"{"dial1":"x"}"#).unwrap();
        app.reload();
        app.raw_prepare();
        assert!(app.raw.shown.is_none());
        let messages = log.lock().unwrap().clone();
        assert_eq!(messages.last().unwrap(), &[0x02, 0, 0, 0, 0]);
        assert!(app.dirty.contains("dial-1"));
        assert_eq!(app.slots[0].last, None, "dials must be sent again");
    }

    #[test]
    fn a_new_dial_appearance_resends_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("akp05.sock");
        let log = fake_akp05(&socket);
        let mut app = raw_app(dir.path(), &socket);
        deliver(&mut app);
        // OpenDeck re-announcing a dial clears `shown`, like a reconnect.
        app.raw.shown = None;
        deliver(&mut app);
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[test]
    fn oversized_raw_frames_degrade_to_text_only() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("akp05.sock");
        let app = raw_app(dir.path(), &socket);
        let mut noise = RgbImage::new(800, 128);
        for (x, y, pixel) in noise.enumerate_pixels_mut() {
            let v = (x * 73 + y * 151) as u8;
            *pixel = image::Rgb([v, v.wrapping_mul(3), v.wrapping_add(77)]);
        }
        assert!(app.raw_over_budget(&noise).is_some());
        assert!(app.raw_over_budget(&RgbImage::new(800, 128)).is_none());
    }
}
