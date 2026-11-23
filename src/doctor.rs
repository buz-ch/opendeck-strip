use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;

use crate::install;
use crate::status::{self, StripMode};

pub fn run() -> Result<bool> {
    let mut ok = true;
    let mut warn = 0;
    let config = crate::config::Config::load();

    let status_path = status::status_path();
    println!(
        "status file: {} (from {})",
        status_path.display(),
        status::path_source()
    );
    if status_path.exists() {
        match status::load(&status_path) {
            Ok(status) => println!(
                "  readable: {} slot(s), strip {}",
                status.named_slots().len(),
                if status.strip_active() { "on" } else { "off" }
            ),
            Err(err) => {
                println!("  FAIL: {err:#}");
                ok = false;
            }
        }
    } else {
        println!("  missing; the plugin will create it on the first write");
    }

    if config.warnings.is_empty() {
        println!("config: no warnings");
    } else {
        println!("config warnings:");
        for warning in &config.warnings {
            println!("  warning: {warning}");
        }
        warn += config.warnings.len();
    }

    #[cfg(unix)]
    match open_deck_status_path() {
        Ok(Some((pid, plugin_path))) => {
            println!("OpenDeck process: pid {pid}");
            if plugin_path == status_path {
                println!(
                    "  status file: {} (same as this shell)",
                    plugin_path.display()
                );
            } else {
                println!(
                    "  status file: {} (DIFFERS from this shell)",
                    plugin_path.display()
                );
                println!("  set OPENDECK_STATUS_FILE in OpenDeck's environment");
                ok = false;
            }
        }
        Ok(None) => {
            println!("OpenDeck process: not running; cannot compare the plugin environment")
        }
        Err(err) => println!("OpenDeck process: cannot inspect /proc: {err}"),
    }

    let roots = install::plugin_roots();
    println!("plugin directories:");
    let mut selected = None;
    for root in &roots {
        let dir = install::installed_dir(root);
        if dir.exists() {
            selected = Some(root.clone());
            let manifest = install::read_installed_manifest(root);
            match manifest {
                Ok(manifest) => {
                    let binary = dir.join("opendeck-strip");
                    let executable = binary.exists() && {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            fs::metadata(&binary)
                                .map(|m| m.permissions().mode() & 0o111 != 0)
                                .unwrap_or(false)
                        }
                        #[cfg(not(unix))]
                        {
                            binary.exists()
                        }
                    };
                    println!(
                        "  ok: {} (version {}, binary {})",
                        dir.display(),
                        manifest["Version"].as_str().unwrap_or("?"),
                        if executable { "executable" } else { "MISSING" }
                    );
                    if !executable {
                        ok = false;
                    }
                }
                Err(err) => {
                    println!("  FAIL: {}: {err:#}", dir.display());
                    ok = false;
                }
            }
        } else {
            println!("  not installed: {}", dir.display());
        }
    }
    if selected.is_none() {
        println!("  run `opendeck-strip install` (quit OpenDeck first)");
        ok = false;
    }

    if roots.get(1).is_some_and(|p| selected.as_ref() == Some(p)) {
        println!("flatpak:");
        println!("  OpenDeck runs in a sandbox; XDG_RUNTIME_DIR is shared, but");
        println!("  OPENDECK_STATUS_FILE must be passed with `flatpak override --env=`");
        println!("  if you set it.");
    }

    match zellij_version() {
        Ok(Some(version)) => {
            println!("zellij: {}.{}.{} on PATH", version.0, version.1, version.2);
            if version < (0, 46, 0) {
                println!("  FAIL: `follow` needs zellij 0.46.0 or newer");
                ok = false;
            } else {
                println!("  ok: supports the deck pipe");
            }
            if config.follow {
                match crate::follow::live_sessions(Path::new("zellij")) {
                    Ok(names) if names.is_empty() => {
                        println!("  warning: follow = true but no live zellij session");
                        warn += 1;
                    }
                    Ok(names) => println!("  live session(s): {}", names.join(", ")),
                    Err(err) => {
                        println!("  warning: cannot list sessions: {err:#}");
                        warn += 1;
                    }
                }
            }
        }
        Ok(None) => {
            println!("zellij: on PATH but the version cannot be parsed");
            warn += 1;
        }
        Err(err) => {
            println!("zellij: not usable from PATH ({err})");
            warn += 1;
        }
    }

    match hidraw_for(0x6603, 0x1007) {
        Ok(Some(dev)) => {
            print!("hidraw: {} ", dev.display());
            match fs::File::open(&dev) {
                Ok(_) => println!("readable"),
                Err(err) => {
                    println!("NOT readable ({err})");
                    println!("  install 40-opendeck-akp05.rules and reload udev");
                    ok = false;
                }
            }
        }
        Ok(None) => {
            println!("hidraw: no 6603:1007 device found");
            warn += 1;
        }
        Err(err) => {
            println!("hidraw: cannot inspect /sys/class/hidraw: {err}");
            warn += 1;
        }
    }

    let akp05 = find_akp05(&roots);
    match akp05 {
        Some((dir, version)) => {
            let binary = fs::read_dir(&dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("opendeck-akp05")
                });
            println!("AKP05 plugin: {} (version {version})", dir.display());
            if let Some(entry) = binary
                && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified())
            {
                println!("  binary built: {modified:?}");
            }
            match version_check(parse_version(&version), config.strip_mode()) {
                VersionCheck::Ok => {
                    println!("  ok: v0.12.0+ supports the N4E continuous strip")
                }
                VersionCheck::TilesOnly => {
                    println!("  warning: continuous mode needs AKP05 v0.12.0 or newer");
                    println!("  tiles mode (configured) still works with older builds");
                    warn += 1;
                }
                VersionCheck::TooOld => {
                    println!("  FAIL: the continuous strip needs AKP05 v0.12.0 or newer");
                    println!("  tiles mode still works with older builds");
                    ok = false;
                }
                VersionCheck::Unparsed => println!("  warning: cannot parse the AKP05 version"),
            }
            match akp05_full_strip_line() {
                Some(line) => println!("  {line}"),
                None => println!(
                    "  no full-strip line found in the AKP05 log (not connected yet, log rotated, or the plugin predates the line)"
                ),
            }
        }
        None => {
            println!("AKP05 plugin: not found in the OpenDeck plugin directories");
            warn += 1;
        }
    }

    let (lines, raw_warning) = raw_socket_report(
        crate::raw_client::socket_dir().as_deref(),
        config.strip_mode(),
    );
    for line in lines {
        println!("{line}");
    }
    if raw_warning {
        warn += 1;
    }

    if warn > 0 {
        println!("{warn} warning(s)");
    }
    Ok(ok)
}

