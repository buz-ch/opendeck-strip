use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use crate::config::Config;
use crate::encode;
use crate::render::{Detail, Renderer};
use crate::status::{self, StripMode};

/// Show the seam test pattern through the plugin, then clear the strip.
pub fn run(seconds: Option<u64>) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;

    let status_path = status::status_path();
    let config = Config::load();
    let current = status::load(&status_path).ok();
    let mode = current
        .as_ref()
        .and_then(|status| status.strip_mode())
        .unwrap_or_else(|| config.strip_mode());
    let gap = current
        .as_ref()
        .and_then(|status| status.strip_gap())
        .unwrap_or(config.strip_gap)
        .min(32);
    let mut rendered = Renderer::load(&config).test_pattern(mode, gap, Detail::Full);
    if mode == StripMode::Raw {
        // The plugin takes raw frames as four 200x128 slices.
        rendered.images = crate::render::raw_slices(&rendered.images[0]);
    }
    let dir = status::work_dir(&status_path, "opendeck-strip-test")?;
    let spec = format!("@slices:{}#{}", dir.display(), status::generation());

    let outcome = show(
        &status_path,
        &dir,
        &spec,
        &rendered.images,
        mode,
        seconds,
        &stop,
    );
    let mut removed = false;
    let cleared = status::update(&status_path, |status| {
        if status.strip_text() == Some(spec.as_str()) {
            status.remove("strip");
            removed = true;
        }
    })
    .is_ok();
    let _ = std::fs::remove_dir_all(&dir);
    if cleared && removed {
        println!("strip cleared");
    }
    outcome
}

fn show(
    status_path: &Path,
    dir: &Path,
    spec: &str,
    images: &[image::RgbImage],
    mode: StripMode,
    seconds: Option<u64>,
    stop: &AtomicBool,
) -> Result<()> {
    for (index, slice) in images.iter().enumerate() {
        let bytes = encode::png_bytes(slice)?;
        status::write_bytes_atomic(&dir.join(format!("slice{index}.png")), &bytes)?;
    }
    status::update(status_path, |status| {
        status.set_raw("strip", json!(spec));
    })?;

    match seconds {
        Some(0) => {}
        Some(seconds) => {
            println!("{mode} test pattern on the strip for {seconds}s; Ctrl-C clears it");
            wait(stop, Some(Duration::from_secs(seconds)));
        }
        None => {
            println!("{mode} test pattern on the strip; Ctrl-C to clear");
            wait(stop, None);
        }
    }
    Ok(())
}

pub fn wait(stop: &AtomicBool, duration: Option<Duration>) {
    let started = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if duration.is_some_and(|limit| started.elapsed() >= limit) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
