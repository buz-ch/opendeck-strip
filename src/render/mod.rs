pub mod canvas;
pub mod fonts;
pub mod layout;
pub mod markup;

use image::RgbImage;
use serde_json::Value;

use crate::color::Rgb;
use crate::config::Config;
use crate::status::{SlotValue, Styles};

use self::canvas::Canvas;
use self::fonts::Fonts;
use self::layout::{
    Align, DrawRequest, Grid, LayoutOptions, Rect, SizeSpec, VAlign, auto_fit_size, draw_text,
    has_tab,
};
use self::markup::{Run, parse_text, plain_of_lines, spans_to_lines};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Full,
    /// Text only: no run backgrounds, no bar.
    TextOnly,
}

#[derive(Clone, Debug)]
pub struct RenderConfig {
    pub default_color: Rgb,
    pub default_bg: Rgb,
    pub min_size: f32,
    pub max_size: f32,
    pub line_height: f32,
    pub padding: f32,
    pub word_wrap: bool,
    pub uniform_strip_size: bool,
    pub uniform_skip_first: bool,
    pub align: Align,
    pub valign: VAlign,
    pub bar_color: Rgb,
    pub bar_bg: Rgb,
    pub column_gap: u32,
}

impl RenderConfig {
    pub fn from_config(config: &Config) -> Self {
        Self {
            default_color: Rgb::parse(&config.default_color).unwrap_or(Rgb::WHITE),
            default_bg: Rgb::parse(&config.default_bg).unwrap_or(Rgb::BLACK),
            min_size: config.min_font_size,
            max_size: config.max_font_size,
            line_height: config.line_height,
            padding: config.padding,
            word_wrap: config.word_wrap,
            uniform_strip_size: config.strip_uniform_size,
            uniform_skip_first: config.strip_uniform_skip_first,
            align: Align::parse(&config.align).unwrap_or(Align::Center),
            valign: VAlign::parse(&config.valign).unwrap_or(VAlign::Middle),
            bar_color: Rgb::parse(&config.bar_color).unwrap_or(Rgb(85, 204, 255)),
            bar_bg: Rgb::parse(&config.bar_bg).unwrap_or(Rgb(32, 40, 48)),
            column_gap: config.column_gap,
        }
    }
}

pub struct Renderer {
    pub fonts: Fonts,
    pub config: RenderConfig,
}

#[derive(Clone, Debug)]
pub struct SlotSpec {
    pub lines: Vec<Vec<Run>>,
    pub base_size: SizeSpec,
    pub color: Rgb,
    pub bg: Rgb,
    pub align: Align,
    /// The alignment the value itself set; a grid uses it, else `Left`.
    pub explicit_align: Option<Align>,
    pub valign: VAlign,
    pub bar: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct Rendered {
    pub images: Vec<RgbImage>,
    pub truncated: bool,
}

impl Renderer {
    pub fn load(config: &Config) -> Self {
        Self {
            fonts: Fonts::load(config),
            config: RenderConfig::from_config(config),
        }
    }

    pub fn embedded() -> Self {
        Self {
            fonts: Fonts::embedded().clone(),
            config: RenderConfig::from_config(&Config::default()),
        }
    }

    pub fn spec(&self, value: &SlotValue) -> SlotSpec {
        let (styled, text, spans) = match value {
            SlotValue::Text(text) => (None, text.as_str(), None),
            SlotValue::Styled(styles) => (
                Some(styles),
                styles.text.as_deref().unwrap_or(""),
                styles.spans.as_deref(),
            ),
        };

        let mut lines = match (styled, spans) {
            (_, Some(spans)) if !spans.is_empty() => spans_to_lines(spans),
            _ => parse_text(text, false),
        };

        let base_mono = styled_font_is_mono(value);
        if base_mono {
            for line in &mut lines {
                for run in line {
                    run.mono = true;
                }
            }
        }

        let color = styled
            .and_then(|s| s.color.as_deref())
            .and_then(Rgb::parse)
            .unwrap_or(self.config.default_color);
        let bg = styled
            .and_then(|s| s.bg.as_deref())
            .and_then(Rgb::parse)
            .unwrap_or(self.config.default_bg);
        let explicit_align = styled
            .and_then(|s| s.align.as_deref())
            .and_then(Align::parse);
        let align = explicit_align.unwrap_or(self.config.align);
        let valign = styled
            .and_then(|s| s.valign.as_deref())
            .and_then(VAlign::parse)
            .unwrap_or(self.config.valign);
        let base_size = styled
            .and_then(|s| s.size.as_ref())
            .map(parse_size)
            .unwrap_or(SizeSpec::Auto);
        let bar = styled.and_then(|s| s.bar).map(|v| v.clamp(0.0, 1.0));

        SlotSpec {
            lines,
            base_size,
            color,
            bg,
            align,
            explicit_align,
            valign,
            bar,
        }
    }