/// Report lines for the AKP05 raw strip socket, and whether they are a warning.
fn raw_socket_report(dir: Option<&Path>, mode: StripMode) -> (Vec<String>, bool) {
    let sockets = dir.map(crate::raw_client::list_sockets).unwrap_or_default();
    if !sockets.is_empty() {
        let mut lines = vec!["AKP05 raw socket: present".to_owned()];
        lines.extend(sockets.iter().map(|path| format!("  {}", path.display())));
        if mode != StripMode::Raw {
            lines.push("  strip_mode is not raw; it is only used by strip_mode = raw".to_owned());
        }
        return (lines, false);
    }
    if mode == StripMode::Raw {
        return (
            vec![
                "AKP05 raw socket: none found".to_owned(),
                "  strip_mode is raw, so the strip falls back to continuous dial images".to_owned(),
                "  set strip_raw = true in AKP05's config.toml and reconnect the deck".to_owned(),
            ],
            true,
        );
    }
    (
        vec![
            "AKP05 raw socket: none (only needed for strip_mode = raw; enable strip_raw in AKP05)"
                .to_owned(),
        ],
        false,
    )
}

const AKP05_LOG: &str = "com.github.ambiso.opendeck-akp05.sdPlugin.log";

/// `zellij --version` prints `zellij 0.46.0`; pick the first version-shaped token.
/// `Ok(None)` means it ran but printed no version; a failed run is an error.
fn zellij_version() -> std::io::Result<Option<(u64, u64, u64)>> {
    let output = Command::new("zellij")
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    version_output(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
    )
}

fn version_output(code: Option<i32>, stdout: &str) -> std::io::Result<Option<(u64, u64, u64)>> {
    if code != Some(0) {
        return Err(std::io::Error::other(format!(
            "`zellij --version` exited with {}",
            code.map_or("a signal".to_owned(), |code| code.to_string())
        )));
    }
    Ok(stdout.split_whitespace().find_map(parse_version))
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

#[derive(Debug, PartialEq, Eq)]
enum VersionCheck {
    Ok,
    /// Too old for continuous mode, but `tiles` is configured: warn only.
    TilesOnly,
    TooOld,
    Unparsed,
}

fn version_check(version: Option<(u64, u64, u64)>, mode: StripMode) -> VersionCheck {
    match version {
        Some(v) if v >= (0, 12, 0) => VersionCheck::Ok,
        Some(_) if mode == StripMode::Tiles => VersionCheck::TilesOnly,
        Some(_) => VersionCheck::TooOld,
        None => VersionCheck::Unparsed,
    }
}

fn akp05_full_strip_line() -> Option<String> {
    newest_full_strip_line(&akp05_log_paths())
}

/// The full-strip line from the most recently modified log that has one.
fn newest_full_strip_line(paths: &[PathBuf]) -> Option<String> {
    paths
        .iter()
        .filter_map(|path| {
            let line = full_strip_line_in(path)?;
            let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
            Some((modified, line))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, line)| line)
}

fn akp05_log_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        paths.push(
            PathBuf::from(data)
                .join("opendeck/logs/plugins")
                .join(AKP05_LOG),
        );
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        let home = PathBuf::from(home);
        paths.push(
            home.join(".local/share/opendeck/logs/plugins")
                .join(AKP05_LOG),
        );
        paths.push(
            home.join(".var/app")
                .join(install::FLATPAK_ID)
                .join("data/opendeck/logs/plugins")
                .join(AKP05_LOG),
        );
    }
    paths
}

