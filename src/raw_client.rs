//! Client for the AKP05 raw strip socket.
//!
//! With `strip_raw = true`, the AKP05 device plugin serves one Unix socket per
//! deck at `$XDG_RUNTIME_DIR/opendeck-akp05/<device-id>.sock`. A message is a
//! type byte (`0x01` frame, `0x02` clear), a 32-bit big-endian payload length
//! and the payload. Every message gets one reply byte: `0x00` is success.
//!
//! This module is blocking and uses short timeouts, so a stuck device plugin
//! cannot stall the caller for long.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::RgbImage;

use crate::encode;

/// Largest payload the device plugin accepts.
pub const MAX_FRAME_BYTES: usize = 512 * 1024;
/// Connect, write and reply timeout.
pub const TIMEOUT: Duration = Duration::from_secs(2);

const TYPE_FRAME: u8 = 0x01;
const TYPE_CLEAR: u8 = 0x02;

/// Why a raw socket call failed.
#[derive(Debug)]
pub enum RawError {
    /// No socket could be located.
    NoSocket(String),
    /// More than one socket exists and none was chosen.
    Ambiguous(Vec<PathBuf>),
    /// The payload is over the device plugin's limit; nothing was sent.
    TooLarge(usize),
    /// The frame could not be encoded.
    Encode(String),
    /// Connecting, writing or reading failed.
    Io(io::Error),
    /// The device plugin answered with a non-zero reply byte.
    Rejected(u8),
}

impl RawError {
    /// Whether retrying the same frame later may succeed.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Io(_) => true,
            Self::Rejected(code) => *code == 0x05,
            _ => false,
        }
    }
}

impl fmt::Display for RawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSocket(detail) => write!(f, "no AKP05 raw socket: {detail}"),
            Self::Ambiguous(paths) => {
                write!(
                    f,
                    "several AKP05 raw sockets found, pick one with --socket:"
                )?;
                for path in paths {
                    write!(f, " {}", path.display())?;
                }
                Ok(())
            }
            Self::TooLarge(bytes) => write!(
                f,
                "frame is {bytes} bytes, over the {MAX_FRAME_BYTES} byte limit"
            ),
            Self::Encode(detail) => write!(f, "cannot encode frame: {detail}"),
            Self::Io(err) => write!(f, "socket error: {err}"),
            Self::Rejected(code) => write!(f, "AKP05 rejected the message: {}", reply_name(*code)),
        }
    }
}

impl std::error::Error for RawError {}

impl From<io::Error> for RawError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Human-readable name of a reply byte.
pub fn reply_name(code: u8) -> String {
    match code {
        0x00 => "ok".to_owned(),
        0x01 => "malformed message or undecodable image (0x01)".to_owned(),
        0x02 => "unsupported encoding or colour model (0x02)".to_owned(),
        0x03 => "wrong size, expected 800x128 (0x03)".to_owned(),
        0x04 => "payload too large (0x04)".to_owned(),
        0x05 => "device write failed or deck asleep (0x05)".to_owned(),
        0x06 => "unknown message type (0x06)".to_owned(),
        other => format!("unknown reply 0x{other:02x}"),
    }
}

/// The device plugin's file name for a device id: anything outside
/// `[A-Za-z0-9._-]` becomes `_`, and a leading dot or an empty name gets a
/// leading `_`.
pub fn sanitize(device_id: &str) -> String {
    let mut name: String = device_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() || name.starts_with('.') {
        name.insert(0, '_');
    }
    name
}

/// `$XDG_RUNTIME_DIR/opendeck-akp05`, if the runtime directory is known.
pub fn socket_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(runtime).join("opendeck-akp05"))
}

/// The socket path the device plugin uses for `device_id` inside `dir`.
pub fn socket_path_in(dir: &Path, device_id: &str) -> PathBuf {
    dir.join(format!("{}.sock", sanitize(device_id)))
}

/// The socket path for `device_id` in the runtime directory.
pub fn socket_path_for_device(device_id: &str) -> Option<PathBuf> {
    Some(socket_path_in(&socket_dir()?, device_id))
}

/// Every `*.sock` file in `dir`, sorted.
pub fn list_sockets(dir: &Path) -> Vec<PathBuf> {
    let mut sockets: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sock"))
        .collect();
    sockets.sort();
    sockets
}

/// Chooses the socket to use: `explicit` if given, otherwise the single socket
/// in the runtime directory.
pub fn find_socket(explicit: Option<&Path>) -> Result<PathBuf, RawError> {
    if let Some(path) = explicit {
        return Ok(path.to_owned());
    }
    let dir =
        socket_dir().ok_or_else(|| RawError::NoSocket("XDG_RUNTIME_DIR is not set".to_owned()))?;
    find_socket_in(&dir)
}

/// Like [`find_socket`], for a given socket directory.
pub fn find_socket_in(dir: &Path) -> Result<PathBuf, RawError> {
    let mut sockets = list_sockets(dir);
    match sockets.len() {
        0 => Err(RawError::NoSocket(format!(
            "nothing in {} (is strip_raw = true set for AKP05 and the deck connected?)",
            dir.display()
        ))),
        1 => Ok(sockets.remove(0)),
        _ => Err(RawError::Ambiguous(sockets)),
    }
}

/// Sends `frame` as a PNG and waits for the reply.
pub fn send_frame(path: &Path, frame: &RgbImage) -> Result<(), RawError> {
    let png = encode::png_bytes(frame).map_err(|err| RawError::Encode(format!("{err:#}")))?;
    send_frame_bytes(path, &png)
}