    /// Split a strip value into independent segment specs. `|` splits; the
    /// markup is parsed after the split so tags cannot cross a boundary.
    pub fn spec_segments(&self, value: &SlotValue) -> Vec<SlotSpec> {
        let base = self.spec(value);
        let text = match value {
            SlotValue::Text(text) => Some(text.as_str()),
            SlotValue::Styled(styles) if styles.spans.is_none() => styles.text.as_deref(),
            _ => None,
        };
        let Some(text) = text else {
            return vec![base];
        };
        if !text.contains('|') {
            return vec![base];
        }
        let base_mono = styled_font_is_mono(value);
        markup::split_segments(text)
            .into_iter()
            .map(|part| {
                let mut lines = parse_text(part, false);
                if base_mono {
                    for line in &mut lines {
                        for run in line {
                            run.mono = true;
                        }
                    }
                }
                SlotSpec {
                    lines,
                    ..base.clone()
                }
            })
            .collect()
    }

    pub fn render_slot(
        &self,
        spec: &SlotSpec,
        width: u32,
        height: u32,
        detail: Detail,
    ) -> Rendered {
        let mut canvas = Canvas::new(width, height, spec.bg);
        let result = self.draw_segment(
            &mut canvas,
            spec,
            Rect {
                x: 0.0,
                y: 0.0,
                width: width as f32,
                height: height as f32,
            },
            detail,
            None,
        );
        Rendered {
            images: vec![canvas.into_image()],
            truncated: result,
        }
    }

