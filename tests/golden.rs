use std::path::PathBuf;

use image::RgbImage;
use opendeck_strip::render::{
    Detail, Renderer, continuous_preview, raw_frame, styled_value, tiles_preview,
};
use opendeck_strip::status::{SlotValue, StripMode};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn compare(name: &str, actual: &RgbImage) {
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        actual.save(&path).unwrap();
        return;
    }
    let golden = image::open(&path)
        .unwrap_or_else(|err| {
            panic!(
                "missing {}: {err}; run UPDATE_GOLDEN=1 cargo test",
                path.display()
            )
        })
        .to_rgb8();
    assert_eq!(golden.dimensions(), actual.dimensions(), "{name}");
    let total = (golden.width() * golden.height()) as f64;
    let mut differing = 0usize;
    let mut sum = 0u64;
    for (a, b) in golden.pixels().zip(actual.pixels()) {
        let diff = (0..3)
            .map(|i| a.0[i].abs_diff(b.0[i]) as u32)
            .max()
            .unwrap();
        if diff > 16 {
            differing += 1;
        }
        sum += diff as u64;
    }
    let mean = sum as f64 / (total * 3.0);
    assert!(
        differing as f64 / total < 0.005,
        "{name}: {differing}/{total} pixels differ"
    );
    assert!(mean < 2.0, "{name}: mean channel difference {mean}");
}

fn strip(name: &str, value: &SlotValue, mode: StripMode, gap: u32) {
    let renderer = Renderer::embedded();
    let rendered = renderer.render_strip(value, mode, gap, Detail::Full);
    let preview = match mode {
        StripMode::Continuous => continuous_preview(&rendered.images),
        StripMode::Tiles => tiles_preview(&rendered.images, gap, true),
        StripMode::Raw => raw_frame(&rendered.images),
    };
    compare(name, &preview);
}

fn dial(name: &str, value: &SlotValue) {
    let renderer = Renderer::embedded();
    let spec = renderer.spec(value);
    let rendered = renderer.render_slot(&spec, 200, 100, Detail::Full);
    compare(name, &rendered.images[0]);
}

#[test]
fn golden_strip_plain() {
    strip(
        "strip_plain",
        &SlotValue::from_text("READY"),
        StripMode::Continuous,
        28,
    );
}

#[test]
fn golden_strip_mixed_formatting() {
    strip(
        "strip_mixed",
        &SlotValue::from_text(
            "[b]BUILD[/b] [c=green]OK[/c] [s=1.4]42[/s]% [u]link[/u] [bg=#202020]bg[/bg]",
        ),
        StripMode::Continuous,
        28,
    );
}

#[test]
fn golden_strip_non_latin() {
    strip(
        "strip_nonlatin",
        &SlotValue::from_text("日本語 — naïve café"),
        StripMode::Continuous,
        28,
    );
}

#[test]
fn golden_strip_tiles() {
    strip(
        "strip_tiles",
        &SlotValue::from_text("CPU|MEM|NET|DISK"),
        StripMode::Tiles,
        28,
    );
}

#[test]
fn golden_dial_multiline() {
    dial("dial_multiline", &SlotValue::from_text("INBOX\n3"));
}

#[test]
fn golden_dial_style_and_bar() {
    let value: SlotValue = serde_json::from_str(
        r##"{"text":"DISK","bg":"#101820","color":"#55ccff","bar":0.72,"size":34}"##,
    )
    .unwrap();
    dial("dial_bar", &value);
}

#[test]
fn golden_test_pattern() {
    let renderer = Renderer::embedded();
    let rendered = renderer.test_pattern(StripMode::Continuous, 28, Detail::Full);
    compare("test_pattern", &continuous_preview(&rendered.images));
}

#[test]
fn styled_helper_is_renderable() {
    let renderer = Renderer::embedded();
    let value = styled_value("X", None, None);
    let spec = renderer.spec(&value);
    assert_eq!(spec.bg, opendeck_strip::color::Rgb::BLACK);
}

#[test]
fn golden_strip_raw() {
    strip(
        "strip_raw",
        &SlotValue::from_text("RAW|[c=green]OK|[b]42[/b]%|[bg=#202020]bg[/bg]"),
        StripMode::Raw,
        28,
    );
}

#[test]
fn golden_strip_grid() {
    // The zjsb `lines` output for the `busy` scenario: five rows, four cells.
    let busy = concat!(
        "[c=orange]● 2[/c]\t[b]claude[/b]\t[c=orange]needs login[/c]\tlogin expired\n",
        "[c=red]✗ 1[/c]\t[b]codex[/b]\t[c=orange]needs approval[/c]\tapply patch\n",
        "[c=green]✓ 1[/c]\t[b]opencode[/b]\t[c=red]failed[/c]\trate limited\n",
        "[c=#55ccff]» 2[/c]\t[b]claude[/b]\t[c=green]done[/c]\t\n",
        "\t[b]+2 more[/b]\t[c=#55ccff]working[/c]\tsee inbox",
    );
    strip(
        "strip_grid",
        &SlotValue::from_text(busy),
        StripMode::Raw,
        28,
    );
}

#[test]
fn golden_raw_test_pattern() {
    let renderer = Renderer::embedded();
    compare("raw_test_pattern", &renderer.raw_test_pattern());
}
