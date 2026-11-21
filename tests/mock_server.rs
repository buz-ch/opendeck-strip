use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use image::RgbImage;
use opendeck_strip::status::{self, Status};
use serde_json::{Value, json};
use tungstenite::Message;
use tungstenite::WebSocket;

type ServerSocket = WebSocket<std::net::TcpStream>;

struct Harness {
    child: Child,
    socket: ServerSocket,
    dir: tempfile::TempDir,
    path: PathBuf,
}

impl Harness {
    fn start() -> Self {
        Self::start_with_config(|_| String::new())
    }

    /// `extra` returns additional config.toml lines; it gets the temp dir.
    fn start_with_config(extra: impl FnOnce(&std::path::Path) -> String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, format!("redraw_hz = 50\n{}", extra(dir.path()))).unwrap();

        let child = Command::new(env!("CARGO_BIN_EXE_opendeck-strip"))
            .args([
                "-port",
                &port.to_string(),
                "-pluginUUID",
                "local.statusdisplay.sdPlugin",
                "-registerEvent",
                "registerPlugin",
                "-info",
                "{}",
            ])
            .env("OPENDECK_STATUS_FILE", &path)
            .env("OPENDECK_STRIP_CONFIG", &config)
            .env("RUST_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        socket
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        Self {
            child,
            socket,
            dir,
            path,
        }
    }

    fn send(&mut self, value: Value) {
        self.socket.send(Message::text(value.to_string())).unwrap();
    }

    fn drain(&mut self, window: Duration) -> Vec<Value> {
        let deadline = Instant::now() + window;
        let mut out = Vec::new();
        while Instant::now() < deadline {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    if let Ok(value) = serde_json::from_str::<Value>(text.as_str()) {
                        out.push(value);
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => {}
            }
        }
        out
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        status::update(&self.path, f).unwrap();
    }

    fn write_slices(&self, colors: [[u8; 3]; 4]) -> PathBuf {
        let dir = self.dir.path().join("slices");
        std::fs::create_dir_all(&dir).unwrap();
        for (index, color) in colors.into_iter().enumerate() {
            let image = RgbImage::from_pixel(200, 100, image::Rgb(color));
            let bytes = opendeck_strip::encode::png_bytes(&image).unwrap();
            status::write_bytes_atomic(&dir.join(format!("slice{index}.png")), &bytes).unwrap();
        }
        dir
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn appear(context: &str, controller: &str, row: u64, column: u64) -> Value {
    json!({
        "event": "willAppear",
        "action": "local.statusdisplay.slot",
        "context": context,
        "device": "n4e-test",
        "payload": {
            "settings": {},
            "coordinates": {"row": row, "column": column},
            "controller": controller,
            "state": 0,
            "isInMultiAction": false
        }
    })
}

fn disappear(context: &str) -> Value {
    json!({
        "event": "willDisappear",
        "action": "local.statusdisplay.slot",
        "context": context,
        "device": "n4e-test",
        "payload": {}
    })
}

fn find<'a>(messages: &'a [Value], event: &str, context: &str) -> Option<&'a Value> {
    messages
        .iter()
        .find(|m| m["event"] == event && m["context"] == context)
}

fn title(messages: &[Value], context: &str) -> Option<String> {
    find(messages, "setTitle", context).map(|m| {
        m["payload"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    })
}

fn feedback<'a>(messages: &'a [Value], context: &str) -> Option<&'a Value> {
    messages
        .iter()
        .rev()
        .find(|m| m["event"] == "setFeedback" && m["context"] == context)
        .map(|m| &m["payload"])
}

fn feedbacks<'a>(messages: &'a [Value], context: &str) -> Vec<&'a Value> {
    messages
        .iter()
        .filter(|m| m["event"] == "setFeedback" && m["context"] == context)
        .map(|m| &m["payload"])
        .collect()
}

fn image_feedback<'a>(messages: &'a [Value], context: &str) -> Option<&'a Value> {
    messages
        .iter()
        .rev()
        .filter(|m| m["event"] == "setFeedback" && m["context"] == context)
        .map(|m| &m["payload"])
        .find(|payload| payload["full-canvas"]["value"].is_string())
}