    /// Render the full strip. Continuous and tiles modes return four dial
    /// images; raw mode returns one 800x128 image.
    pub fn render_strip(
        &self,
        value: &SlotValue,
        mode: crate::status::StripMode,
        gap: u32,
        detail: Detail,
    ) -> Rendered {
        let (width, height) = match mode {
            crate::status::StripMode::Continuous => (800u32, 100u32),
            crate::status::StripMode::Tiles => (176 * 4 + gap * 3, 88u32),
            crate::status::StripMode::Raw => (800u32, 128u32),
        };
        let logical_height = height as f32;
        let segments = self.spec_segments(value);
        let grid = self.strip_grid(&segments, value, mode);
        let bg = segments
            .first()
            .map(|s| s.bg)
            .unwrap_or(self.config.default_bg);
        let mut canvas = Canvas::new(width, height, bg);

        let mut truncated = false;
        let segment_count = segments.len().min(4);
        if segments.len() > 4 {
            log::warn!(
                "strip has {} segments; using the first four",
                segments.len()
            );
        }
        let rects: Vec<Rect> = (0..segment_count)
            .map(|index| {
                if segment_count == 1 {
                    return Rect {
                        x: 0.0,
                        y: 0.0,
                        width: width as f32,
                        height: logical_height,
                    };
                }
                let (x, w) = match mode {
                    crate::status::StripMode::Continuous | crate::status::StripMode::Raw => {
                        (index as f32 * 200.0, 200.0)
                    }
                    crate::status::StripMode::Tiles => {
                        let step = 176.0 + gap as f32;
                        (index as f32 * step, 176.0)
                    }
                };
                Rect {
                    x,
                    y: 0.0,
                    width: w,
                    height: logical_height,
                }
            })
            .collect();

        // Optionally give every auto-sized segment the smallest size any of
        // them needs, so the segments do not end up with different fonts.
        let uniform = if self.config.uniform_strip_size && segment_count > 1 {
            let options = self.layout_options();
            let skip_first = self.config.uniform_skip_first;
            segments
                .iter()
                .zip(&rects)
                .enumerate()
                .filter(|(i, (s, _))| s.base_size == SizeSpec::Auto && !(skip_first && *i == 0))
                .map(|(_, (s, r))| {
                    let lines = if detail == Detail::TextOnly {
                        strip_backgrounds(&s.lines)
                    } else {
                        s.lines.clone()
                    };
                    let bar = detail != Detail::TextOnly && s.bar.is_some();
                    auto_fit_size(&lines, *r, bar, &options, &self.fonts)
                })
                .reduce(f32::min)
        } else {
            None
        };

        for (index, (segment, rect)) in segments.iter().take(segment_count).zip(rects).enumerate() {
            let pinned;
            let keep_own = self.config.uniform_skip_first && index == 0;
            let segment = match uniform {
                Some(px) if segment.base_size == SizeSpec::Auto && !keep_own => {
                    pinned = SlotSpec {
                        base_size: SizeSpec::Px(px),
                        ..segment.clone()
                    };
                    &pinned
                }
                _ => segment,
            };
            truncated |= self.draw_segment(&mut canvas, segment, rect, detail, grid);
        }

        let image = canvas.into_image();
        if mode == crate::status::StripMode::Raw {
            // One whole 800x128 frame; there are no dial slices in raw mode.
            return Rendered {
                images: vec![image],
                truncated,
            };
        }
        let (tile_width, tile_step) = match mode {
            crate::status::StripMode::Continuous => (200u32, 200u32),
            crate::status::StripMode::Tiles => (176u32, 176 + gap),
            crate::status::StripMode::Raw => unreachable!("handled above"),
        };
        let mut images = Vec::with_capacity(4);
        for index in 0..4u32 {
            let x = index * tile_step;
            images.push(image::imageops::crop_imm(&image, x, 0, tile_width, height).to_image());
        }
        Rendered { images, truncated }
    }

    /// Tab columns apply to one full-width segment in raw or continuous mode
    /// without word wrap.
    fn strip_grid(
        &self,
        segments: &[SlotSpec],
        value: &SlotValue,
        mode: crate::status::StripMode,
    ) -> Option<Grid> {
        let [segment] = segments else {
            return None;
        };
        if self.config.word_wrap || !has_tab(&segment.lines) {
            return None;
        }
        match mode {
            crate::status::StripMode::Raw | crate::status::StripMode::Continuous => Some(Grid {
                gap_spaces: self.config.column_gap,
                mono: styled_font_is_mono(value),
            }),
            crate::status::StripMode::Tiles => {
                log::warn!(
                    "tabs in a strip value need raw or continuous mode; drawing them as spaces"
                );
                None
            }
        }
    }

    fn layout_options(&self) -> LayoutOptions {
        LayoutOptions {
            min_size: self.config.min_size,
            max_size: self.config.max_size,
            line_height: self.config.line_height,
            padding: self.config.padding,
            word_wrap: self.config.word_wrap,
            default_align: self.config.align,
            default_valign: self.config.valign,
        }
    }

    fn draw_segment(
        &self,
        canvas: &mut Canvas,
        spec: &SlotSpec,
        rect: Rect,
        detail: Detail,
        grid: Option<Grid>,
    ) -> bool {
        let options = self.layout_options();
        let bar = if detail == Detail::TextOnly {
            None
        } else {
            spec.bar
        };
        let lines = if detail == Detail::TextOnly {
            strip_backgrounds(&spec.lines)
        } else {
            spec.lines.clone()
        };
        let align = match grid {
            Some(_) => spec.explicit_align.unwrap_or(Align::Left),
            None => spec.align,
        };
        let request = DrawRequest {
            lines: &lines,
            base_size: spec.base_size,
            align,
            valign: spec.valign,
            color: spec.color,
            bar,
            bar_color: self.config.bar_color,
            bar_bg: self.config.bar_bg,
            grid,
            options: &options,
        };
        draw_text(canvas, rect, &request, &self.fonts).truncated
    }

