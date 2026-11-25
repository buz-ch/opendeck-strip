//! Adversarial behaviour tests for the renderer. They assert observable
//! contracts (dimensions, no panic, bounded ink, completion) rather than glyph
//! shapes.

use std::time::Instant;

use image::RgbImage;
use opendeck_strip::color::Rgb;
use opendeck_strip::render::markup::escape_markup;
use opendeck_strip::render::{Detail, Rendered, Renderer};
use opendeck_strip::status::{SlotValue, SpanSpec, StripMode, Styles};

const DEFAULT_BG: Rgb = Rgb::BLACK;

fn renderer() -> Renderer {
    Renderer::embedded()
}

fn render_value(renderer: &Renderer, value: &SlotValue, detail: Detail) -> Rendered {
    renderer.render_slot(&renderer.spec(value), 200, 100, detail)
}

fn render_text(renderer: &Renderer, text: &str) -> Rendered {
    render_value(renderer, &SlotValue::from_text(text), Detail::Full)
}

fn ink(image: &RgbImage, bg: Rgb) -> usize {
    image.pixels().filter(|p| p.0 != [bg.0, bg.1, bg.2]).count()
}

fn count_where(image: &RgbImage, f: impl Fn([u8; 3]) -> bool) -> usize {
    image.pixels().filter(|p| f(p.0)).count()
}

fn styled(json: &str) -> SlotValue {
    serde_json::from_str(json).unwrap()
}

fn assert_full_dial(rendered: &Rendered) {
    assert_eq!(rendered.images.len(), 1);
    assert_eq!(rendered.images[0].dimensions(), (200, 100));
}

#[test]
fn empty_and_whitespace_text_draws_a_full_dial() {
    let renderer = renderer();
    let cases = ["", " ", "     ", "\n", "\n\n\n", "\t\r\n"];
    for text in cases {
        let rendered = render_text(&renderer, text);
        assert_full_dial(&rendered);
    }
}

#[test]
fn forty_newlines_in_a_dial_does_not_panic() {
    let renderer = renderer();
    let rendered = render_text(&renderer, &"\n".repeat(40));
    assert_full_dial(&rendered);
}

#[test]
fn very_long_single_word_truncates_quickly() {
    let renderer = renderer();
    let word = "W".repeat(10_000);
    let start = Instant::now();
    let rendered = render_text(&renderer, &word);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f32() < 5.0, "took {elapsed:?}");
    assert_full_dial(&rendered);
    assert!(rendered.truncated, "oversized word must report truncation");
    // Clipping keeps the (little) ink that is drawn inside the image.
    assert!(ink(&rendered.images[0], DEFAULT_BG) > 0);
}

#[test]
fn very_long_markup_run_truncates_quickly() {
    let renderer = renderer();
    let mut text = String::new();
    for _ in 0..5_000 {
        text.push_str("[b]x[/b] ");
    }
    let start = Instant::now();
    let rendered = render_text(&renderer, &text);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs_f32() < 5.0, "took {elapsed:?}");
    assert_full_dial(&rendered);
    assert!(rendered.truncated, "oversized run must report truncation");
}

