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

/// Draw an unsplit strip value as tab-separated columns.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    pub gap_spaces: u32,
    pub mono: bool,
}

#[derive(Clone, Debug)]
pub struct LayoutOptions {
    pub min_size: f32,
    pub max_size: f32,
    pub line_height: f32,
    pub padding_horizontal: f32,
    pub padding_vertical: f32,
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
    pub grid: Option<Grid>,
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

fn inner_rect(rect: Rect, bar_height: f32, horizontal: f32, vertical: f32) -> Rect {
    Rect {
        x: rect.x + horizontal,
        y: rect.y + vertical,
        width: (rect.width - horizontal * 2.0).max(1.0),
        height: (rect.height - vertical * 2.0 - bar_height).max(1.0),
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
    let inner = inner_rect(
        rect,
        bar_height_for(rect, has_bar),
        options.padding_horizontal,
        options.padding_vertical,
    );
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
    let padding_vertical = request.options.padding_vertical;
    let bar_height = bar_height_for(rect, request.bar.is_some());
    let inner = inner_rect(
        rect,
        bar_height,
        request.options.padding_horizontal,
        padding_vertical,
    );

    let (size, lines, truncated) = match request.grid {
        Some(grid) => {
            let fitted = fit_grid(
                request.lines,
                request.base_size,
                &inner,
                grid,
                request.options,
                fonts,
            );
            let truncated = fitted.truncated;
            (fitted.size, fitted.lines(), truncated)
        }
        None => fit_lines(request, &inner, fonts),
    };

    paint_lines(canvas, rect, &inner, &lines, request, fonts);

    if let Some(fraction) = request.bar {
        let fraction = fraction.clamp(0.0, 1.0);
        let bar_y = rect.y + rect.height - padding_vertical - bar_height;
        let width = (rect.width - request.options.padding_horizontal * 2.0).max(0.0);
        canvas.fill_rect(
            rect.x + request.options.padding_horizontal,
            bar_y,
            width,
            bar_height,
            request.bar_bg,
        );
        canvas.fill_rect(
            rect.x + request.options.padding_horizontal,
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

/// Fit and truncate plain lines, then paint them into `inner`.
fn fit_lines(
    request: &DrawRequest<'_>,
    inner: &Rect,
    fonts: &Fonts,
) -> (f32, Vec<LineLayout>, bool) {
    let (size, mut lines, mut truncated) = fit(
        request.lines,
        request.base_size,
        inner,
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
    (size, lines, truncated)
}

fn paint_lines(
    canvas: &mut Canvas,
    rect: Rect,
    inner: &Rect,
    lines: &[LineLayout],
    request: &DrawRequest<'_>,
    fonts: &Fonts,
) {
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

    for line in lines {
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
}

/// Whether any run of any line holds a tab.
pub fn has_tab(lines: &[Vec<Run>]) -> bool {
    lines.iter().flatten().any(|run| run.text.contains('\t'))
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

fn keep_visible_rows(rows: Vec<GridRow>, height: f32) -> Vec<GridRow> {
    let mut out = Vec::new();
    let mut used = 0.0;
    for row in rows {
        if used + row.height > height + 0.5 && !out.is_empty() {
            break;
        }
        used += row.height;
        out.push(row);
    }
    out
}

#[derive(Clone, Debug)]
struct CellLayout {
    placements: Vec<PlacedRun>,
    width: f32,
    ascent: f32,
    descent: f32,
    max_size: f32,
    /// Plain text of the cell, for the lost-content check and truncation tests.
    text: String,
}

#[derive(Clone, Debug)]
struct GridRow {
    cells: Vec<CellLayout>,
    ascent: f32,
    descent: f32,
    height: f32,
}

struct FittedGrid {
    size: f32,
    rows: Vec<GridRow>,
    columns: Vec<f32>,
    gap: f32,
    truncated: bool,
}

impl FittedGrid {
    fn width(&self) -> f32 {
        columns_width(&self.columns, self.gap)
    }

    /// Rows with the cell placements shifted into their columns.
    fn lines(&self) -> Vec<LineLayout> {
        let width = self.width();
        self.rows
            .iter()
            .map(|row| {
                let mut placements = Vec::new();
                let mut x = 0.0;
                for (index, column_width) in self.columns.iter().enumerate() {
                    if let Some(cell) = row.cells.get(index) {
                        for placed in &cell.placements {
                            let mut placed = placed.clone();
                            placed.x += x;
                            placements.push(placed);
                        }
                    }
                    x += column_width + self.gap;
                }
                LineLayout {
                    placements,
                    width,
                    ascent: row.ascent,
                    descent: row.descent,
                    height: row.height,
                }
            })
            .collect()
    }
}

fn columns_width(columns: &[f32], gap: f32) -> f32 {
    let widths: f32 = columns.iter().sum();
    widths + gap * columns.len().saturating_sub(1) as f32
}

/// Split a line's runs at tabs into cells, keeping each run's style across a
/// tab. A line without a tab is one cell; an empty line is one empty cell.
fn split_cells(runs: &[Run]) -> Vec<Vec<Run>> {
    let mut cells: Vec<Vec<Run>> = Vec::new();
    let mut current: Vec<Run> = Vec::new();
    for run in runs {
        let mut pieces = run.text.split('\t').peekable();
        while let Some(piece) = pieces.next() {
            if !piece.is_empty() {
                let mut run = run.clone();
                run.text = piece.to_owned();
                push_merged(&mut current, run);
            }
            if pieces.peek().is_some() {
                cells.push(std::mem::take(&mut current));
            }
        }
    }
    cells.push(current);
    cells
}

fn split_cells_all(lines: &[Vec<Run>]) -> Vec<Vec<Vec<Run>>> {
    let mut rows: Vec<Vec<Vec<Run>>> = lines.iter().map(|line| split_cells(line)).collect();
    if rows.is_empty() {
        rows.push(vec![Vec::new()]);
    }
    rows
}

fn space_advance(size: f32, mono: bool, fonts: &Fonts) -> f32 {
    match fonts.resolve(mono, false, false, ' ') {
        GlyphSource::Glyph { face, id } => scaled(face, size, size).h_advance(id),
        GlyphSource::Placeholder { .. } => size * 0.6,
    }
}

fn grid_gap(gap_spaces: u32, size: f32, mono: bool, fonts: &Fonts) -> f32 {
    if gap_spaces == 0 {
        return 0.0;
    }
    gap_spaces as f32 * space_advance(size, mono, fonts)
}

fn measure_cell(runs: &[Run], size: f32, fonts: &Fonts) -> CellLayout {
    let measured = measure(runs, size, fonts);
    let text = measured
        .placements
        .iter()
        .map(|placed| placed.run.text.as_str())
        .collect();
    CellLayout {
        placements: measured.placements,
        width: measured.width,
        ascent: measured.ascent,
        descent: measured.descent,
        max_size: measured.max_size,
        text,
    }
}

fn measure_row(cells: &[Vec<Run>], size: f32, options: &LayoutOptions, fonts: &Fonts) -> GridRow {
    let cells: Vec<CellLayout> = cells
        .iter()
        .map(|cell| measure_cell(cell, size, fonts))
        .collect();
    let mut ascent = 0.0f32;
    let mut descent = 0.0f32;
    let mut max_size = 0.0f32;
    for cell in &cells {
        ascent = ascent.max(cell.ascent);
        descent = descent.min(cell.descent);
        max_size = max_size.max(cell.max_size);
    }
    let height = (max_size * options.line_height)
        .max(ascent - descent)
        .max(1.0);
    GridRow {
        cells,
        ascent,
        descent,
        height,
    }
}

fn build_grid(
    cells: &[Vec<Vec<Run>>],
    size: f32,
    grid: Grid,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> (Vec<GridRow>, f32) {
    let gap = grid_gap(grid.gap_spaces, size, grid.mono, fonts);
    let rows = cells
        .iter()
        .map(|row| measure_row(row, size, options, fonts))
        .collect();
    (rows, gap)
}

fn column_widths(rows: &[GridRow]) -> Vec<f32> {
    let count = rows.iter().map(|row| row.cells.len()).max().unwrap_or(1);
    (0..count)
        .map(|index| {
            rows.iter()
                .filter_map(|row| row.cells.get(index))
                .map(|cell| cell.width)
                .fold(0.0, f32::max)
        })
        .collect()
}

fn grid_fits(rows: &[GridRow], columns: &[f32], gap: f32, inner: &Rect) -> bool {
    let height: f32 = rows.iter().map(|row| row.height).sum();
    height <= inner.height + 0.5 && columns_width(columns, gap) <= inner.width + 0.5
}

fn fit_grid(
    source: &[Vec<Run>],
    base: SizeSpec,
    inner: &Rect,
    grid: Grid,
    options: &LayoutOptions,
    fonts: &Fonts,
) -> FittedGrid {
    let cells = split_cells_all(source);
    let (size, mut rows, gap, mut truncated) = match base {
        SizeSpec::Px(px) => {
            let min = options.min_size.max(1.0);
            let max = options.max_size.max(min);
            let size = px.clamp(min, max);
            let (rows, gap) = build_grid(&cells, size, grid, options, fonts);
            (size, rows, gap, false)
        }
        SizeSpec::Auto => {
            let mut lo = options.min_size.max(1.0);
            let mut hi = options.max_size.max(lo);
            let mut best: Option<(f32, Vec<GridRow>, f32)> = None;
            for _ in 0..24 {
                let mid = (lo + hi) * 0.5;
                let (rows, gap) = build_grid(&cells, mid, grid, options, fonts);
                let columns = column_widths(&rows);
                if grid_fits(&rows, &columns, gap, inner) {
                    best = Some((mid, rows, gap));
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            match best {
                Some((size, rows, gap)) => (size, rows, gap, false),
                None => {
                    let size = options.min_size.max(1.0);
                    let (rows, gap) = build_grid(&cells, size, grid, options, fonts);
                    (size, rows, gap, true)
                }
            }
        }
    };

    // Drop the bottom rows that do not fit, then measure columns from the
    // remaining rows so a short grid is not narrowed by a hidden row.
    let total_height: f32 = rows.iter().map(|row| row.height).sum();
    if total_height > inner.height + 0.5 {
        rows = keep_visible_rows(rows, inner.height);
        truncated = true;
    }
    let mut columns = column_widths(&rows);

    while columns_width(&columns, gap) > inner.width + 0.5 {
        let last = columns.len() - 1;
        if last == 0 {
            truncate_column(&mut rows, 0, size, inner.width, fonts, &mut truncated);
            columns = column_widths(&rows);
            if columns[0] > inner.width {
                columns[0] = inner.width;
            }
            break;
        }
        let before: f32 = columns[..last].iter().sum();
        let avail = inner.width - before - gap * last as f32;
        let needed = rows
            .iter()
            .filter_map(|row| {
                row.cells
                    .get(last)
                    .filter(|cell| cell.width > avail + 0.5)
                    .map(|cell| ellipsis_width(cell, size, fonts))
            })
            .fold(0.0, f32::max);
        if avail > 0.5 && needed <= avail + 0.5 {
            truncate_column(&mut rows, last, size, avail, fonts, &mut truncated);
            columns = column_widths(&rows);
            if columns[last] > avail {
                columns[last] = avail;
            }
            break;
        }
        drop_column(&mut rows, last, size, columns[last - 1], fonts);
        columns = column_widths(&rows);
        truncated = true;
    }

    FittedGrid {
        size,
        rows,
        columns,
        gap,
        truncated,
    }
}

fn cell_style(cell: &CellLayout) -> Run {
    cell.placements
        .last()
        .map(|placed| placed.run.clone())
        .unwrap_or_default()
}

fn ellipsis_width(cell: &CellLayout, size: f32, fonts: &Fonts) -> f32 {
    let run = Run {
        text: "…".to_owned(),
        ..cell_style(cell)
    };
    measure_width(std::slice::from_ref(&run), size, fonts)
}

fn cell_as_line(cell: &CellLayout) -> LineLayout {
    LineLayout {
        placements: cell.placements.clone(),
        width: cell.width,
        ascent: cell.ascent,
        descent: cell.descent,
        height: 0.0,
    }
}

fn cell_from_line(cell: &CellLayout, line: LineLayout) -> CellLayout {
    let text = line
        .placements
        .iter()
        .map(|placed| placed.run.text.as_str())
        .collect();
    CellLayout {
        placements: line.placements,
        width: line.width,
        text,
        ..cell.clone()
    }
}

fn empty_cell(size: f32, fonts: &Fonts) -> CellLayout {
    measure_cell(&[], size, fonts)
}

fn truncate_column(
    rows: &mut [GridRow],
    index: usize,
    size: f32,
    max_width: f32,
    fonts: &Fonts,
    truncated: &mut bool,
) {
    for row in rows {
        if let Some(cell) = row.cells.get_mut(index)
            && cell.width > max_width + 0.5
        {
            let line = truncate_line(&cell_as_line(cell), size, max_width, fonts);
            *cell = cell_from_line(cell, line);
            *truncated = true;
        }
    }
}

/// Drop `index` from every row. Rows that held content there get `…` appended
/// to the cell in the last kept column, narrowed to that column.
fn drop_column(rows: &mut [GridRow], index: usize, size: f32, keep_width: f32, fonts: &Fonts) {
    for row in rows {
        let lost_content = row
            .cells
            .get(index)
            .is_some_and(|cell| !cell.text.is_empty());
        row.cells.truncate(index);
        if !lost_content {
            continue;
        }
        while row.cells.len() < index {
            row.cells.push(empty_cell(size, fonts));
        }
        let cell = &row.cells[index - 1];
        let line = truncate_line_forced(&cell_as_line(cell), size, keep_width, fonts);
        let cell = cell_from_line(cell, line);
        row.cells[index - 1] = cell;
    }
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
    truncate_line_impl(line, size, max_width, fonts, false)
}

/// Like `truncate_line`, but always ends in `…`, even a cell that fits.
fn truncate_line_forced(line: &LineLayout, size: f32, max_width: f32, fonts: &Fonts) -> LineLayout {
    truncate_line_impl(line, size, max_width, fonts, true)
}

fn truncate_line_impl(
    line: &LineLayout,
    size: f32,
    max_width: f32,
    fonts: &Fonts,
    force: bool,
) -> LineLayout {
    if !force && line.width <= max_width + 0.5 {
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
        padding_horizontal: 0.0,
        padding_vertical: 0.0,
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
            padding_horizontal: 2.0,
            padding_vertical: 2.0,
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
            grid: None,
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
            grid: None,
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
            grid: None,
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

    const RED: Rgb = Rgb(255, 0, 0);

    fn red(text: &str) -> Run {
        Run {
            text: text.to_owned(),
            color: Some(RED),
            ..Default::default()
        }
    }

    fn draw_grid(
        lines: &[Vec<Run>],
        w: u32,
        h: u32,
        align: Align,
        size: SizeSpec,
        gap_spaces: u32,
        mono: bool,
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
            grid: Some(Grid { gap_spaces, mono }),
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

    /// Bounds of the red cell; glyph interiors are not always fully opaque.
    fn red_bounds(canvas: &Canvas) -> Option<(i32, i32, i32, i32)> {
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        for y in 0..canvas.height as i32 {
            for x in 0..canvas.width as i32 {
                let reddish = canvas
                    .get(x, y)
                    .is_some_and(|p| p.0 > 150 && p.1 < 100 && p.2 < 100);
                if reddish {
                    let b = bounds.get_or_insert((x, y, x, y));
                    b.0 = b.0.min(x);
                    b.1 = b.1.min(y);
                    b.2 = b.2.max(x);
                    b.3 = b.3.max(y);
                }
            }
        }
        bounds
    }

    fn red_x(canvas: &Canvas) -> i32 {
        red_bounds(canvas).expect("the red cell is drawn").0
    }

    fn cell_texts(
        lines: &[Vec<Run>],
        w: f32,
        h: f32,
        size: SizeSpec,
        gap_spaces: u32,
        mono: bool,
    ) -> (Vec<Vec<String>>, bool) {
        let opts = options();
        let inner = inner_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                width: w,
                height: h,
            },
            0.0,
            opts.padding_horizontal,
            opts.padding_vertical,
        );
        let fitted = fit_grid(
            lines,
            size,
            &inner,
            Grid { gap_spaces, mono },
            &opts,
            Fonts::embedded(),
        );
        let texts = fitted
            .rows
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.text.clone()).collect())
            .collect();
        (texts, fitted.truncated)
    }

    fn same_image(a: &Canvas, b: &Canvas) -> bool {
        a.width == b.width
            && a.height == b.height
            && (0..a.height as i32).all(|y| (0..a.width as i32).all(|x| a.get(x, y) == b.get(x, y)))
    }

    #[test]
    fn tab_without_a_grid_still_draws_four_spaces() {
        let with_tab = vec![vec![Run::plain("\ta")]];
        let with_spaces = vec![vec![Run::plain("    a")]];
        let (a, _) = draw(&with_tab, 200, 60, Align::Left, SizeSpec::Px(20.0));
        let (b, _) = draw(&with_spaces, 200, 60, Align::Left, SizeSpec::Px(20.0));
        assert!(same_image(&a, &b));
    }

    #[test]
    fn has_tab_finds_tabs_in_any_run() {
        assert!(has_tab(&[vec![Run::plain("a"), Run::plain("\tb")]]));
        assert!(!has_tab(&[vec![Run::plain("a"), Run::plain("b")]]));
        assert!(!has_tab(&[]));
    }

    #[test]
    fn split_keeps_style_across_a_tab() {
        let cells = split_cells(&[Run {
            text: "a\tb".to_owned(),
            bold: true,
            ..Default::default()
        }]);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0][0].text, "a");
        assert_eq!(cells[1][0].text, "b");
        assert!(cells.iter().all(|cell| cell[0].bold));
    }

    #[test]
    fn empty_lines_and_leading_or_trailing_tabs_keep_their_cells() {
        let rows = vec![
            vec![Run::plain("\ta")],
            vec![Run::plain("a\t")],
            vec![],
            vec![Run::plain("a\tb\tc")],
        ];
        let (cells, _) = cell_texts(&rows, 400.0, 200.0, SizeSpec::Px(20.0), 2, false);
        assert_eq!(cells[0], vec!["", "a"]);
        assert_eq!(cells[1], vec!["a", ""]);
        assert_eq!(cells[2], vec![""]);
        assert_eq!(cells[3], vec!["a", "b", "c"]);
    }

    #[test]
    fn a_line_without_tabs_is_one_cell() {
        let rows = vec![
            vec![Run::plain("a much wider first cell")],
            vec![Run::plain("x\tY")],
        ];
        let (cells, _) = cell_texts(&rows, 400.0, 100.0, SizeSpec::Px(20.0), 2, false);
        assert_eq!(cells[0], vec!["a much wider first cell"]);
        assert_eq!(cells[1], vec!["x", "Y"]);
    }

    #[test]
    fn second_column_starts_at_the_same_x_for_every_row() {
        let short = vec![vec![Run::plain("i\t"), red("X")]];
        let long = vec![vec![Run::plain("wide first cell\t"), red("X")]];
        let both = vec![
            vec![Run::plain("wide first cell\t"), red("X")],
            vec![Run::plain("i\t"), red("X")],
        ];
        let (only_short, _) =
            draw_grid(&short, 500, 100, Align::Left, SizeSpec::Px(20.0), 2, false);
        let (only_long, _) = draw_grid(&long, 500, 100, Align::Left, SizeSpec::Px(20.0), 2, false);
        let (combined, _) = draw_grid(&both, 500, 100, Align::Left, SizeSpec::Px(20.0), 2, false);
        assert!(red_x(&only_long) > red_x(&only_short));
        assert!((red_x(&combined) - red_x(&only_long)).abs() <= 1);
    }

    #[test]
    fn a_tabless_line_widens_column_one() {
        let tabbed = vec![vec![Run::plain("i\t"), red("X")]];
        let titled = vec![
            vec![Run::plain("a wide title")],
            vec![Run::plain("i\t"), red("X")],
        ];
        let (without, _) = draw_grid(&tabbed, 500, 100, Align::Left, SizeSpec::Px(20.0), 2, false);
        let (with, _) = draw_grid(&titled, 500, 100, Align::Left, SizeSpec::Px(20.0), 2, false);
        assert!(red_x(&with) > red_x(&without));
    }

    #[test]
    fn gap_spaces_move_the_second_column() {
        let fonts = Fonts::embedded();
        let advance = space_advance(20.0, false, fonts);
        let rows = || vec![vec![Run::plain("ii\t"), red("X")]];
        let (none, _) = draw_grid(&rows(), 500, 60, Align::Left, SizeSpec::Px(20.0), 0, false);
        let (two, _) = draw_grid(&rows(), 500, 60, Align::Left, SizeSpec::Px(20.0), 2, false);
        let (sixteen, _) = draw_grid(&rows(), 500, 60, Align::Left, SizeSpec::Px(20.0), 16, false);
        let moved_two = (red_x(&two) - red_x(&none)) as f32;
        let moved_sixteen = (red_x(&sixteen) - red_x(&none)) as f32;
        assert!((moved_two - 2.0 * advance).abs() <= 1.5, "{moved_two}");
        assert!(
            (moved_sixteen - 16.0 * advance).abs() <= 1.5,
            "{moved_sixteen}"
        );
    }

    #[test]
    fn mono_gap_differs_from_the_proportional_face() {
        let fonts = Fonts::embedded();
        let rows = || vec![vec![Run::plain("ii\t"), red("X")]];
        let (sans, _) = draw_grid(&rows(), 500, 60, Align::Left, SizeSpec::Px(20.0), 4, false);
        let (mono, _) = draw_grid(&rows(), 500, 60, Align::Left, SizeSpec::Px(20.0), 4, true);
        let expected = 4.0 * (space_advance(20.0, true, fonts) - space_advance(20.0, false, fonts));
        let actual = (red_x(&mono) - red_x(&sans)) as f32;
        assert!((actual - expected).abs() <= 1.5, "{actual} vs {expected}");
    }

    #[test]
    fn alignment_moves_the_grid_as_a_block() {
        let rows = vec![vec![Run::plain("aa\t"), red("X")]];
        let (left, _) = draw_grid(&rows, 400, 60, Align::Left, SizeSpec::Px(20.0), 2, false);
        let (center, _) = draw_grid(&rows, 400, 60, Align::Center, SizeSpec::Px(20.0), 2, false);
        let (right, _) = draw_grid(&rows, 400, 60, Align::Right, SizeSpec::Px(20.0), 2, false);
        assert!(red_x(&left) < red_x(&center));
        assert!(red_x(&center) < red_x(&right));
        assert!(red_bounds(&right).unwrap().2 < 400);
    }

    #[test]
    fn auto_size_shrinks_with_more_columns_and_stays_in_bounds() {
        let mut previous = f32::MAX;
        for columns in 1..=8 {
            let text = vec!["abc"; columns].join("\t");
            let rows = vec![vec![Run::plain(text)]];
            let (canvas, result) =
                draw_grid(&rows, 400, 128, Align::Left, SizeSpec::Auto, 2, false);
            assert!(
                result.font_size <= previous + 0.01,
                "{columns} columns: {} > {previous}",
                result.font_size
            );
            previous = result.font_size;
            let (_, _, max_x, _) = bounds(&canvas);
            assert!(max_x < 400, "{columns} columns overflow: {max_x}");
        }
        assert!(previous < 64.0);
    }

    #[test]
    fn a_slightly_too_wide_last_column_is_ellipsized_not_dropped() {
        let size = 20.0;
        let fonts = Fonts::embedded();
        let first = measure_width(&[Run::plain("aaaa")], size, fonts);
        let second = measure_width(&[Run::plain("bbbbbbbbbbbb")], size, fonts);
        let gap = 2.0 * space_advance(size, false, fonts);
        let ellipsis = measure_width(&[Run::plain("…")], size, fonts);
        let inner = first + gap + ellipsis + 6.0;
        assert!(ellipsis + 6.0 < second);
        let rows = vec![vec![Run::plain("aaaa\tbbbbbbbbbbbb")]];
        let (cells, truncated) = cell_texts(&rows, inner + 4.0, 60.0, SizeSpec::Px(size), 2, false);
        assert!(truncated);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].len(), 2);
        assert_eq!(cells[0][0], "aaaa");
        assert!(cells[0][1].ends_with('…'), "{:?}", cells[0][1]);
        assert!(cells[0][1].len() < "bbbbbbbbbbbb".len());
    }

    #[test]
    fn a_too_wide_first_column_drops_the_others_and_ellipsizes() {
        let rows = vec![vec![Run::plain("aaaaaaaaaaaaaaaa\tb")]];
        let (cells, truncated) = cell_texts(&rows, 34.0, 60.0, SizeSpec::Px(20.0), 2, false);
        assert!(truncated);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].len(), 1);
        assert!(cells[0][0].ends_with('…'), "{:?}", cells[0][0]);
    }

    #[test]
    fn a_dropped_column_ellipsizes_even_an_empty_last_kept_cell() {
        let size = 20.0;
        let fonts = Fonts::embedded();
        let first = measure_width(&[Run::plain("dd")], size, fonts);
        let second = measure_width(&[Run::plain("bb")], size, fonts);
        let gap = 2.0 * space_advance(size, false, fonts);
        let ellipsis = measure_width(&[Run::plain("…")], size, fonts);
        let avail = (ellipsis - 2.0).max(0.5);
        let inner = first + second + 2.0 * gap + avail;
        let rows = vec![vec![Run::plain("dd\tbb\tcc")], vec![Run::plain("dd\t\tcc")]];
        let (cells, truncated) =
            cell_texts(&rows, inner + 4.0, 100.0, SizeSpec::Px(size), 2, false);
        assert!(truncated);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].len(), 2, "{cells:?}");
        assert_eq!(cells[1][1], "…", "{cells:?}");
        assert!(cells[0][1].ends_with('…'), "{cells:?}");
    }

    #[test]
    fn rows_that_do_not_fit_vertically_are_dropped_from_the_bottom() {
        let rows: Vec<Vec<Run>> = (0..10).map(|_| vec![Run::plain("x")]).collect();
        let (cells, truncated) = cell_texts(&rows, 100.0, 30.0, SizeSpec::Px(20.0), 2, false);
        assert!(truncated);
        assert!(cells.len() < 10, "{}", cells.len());
        assert!(!cells.is_empty());
    }

    #[test]
    fn a_grid_of_one_row_and_column_still_draws() {
        let rows = vec![vec![Run::plain("hello")]];
        let (canvas, result) = draw_grid(&rows, 200, 60, Align::Left, SizeSpec::Px(20.0), 2, false);
        assert!(!result.truncated);
        assert!(bounds(&canvas).2 > 0);
    }
}