    pub fn test_pattern(
        &self,
        mode: crate::status::StripMode,
        gap: u32,
        detail: Detail,
    ) -> Rendered {
        if mode == crate::status::StripMode::Raw {
            return Rendered {
                images: vec![self.raw_test_pattern()],
                truncated: false,
            };
        }
        let (width, height) = match mode {
            crate::status::StripMode::Continuous => (800u32, 100u32),
            crate::status::StripMode::Tiles => (176 * 4 + gap * 3, 88u32),
            crate::status::StripMode::Raw => unreachable!("handled above"),
        };
        let mut canvas = Canvas::new(width, height, Rgb::BLACK);
        if detail == Detail::Full {
            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    let r = (x as f32 / width as f32 * 80.0) as u8;
                    let b = (y as f32 / height as f32 * 80.0) as u8;
                    canvas.put(x, y, Rgb(r, 0, b));
                }
            }
        }
        let line = Rgb(255, 255, 255);
        let steps = width.max(height) * 2;
        for i in 0..steps {
            let x = (i as f32 / steps as f32 * width as f32) as i32;
            let y = (i as f32 / steps as f32 * height as f32) as i32;
            canvas.put(x, y, line);
            canvas.put(x, y - 1, line);
        }
        let marker = Rgb(255, 80, 40);
        let boundaries: Vec<u32> = match mode {
            crate::status::StripMode::Continuous | crate::status::StripMode::Raw => {
                vec![0, 200, 400, 600, 799]
            }
            crate::status::StripMode::Tiles => {
                let mut out = Vec::new();
                for index in 0..5u32 {
                    out.push((index * (176 + gap)).min(width - 1));
                }
                out
            }
        };
        for x in boundaries {
            for y in 0..8i32 {
                canvas.put(x as i32, y, marker);
                canvas.put(x as i32, height as i32 - 1 - y, marker);
            }
        }
        let image = canvas.into_image();
        let (tile_width, tile_step) = match mode {
            crate::status::StripMode::Continuous | crate::status::StripMode::Raw => {
                (200u32, 200u32)
            }
            crate::status::StripMode::Tiles => (176u32, 176 + gap),
        };
        let images = (0..4u32)
            .map(|index| {
                image::imageops::crop_imm(&image, index * tile_step, 0, tile_width, height)
                    .to_image()
            })
            .collect();
        Rendered {
            images,
            truncated: false,
        }
    }
}

/// Quadrant background colours of the raw test pattern. None of them is black.
pub const RAW_QUADRANTS: [Rgb; 4] = [
    Rgb(0, 40, 120),
    Rgb(0, 100, 90),
    Rgb(90, 30, 120),
    Rgb(120, 70, 0),
];
/// One-pixel frame on all four edges of the raw test pattern.
pub const RAW_BORDER: Rgb = Rgb(255, 255, 255);
/// The two middle rows (y = 63 and 64) of the raw test pattern.
pub const RAW_MIDDLE_ROW: Rgb = Rgb(255, 0, 255);
/// The two middle columns (x = 399 and 400) of the raw test pattern.
pub const RAW_MIDDLE_COLUMN: Rgb = Rgb(255, 255, 0);
/// Seam markers at x = 200 and x = 600 on the top and bottom rows.
pub const RAW_SEAM: Rgb = Rgb(255, 128, 0);
/// Background of the ruler pattern.
pub const RAW_RULER_BACKGROUND: Rgb = Rgb(128, 128, 128);
/// One-pixel ruler bands, from the outer edge inward, the same on all four edges.
pub const RAW_RULER_BANDS: [(&str, Rgb); 8] = [
    ("white", Rgb(255, 255, 255)),
    ("red", Rgb(255, 0, 0)),
    ("green", Rgb(0, 255, 0)),
    ("blue", Rgb(0, 0, 255)),
    ("yellow", Rgb(255, 255, 0)),
    ("cyan", Rgb(0, 255, 255)),
    ("magenta", Rgb(255, 0, 255)),
    ("orange", Rgb(255, 128, 0)),
];