#[test]
fn explicit_sizes_are_safe() {
    let renderer = renderer();
    // Zero, negative and a non-numeric NaN fall back to auto and still draw.
    for json in [
        r#"{"text":"A","size":0}"#,
        r#"{"text":"A","size":-5}"#,
        r#"{"text":"A","size":"NaN"}"#,
    ] {
        let value = styled(json);
        let rendered = render_value(&renderer, &value, Detail::Full);
        assert_full_dial(&rendered);
        assert!(ink(&rendered.images[0], DEFAULT_BG) > 0, "{json}");
    }
    // A huge explicit size must neither panic nor hang.
    let start = Instant::now();
    let rendered = render_value(
        &renderer,
        &styled(r#"{"text":"A","size":1000000}"#),
        Detail::Full,
    );
    assert!(start.elapsed().as_secs_f32() < 5.0);
    assert_full_dial(&rendered);
}

#[test]
fn bar_out_of_range_is_clamped() {
    let renderer = renderer();
    let under = render_value(&renderer, &styled(r#"{"text":"","bar":-1}"#), Detail::Full);
    let over = render_value(&renderer, &styled(r#"{"text":"","bar":2}"#), Detail::Full);
    assert_full_dial(&under);
    assert_full_dial(&over);
    // -1 clamps to an empty bar: only the trough colour is visible.
    assert_eq!(under.images[0].get_pixel(100, 90).0, [32, 40, 48]);
    // 2 clamps to a full bar: the fill colour reaches the middle.
    assert_eq!(over.images[0].get_pixel(100, 90).0, [85, 204, 255]);
}

#[test]
fn bar_nan_from_a_direct_styles_value_does_not_panic() {
    let renderer = renderer();
    let value = SlotValue::Styled(Styles {
        text: Some("X".into()),
        bar: Some(f32::NAN),
        ..Default::default()
    });
    let rendered = render_value(&renderer, &value, Detail::Full);
    assert_full_dial(&rendered);
}

#[test]
fn malformed_markup_does_not_panic() {
    let renderer = renderer();
    for text in [
        "[b]A|B[/b]",
        "unmatched [b] tag",
        "unmatched [ bracket",
        "]]",
        "[[",
        "[[[",
        "[[b]literal",
        "[b][b]nested[/b][/b]",
        "[b]x[/i]",
        "[wat]unknown[/wat]",
        "[c=#zzz]bad[/c]",
        "[bg=]bad[/bg]",
        "[s=abc]bad[/s]",
        "[s=100]big[/s]",
        "[/b]",
        "[",
        "]",
    ] {
        let rendered = render_text(&renderer, text);
        assert_full_dial(&rendered);
    }
}

#[test]
fn invalid_tags_render_literally() {
    let renderer = renderer();
    // An unknown or malformed tag is emitted as literal text, so the image is
    // pixel-identical to the explicitly escaped form.
    for text in [
        "A[c=#zzz]B",
        "A[bg=]B",
        "A[s=abc]B",
        "A[wat]B",
        "A[B",
        "A[/b]B",
    ] {
        let plain = render_text(&renderer, text);
        let escaped = render_text(&renderer, &escape_markup(text));
        assert_eq!(plain.images[0].dimensions(), escaped.images[0].dimensions());
        assert_eq!(
            plain.images[0].as_raw(),
            escaped.images[0].as_raw(),
            "{text} did not render literally"
        );
    }
}

#[test]
fn split_segments_keep_their_own_styles() {
    let renderer = renderer();
    let rendered = renderer.render_strip(
        &SlotValue::from_text("[c=red]A|[c=blue]B"),
        StripMode::Continuous,
        28,
        Detail::Full,
    );
    assert_eq!(rendered.images.len(), 4);
    for image in &rendered.images {
        assert_eq!(image.dimensions(), (200, 100));
    }
    // Red ink only in the first segment, blue only in the second.
    assert!(count_where(&rendered.images[0], |p| p[0] > 200 && p[1] < 130) > 0);
    assert!(count_where(&rendered.images[1], |p| p[2] > 200) > 0);
    assert_eq!(
        count_where(&rendered.images[1], |p| p[0] > 200 && p[1] < 130),
        0
    );
}

#[test]
fn tags_do_not_cross_a_segment_boundary() {
    let renderer = renderer();
    let rendered = renderer.render_strip(
        &SlotValue::from_text("[b]A|B[/b]"),
        StripMode::Continuous,
        28,
        Detail::Full,
    );
    assert_eq!(rendered.images.len(), 4);
    for image in &rendered.images {
        assert_eq!(image.dimensions(), (200, 100));
        // Ink stays inside its own tile, not clipped into a neighbour.
        for (x, _, pixel) in image.enumerate_pixels() {
            if x == 0 || x == 199 {
                assert_eq!(pixel.0, [0, 0, 0]);
            }
        }
    }
    assert!(ink(&rendered.images[0], DEFAULT_BG) > 0);
    assert!(ink(&rendered.images[1], DEFAULT_BG) > 0);
}

#[test]
fn span_variants_do_not_panic() {
    let renderer = renderer();
    let empty_spans = SlotValue::Styled(Styles {
        text: Some("fallback".into()),
        spans: Some(Vec::new()),
        ..Default::default()
    });
    assert!(
        ink(
            &render_value(&renderer, &empty_spans, Detail::Full).images[0],
            DEFAULT_BG
        ) > 0
    );

    let spans = SlotValue::Styled(Styles {
        text: Some("ignored".into()),
        spans: Some(vec![
            SpanSpec {
                text: String::new(),
                ..Default::default()
            },
            SpanSpec {
                text: "one\ntwo\n\nthree".into(),
                color: Some("not-a-color".into()),
                bg: Some("#gg0000".into()),
                size: Some(100.0),
                ..Default::default()
            },
            SpanSpec {
                text: "clamped".into(),
                size: Some(-5.0),
                ..Default::default()
            },
            SpanSpec {
                text: "nan".into(),
                size: Some(f32::NAN),
                ..Default::default()
            },
        ]),
        ..Default::default()
    });
    let rendered = render_value(&renderer, &spans, Detail::Full);
    assert_full_dial(&rendered);
    assert!(ink(&rendered.images[0], DEFAULT_BG) > 0);
}

#[test]
fn split_span_across_newlines_renders() {
    let renderer = renderer();
    let spans = SlotValue::Styled(Styles {
        text: None,
        spans: Some(vec![SpanSpec {
            text: "\n\na\n\nb\n".into(),
            ..Default::default()
        }]),
        ..Default::default()
    });
    let rendered = render_value(&renderer, &spans, Detail::Full);
    assert_full_dial(&rendered);
}

#[test]
fn control_characters_and_non_latin_do_not_panic() {
    let renderer = renderer();
    // `\r` is dropped and `\t` becomes spaces, so they are whitespace: only
    // "no panic" is checked.
    for text in ["\r", "\r\r", "\t", " \t "] {
        assert_full_dial(&render_text(&renderer, text));
    }
    for text in ["\0", "\u{7f}", "\0\r\t\u{7f}", "日本語", "😀", "e\u{0301}"] {
        let rendered = render_text(&renderer, text);
        assert_full_dial(&rendered);
        assert!(
            ink(&rendered.images[0], DEFAULT_BG) > 0,
            "{text:?} produced no ink"
        );
    }
}

#[test]
fn continuous_and_tiles_have_expected_geometry() {
    let renderer = renderer();
    let value = SlotValue::from_text("A|B|C|D");
    let continuous = renderer.render_strip(&value, StripMode::Continuous, 28, Detail::Full);
    assert_eq!(continuous.images.len(), 4);
    for image in &continuous.images {
        assert_eq!(image.dimensions(), (200, 100));
        assert!(ink(image, DEFAULT_BG) > 0);
    }
    let tiles = renderer.render_strip(&value, StripMode::Tiles, 28, Detail::Full);
    assert_eq!(tiles.images.len(), 4);
    for image in &tiles.images {
        assert_eq!(image.dimensions(), (176, 88));
        assert!(ink(image, DEFAULT_BG) > 0);
    }
}

#[test]
fn blank_segments_stay_black() {
    let renderer = renderer();
    let value = SlotValue::from_text("A|||");
    for mode in [StripMode::Continuous, StripMode::Tiles] {
        let rendered = renderer.render_strip(&value, mode, 28, Detail::Full);
        assert_eq!(rendered.images.len(), 4);
        assert!(ink(&rendered.images[0], DEFAULT_BG) > 0);
        for image in &rendered.images[1..] {
            assert!(
                image.pixels().all(|p| p.0 == [0, 0, 0]),
                "{mode:?} blank segment is not black"
            );
        }
    }
}

#[test]
fn tiny_boxes_do_not_panic() {
    let renderer = renderer();
    let spec = renderer.spec(&SlotValue::from_text("WW"));
    for (width, height) in [(1u32, 1u32), (2, 1)] {
        let rendered = renderer.render_slot(&spec, width, height, Detail::Full);
        assert_eq!(rendered.images.len(), 1);
        assert_eq!(rendered.images[0].dimensions(), (width, height));
    }
}

#[test]
fn zero_sized_box_is_reported_not_panicking() {
    let renderer = renderer();
    let spec = renderer.spec(&SlotValue::from_text("A"));
    let rendered = renderer.render_slot(&spec, 0, 0, Detail::Full);
    assert_eq!(rendered.images.len(), 1);
    assert_eq!(rendered.images[0].dimensions(), (0, 0));
}

#[test]
fn text_only_drops_bar_and_run_backgrounds() {
    let renderer = renderer();
    let value = styled(r##"{"text":"[bg=red]X[/bg]","bar":0.4}"##);
    let full = render_value(&renderer, &value, Detail::Full);
    let text_only = render_value(&renderer, &value, Detail::TextOnly);
    assert_full_dial(&full);
    assert_full_dial(&text_only);

    let red = |p: [u8; 3]| p == [255, 80, 80];
    assert!(
        count_where(&full.images[0], red) > 0,
        "Full lost the run bg"
    );
    assert_eq!(
        count_where(&text_only.images[0], red),
        0,
        "TextOnly kept the run bg"
    );

    // The bar trough appears in Full but not in TextOnly.
    assert_eq!(full.images[0].get_pixel(100, 90).0, [32, 40, 48]);
    assert_eq!(text_only.images[0].get_pixel(100, 90).0, [0, 0, 0]);
}

#[test]
fn object_colors_change_pixels() {
    let renderer = renderer();
    let rendered = render_value(
        &renderer,
        &styled(r##"{"text":"X","bg":"#101820","color":"#ff8800"}"##),
        Detail::Full,
    );
    assert_full_dial(&rendered);
    // Background reaches every corner.
    assert_eq!(rendered.images[0].get_pixel(0, 0).0, [16, 24, 32]);
    assert_eq!(rendered.images[0].get_pixel(199, 99).0, [16, 24, 32]);
    // The glyph is drawn in the requested orange.
    let orange = count_where(&rendered.images[0], |p| {
        p[0] > 200 && p[1] > 90 && p[1] < 170 && p[2] < 60
    });
    assert!(orange > 0, "text colour was not applied");
}

#[test]
fn raw_mode_renders_one_full_height_frame() {
    use opendeck_strip::color::Rgb;
    let renderer = renderer();
    let value = SlotValue::Styled(opendeck_strip::status::Styles {
        text: Some("A|B|C|D".to_owned()),
        bg: Some("#102030".to_owned()),
        ..Default::default()
    });
    let rendered = renderer.render_strip(&value, StripMode::Raw, 28, Detail::Full);
    assert_eq!(rendered.images.len(), 1);
    let frame = &rendered.images[0];
    assert_eq!(frame.dimensions(), (800, 128));
    // The background reaches the first and last rows: no letterbox bands.
    let bg = Rgb::parse("#102030").unwrap();
    for (x, y) in [(5, 0), (400, 0), (795, 0), (5, 127), (400, 127), (795, 127)] {
        assert_eq!(frame.get_pixel(x, y).0, [bg.0, bg.1, bg.2], "({x},{y})");
    }
}

fn raw_pixel(frame: &image::RgbImage, x: u32, y: u32) -> [u8; 3] {
    frame.get_pixel(x, y).0
}

#[test]
fn raw_test_pattern_is_a_coloured_full_frame() {
    use opendeck_strip::color::Rgb;
    use opendeck_strip::render::{
        RAW_BORDER, RAW_MIDDLE_COLUMN, RAW_MIDDLE_ROW, RAW_QUADRANTS, RAW_SEAM,
    };
    let renderer = renderer();
    let frame = renderer.raw_test_pattern();
    assert_eq!(frame.dimensions(), (800, 128));
    let rgb = |c: Rgb| [c.0, c.1, c.2];

    // Nothing is black, in particular not the old 14 px bands.
    assert!(
        frame.pixels().all(|p| p.0 != [0, 0, 0]),
        "pattern has black pixels"
    );

    // One-pixel border on all four edges.
    for x in 0..800 {
        assert_eq!(raw_pixel(&frame, x, 0), rgb(RAW_BORDER), "top x={x}");
        assert_eq!(raw_pixel(&frame, x, 127), rgb(RAW_BORDER), "bottom x={x}");
    }
    for y in 0..128 {
        assert_eq!(raw_pixel(&frame, 0, y), rgb(RAW_BORDER), "left y={y}");
        assert_eq!(raw_pixel(&frame, 799, y), rgb(RAW_BORDER), "right y={y}");
    }

    // Each quadrant has its own non-black background, clear of text.
    for (index, color) in RAW_QUADRANTS.iter().enumerate() {
        let x = index as u32 * 200 + 10;
        assert_eq!(raw_pixel(&frame, x, 100), rgb(*color), "quadrant {index}");
        assert_ne!(rgb(*color), [0, 0, 0]);
    }
    let distinct: std::collections::HashSet<_> = RAW_QUADRANTS.iter().collect();
    assert_eq!(distinct.len(), 4, "quadrant colours must differ");

    // Magenta middle row and yellow middle column.
    for x in [5, 790] {
        for y in [63, 64] {
            assert_eq!(
                raw_pixel(&frame, x, y),
                rgb(RAW_MIDDLE_ROW),
                "row ({x},{y})"
            );
        }
    }
    for x in [399, 400] {
        for y in [2, 125] {
            assert_eq!(
                raw_pixel(&frame, x, y),
                rgb(RAW_MIDDLE_COLUMN),
                "column ({x},{y})"
            );
        }
    }

    // Seam ticks at x = 200 and 600.
    for x in [200, 600] {
        for y in [5, 122] {
            assert_eq!(raw_pixel(&frame, x, y), rgb(RAW_SEAM), "seam ({x},{y})");
        }
    }

    // Upright text leaves light ink in the title and in each quadrant label.
    let light = |p: &image::Rgb<u8>| p.0.iter().all(|v| *v > 200);
    let title_ink = frame
        .enumerate_pixels()
        .filter(|(x, y, p)| (150..650).contains(x) && (12..58).contains(y) && light(p))
        .count();
    assert!(title_ink > 100, "title has no ink: {title_ink}");
    for index in 0..4u32 {
        let ink = frame
            .enumerate_pixels()
            .filter(|(x, y, p)| {
                (index * 200 + 50..index * 200 + 150).contains(x)
                    && (70..118).contains(y)
                    && light(p)
            })
            .count();
        assert!(ink > 30, "label {} has no ink: {ink}", index + 1);
    }
}

#[test]
fn raw_slices_roundtrip_through_raw_frame() {
    use opendeck_strip::render::{raw_frame, raw_slices};
    let frame = renderer().raw_test_pattern();
    let slices = raw_slices(&frame);
    assert_eq!(slices.len(), 4);
    for slice in &slices {
        assert_eq!(slice.dimensions(), (200, 128));
    }
    assert_eq!(raw_frame(&slices), frame);
    assert_eq!(raw_frame(std::slice::from_ref(&frame)), frame);
}

// Tab columns on the full-width strip. Positions are read from coloured
// backgrounds and glyphs, never from glyph shapes.

const GRID_TEXT: &str = "[bg=#ff0000]A[/bg]\t[bg=#00ff00]B[/bg]\n\
                         [bg=#0000ff]CCCC[/bg]\t[bg=#ffff00]B[/bg]";

fn raw_image(renderer: &Renderer, value: &SlotValue) -> RgbImage {
    renderer
        .render_strip(value, StripMode::Raw, 0, Detail::Full)
        .images
        .remove(0)
}

fn x_span(image: &RgbImage, color: impl Fn([u8; 3]) -> bool) -> (u32, u32) {
    let mut min = u32::MAX;
    let mut max = 0;
    for (x, _, pixel) in image.enumerate_pixels() {
        if color(pixel.0) {
            min = min.min(x);
            max = max.max(x);
        }
    }
    assert!(min != u32::MAX, "colour not found");
    (min, max)
}

fn y_span(image: &RgbImage, color: impl Fn([u8; 3]) -> bool) -> (u32, u32) {
    let mut min = u32::MAX;
    let mut max = 0;
    for (_, y, pixel) in image.enumerate_pixels() {
        if color(pixel.0) {
            min = min.min(y);
            max = max.max(y);
        }
    }
    assert!(min != u32::MAX, "colour not found");
    (min, max)
}

fn is_red(p: [u8; 3]) -> bool {
    p[0] > 150 && p[1] < 60 && p[2] < 60
}

fn is_green(p: [u8; 3]) -> bool {
    p[1] > 150 && p[0] < 60 && p[2] < 60
}

fn is_blue(p: [u8; 3]) -> bool {
    p[2] > 150 && p[0] < 60 && p[1] < 60
}

fn is_yellow(p: [u8; 3]) -> bool {
    p[0] > 150 && p[1] > 150 && p[2] < 60
}

#[test]
fn tabs_in_a_dial_render_as_four_spaces() {
    let renderer = renderer();
    let with_tab = render_value(&renderer, &SlotValue::from_text("a\tb"), Detail::Full);
    let with_spaces = render_value(&renderer, &SlotValue::from_text("a    b"), Detail::Full);
    assert_eq!(with_tab.images, with_spaces.images);
}

#[test]
fn tabs_in_a_segmented_strip_render_as_four_spaces() {
    let renderer = renderer();
    let with_tab = renderer.render_strip(
        &SlotValue::from_text("a\tb|c"),
        StripMode::Raw,
        28,
        Detail::Full,
    );
    let with_spaces = renderer.render_strip(
        &SlotValue::from_text("a    b|c"),
        StripMode::Raw,
        28,
        Detail::Full,
    );
    assert_eq!(with_tab.images, with_spaces.images);
}

#[test]
fn tabs_in_tiles_render_as_four_spaces() {
    let renderer = renderer();
    let tabbed = SlotValue::from_text("ab\tcd\nef\tgh");
    let spaced = SlotValue::from_text("ab    cd\nef    gh");
    let with_tab = renderer.render_strip(&tabbed, StripMode::Tiles, 28, Detail::Full);
    let with_spaces = renderer.render_strip(&spaced, StripMode::Tiles, 28, Detail::Full);
    assert_eq!(with_tab.images, with_spaces.images);
}

#[test]
fn tabs_with_word_wrap_render_as_four_spaces() {
    let mut renderer = renderer();
    renderer.config.word_wrap = true;
    let with_tab = renderer.render_strip(
        &SlotValue::from_text("a\tb"),
        StripMode::Raw,
        0,
        Detail::Full,
    );
    let with_spaces = renderer.render_strip(
        &SlotValue::from_text("a    b"),
        StripMode::Raw,
        0,
        Detail::Full,
    );
    assert_eq!(with_tab.images, with_spaces.images);
}

#[test]
fn tabs_in_tiles_with_word_wrap_render_as_four_spaces() {
    let mut renderer = renderer();
    renderer.config.word_wrap = true;
    let tabbed = SlotValue::from_text("ab\tcd\nef\tgh");
    let spaced = SlotValue::from_text("ab    cd\nef    gh");
    let with_tab = renderer.render_strip(&tabbed, StripMode::Tiles, 28, Detail::Full);
    let with_spaces = renderer.render_strip(&spaced, StripMode::Tiles, 28, Detail::Full);
    assert_eq!(with_tab.images, with_spaces.images);
}

#[test]
fn a_column_starts_at_the_same_x_in_rows_with_different_first_cells() {
    let frame = raw_image(&renderer(), &SlotValue::from_text(GRID_TEXT));
    let (red_start, red_end) = x_span(&frame, is_red);
    let (green_start, _) = x_span(&frame, is_green);
    let (blue_start, _) = x_span(&frame, is_blue);
    let (yellow_start, _) = x_span(&frame, is_yellow);
    assert_eq!(green_start, yellow_start, "column 2 is not aligned");
    assert_eq!(red_start, blue_start, "column 1 is not aligned");
    assert!(green_start > red_end, "column 2 does not follow column 1");
}

#[test]
fn a_line_without_tabs_widens_column_one() {
    let renderer = renderer();
    let title = "[bg=#ff0000]a long title line[/bg]\n";
    let rows = "[bg=#00ff00]A[/bg]\t[bg=#0000ff]B[/bg]";
    let without = raw_image(&renderer, &SlotValue::from_text(rows));
    let with = raw_image(&renderer, &SlotValue::from_text(format!("{title}{rows}")));
    assert!(
        x_span(&with, is_blue).0 > x_span(&without, is_blue).0,
        "the title line did not widen column 1"
    );
}

#[test]
fn spans_values_make_grids_like_text_values() {
    let renderer = renderer();
    let span = |text: &str, bg: &str| SpanSpec {
        text: text.to_owned(),
        bg: Some(bg.to_owned()),
        ..Default::default()
    };
    let value = SlotValue::Styled(Styles {
        spans: Some(vec![
            span("A", "#ff0000"),
            span("\tB", "#00ff00"),
            span("\nCCCC", "#0000ff"),
            span("\tB", "#ffff00"),
        ]),
        ..Default::default()
    });
    let frame = raw_image(&renderer, &value);
    assert_eq!(
        x_span(&frame, is_green).0,
        x_span(&frame, is_yellow).0,
        "spans value did not form aligned columns"
    );
}

#[test]
fn grids_are_left_aligned_and_ignore_the_global_align() {
    use opendeck_strip::render::layout::Align;
    let mut renderer = renderer();
    let plain = SlotValue::from_text(GRID_TEXT);
    let left = raw_image(&renderer, &plain);
    renderer.config.align = Align::Right;
    let global_right = raw_image(&renderer, &plain);
    assert_eq!(left, global_right, "global align moved a grid");
    renderer.config.align = Align::Center;
    assert_eq!(
        left,
        raw_image(&renderer, &plain),
        "global centre moved a grid"
    );
}

#[test]
fn a_grid_honours_its_own_align() {
    let renderer = renderer();
    let left_value = SlotValue::from_text(GRID_TEXT);
    let right_value = SlotValue::Styled(Styles {
        text: Some(GRID_TEXT.to_owned()),
        align: Some("right".to_owned()),
        ..Default::default()
    });
    let left = raw_image(&renderer, &left_value);
    let right = raw_image(&renderer, &right_value);
    assert_ne!(left, right, "align was not honoured");
    let (left_red, _) = x_span(&left, is_red);
    let (right_red, _) = x_span(&right, is_red);
    assert!(
        right_red > left_red,
        "right-aligned grid did not move right"
    );
    let left_width = x_span(&left, is_yellow).1 - left_red;
    let right_width = x_span(&right, is_yellow).1 - right_red;
    assert_eq!(left_width, right_width, "the grid changed width");
}

#[test]
fn the_global_align_still_applies_without_a_tab() {
    use opendeck_strip::render::layout::Align;
    let mut renderer = renderer();
    let value = SlotValue::from_text("ab\ncd");
    renderer.config.align = Align::Left;
    let left = raw_image(&renderer, &value);
    renderer.config.align = Align::Right;
    let right = raw_image(&renderer, &value);
    assert_ne!(left, right, "global align was ignored without a tab");
}

#[test]
fn text_only_detail_keeps_grid_columns() {
    let renderer = renderer();
    let text = "[c=#ff0000]A[/c]\t[c=#00ff00]B[/c]\n\
                [c=#0000ff]CCCC[/c]\t[c=#ffff00]B[/c]";
    let value = SlotValue::from_text(text);
    let text_only = renderer
        .render_strip(&value, StripMode::Raw, 0, Detail::TextOnly)
        .images
        .remove(0);
    assert_eq!(
        x_span(&text_only, is_green).0,
        x_span(&text_only, is_yellow).0,
        "TextOnly lost the grid"
    );
    let spaced = SlotValue::from_text(text.replace('\t', "    "));
    let with_spaces = renderer
        .render_strip(&spaced, StripMode::Raw, 0, Detail::TextOnly)
        .images
        .remove(0);
    assert_ne!(text_only, with_spaces, "TextOnly did not form a grid");
}

#[test]
fn column_gap_moves_the_second_column() {
    let mut renderer = renderer();
    let value = SlotValue::from_text(GRID_TEXT);
    renderer.config.column_gap = 0;
    let tight = raw_image(&renderer, &value);
    renderer.config.column_gap = 8;
    let wide = raw_image(&renderer, &value);
    assert!(
        x_span(&wide, is_green).0 > x_span(&tight, is_green).0,
        "column_gap was not wired to the renderer"
    );
}

#[test]
fn render_config_takes_the_layout_insets_from_config() {
    use opendeck_strip::config::Config;
    use opendeck_strip::render::RenderConfig;
    let config = Config {
        column_gap: 7,
        padding_horizontal: 9.0,
        padding_vertical: 3.0,
        ..Config::default()
    };
    let resolved = RenderConfig::from_config(&config);
    assert_eq!(resolved.column_gap, 7);
    assert_eq!(resolved.padding_horizontal, 9.0);
    assert_eq!(resolved.padding_vertical, 3.0);
}

#[test]
fn padding_axes_move_the_content_independently() {
    let mut renderer = renderer();
    let value = SlotValue::from_text(GRID_TEXT);
    let base = raw_image(&renderer, &value);
    let base_x = x_span(&base, is_red).0;
    let base_y = y_span(&base, is_red).0;

    renderer.config.padding_horizontal = 20.0;
    let horizontal = raw_image(&renderer, &value);
    assert!(
        x_span(&horizontal, is_red).0 >= base_x + 15,
        "horizontal padding did not move the grid"
    );
    assert_eq!(y_span(&horizontal, is_red).0, base_y);

    let mut renderer = Renderer::embedded();
    renderer.config.padding_vertical = 12.0;
    let vertical = raw_image(&renderer, &value);
    assert_eq!(x_span(&vertical, is_red).0, base_x);
    assert!(
        y_span(&vertical, is_red).0 > base_y,
        "vertical padding did not move the grid"
    );
}

#[test]
fn a_continuous_strip_with_tabs_is_a_grid() {
    let renderer = renderer();
    let with_tab = renderer.render_strip(
        &SlotValue::from_text("ab\tcd\nef\tgh"),
        StripMode::Continuous,
        28,
        Detail::Full,
    );
    let with_spaces = renderer.render_strip(
        &SlotValue::from_text("ab    cd\nef    gh"),
        StripMode::Continuous,
        28,
        Detail::Full,
    );
    assert_ne!(
        with_tab.images, with_spaces.images,
        "continuous mode did not form a grid"
    );
}
