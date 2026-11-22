use ab_glyph::{GlyphId, ScaleFont};

use crate::color::Rgb;
use crate::render::canvas::Canvas;
use crate::render::fonts::{Face, Fonts, GlyphSource, scaled};

use super::markup::Run;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Self::Left),
            "center" | "centre" | "middle" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    fn factor(self) -> f32 {
        match self {
            Self::Left => 0.0,
            Self::Center => 0.5,
            Self::Right => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" => Some(Self::Top),
            "middle" | "center" | "centre" => Some(Self::Middle),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    fn factor(self) -> f32 {
        match self {
            Self::Top => 0.0,
            Self::Middle => 0.5,
            Self::Bottom => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SizeSpec {
    Auto,
    Px(f32),
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub min_size: f32,
    pub max_size: f32,
    pub line_height: f32,
    pub padding: f32,
    pub word_wrap: bool,
    pub default_align: Align,
    pub default_valign: VAlign,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct DrawRequest<'a> {
    pub lines: &'a [Vec<Run>],
    pub base_size: SizeSpec,
    pub align: Align,
    pub valign: VAlign,
    pub color: Rgb,
    pub bar: Option<f32>,
    pub bar_color: Rgb,
    pub bar_bg: Rgb,
    pub options: &'a LayoutOptions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawResult {
    pub font_size: f32,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
struct PlacedRun {
    run: Run,
    x: f32,
    width: f32,
    size: f32,
}

#[derive(Clone, Debug)]
struct LineLayout {
    placements: Vec<PlacedRun>,
    width: f32,
    ascent: f32,
    descent: f32,
    height: f32,
}

struct Measured {
    placements: Vec<PlacedRun>,
    width: f32,
    ascent: f32,
    descent: f32,
    max_size: f32,
}

fn bar_height_for(rect: Rect, has_bar: bool) -> f32 {
    if has_bar {
        (rect.height * 0.10).max(3.0)
    } else {
        0.0
    }
}

fn inner_rect(rect: Rect, bar_height: f32, padding: f32) -> Rect {
    Rect {
        x: rect.x + padding,
        y: rect.y + padding,
        width: (rect.width - padding * 2.0).max(1.0),
        height: (rect.height - padding * 2.0 - bar_height).max(1.0),
    }
}

/// The font size automatic fitting would pick for `lines` in `rect`, without
/// drawing. Lets a caller give several boxes one common size.
pub fn auto_fit_size(
    lines: &[Vec<Run>],
    rect: Rect,
    has_bar: bool,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> f32 {
    let inner = inner_rect(rect, bar_height_for(rect, has_bar), options.padding);
    fit(
        lines,
        SizeSpec::Auto,
        &inner,
        options.word_wrap,
        options,
        fonts,
    )
    .0
}

pub fn draw_text(
    canvas: &mut Canvas,
    rect: Rect,
    request: &DrawRequest<'_>,
    fonts: &Fonts,
) -> DrawResult {
    let padding = request.options.padding;
    let bar_height = bar_height_for(rect, request.bar.is_some());
    let inner = inner_rect(rect, bar_height, padding);

    let (size, mut lines, mut truncated) = fit(
        request.lines,
        request.base_size,
        &inner,
        request.options.word_wrap,
        request.options,
        fonts,
    );

    let total_height: f32 = lines.iter().map(|l| l.height).sum();
    if total_height > inner.height + 0.5 {
        lines = keep_visible_lines(lines, inner.height);
        truncated = true;
    }
    for line in &mut lines {
        if line.width > inner.width + 0.5 {
            *line = truncate_line(line, size, inner.width, fonts);
            truncated = true;
        }
    }

    let total_height: f32 = lines.iter().map(|l| l.height).sum();
    let y_start = inner.y + (inner.height - total_height) * request.valign.factor();
    let mut y = y_start;

    let clip = (
        rect.x.floor() as i32,
        rect.y.floor() as i32,
        (rect.x + rect.width).ceil() as i32,
        (rect.y + rect.height).ceil() as i32,
    );
    canvas.set_clip(Some(clip));

    for line in &lines {
        let line_top = y;
        let baseline =
            (line_top + (line.height - (line.ascent - line.descent)) * 0.5 + line.ascent).round();
        let x_start = inner.x + (inner.width - line.width) * request.align.factor();

        for placed in &line.placements {
            let color = placed.run.color.unwrap_or(request.color);
            if let Some(bg) = placed.run.bg {
                canvas.fill_rect(x_start + placed.x, line_top, placed.width, line.height, bg);
            }
            draw_run(canvas, placed, x_start + placed.x, baseline, color, fonts);
            if placed.run.underline {
                let thickness = (placed.size * 0.07).max(1.0);
                canvas.fill_rect(
                    x_start + placed.x,
                    baseline + (placed.size * 0.12).max(1.0),
                    placed.width,
                    thickness,
                    color,
                );
            }
        }
        y += line.height;
    }

    canvas.clear_clip();

    if let Some(fraction) = request.bar {
        let fraction = fraction.clamp(0.0, 1.0);
        let bar_y = rect.y + rect.height - padding - bar_height;
        let width = (rect.width - padding * 2.0).max(0.0);
        canvas.fill_rect(rect.x + padding, bar_y, width, bar_height, request.bar_bg);
        canvas.fill_rect(
            rect.x + padding,
            bar_y,
            width * fraction,
            bar_height,
            request.bar_color,
        );
    }

    DrawResult {
        font_size: size,
        truncated,
    }
}

fn fit(
    source: &[Vec<Run>],
    base: SizeSpec,
    inner: &Rect,
    wrap: bool,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> (f32, Vec<LineLayout>, bool) {
    match base {
        SizeSpec::Px(px) => {
            let min = options.min_size.max(1.0);
            let max = options.max_size.max(min);
            let size = px.clamp(min, max);
            let lines = layout_lines(source, size, inner.width, wrap, options, fonts);
            (size, lines, false)
        }
        SizeSpec::Auto => {
            let mut lo = options.min_size.max(1.0);
            let mut hi = options.max_size.max(lo);
            let mut best: Option<(f32, Vec<LineLayout>)> = None;
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                let lines = layout_lines(source, mid, inner.width, wrap, options, fonts);
                if lines_fit(&lines, inner) {
                    best = Some((mid, lines));
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            if let Some((size, lines)) = best {
                return (size, lines, false);
            }
            let size = options.min_size.max(1.0);
            let lines = layout_lines(source, size, inner.width, wrap, options, fonts);
            (size, lines, true)
        }
    }
}

fn lines_fit(lines: &[LineLayout], inner: &Rect) -> bool {
    let total: f32 = lines.iter().map(|l| l.height).sum();
    total <= inner.height + 0.5 && lines.iter().all(|l| l.width <= inner.width + 0.5)
}

fn keep_visible_lines(lines: Vec<LineLayout>, height: f32) -> Vec<LineLayout> {
    let mut out = Vec::new();
    let mut used = 0.0;
    for line in lines {
        if used + line.height > height + 0.5 && !out.is_empty() {
            break;
        }
        used += line.height;
        out.push(line);
    }
    out
}

fn layout_lines(
    source: &[Vec<Run>],
    size: f32,
    max_width: f32,
    wrap: bool,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> Vec<LineLayout> {
    let mut lines = Vec::new();
    for explicit in source {
        let parts: Vec<Vec<Run>> = if wrap {
            wrap_line(explicit, size, max_width, fonts)
        } else {
            vec![explicit.clone()]
        };
        for part in parts {
            lines.push(measure_line(&part, size, options, fonts));
        }
    }
    if lines.is_empty() {
        lines.push(measure_line(&[], size, options, fonts));
    }
    lines
}

fn measure_line(
    runs: &[Run],
    base_size: f32,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> LineLayout {
    let measured = measure(runs, base_size, fonts);
    let height = (measured.max_size * options.line_height)
        .max(measured.ascent - measured.descent)
        .max(1.0);
    LineLayout {
        placements: measured.placements,
        width: measured.width,
        ascent: measured.ascent,
        descent: measured.descent,
        height,
    }
}

fn measure_width(runs: &[Run], base_size: f32, fonts: &Fonts) -> f32 {
    measure(runs, base_size, fonts).width
}

fn measure(runs: &[Run], base_size: f32, fonts: &Fonts) -> Measured {
    let mut placements = Vec::with_capacity(runs.len());
    let mut x = 0.0f32;
    let mut ascent = 0.0f32;
    let mut descent = 0.0f32;
    let mut max_size = 0.0f32;
    let mut prev: Option<(&Face, GlyphId)> = None;

    for run in runs {
        let size = (base_size * run.size).max(1.0);
        max_size = max_size.max(size);
        let mut run_start = x;
        let mut first = true;

        for c in expand_chars(&run.text) {
            match fonts.resolve(run.mono, run.bold, run.italic, c) {
                GlyphSource::Glyph { face, id } => {
                    let font = scaled(face, size, size);
                    if let Some((prev_face, prev_id)) = prev
                        && std::ptr::eq(prev_face, face)
                    {
                        x += font.kern(prev_id, id);
                    }
                    if first {
                        run_start = x;
                        first = false;
                    }
                    ascent = ascent.max(font.ascent());
                    descent = descent.min(font.descent());
                    x += font.h_advance(id);
                    prev = Some((face, id));
                }
                GlyphSource::Placeholder { face } => {
                    if first {
                        run_start = x;
                        first = false;
                    }
                    let font = scaled(face, size, size);
                    ascent = ascent.max(font.ascent());
                    descent = descent.min(font.descent());
                    x += size * 0.6;
                    prev = None;
                }
            }
        }

        placements.push(PlacedRun {
            run: run.clone(),
            x: run_start,
            width: (x - run_start).max(0.0),
            size,
        });
    }

    if placements.is_empty() {
        let font = scaled(fonts.face(false, false, false), base_size, base_size);
        ascent = font.ascent();
        descent = font.descent();
        max_size = base_size;
    }

    Measured {
        placements,
        width: x,
        ascent,
        descent,
        max_size,
    }
}

fn draw_run(
    canvas: &mut Canvas,
    placed: &PlacedRun,
    start_x: f32,
    baseline_y: f32,
    color: Rgb,
    fonts: &Fonts,
) {
    let run = &placed.run;
    let size = placed.size;
    let mut x = start_x;
    let mut prev: Option<(&Face, GlyphId)> = None;

    for c in expand_chars(&run.text) {
        match fonts.resolve(run.mono, run.bold, run.italic, c) {
            GlyphSource::Glyph { face, id } => {
                let font = scaled(face, size, size);
                if let Some((prev_face, prev_id)) = prev
                    && std::ptr::eq(prev_face, face)
                {
                    x += font.kern(prev_id, id);
                }
                canvas.draw_glyph(face, id, size, size, x, baseline_y, color);
                x += font.h_advance(id);
                prev = Some((face, id));
            }
            GlyphSource::Placeholder { face } => {
                canvas.draw_placeholder(face, size, size, x, baseline_y, color);
                x += size * 0.6;
                prev = None;
            }
        }
    }
}

fn expand_chars(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.chars().count());
    for c in text.chars() {
        match c {
            '\t' => out.extend(std::iter::repeat_n(' ', 4)),
            '\r' => {}
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

fn wrap_line(runs: &[Run], size: f32, max_width: f32, fonts: &Fonts) -> Vec<Vec<Run>> {
    let words = split_words(runs);
    if words.is_empty() {
        return vec![Vec::new()];
    }
    let mut lines: Vec<Vec<Run>> = Vec::new();
    let mut current: Vec<Run> = Vec::new();
    for word in words {
        let mut candidate = current.clone();
        for run in &word {
            push_merged(&mut candidate, run.clone());
        }
        if !current.is_empty() && measure_width(&candidate, size, fonts) > max_width + 0.5 {
            lines.push(std::mem::take(&mut current));
        }
        for run in word {
            push_merged(&mut current, run);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

fn split_words(runs: &[Run]) -> Vec<Vec<Run>> {
    let mut words = Vec::new();
    let mut current: Vec<Run> = Vec::new();
    let mut chunk = String::new();
    let mut chunk_style: Option<Run> = None;

    fn flush_chunk(current: &mut Vec<Run>, chunk: &mut String, style: &mut Option<Run>) {
        if chunk.is_empty() {
            return;
        }
        let mut run = style.clone().unwrap_or_default();
        run.text = std::mem::take(chunk);
        push_merged(current, run);
        *style = None;
    }

    for run in runs {
        for c in run.text.chars() {
            let style_matches = chunk_style.as_ref().is_some_and(|style| {
                style.bold == run.bold
                    && style.italic == run.italic
                    && style.underline == run.underline
                    && style.mono == run.mono
                    && style.color == run.color
                    && style.bg == run.bg
                    && (style.size - run.size).abs() < f32::EPSILON
            });
            if !style_matches {
                flush_chunk(&mut current, &mut chunk, &mut chunk_style);
                chunk_style = Some(Run {
                    text: String::new(),
                    bold: run.bold,
                    italic: run.italic,
                    underline: run.underline,
                    mono: run.mono,
                    color: run.color,
                    bg: run.bg,
                    size: run.size,
                });
            }
            chunk.push(c);
            if c == ' ' {
                flush_chunk(&mut current, &mut chunk, &mut chunk_style);
                words.push(std::mem::take(&mut current));
            }
        }
    }
    flush_chunk(&mut current, &mut chunk, &mut chunk_style);
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn push_merged(runs: &mut Vec<Run>, run: Run) {
    if run.text.is_empty() {
        return;
    }
    if let Some(last) = runs.last_mut()
        && same_style(last, &run)
    {
        last.text.push_str(&run.text);
    } else {
        runs.push(run);
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.mono == b.mono
        && a.color == b.color
        && a.bg == b.bg
        && (a.size - b.size).abs() < f32::EPSILON
}

fn truncate_line(line: &LineLayout, size: f32, max_width: f32, fonts: &Fonts) -> LineLayout {
    if line.width <= max_width + 0.5 {
        return line.clone();
    }
    let fallback_style = line
        .placements
        .last()
        .map(|p| p.run.clone())
        .unwrap_or_default();
    let ellipsis = Run {
        text: "…".to_owned(),
        ..fallback_style.clone()
    };
    let ellipsis_width = measure_width(std::slice::from_ref(&ellipsis), size, fonts);
    let budget = (max_width - ellipsis_width).max(0.0);

    let mut out: Vec<Run> = Vec::new();
    let mut cursor = 0.0f32;
    let mut prev: Option<(&Face, GlyphId)> = None;
    let mut last_style = fallback_style;

    'outer: for placed in &line.placements {
        let run = &placed.run;
        for c in expand_chars(&run.text) {
            match fonts.resolve(run.mono, run.bold, run.italic, c) {
                GlyphSource::Glyph { face, id } => {
                    let font = scaled(face, placed.size, placed.size);
                    let kern = match prev {
                        Some((prev_face, prev_id)) if std::ptr::eq(prev_face, face) => {
                            font.kern(prev_id, id)
                        }
                        _ => 0.0,
                    };
                    let advance = kern + font.h_advance(id);
                    if cursor + advance > budget {
                        break 'outer;
                    }
                    cursor += advance;
                    prev = Some((face, id));
                }
                GlyphSource::Placeholder { face } => {
                    let advance = placed.size * 0.6;
                    if cursor + advance > budget {
                        break 'outer;
                    }
                    cursor += advance;
                    prev = None;
                    let _ = face;
                }
            }
            let mut out_run = run.clone();
            out_run.text = c.to_string();
            push_merged(&mut out, out_run.clone());
            last_style = out_run;
        }
    }

    out.push(Run {
        text: "…".to_owned(),
        ..last_style
    });
    measure_line(&out, size, &default_options(), fonts)
}

fn default_options() -> LayoutOptions {
    LayoutOptions {
        min_size: 1.0,
        max_size: 1000.0,
        line_height: 1.0,
        padding: 0.0,
        word_wrap: false,
        default_align: Align::Left,
        default_valign: VAlign::Top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> LayoutOptions {
        LayoutOptions {
            min_size: 8.0,
            max_size: 64.0,
            line_height: 1.15,
            padding: 2.0,
            word_wrap: false,
            default_align: Align::Center,
            default_valign: VAlign::Middle,
        }
    }

    fn draw(
        lines: &[Vec<Run>],
        w: u32,
        h: u32,
        align: Align,
        size: SizeSpec,
    ) -> (Canvas, DrawResult) {
        let mut canvas = Canvas::new(w, h, Rgb::BLACK);
        let opts = options();
        let request = DrawRequest {
            lines,
            base_size: size,
            align,
            valign: VAlign::Middle,
            color: Rgb::WHITE,
            bar: None,
            bar_color: Rgb::WHITE,
            bar_bg: Rgb::BLACK,
            options: &opts,
        };
        let result = draw_text(
            &mut canvas,
            Rect {
                x: 0.0,
                y: 0.0,
                width: w as f32,
                height: h as f32,
            },
            &request,
            Fonts::embedded(),
        );
        (canvas, result)
    }

    fn bounds(canvas: &Canvas) -> (i32, i32, i32, i32) {
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for y in 0..canvas.height as i32 {
            for x in 0..canvas.width as i32 {
                if canvas.get(x, y).is_some_and(|p| p != Rgb::BLACK) {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        (min_x, min_y, max_x, max_y)
    }

    #[test]
    fn auto_fit_is_monotonic() {
        let runs = vec![vec![Run::plain("HELLO")]];
        let mut previous = 0.0;
        for width in [40, 60, 80, 120] {
            let (_, result) = draw(&runs, width, 60, Align::Center, SizeSpec::Auto);
            assert!(
                result.font_size >= previous,
                "{width}: {}",
                result.font_size
            );
            previous = result.font_size;
        }
    }

    #[test]
    fn alignment_moves_the_ink() {
        let runs = vec![vec![Run::plain("III")]];
        let (left, _) = draw(&runs, 120, 60, Align::Left, SizeSpec::Px(20.0));
        let (right, _) = draw(&runs, 120, 60, Align::Right, SizeSpec::Px(20.0));
        assert!(bounds(&left).0 < 20);
        assert!(bounds(&right).2 > 100);
    }

    #[test]
    fn narrow_box_truncates_with_ellipsis() {
        let runs = vec![vec![Run::plain("a very long line indeed")]];
        let (canvas, result) = draw(&runs, 60, 30, Align::Left, SizeSpec::Px(20.0));
        assert!(result.truncated);
        assert!(bounds(&canvas).2 <= 60);
    }

    #[test]
    fn non_latin_uses_placeholder_not_panic() {
        let runs = vec![vec![Run::plain("日本語")]];
        let (canvas, _) = draw(&runs, 200, 60, Align::Center, SizeSpec::Auto);
        assert!(bounds(&canvas).2 > 0);
    }

    #[test]
    fn oversized_explicit_size_is_clamped_and_visible() {
        let runs = vec![vec![Run::plain("A")]];
        let (canvas, result) = draw(&runs, 200, 100, Align::Center, SizeSpec::Px(1_000_000.0));
        assert!(result.font_size <= 64.0, "{}", result.font_size);
        assert!(bounds(&canvas).2 >= 0, "clamped text is visible");
    }

    #[test]
    fn a_tall_line_is_clipped_not_dropped() {
        let runs = vec![vec![Run::plain("A")]];
        let mut canvas = Canvas::new(200, 100, Rgb::BLACK);
        let mut opts = options();
        opts.max_size = 96.0;
        let request = DrawRequest {
            lines: &runs,
            base_size: SizeSpec::Px(96.0),
            align: Align::Center,
            valign: VAlign::Middle,
            color: Rgb::WHITE,
            bar: None,
            bar_color: Rgb::WHITE,
            bar_bg: Rgb::BLACK,
            options: &opts,
        };
        draw_text(
            &mut canvas,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 100.0,
            },
            &request,
            Fonts::embedded(),
        );
        assert!(bounds(&canvas).2 >= 0, "clipped text is still visible");
    }

    #[test]
    fn explicit_size_is_respected() {
        let runs = vec![vec![Run::plain("A")]];
        let (_, result) = draw(&runs, 200, 60, Align::Center, SizeSpec::Px(24.0));
        assert!((result.font_size - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn wrap_does_not_break_at_style_boundaries() {
        let runs = [
            Run {
                text: "aaa".to_owned(),
                bold: true,
                ..Default::default()
            },
            Run::plain("bbb"),
        ];
        let lines = wrap_line(&runs, 20.0, 40.0, Fonts::embedded());
        assert_eq!(lines.len(), 1);
        assert_eq!(crate::render::markup::plain_of_runs(&lines[0]), "aaabbb");
    }

    #[test]
    fn wrap_uses_more_lines() {
        let runs = vec![vec![Run::plain("one two three four")]];
        let mut canvas = Canvas::new(120, 80, Rgb::BLACK);
        let mut opts = options();
        opts.word_wrap = true;
        let request = DrawRequest {
            lines: &runs,
            base_size: SizeSpec::Px(20.0),
            align: Align::Center,
            valign: VAlign::Middle,
            color: Rgb::WHITE,
            bar: None,
            bar_color: Rgb::WHITE,
            bar_bg: Rgb::BLACK,
            options: &opts,
        };
        draw_text(
            &mut canvas,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 80.0,
            },
            &request,
            Fonts::embedded(),
        );
        let (_, top, _, bottom) = bounds(&canvas);
        assert!(bottom - top > 25);
    }
}