impl Renderer {
    /// An 800x128 frame that measures how many outer rows and columns the
    /// device hides: eight one-pixel colour bands on every edge on a grey
    /// background, in the order of `RAW_RULER_BANDS` from the outer edge
    /// inward. The top and bottom bands win in the corners.
    pub fn raw_ruler_pattern(&self) -> RgbImage {
        let (width, height) = (800i32, 128i32);
        let mut canvas = Canvas::new(800, 128, RAW_RULER_BACKGROUND);
        let title = self.spec(&styled_value(
            "RULER: count bands from each edge",
            Some(Rgb::WHITE),
            None,
        ));
        self.draw_segment(
            &mut canvas,
            &title,
            Rect {
                x: 60.0,
                y: 30.0,
                width: 680.0,
                height: 68.0,
            },
            Detail::Full,
            None,
        );
        for (index, (_, color)) in RAW_RULER_BANDS.iter().enumerate() {
            let i = index as i32;
            for y in i..height - i {
                canvas.put(i, y, *color);
                canvas.put(width - 1 - i, y, *color);
            }
            for x in i..width - i {
                canvas.put(x, i, *color);
                canvas.put(x, height - 1 - i, *color);
            }
        }
        canvas.into_image()
    }

    /// An 800x128 test frame for the AKP05 raw strip socket. It has a coloured
    /// background, so black bands, seams, shifts and a wrong orientation all
    /// show on the device: four different quadrant colours, a white border on
    /// every edge, a magenta middle row, a yellow middle column, orange seam
    /// ticks at x = 200 and 600, upright text and the quadrant numbers 1-4.
    pub fn raw_test_pattern(&self) -> RgbImage {
        let (width, height) = (800i32, 128i32);
        let mut canvas = Canvas::new(800, 128, RAW_QUADRANTS[0]);
        for (index, color) in RAW_QUADRANTS.iter().enumerate() {
            canvas.fill_rect(index as f32 * 200.0, 0.0, 200.0, height as f32, *color);
        }

        let title = self.spec(&styled_value("RAW 800x128", Some(Rgb::WHITE), None));
        self.draw_segment(
            &mut canvas,
            &title,
            Rect {
                x: 120.0,
                y: 10.0,
                width: 560.0,
                height: 46.0,
            },
            Detail::Full,
            None,
        );
        for index in 0..4 {
            let label = self.spec(&styled_value(
                (index + 1).to_string(),
                Some(Rgb::WHITE),
                None,
            ));
            self.draw_segment(
                &mut canvas,
                &label,
                Rect {
                    x: index as f32 * 200.0 + 50.0,
                    y: 70.0,
                    width: 100.0,
                    height: 48.0,
                },
                Detail::Full,
                None,
            );
        }

        for x in 0..width {
            canvas.put(x, 63, RAW_MIDDLE_ROW);
            canvas.put(x, 64, RAW_MIDDLE_ROW);
        }
        for y in 0..height {
            canvas.put(399, y, RAW_MIDDLE_COLUMN);
            canvas.put(400, y, RAW_MIDDLE_COLUMN);
        }
        for x in [200, 600] {
            for y in 0..10 {
                canvas.put(x, y, RAW_SEAM);
                canvas.put(x, height - 1 - y, RAW_SEAM);
            }
        }
        for x in 0..width {
            canvas.put(x, 0, RAW_BORDER);
            canvas.put(x, height - 1, RAW_BORDER);
        }
        for y in 0..height {
            canvas.put(0, y, RAW_BORDER);
            canvas.put(width - 1, y, RAW_BORDER);
        }
        canvas.into_image()
    }
}

fn strip_backgrounds(lines: &[Vec<Run>]) -> Vec<Vec<Run>> {
    lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|run| Run {
                    bg: None,
                    ..run.clone()
                })
                .collect()
        })
        .collect()
}