/// Sends an already encoded PNG or JPEG and waits for the reply.
pub fn send_frame_bytes(path: &Path, bytes: &[u8]) -> Result<(), RawError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(RawError::TooLarge(bytes.len()));
    }
    exchange(path, TYPE_FRAME, bytes)
}

/// Ends raw mode and waits for the reply.
pub fn send_clear(path: &Path) -> Result<(), RawError> {
    exchange(path, TYPE_CLEAR, &[])
}

fn exchange(path: &Path, kind: u8, payload: &[u8]) -> Result<(), RawError> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    let mut message = Vec::with_capacity(5 + payload.len());
    message.push(kind);
    message.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    message.extend_from_slice(payload);
    stream.write_all(&message)?;
    stream.flush()?;

    let mut reply = [0u8; 1];
    stream.read_exact(&mut reply)?;
    match reply[0] {
        0x00 => Ok(()),
        code => Err(RawError::Rejected(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread::{self, JoinHandle};

    /// A fake device plugin that answers one message with `reply` and returns
    /// the bytes it received.
    fn fake_server(dir: &Path, reply: u8) -> (PathBuf, JoinHandle<Vec<u8>>) {
        let path = dir.join("n4-TEST.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header = [0u8; 5];
            stream.read_exact(&mut header).unwrap();
            let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
            let mut payload = vec![0u8; len];
            stream.read_exact(&mut payload).unwrap();
            stream.write_all(&[reply]).unwrap();
            [header.to_vec(), payload].concat()
        });
        (path, handle)
    }

    #[test]
    fn sends_a_png_frame_with_the_documented_framing() {
        let dir = tempfile::tempdir().unwrap();
        let (path, server) = fake_server(dir.path(), 0x00);
        let frame = RgbImage::from_pixel(800, 128, image::Rgb([10, 20, 30]));
        send_frame(&path, &frame).unwrap();

        let received = server.join().unwrap();
        assert_eq!(received[0], 0x01);
        let len = u32::from_be_bytes(received[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, received.len() - 5);
        let decoded = encode::decode_png(&received[5..]).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn sends_clear_with_an_empty_payload() {
        let dir = tempfile::tempdir().unwrap();
        let (path, server) = fake_server(dir.path(), 0x00);
        send_clear(&path).unwrap();
        assert_eq!(server.join().unwrap(), [0x02, 0, 0, 0, 0]);
    }

    #[test]
    fn non_zero_replies_are_typed_errors() {
        let dir = tempfile::tempdir().unwrap();
        let (path, server) = fake_server(dir.path(), 0x03);
        let err = send_frame_bytes(&path, b"not really an image").unwrap_err();
        server.join().unwrap();
        assert!(matches!(err, RawError::Rejected(0x03)));
        assert!(!err.is_transient());
        assert!(err.to_string().contains("800x128"));
        assert!(RawError::Rejected(0x05).is_transient());
    }

    #[test]
    fn oversized_frames_are_rejected_locally() {
        let dir = tempfile::tempdir().unwrap();
        // No server: the call must fail before it connects.
        let path = dir.path().join("missing.sock");
        let err = send_frame_bytes(&path, &vec![0u8; MAX_FRAME_BYTES + 1]).unwrap_err();
        assert!(matches!(err, RawError::TooLarge(n) if n == MAX_FRAME_BYTES + 1));
        let exactly = send_frame_bytes(&path, &vec![0u8; MAX_FRAME_BYTES]).unwrap_err();
        assert!(
            matches!(exactly, RawError::Io(_)),
            "limit itself is allowed"
        );
    }

    #[test]
    fn a_missing_socket_is_a_transient_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = send_clear(&dir.path().join("gone.sock")).unwrap_err();
        assert!(matches!(err, RawError::Io(_)));
        assert!(err.is_transient());
    }

    #[test]
    fn finds_the_only_socket() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_socket_in(dir.path()),
            Err(RawError::NoSocket(_))
        ));
        let (path, server) = fake_server(dir.path(), 0x00);
        assert_eq!(find_socket_in(dir.path()).unwrap(), path);
        // Unrelated files are ignored.
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert_eq!(find_socket_in(dir.path()).unwrap(), path);
        // Let the server thread finish.
        send_clear(&path).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn several_sockets_are_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let _a = UnixListener::bind(dir.path().join("a.sock")).unwrap();
        let _b = UnixListener::bind(dir.path().join("b.sock")).unwrap();
        match find_socket_in(dir.path()) {
            Err(RawError::Ambiguous(paths)) => assert_eq!(paths.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn explicit_paths_win() {
        let path = Path::new("/somewhere/else.sock");
        assert_eq!(find_socket(Some(path)).unwrap(), path);
    }

    #[test]
    fn socket_names_match_the_device_plugin() {
        let dir = Path::new("/run/user/1000/opendeck-akp05");
        assert_eq!(
            socket_path_in(dir, "n4-ABC123"),
            Path::new("/run/user/1000/opendeck-akp05/n4-ABC123.sock")
        );
        assert_eq!(
            socket_path_in(dir, "../etc/x"),
            Path::new("/run/user/1000/opendeck-akp05/_.._etc_x.sock")
        );
        assert_eq!(
            socket_path_in(dir, ""),
            Path::new("/run/user/1000/opendeck-akp05/_.sock")
        );
    }
}