fn image_from_feedback(feedback: &Value) -> RgbImage {
    let url = feedback["full-canvas"]["value"]
        .as_str()
        .expect("image feedback has a data URL");
    let (_, payload) = url.split_once(',').expect("data URL");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .unwrap();
    opendeck_strip::encode::decode_png(&bytes).unwrap()
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<i32> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn plugin_protocol_end_to_end() {
    let mut h = Harness::start();

    // 1-2: registration.
    let register = h.drain(Duration::from_millis(300));
    assert_eq!(register[0]["event"], "registerPlugin");
    assert_eq!(register[0]["uuid"], "local.statusdisplay.sdPlugin");

    // Bind a key, three more keys, and all four dials.
    h.send(appear("ctx-key-1-1", "Keypad", 0, 0));
    h.send(appear("ctx-key-1-2", "Keypad", 0, 1));
    h.send(appear("ctx-key-1-3", "Keypad", 0, 2));
    for index in 0..4 {
        h.send(appear(
            &format!("ctx-dial-{}", index + 1),
            "Encoder",
            0,
            index,
        ));
    }
    let messages = h.drain(Duration::from_millis(600));

    // 3-6: blank slots are blank, not the action name.
    assert_eq!(title(&messages, "ctx-key-1-1").as_deref(), Some(""));
    let dial1 = feedback(&messages, "ctx-dial-1").expect("dial feedback");
    assert_eq!(dial1["title"]["enabled"], false);
    assert_eq!(dial1["canvas"]["enabled"], false);
    assert_eq!(dial1["full-canvas"]["enabled"], false);

    // Enter text and labels.
    h.update(|status| {
        status.set_raw("labels", Value::Bool(true));
        status.set_slot("key1_2", status::SlotValue::from_text("A"));
        status.set_slot("dial2", status::SlotValue::from_text("B"));
    });
    let messages = h.drain(Duration::from_millis(1000));
    // 7-10: labels and values.
    assert_eq!(title(&messages, "ctx-key-1-1").as_deref(), Some("key1_1"));
    assert_eq!(title(&messages, "ctx-key-1-2").as_deref(), Some("A"));
    assert_eq!(title(&messages, "ctx-dial-2").as_deref(), Some("B"));
    let dial2 = feedback(&messages, "ctx-dial-2").expect("dial2 feedback");
    assert_eq!(dial2["title"]["enabled"], true);

    // An empty strip object is not strip mode; dials keep their titles.
    h.update(|status| {
        status.set_raw("strip", json!({}));
        status.set_slot("dial2", status::SlotValue::from_text("B2"));
    });
    let messages = h.drain(Duration::from_millis(800));
    assert_eq!(title(&messages, "ctx-dial-2").as_deref(), Some("B2"));
    assert!(image_feedback(&messages, "ctx-dial-2").is_none());
    assert!(
        feedbacks(&messages, "ctx-dial-2")
            .iter()
            .any(|payload| payload["title"]["enabled"] == true)
    );
    h.update(|status| {
        status.remove("strip");
    });
    h.drain(Duration::from_millis(200));

    // 11-14: entering strip mode sends empty titles first, then images.
    h.update(|status| {
        status.set_raw("strip", json!("A|B|C|D"));
    });
    let messages = h.drain(Duration::from_millis(1200));
    let mut slices = Vec::new();
    for index in 1..=4 {
        let context = format!("ctx-dial-{index}");
        assert_eq!(
            title(&messages, &context).as_deref(),
            Some(""),
            "empty title before strip image"
        );
        let payload = feedback(&messages, &context).expect("strip feedback");
        let image = image_from_feedback(payload);
        assert_eq!(image.dimensions(), (200, 100));
        assert!(image.pixels().any(|p| p.0 != [0, 0, 0]));
        slices.push(image);
    }
    for pair in slices.windows(2) {
        assert_ne!(pair[0].as_raw(), pair[1].as_raw());
    }

    // 15: changing strip text redraws it.
    h.update(|status| {
        status.set_raw("strip", json!("W|X|Y|Z"));
    });
    let messages = h.drain(Duration::from_millis(1000));
    let changed = image_from_feedback(feedback(&messages, "ctx-dial-2").unwrap());
    assert_ne!(slices[1].as_raw(), changed.as_raw());

    // 16: @slices images are sent verbatim.
    let dir = h.write_slices([[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]]);
    h.update(|status| {
        status.set_raw("strip", json!(format!("@slices:{}#1", dir.display())));
    });
    let messages = h.drain(Duration::from_millis(1200));
    for (index, expected) in [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]]
        .into_iter()
        .enumerate()
    {
        let context = format!("ctx-dial-{}", index + 1);
        let image = image_from_feedback(image_feedback(&messages, &context).unwrap());
        assert_eq!(image.get_pixel(100, 50).0, expected);
    }

    // 17: clearing the strip empties each dial and restores the title layout.
    h.update(|status| {
        status.remove("strip");
        for index in 1..=4 {
            status.remove(&format!("dial{index}"));
        }
    });
    let messages = h.drain(Duration::from_millis(1200));
    for index in 1..=4 {
        let context = format!("ctx-dial-{index}");
        assert_eq!(title(&messages, &context).as_deref(), Some(""));
        let payloads = feedbacks(&messages, &context);
        assert!(
            payloads.iter().any(|p| p["title"]["enabled"] == true),
            "restored title layout"
        );
        assert!(
            payloads.iter().any(|p| p["title"]["enabled"] == false),
            "image cleared before the title layout"
        );
    }

    // 18: slots that disappear receive no more updates.
    h.send(disappear("ctx-key-1-3"));
    h.drain(Duration::from_millis(200));
    h.update(|status| {
        status.set_slot("key1_3", status::SlotValue::from_text("Y"));
    });
    let messages = h.drain(Duration::from_millis(1000));
    assert!(
        messages.iter().all(|m| m["context"] != "ctx-key-1-3"),
        "disappeared slot was updated"
    );

    // 19: closing the socket exits cleanly with code 0.
    h.socket.close(None).ok();
    let code = wait_for_exit(&mut h.child, Duration::from_secs(5));
    assert_eq!(code, Some(0));
}