fn styled_font_is_mono(value: &SlotValue) -> bool {
    match value {
        SlotValue::Styled(styles) => styles.font.as_deref().is_some_and(|font| {
            matches!(
                font.trim().to_ascii_lowercase().as_str(),
                "mono" | "monospace"
            )
        }),
        SlotValue::Text(_) => false,
    }
}

fn parse_size(value: &Value) -> SizeSpec {
    match value {
        Value::Number(n) => n
            .as_f64()
            .filter(|v| *v > 0.0)
            .map(|v| SizeSpec::Px(v as f32))
            .unwrap_or(SizeSpec::Auto),
        Value::String(s) if s.trim().eq_ignore_ascii_case("auto") => SizeSpec::Auto,
        Value::String(s) => s
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| *v > 0.0)
            .map(SizeSpec::Px)
            .unwrap_or(SizeSpec::Auto),
        _ => SizeSpec::Auto,
    }
}

/// Flatten a slot value for OpenDeck's own title rendering.
pub fn slot_plain_text(value: &SlotValue) -> String {
    match value {
        SlotValue::Text(text) => plain_of_lines(&parse_text(text, false)),
        SlotValue::Styled(styles) => {
            if let Some(spans) = styles.spans.as_ref().filter(|s| !s.is_empty()) {
                spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("")
            } else {
                plain_of_lines(&parse_text(styles.text.as_deref().unwrap_or(""), false))
            }
        }
    }
}

pub fn runs_have_style(lines: &[Vec<Run>]) -> bool {
    lines.iter().flatten().any(|run| {
        run.bold
            || run.italic
            || run.underline
            || run.mono
            || run.color.is_some()
            || run.bg.is_some()
            || (run.size - 1.0).abs() > f32::EPSILON
    })
}

/// A dial needs our renderer when a plain OpenDeck title cannot express the style.
pub fn needs_image(value: &SlotValue) -> bool {
    match value {
        SlotValue::Text(text) => runs_have_style(&parse_text(text, false)),
        SlotValue::Styled(styles) => {
            styles.color.is_some()
                || styles.bg.is_some()
                || styles.bar.is_some()
                || styles.size.is_some()
                || styles.font.is_some()
                || styles.align.is_some()
                || styles.valign.is_some()
                || styles.spans.as_ref().is_some_and(|s| !s.is_empty())
                || runs_have_style(&parse_text(styles.text.as_deref().unwrap_or(""), false))
        }
    }
}

/// Join four 200x100 slices and centre the result in the 800x128 strip.
///
/// AKP05 v0.12 preserves the dial aspect ratio, so the device shows the same
/// 14 px black bands above and below the picture.
pub fn continuous_preview(slices: &[RgbImage]) -> RgbImage {
    let mut content = RgbImage::new(800, 100);
    for (index, slice) in slices.iter().take(4).enumerate() {
        image::imageops::replace(&mut content, slice, (index * 200) as i64, 0);
    }
    let mut canvas = RgbImage::new(800, 128);
    image::imageops::replace(&mut canvas, &content, 0, 14);
    canvas
}

/// The 800x128 frame for raw mode: a single image is used as is, four 200x128
/// slices (the `@slices` form) are joined side by side. There are no bands.
pub fn raw_frame(images: &[RgbImage]) -> RgbImage {
    if let [single] = images {
        return single.clone();
    }
    let mut canvas = RgbImage::new(800, 128);
    for (index, slice) in images.iter().take(4).enumerate() {
        image::imageops::replace(&mut canvas, slice, (index * 200) as i64, 0);
    }
    canvas
}

/// Cut an 800x128 raw frame into the four 200x128 `@slices` images.
pub fn raw_slices(frame: &RgbImage) -> Vec<RgbImage> {
    (0..4u32)
        .map(|index| image::imageops::crop_imm(frame, index * 200, 0, 200, 128).to_image())
        .collect()
}