const TAIL_BYTES: u64 = 1024 * 1024;

fn full_strip_line_in(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .ok()?;
    let mut bytes = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut bytes).ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .rev()
        .find(|line| line.contains("full-width touch strip"))
        .map(|line| line.trim().to_owned())
}

fn find_akp05(roots: &[PathBuf]) -> Option<(PathBuf, String)> {
    for root in roots {
        let dir = root.join("com.github.ambiso.opendeck-akp05.sdPlugin");
        let manifest = dir.join("manifest.json");
        if let Ok(text) = fs::read_to_string(&manifest)
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
        {
            let version = value["Version"].as_str().unwrap_or("?").to_owned();
            return Some((dir, version));
        }
    }
    None
}

#[cfg(unix)]
fn open_deck_status_path() -> Result<Option<(u32, PathBuf)>, String> {
    let entries = fs::read_dir("/proc").map_err(|err| err.to_string())?;
    let mut best: Option<(u8, (u32, PathBuf))> = None;
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(comm) = fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let exe_is_opendeck = fs::read_link(entry.path().join("exe"))
            .ok()
            .and_then(|path| path.file_name().map(|name| name == "opendeck"))
            .unwrap_or(false);
        if !exe_is_opendeck && comm.trim() != "opendeck" {
            continue;
        }
        let environ = match fs::read(entry.path().join("environ")) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => continue,
        };
        let Some(rank) = path_environment_rank(&environ) else {
            continue;
        };
        if best.as_ref().is_none_or(|(best_rank, _)| rank > *best_rank) {
            best = Some((rank, (pid, status_path_from_environ(&environ))));
        }
    }
    Ok(best.map(|(_, candidate)| candidate))
}

#[cfg(unix)]
fn path_environment_rank(environ: &[u8]) -> Option<u8> {
    let mut rank = None;
    for pair in environ.split(|byte| *byte == 0) {
        if pair.starts_with(b"OPENDECK_STATUS_FILE=") {
            return Some(2);
        }
        if pair.starts_with(b"XDG_RUNTIME_DIR=") {
            rank = Some(1);
        } else if pair.starts_with(b"TMPDIR=") && rank.is_none() {
            rank = Some(0);
        }
    }
    rank
}

#[cfg(unix)]
fn status_path_from_environ(environ: &[u8]) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let mut status_file = None;
    let mut runtime = None;
    let mut temp = None;
    for pair in environ.split(|byte| *byte == 0) {
        let Some(eq) = pair.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (key, value) = (&pair[..eq], &pair[eq + 1..]);
        match key {
            b"OPENDECK_STATUS_FILE" if !value.is_empty() => status_file = Some(value),
            b"XDG_RUNTIME_DIR" if !value.is_empty() => runtime = Some(value),
            b"TMPDIR" if !value.is_empty() => temp = Some(value),
            _ => {}
        }
    }
    let to_path = |bytes: &[u8]| PathBuf::from(OsString::from_vec(bytes.to_vec()));
    if let Some(value) = status_file {
        to_path(value)
    } else if let Some(value) = runtime {
        to_path(value).join(status::STATUS_FILE_NAME)
    } else if let Some(value) = temp {
        to_path(value).join(status::STATUS_FILE_NAME)
    } else {
        PathBuf::from("/tmp").join(status::STATUS_FILE_NAME)
    }
}

