use ab_glyph::{Font, FontArc, Glyph, GlyphId, PxScale, ScaleFont, point};
use image::RgbImage;

use crate::color::{Rgb, blend_linear};

use super::fonts::Face;

pub struct Canvas {
    pub width: u32,
    pub height: u32,
    data: Vec<u8>,
    /// Inclusive-exclusive (x0, y0, x1, y1) output clip.
    clip: Option<(i32, i32, i32, i32)>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, background: Rgb) -> Self {
        let mut data = vec![0u8; width as usize * height as usize * 3];
        for pixel in data.as_chunks_mut::<3>().0 {
            *pixel = [background.0, background.1, background.2];
        }
        Self {
            width,
            height,
            data,
            clip: None,
        }
    }

    pub fn from_image(image: &RgbImage) -> Self {
        Self {
            width: image.width(),
            height: image.height(),
            data: image.as_raw().clone(),
            clip: None,
        }
    }

    pub fn set_clip(&mut self, clip: Option<(i32, i32, i32, i32)>) {
        self.clip = clip;
    }

    pub fn clear_clip(&mut self) {
        self.clip = None;
    }

    pub fn into_image(self) -> RgbImage {
        RgbImage::from_raw(self.width, self.height, self.data).expect("canvas dimensions match")
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Rgb> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        Some(Rgb(self.data[i], self.data[i + 1], self.data[i + 2]))
    }

    pub fn put(&mut self, x: i32, y: i32, color: Rgb) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if let Some((x0, y0, x1, y1)) = self.clip
            && (x < x0 || y < y0 || x >= x1 || y >= y1)
        {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        self.data[i] = color.0;
        self.data[i + 1] = color.1;
        self.data[i + 2] = color.2;
    }

    pub fn blend(&mut self, x: i32, y: i32, color: Rgb, coverage: f32) {
        if coverage <= 0.0 {
            return;
        }
        if let Some(dst) = self.get(x, y) {
            self.put(x, y, blend_linear(dst, color, coverage));
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, width: f32, height: f32, color: Rgb) {
        let x0 = x.round() as i32;
        let y0 = y.round() as i32;
        let x1 = (x + width).round() as i32;
        let y1 = (y + height).round() as i32;
        for py in y0.max(0)..y1.min(self.height as i32) {
            for px in x0.max(0)..x1.min(self.width as i32) {
                self.put(px, py, color);
            }
        }
    }

    pub fn overlay(&mut self, src: &RgbImage, x: i32, y: i32) {
        for (sx, sy, pixel) in src.enumerate_pixels() {
            let dst_x = x + sx as i32;
            let dst_y = y + sy as i32;
            self.put(dst_x, dst_y, Rgb(pixel[0], pixel[1], pixel[2]));
        }
    }

    /// Draw one glyph with non-uniform scale. `baseline_y` is in output pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_glyph(
        &mut self,
        face: &Face,
        id: GlyphId,
        size_x: f32,
        size_y: f32,
        origin_x: f32,
        baseline_y: f32,
        color: Rgb,
    ) {
        let offsets: &[f32] = if face.synthetic_bold && size_x >= 8.0 {
            &[-0.2, 0.2]
        } else {
            &[0.0]
        };
        let shear = if face.synthetic_italic { 0.2126 } else { 0.0 };
        for offset in offsets {
            self.draw_glyph_once(
                &face.font,
                id,
                size_x,
                size_y,
                origin_x + offset,
                baseline_y,
                color,
                shear,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_glyph_once(
        &mut self,
        font: &FontArc,
        id: GlyphId,
        size_x: f32,
        size_y: f32,
        origin_x: f32,
        baseline_y: f32,
        color: Rgb,
        shear: f32,
    ) {
        let scaled = font.as_scaled(PxScale {
            x: size_x,
            y: size_y,
        });
        let glyph: Glyph = id.with_scale_and_position(
            PxScale {
                x: size_x,
                y: size_y,
            },
            point(origin_x, baseline_y),
        );
        let Some(outlined) = scaled.outline_glyph(glyph) else {
            return;
        };
        let bounds = outlined.px_bounds();
        let base_x = bounds.min.x.round() as i32;
        let base_y = bounds.min.y.round() as i32;
        outlined.draw(|gx, gy, coverage| {
            let mut px = base_x + gx as i32;
            let py = base_y + gy as i32;
            if shear != 0.0 {
                px += (shear * (baseline_y - py as f32)).round() as i32;
            }
            self.blend(px, py, color, coverage);
        });
    }

    /// A visible box for glyphs no embedded font can draw.
    pub fn draw_placeholder(
        &mut self,
        face: &Face,
        size_x: f32,
        size_y: f32,
        origin_x: f32,
        baseline_y: f32,
        color: Rgb,
    ) {
        let scaled = face.font.as_scaled(PxScale {
            x: size_x,
            y: size_y,
        });
        let ascent = scaled.ascent();
        let stroke = (size_x * 0.08).max(1.0);
        let left = origin_x.round() + size_x * 0.08;
        let right = left + size_x * 0.5;
        let top = baseline_y.round() - ascent * 0.72;
        let bottom = baseline_y.round() - stroke;
        for px in left as i32..=right as i32 {
            self.put(px, top as i32, color);
            self.put(px, top as i32 + stroke as i32, color);
            self.put(px, bottom as i32, color);
            self.put(px, bottom as i32 - stroke as i32, color);
        }
        for py in top as i32..=bottom as i32 {
            self.put(left as i32, py, color);
            self.put(right as i32, py, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_and_bounds_are_clipped() {
        let mut canvas = Canvas::new(10, 10, Rgb::BLACK);
        canvas.fill_rect(-5.0, -5.0, 8.0, 8.0, Rgb::WHITE);
        assert_eq!(canvas.get(0, 0), Some(Rgb::WHITE));
        assert_eq!(canvas.get(3, 3), Some(Rgb::BLACK));
        canvas.put(-1, -1, Rgb::WHITE);
        assert_eq!(canvas.into_image().width(), 10);
    }
}
