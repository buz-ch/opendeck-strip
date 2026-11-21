use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::{Rgb, RgbImage};
use serde_json::json;

use crate::color::Rgb as Color;
use crate::config::Config;
use crate::encode;
use crate::render::{Detail, Renderer};
use crate::status::{self, SlotValue};

pub struct Options {
    pub start: usize,
    pub step: usize,
    pub every: u64,
    pub max: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            start: 1000,
            step: 500,
            every: 1,
            max: 40000,
        }
    }
}

pub fn run(options: Options) -> Result<()> {
    let status_path = status::status_path();
    let config = Config::load();
    let mode = status::load(&status_path)
        .ok()
        .and_then(|status| status.strip_mode())
        .unwrap_or_else(|| config.strip_mode());
    if mode != crate::status::StripMode::Continuous {
        anyhow::bail!(
            "size-test needs continuous strip mode; run `opendeck-strip set strip_mode continuous` \
             or set strip_mode in the config"
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, stop.clone())?;
    signal_hook::flag::register(signal_hook::consts::SIGTERM, stop.clone())?;

    let renderer = Renderer::load(&config);
    let floor = encode::blank_floor().context("cannot measure the blank strip")?;
    println!("blank floor: {floor} bytes");

    let dir = status::work_dir(&status_path, "opendeck-strip-sizetest")?;
    let mut generation = status::generation();
    let mut last_spec: Option<String> = None;
    let mut target = options.start;
    let step = options.step.max(1);

    let outcome = (|| -> Result<()> {
        while target <= options.max && !stop.load(Ordering::Relaxed) {
            if target < floor {
                println!("{target}: below the blank floor ({floor}), skipping");
            } else {
                let picture = render_target(&renderer, target);
                let (best_slices, best_bytes) = search(target, &picture, floor, &stop)?;
                for (index, slice) in best_slices.iter().enumerate() {
                    let bytes = encode::png_bytes(slice)?;
                    status::write_bytes_atomic(&dir.join(format!("slice{index}.png")), &bytes)?;
                }
                generation += 1;
                let spec = slices_spec(&dir, generation);
                status::update(&status_path, |status| {
                    status.set_raw("strip", json!(&spec));
                })?;
                last_spec = Some(spec);
                if best_bytes <= target {
                    println!(
                        "{target}: {best_bytes} bytes ({:+.1}%)",
                        (best_bytes as f64 - target as f64) / target as f64 * 100.0
                    );
                } else {
                    println!(
                        "{target}: over target at {best_bytes} bytes (label alone is too large)"
                    );
                }
            }
            let _ = std::io::stdout().flush();
            let started_wait = Instant::now();
            let wait = Duration::from_secs(options.every);
            while started_wait.elapsed() < wait {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            target += step;
        }
        Ok(())
    })();

    if let Some(spec) = &last_spec {
        clear_strip(&status_path, spec);
    }
    let _ = std::fs::remove_dir_all(&dir);
    if stop.load(Ordering::Relaxed) {
        println!("stopped; strip cleared");
    } else {
        println!("done; strip cleared");
    }
    outcome
}

fn slices_spec(dir: &Path, generation: u64) -> String {
    format!("@slices:{}#{generation}", dir.display())
}

fn clear_strip(status_path: &Path, spec: &str) {
    let _ = status::update(status_path, |status| {
        if status.strip_text() == Some(spec) {
            status.remove("strip");
        }
    });
}

/// The label is part of the picture and therefore part of the size. It is drawn
/// once per target and then reused for every candidate noise count.
fn render_target(renderer: &Renderer, target: usize) -> RgbImage {
    let value = SlotValue::Styled(crate::status::Styles {
        text: Some(target.to_string()),
        color: Some(Color::WHITE.to_string()),
        bg: Some(Color::BLACK.to_string()),
        size: Some(json!(16)),
        ..Default::default()
    });
    let spec = renderer.spec(&value);
    let label = renderer.render_slot(&spec, 132, 100, Detail::Full);
    let mut picture = RgbImage::from_pixel(800, 100, Rgb([0, 0, 0]));
    image::imageops::replace(&mut picture, &label.images[0], 2, 0);
    picture
}

fn split(picture: &RgbImage) -> Vec<RgbImage> {
    (0..4)
        .map(|index| image::imageops::crop_imm(picture, index * 200, 0, 200, 100).to_image())
        .collect()
}

/// Binary-search the number of noise pixels whose final JPEG lands under `target`.
fn search(
    target: usize,
    label_picture: &RgbImage,
    floor: usize,
    stop: &AtomicBool,
) -> Result<(Vec<RgbImage>, usize)> {
    let (x0, y0, x1, y1) = (136u32, 0u32, 800u32, 100u32);
    let area = ((x1 - x0) * (y1 - y0)) as usize;
    let build = |count: usize| -> Result<(Vec<RgbImage>, usize)> {
        let mut picture = label_picture.clone();
        fill_noise(&mut picture, x0, y0, x1, y1, count);
        let slices = split(&picture);
        let estimate = encode::estimate_strip(&slices)?;
        Ok((slices, estimate.bytes))
    };

    let (_, base) = build(0)?;
    if base > target {
        return Ok((build(0)?.0, base));
    }
    let mut lo = 0usize;
    let mut hi = area;
    let mut best_bytes = base;
    let mut best = (0, base);
    for _ in 0..20 {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if lo > hi {
            break;
        }
        let mid = (lo + hi) / 2;
        let (_, bytes) = build(mid)?;
        if bytes <= target {
            if bytes > best_bytes {
                best = (mid, bytes);
                best_bytes = bytes;
            }
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    let (slices, bytes) = build(best.0)?;
    if bytes > target && floor < target {
        log::debug!("size-test could not reach {target}; closest is {bytes}");
    }
    Ok((slices, bytes))
}

fn fill_noise(picture: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, count: usize) {
    let stride = (x1 - x0) as usize;
    for index in 0..count {
        let x = x0 + (index % stride) as u32;
        let y = y0 + (index / stride) as u32;
        if y >= y1 {
            break;
        }
        let mut state = (index as u64)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add(0x1234_5678_9abc_def0);
        state ^= state >> 33;
        state = state.wrapping_mul(0xff51_afd7_ed55_8ccd);
        state ^= state >> 29;
        let r = (state & 0xff) as u8;
        let g = ((state >> 8) & 0xff) as u8;
        let b = ((state >> 16) & 0xff) as u8;
        picture.put_pixel(x, y, Rgb([r, g, b]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_grows_the_estimate() {
        let renderer = Renderer::embedded();
        let picture = render_target(&renderer, 12000);
        let (_, base) = {
            let slices = split(&picture);
            (
                slices.clone(),
                encode::estimate_strip(&slices).unwrap().bytes,
            )
        };
        let mut noisy = picture.clone();
        fill_noise(&mut noisy, 136, 0, 800, 100, 40_000);
        let bytes = encode::estimate_strip(&split(&noisy)).unwrap().bytes;
        assert!(bytes > base, "{bytes} <= {base}");
    }

    #[test]
    fn search_hits_the_target_range() {
        let renderer = Renderer::embedded();
        let stop = AtomicBool::new(false);
        let target = 12000;
        let picture = render_target(&renderer, target);
        let (_, bytes) = search(target, &picture, 3000, &stop).unwrap();
        assert!(bytes <= target, "{bytes}");
        assert!(bytes + 2000 >= target, "{bytes}");
    }
}