/// Compose tiles with the dead bands visible. `simulate` uses the physical
/// 800x128 strip and centres the 176x88 tiles, otherwise the logical canvas is tight.
pub fn tiles_preview(slices: &[RgbImage], gap: u32, simulate: bool) -> RgbImage {
    let tight_width = 176 * 4 + gap * 3;
    let (width, height, origin_x, origin_y) = if simulate {
        let origin_x = (800i64 - tight_width as i64).max(0) / 2;
        (800, 128, origin_x, (128i64 - 88) / 2)
    } else {
        (tight_width, 88, 0, 0)
    };
    let mut canvas = RgbImage::new(width, height);
    for (index, slice) in slices.iter().take(4).enumerate() {
        let x = origin_x + (index as u32 * (176 + gap)) as i64;
        image::imageops::replace(&mut canvas, slice, x, origin_y);
    }
    canvas
}

pub fn styled_value(text: impl Into<String>, color: Option<Rgb>, bg: Option<Rgb>) -> SlotValue {
    SlotValue::Styled(Styles {
        text: Some(text.into()),
        color: color.map(|c| c.to_string()),
        bg: bg.map(|c| c.to_string()),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::StripMode;

    fn strip_frame(uniform: bool, text: &str) -> image::RgbImage {
        strip_frame_with(uniform, false, text)
    }

    fn strip_frame_with(uniform: bool, skip_first: bool, text: &str) -> image::RgbImage {
        let mut r = Renderer::embedded();
        r.config.uniform_strip_size = uniform;
        r.config.uniform_skip_first = skip_first;
        let value = SlotValue::Text(text.to_owned());
        r.render_strip(&value, StripMode::Raw, 0, Detail::Full)
            .images
            .remove(0)
    }

    fn first_segment(frame: &image::RgbImage) -> Vec<u8> {
        image::imageops::crop_imm(frame, 0, 0, 200, 128)
            .to_image()
            .into_raw()
    }

    #[test]
    fn uniform_size_shrinks_short_segments_to_the_longest() {
        let text = "Hi|a considerably longer second segment of text";
        let independent = first_segment(&strip_frame(false, text));
        let uniform = first_segment(&strip_frame(true, text));
        assert_ne!(independent, uniform, "short segment keeps its own size");
        // Equal content needs equal sizes, so the switch changes nothing.
        let same = "Hi|Hi";
        assert_eq!(
            first_segment(&strip_frame(false, same)),
            first_segment(&strip_frame(true, same))
        );
    }

    #[test]
    fn uniform_size_can_leave_the_first_segment_alone() {
        let text = "Hi|a considerably longer second segment|another long second segment";
        let independent = first_segment(&strip_frame(false, text));
        let skipped = first_segment(&strip_frame_with(true, true, text));
        assert_eq!(independent, skipped, "segment 1 keeps its own size");
    }

    #[test]
    fn ruler_bands_run_inward_from_every_edge() {
        let frame = Renderer::embedded().raw_ruler_pattern();
        assert_eq!((frame.width(), frame.height()), (800, 128));
        let pixel = |x: u32, y: u32| {
            let p = frame.get_pixel(x, y);
            Rgb(p[0], p[1], p[2])
        };
        for (index, (name, color)) in RAW_RULER_BANDS.iter().enumerate() {
            let i = index as u32;
            // Rows: sample away from the corners and the centre text.
            for x in [100, 300, 500, 700] {
                assert_eq!(pixel(x, i), *color, "top band {name}");
                assert_eq!(pixel(x, 127 - i), *color, "bottom band {name}");
            }
            for y in [20, 108] {
                assert_eq!(pixel(i, y), *color, "left band {name}");
                assert_eq!(pixel(799 - i, y), *color, "right band {name}");
            }
        }
        // Top and bottom bands win in the corners.
        assert_eq!(pixel(0, 0), RAW_RULER_BANDS[0].1);
        assert_eq!(pixel(3, 1), RAW_RULER_BANDS[1].1);
        assert_eq!(pixel(799, 127), RAW_RULER_BANDS[0].1);
        // Inside the bands the background is grey.
        assert_eq!(pixel(20, 64), RAW_RULER_BACKGROUND);
    }

    #[test]
    fn plain_strings_render_and_slice() {
        let renderer = Renderer::embedded();
        let rendered = renderer.render_strip(
            &SlotValue::from_text("HELLO"),
            StripMode::Continuous,
            28,
            Detail::Full,
        );
        assert_eq!(rendered.images.len(), 4);
        for image in &rendered.images {
            assert_eq!(image.dimensions(), (200, 100));
        }
        assert!(
            rendered
                .images
                .iter()
                .any(|i| i.pixels().any(|p| p.0 != [0, 0, 0]))
        );
    }

    #[test]
    fn segments_do_not_cross_boundaries() {
        let renderer = Renderer::embedded();
        let rendered = renderer.render_strip(
            &SlotValue::from_text("LEFT|RIGHT||"),
            StripMode::Continuous,
            28,
            Detail::Full,
        );
        assert_eq!(rendered.images.len(), 4);
        // The first two segments carry ink, the empty ones stay blank.
        assert!(rendered.images[0].pixels().any(|p| p.0 != [0, 0, 0]));
        assert!(rendered.images[1].pixels().any(|p| p.0 != [0, 0, 0]));
        assert!(rendered.images[2].pixels().all(|p| p.0 == [0, 0, 0]));
        assert!(rendered.images[3].pixels().all(|p| p.0 == [0, 0, 0]));
        // Text is clipped to its own segment.
        for image in &rendered.images[..2] {
            for (x, _, pixel) in image.enumerate_pixels() {
                if x == 0 || x == 199 {
                    assert_eq!(pixel.0, [0, 0, 0]);
                }
            }
        }
    }

    #[test]
    fn object_styles_apply() {
        let renderer = Renderer::embedded();
        let value: SlotValue =
            serde_json::from_str(r##"{"text":"X","bg":"#101820","color":"#ff8800","bar":0.5}"##)
                .unwrap();
        assert!(needs_image(&value));
        let rendered = renderer.render_slot(&renderer.spec(&value), 200, 100, Detail::Full);
        assert_eq!(rendered.images[0].get_pixel(0, 0).0, [16, 24, 32]);
    }

    #[test]
    fn plain_text_flattens_markup() {
        let value = SlotValue::from_text("[b]BUILD[/b] [c=green]OK[/c]");
        assert_eq!(slot_plain_text(&value), "BUILD OK");
        let value = SlotValue::from_text("CPU [s=1.6]42[/s]%");
        assert_eq!(slot_plain_text(&value), "CPU 42%");
    }

    #[test]
    fn test_pattern_has_markers() {
        let renderer = Renderer::embedded();
        let rendered = renderer.test_pattern(StripMode::Continuous, 28, Detail::Full);
        assert_eq!(rendered.images.len(), 4);
        assert!(
            rendered
                .images
                .iter()
                .all(|i| i.pixels().any(|p| p.0 != [0, 0, 0]))
        );
    }

    #[test]
    fn mono_font_survives_segment_split() {
        let renderer = Renderer::embedded();
        let value: SlotValue = serde_json::from_str(r#"{"text":"A|B","font":"mono"}"#).unwrap();
        let segments = renderer.spec_segments(&value);
        assert_eq!(segments.len(), 2);
        for segment in &segments {
            assert!(
                segment.lines.iter().flatten().all(|run| run.mono),
                "every run must stay monospaced"
            );
        }
    }

    #[test]
    fn preview_geometry() {
        let slices: Vec<RgbImage> = (0..4).map(|_| RgbImage::new(200, 100)).collect();
        let preview = continuous_preview(&slices);
        assert_eq!(preview.dimensions(), (800, 128));
        let tiles: Vec<RgbImage> = (0..4).map(|_| RgbImage::new(176, 88)).collect();
        let preview = tiles_preview(&tiles, 28, false);
        assert_eq!(preview.dimensions(), (788, 88));
        let preview = tiles_preview(&tiles, 28, true);
        assert_eq!(preview.dimensions(), (800, 128));
    }
}
