//! `follow-zellij`: supervise `zellij pipe --name zj-status-export.follow -a view=state` and apply
//! the documents it carries.
//!
//! The pipe stays open by holding the child's stdin open and passing a `start`
//! payload, which is what makes `zellij pipe` invoke the plugin's `pipe()` at all.
//! Closing the input ends the subscriber on the plugin side.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cli::{self, OnIdle, StreamEnd};
use crate::config::Config;
use crate::status;
use crate::writer::StateHandler;

#[derive(Clone, Debug)]
pub struct Options {
    pub session: Option<String>,
    pub zellij: PathBuf,
    pub stall: Duration,
    pub retry: (Duration, Duration),
    pub grace: Duration,
    pub clear_on_exit: bool,
    pub once: bool,
}

/// Parse `--retry-secs MIN,MAX`.
pub fn parse_retry(text: &str) -> Result<(Duration, Duration)> {
    let (min, max) = text
        .split_once(',')
        .with_context(|| format!("retry seconds must be MIN,MAX, got {text:?}"))?;
    let min: u64 = min
        .trim()
        .parse()
        .with_context(|| format!("retry minimum must be whole seconds, got {min:?}"))?;
    let max: u64 = max
        .trim()
        .parse()
        .with_context(|| format!("retry maximum must be whole seconds, got {max:?}"))?;
    if min == 0 {
        bail!("retry minimum must be at least 1 second");
    }
    if max < min {
        bail!("retry maximum must not be below the minimum");
    }
    Ok((Duration::from_secs(min), Duration::from_secs(max)))
}

/// Whether the session `follow-zellij` may connect to is live.
#[derive(Debug, PartialEq, Eq)]
enum Liveness {
    Live(String),
    Gone,
    /// Sessions could not be listed: neither connect nor clear on a guess.
    Unknown,
}

impl Liveness {
    fn is_gone(&self) -> bool {
        *self == Self::Gone
    }
}

pub fn run(options: Options) -> Result<i32> {
    let path = status::status_path();
    let config = Config::load();
    let mut handler = StateHandler::new(&config, path.clone());
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;

    let mut backoff = Backoff::new(options.retry.0, options.retry.1);
    // One stream state for every connection: the idle clock does not restart
    // with a reconnect.
    let mut stream = cli::Stream::new();
    let mut no_session_since: Option<Instant> = None;
    let mut cleared_for_outage = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            finish(&path, &options, &stop, handler.owned())?;
            return Ok(0);
        }

        let session = match resolve_session(&options.zellij, options.session.as_deref()) {
            Liveness::Live(session) => session,
            liveness => {
                if liveness.is_gone() {
                    let since = *no_session_since.get_or_insert_with(Instant::now);
                    if !cleared_for_outage && since.elapsed() >= options.grace {
                        if !handler.owned().is_empty() {
                            log::info!(
                                "no zellij session for {}s; clearing the deck",
                                options.grace.as_secs()
                            );
                            cli::clear_owned(&path, handler.owned())?;
                            handler.cleared();
                        }
                        cleared_for_outage = true;
                    }
                }
                if options.once {
                    log::error!("no live zellij session");
                    return Ok(1);
                }
                let wait = backoff.next();
                log::info!(
                    "no live zellij session; retrying in {:.0}s",
                    wait.as_secs_f64()
                );
                if wait_or_stop(wait, &stop, &path, &options, &mut handler, &mut stream)? {
                    finish(&path, &options, &stop, handler.owned())?;
                    return Ok(0);
                }
                continue;
            }
        };
        no_session_since = None;
        cleared_for_outage = false;

        let mut child = match spawn(&options.zellij, &session) {
            Ok(child) => child,
            Err(err) => {
                if options.once {
                    return Err(err);
                }
                let wait = backoff.next();
                log::warn!(
                    "cannot start zellij: {err:#}; retrying in {:.0}s",
                    wait.as_secs_f64()
                );
                if wait_or_stop(wait, &stop, &path, &options, &mut handler, &mut stream)? {
                    finish(&path, &options, &stop, handler.owned())?;
                    return Ok(0);
                }
                continue;
            }
        };
        log::info!("following zellij session {session}");

        let lines_before = stream.lines;
        handler.reconnected();
        let stdout = child.stdout.take().expect("stdout is piped");
        let end = cli::run_stream(
            stdout,
            &mut handler,
            Some(options.stall),
            OnIdle::Stop,
            Some(&stop),
            &mut stream,
        );
        // Closing the input lets the plugin drop this subscriber cleanly.
        drop(child.stdin.take());
        stop_child(&mut child);

        let end = match end {
            Ok(end) => end,
            Err(err) => {
                log::error!("the stream failed: {err:#}");
                if options.once {
                    return Ok(1);
                }
                // Reconnect after a backoff rather than leaving a stale deck behind.
                let wait = backoff.next();
                log::info!("reconnecting in {:.0}s", wait.as_secs_f64());
                if wait_or_stop(wait, &stop, &path, &options, &mut handler, &mut stream)? {
                    finish(&path, &options, &stop, handler.owned())?;
                    return Ok(0);
                }
                continue;
            }
        };
        // A held state frame is written before a reconnect, never carried over.
        if matches!(end, StreamEnd::Eof | StreamEnd::Stall) {
            handler.flush(Instant::now())?;
        }
        // Only a line proves the connection works; a child that fails at once
        // keeps backing off.
        if stream.lines > lines_before {
            backoff.reset();
        }
        match end {
            // The reason only; the next line says whether there is a reconnect.
            StreamEnd::Eof => log::info!("stream ended"),
            StreamEnd::Stall => log::info!("no line for {}s", options.stall.as_secs()),
            StreamEnd::Stopped => {
                finish(&path, &options, &stop, handler.owned())?;
                return Ok(0);
            }
            StreamEnd::TooOld => return Ok(2),
        }
        if options.once {
            return Ok(if handler.applied() > 0 { 0 } else { 1 });
        }
        let wait = backoff.next();
        log::info!("reconnecting in {:.0}s", wait.as_secs_f64());
        if wait_or_stop(wait, &stop, &path, &options, &mut handler, &mut stream)? {
            finish(&path, &options, &stop, handler.owned())?;
            return Ok(0);
        }
    }
}