fn hidraw_for(vendor: u16, product: u16) -> std::io::Result<Option<PathBuf>> {
    let base = Path::new("/sys/class/hidraw");
    let mut candidates = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let uevent = entry.path().join("device/uevent");
        let Ok(text) = fs::read_to_string(&uevent) else {
            continue;
        };
        let wanted = format!("HID_ID=0003:{vendor:08X}:{product:08X}");
        let wanted_lower = wanted.to_ascii_lowercase();
        if text
            .lines()
            .any(|line| line.trim().to_ascii_lowercase() == wanted_lower)
        {
            candidates.push(PathBuf::from("/dev").join(name));
        }
    }
    Ok(candidates.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.12.0"), Some((0, 12, 0)));
        assert_eq!(parse_version("v0.12.1-rc1"), Some((0, 12, 1)));
        assert_eq!(parse_version("0.11.1"), Some((0, 11, 1)));
        assert_eq!(parse_version("nope"), None);
    }

    #[test]
    fn finds_the_last_full_strip_log_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("akp05.log");
        fs::write(
            &path,
            "old\nUsing full-width touch strip via ImageSlot(3), firmware V3.Mbox_N4E.02.013\n\
             Using full-width touch strip via ImageSlot(3), firmware V3.Mbox_N4E.02.014\n",
        )
        .unwrap();
        let line = full_strip_line_in(&path).unwrap();
        assert!(line.ends_with("02.014"), "{line}");
    }

    #[test]
    fn parses_zellij_version_output() {
        assert_eq!(parse_version("zellij 0.46.0"), None);
        let parsed = "zellij 0.46.0".split_whitespace().find_map(parse_version);
        assert_eq!(parsed, Some((0, 46, 0)));
    }

    #[test]
    fn a_failing_zellij_version_is_an_error_not_an_unparsed_version() {
        let err = version_output(Some(1), "").expect_err("non-zero exit");
        assert!(err.to_string().contains("exited with 1"), "{err}");
        assert!(version_output(None, "").is_err(), "killed by a signal");
        assert_eq!(
            version_output(Some(0), "zellij 0.46.0").unwrap(),
            Some((0, 46, 0))
        );
        assert_eq!(version_output(Some(0), "something else").unwrap(), None);
    }

    #[test]
    fn version_check_respects_strip_mode() {
        let old = Some((0, 11, 1));
        assert_eq!(
            version_check(Some((0, 12, 0)), StripMode::Continuous),
            VersionCheck::Ok
        );
        assert_eq!(
            version_check(Some((1, 0, 0)), StripMode::Tiles),
            VersionCheck::Ok
        );
        assert_eq!(
            version_check(old, StripMode::Continuous),
            VersionCheck::TooOld
        );
        assert_eq!(
            version_check(old, StripMode::Tiles),
            VersionCheck::TilesOnly
        );
        assert_eq!(
            version_check(None, StripMode::Tiles),
            VersionCheck::Unparsed
        );
    }

    #[test]
    fn newest_log_wins_over_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.log");
        let fresh = dir.path().join("fresh.log");
        let empty = dir.path().join("empty.log");
        fs::write(
            &stale,
            "Using full-width touch strip via ImageSlot(3), old\n",
        )
        .unwrap();
        fs::write(&empty, "nothing relevant\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&fresh, "Using full-width touch strip via Bgpic, new\n").unwrap();
        // Path order puts the stale log first; the empty and missing ones are skipped.
        let paths = [stale, dir.path().join("missing.log"), fresh, empty];
        let line = newest_full_strip_line(&paths).unwrap();
        assert!(line.ends_with("new"), "{line}");
        assert_eq!(
            newest_full_strip_line(&[dir.path().join("missing.log")]),
            None
        );
    }

    #[test]
    fn finds_line_beyond_the_old_64k_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        let mut text = String::from("Using full-width touch strip via ImageSlot(3), early\n");
        text.push_str(&"x".repeat(200 * 1024));
        text.push('\n');
        fs::write(&path, text).unwrap();
        let line = full_strip_line_in(&path).unwrap();
        assert!(line.ends_with("early"), "{line}");
    }

    #[test]
    fn raw_socket_report_covers_each_case() {
        let dir = tempfile::tempdir().unwrap();
        let (lines, warn) = raw_socket_report(Some(dir.path()), StripMode::Continuous);
        assert!(!warn);
        assert!(lines[0].contains("none"));

        let (lines, warn) = raw_socket_report(Some(dir.path()), StripMode::Raw);
        assert!(warn, "raw mode without a socket is a warning");
        assert!(lines.iter().any(|l| l.contains("strip_raw = true")));

        let _socket = std::os::unix::net::UnixListener::bind(dir.path().join("n4-X.sock")).unwrap();
        let (lines, warn) = raw_socket_report(Some(dir.path()), StripMode::Raw);
        assert!(!warn);
        assert!(lines[0].contains("present"));
        assert!(lines[1].contains("n4-X.sock"));

        let (_, warn) = raw_socket_report(None, StripMode::Raw);
        assert!(warn);
    }
}