#[test]
fn slices_failure_is_retried_without_a_status_change() {
    let mut h = Harness::start();
    h.drain(Duration::from_millis(300));
    for index in 0..4 {
        h.send(appear(
            &format!("ctx-dial-{}", index + 1),
            "Encoder",
            0,
            index,
        ));
    }
    h.drain(Duration::from_millis(500));

    let dir = h.dir.path().join("late-slices");
    h.update(|status| {
        status.set_raw("strip", json!(format!("@slices:{}#1", dir.display())));
    });
    let early = h.drain(Duration::from_millis(600));
    assert!(
        image_feedback(&early, "ctx-dial-1").is_none(),
        "missing slices must not be shown as an image"
    );
    assert!(
        feedbacks(&early, "ctx-dial-1")
            .iter()
            .any(|payload| payload["full-canvas"]["enabled"] == false),
        "the failed load should fall back to the title layout"
    );

    let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
    std::fs::create_dir_all(&dir).unwrap();
    for (index, color) in colors.into_iter().enumerate() {
        let image = RgbImage::from_pixel(200, 100, image::Rgb(color));
        let bytes = opendeck_strip::encode::png_bytes(&image).unwrap();
        status::write_bytes_atomic(&dir.join(format!("slice{index}.png")), &bytes).unwrap();
    }

    let messages = h.drain(Duration::from_millis(2500));
    for (index, expected) in colors.into_iter().enumerate() {
        let context = format!("ctx-dial-{}", index + 1);
        let payload = image_feedback(&messages, &context)
            .unwrap_or_else(|| panic!("dial {} did not receive a retried image", index + 1));
        let image = image_from_feedback(payload);
        assert_eq!(image.get_pixel(100, 50).0, expected);
    }
}

/// A fake AKP05 raw socket that records every message it receives.
#[cfg(unix)]
fn fake_raw_socket(path: &std::path::Path) -> std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>> {
    use std::io::{Read, Write};
    let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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

#[cfg(unix)]
#[test]
fn raw_mode_sends_one_frame_to_the_socket_and_no_dial_images() {
    let mut socket_path = None;
    let mut h = Harness::start_with_config(|dir| {
        let path = dir.join("akp05.sock");
        let line = format!("raw_socket = {:?}\n", path.display().to_string());
        socket_path = Some(path);
        line
    });
    let log = fake_raw_socket(&socket_path.unwrap());
    h.drain(Duration::from_millis(300));
    for index in 0..4 {
        h.send(appear(
            &format!("ctx-dial-{}", index + 1),
            "Encoder",
            0,
            index,
        ));
    }
    h.drain(Duration::from_millis(500));

    h.update(|status| {
        status.set_raw("strip_mode", json!("raw"));
        status.set_raw("strip", json!("RAW|A|B|C"));
    });
    let messages = h.drain(Duration::from_millis(1200));
    for index in 1..=4 {
        assert!(
            image_feedback(&messages, &format!("ctx-dial-{index}")).is_none(),
            "dial {index} got an image while the raw frame is shown"
        );
    }
    {
        let received = log.lock().unwrap();
        assert_eq!(received.len(), 1, "exactly one raw frame");
        assert_eq!(received[0][0], 0x01);
        let frame = opendeck_strip::encode::decode_png(&received[0][5..]).unwrap();
        assert_eq!(frame.dimensions(), (800, 128));
    }

    // Removing the strip clears raw mode and brings the dials back.
    h.update(|status| {
        status.remove("strip");
    });
    let messages = h.drain(Duration::from_millis(1200));
    let received = log.lock().unwrap();
    assert_eq!(received.last().unwrap(), &[0x02, 0, 0, 0, 0]);
    assert!(
        feedback(&messages, "ctx-dial-1").is_some(),
        "dials are redrawn after leaving raw mode"
    );
}

#[cfg(unix)]
#[test]
fn raw_mode_without_a_socket_falls_back_to_dial_images() {
    let mut h = Harness::start_with_config(|dir| {
        format!(
            "raw_socket = {:?}\n",
            dir.join("missing.sock").display().to_string()
        )
    });
    h.drain(Duration::from_millis(300));
    for index in 0..4 {
        h.send(appear(
            &format!("ctx-dial-{}", index + 1),
            "Encoder",
            0,
            index,
        ));
    }
    h.drain(Duration::from_millis(500));
    h.update(|status| {
        status.set_raw("strip_mode", json!("raw"));
        status.set_raw("strip", json!("RAW|A|B|C"));
    });
    let messages = h.drain(Duration::from_millis(1500));
    for index in 1..=4 {
        let payload = image_feedback(&messages, &format!("ctx-dial-{index}"))
            .unwrap_or_else(|| panic!("dial {index} did not get a fallback image"));
        assert_eq!(image_from_feedback(payload).dimensions(), (200, 100));
    }
}