fn finish(path: &Path, options: &Options, stop: &AtomicBool, owned: &[String]) -> Result<()> {
    if options.clear_on_exit && stop.load(Ordering::Relaxed) && !owned.is_empty() {
        log::info!("clearing the deck on exit");
        cli::clear_owned(path, owned)?;
    }
    Ok(())
}

fn spawn(zellij: &Path, session: &str) -> Result<Child> {
    let mut child = Command::new(zellij)
        .args([
            "--session",
            session,
            "pipe",
            "--name",
            "zj-status-export.follow",
            "-a",
            "view=state",
            "start",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {}", zellij.display()))?;
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                // zellij reports a missing session, a missing plugin and plugin
                // errors here; debug would hide them at the default level.
                log::warn!("zellij: {line}");
            }
        });
    }
    Ok(child)
}

/// Give the child up to a second to exit after its input closed, then kill it.
fn stop_child(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The session to connect to, and whether it is live. A pinned `--session` is
/// connected to by name but still listed, so a dead one clears the deck after
/// the grace period like an unpinned one.
fn resolve_session(zellij: &Path, explicit: Option<&str>) -> Liveness {
    let live = match live_sessions(zellij) {
        Ok(live) => live,
        Err(err) => {
            log::warn!("cannot list zellij sessions: {err:#}");
            return match explicit {
                Some(name) => Liveness::Live(name.to_owned()),
                None => Liveness::Unknown,
            };
        }
    };
    let is_live = |name: &str| live.iter().any(|session| session == name);
    if let Some(name) = explicit {
        return if is_live(name) {
            Liveness::Live(name.to_owned())
        } else {
            log::info!("zellij session {name} is not live");
            Liveness::Gone
        };
    }
    if let Some(name) = std::env::var_os("ZELLIJ_SESSION_NAME") {
        let name = name.to_string_lossy();
        if is_live(&name) {
            return Liveness::Live(name.into_owned());
        }
    }
    live.into_iter()
        .next()
        .map_or(Liveness::Gone, Liveness::Live)
}

pub(crate) fn live_sessions(zellij: &Path) -> Result<Vec<String>> {
    let output = Command::new(zellij)
        .args(["list-sessions", "--no-formatting"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run {}", zellij.display()))?;
    if !output.status.success() {
        // Zellij exits 1 and writes "No active zellij sessions found." to stderr.
        return Ok(Vec::new());
    }
    Ok(parse_live_sessions(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// One session per line, oldest first. Lines with `(EXITED - attach to resurrect)`
/// are dead; `--short` would also list them, so we never use it.
pub(crate) fn parse_live_sessions(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains("(EXITED"))
        .filter_map(|line| {
            let name = line.split_once(" [").map_or(line, |(name, _)| name).trim();
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect()
}

/// Sleep for `wait` or until `stop` is set; true when stopped. The owned slots
/// are cleared if the stall clock runs out meanwhile, so a hung producer does
/// not keep the deck until the next connection attempt.
fn wait_or_stop(
    wait: Duration,
    stop: &AtomicBool,
    path: &Path,
    options: &Options,
    handler: &mut StateHandler,
    stream: &mut cli::Stream,
) -> Result<bool> {
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if stop.load(Ordering::Relaxed) {
            return Ok(true);
        }
        if stream.clear_if_idle(path, options.stall, handler.owned())? {
            handler.cleared();
        }
        let left = deadline.saturating_duration_since(Instant::now());
        thread::sleep(Duration::from_millis(50).min(left));
    }
    Ok(stop.load(Ordering::Relaxed))
}

struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    fn next(&mut self) -> Duration {
        let wait = self.next;
        self.next = (self.next * 2).min(self.max);
        wait
    }

    fn reset(&mut self) {
        self.next = self.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_seconds_parse_min_and_max() {
        assert_eq!(
            parse_retry("1,10").unwrap(),
            (Duration::from_secs(1), Duration::from_secs(10))
        );
        assert_eq!(
            parse_retry(" 2 , 3 ").unwrap(),
            (Duration::from_secs(2), Duration::from_secs(3))
        );
        assert!(parse_retry("10").is_err());
        assert!(parse_retry("0,10").is_err());
        assert!(parse_retry("5,1").is_err());
        assert!(parse_retry("a,b").is_err());
    }

    #[test]
    fn live_sessions_skip_exited_and_use_the_name() {
        let text = "old [Created 2m 10s ago] (EXITED - attach to resurrect)\n\
                    work [Created 3m 10s ago]\n\
                    play [Created 1s ago]\n";
        assert_eq!(parse_live_sessions(text), ["work", "play"]);
        assert!(parse_live_sessions("").is_empty());
    }

    #[test]
    fn backoff_doubles_to_the_maximum_and_resets() {
        let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(5));
        let waits: Vec<u64> = (0..5).map(|_| backoff.next().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 5, 5]);
        backoff.reset();
        assert_eq!(backoff.next(), Duration::from_secs(1));
    }
}
