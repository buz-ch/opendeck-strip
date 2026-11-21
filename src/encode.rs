use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{
    DynamicImage, ExtendedColorType, ImageEncoder, RgbImage, load_from_memory_with_format,
};

/// OpenDeck's own JPEG quality before it hands encoder images to the device plugin.
pub const OPENDECK_JPEG_QUALITY: u8 = 75;
/// `mirajazz`'s quality for the joined 800x128 strip image.
pub const DEVICE_JPEG_QUALITY: u8 = 90;

#[derive(Clone, Debug)]
pub struct Estimate {
    pub bytes: usize,
    pub jpeg: Vec<u8>,
}

pub fn png_bytes(image: &RgbImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    PngEncoder::new(&mut buf)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .context("failed to encode PNG")?;
    Ok(buf)
}

pub fn png_data_url(image: &RgbImage) -> Result<String> {
    Ok(format!(
        "data:image/png;base64,{}",
        STANDARD.encode(png_bytes(image)?)
    ))
}

pub fn decode_png(bytes: &[u8]) -> Result<RgbImage> {
    Ok(
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .context("failed to decode PNG")?
            .to_rgb8(),
    )
}

pub fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality)
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .context("failed to encode JPEG")?;
    Ok(buf)
}

/// The full pipeline from our four 200x100 slices to the device's 800x128 JPEG:
///
/// 1. OpenDeck renders the 200x100 dial layout and encodes it at quality 75,
///    4:4:4. The `image` 0.25 `JpegEncoder` sets every sampling factor to 1;
///    its doc comment says 4:2:2 but the code is 4:4:4.
/// 2. AKP05 v0.12 `fit_into` centres each 200x100 image in its 200x128 quarter
///    (14 px black bands) and joins the four quarters.
/// 3. `mirajazz` resizes 800x128 (Lanczos3, a no-op), rotates 180 and encodes
///    at quality 90.
pub fn estimate_strip(slices: &[RgbImage]) -> Result<Estimate> {
    if slices.len() != 4 {
        bail!("the strip needs exactly four slices, got {}", slices.len());
    }
    for slice in slices {
        if slice.dimensions() != (200, 100) {
            bail!(
                "strip slices must be 200x100, got {}x{}",
                slice.width(),
                slice.height()
            );
        }
    }

    let mut canvas = RgbImage::new(800, 128);
    for (index, slice) in slices.iter().enumerate() {
        let jpeg = encode_jpeg(slice, OPENDECK_JPEG_QUALITY)?;
        let decoded = load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg)
            .context("failed to decode OpenDeck JPEG")?
            .to_rgb8();
        // v0.12 keeps the 200x100 aspect and centres it in the 200x128 quarter.
        let mut tile = RgbImage::new(200, 128);
        image::imageops::replace(&mut tile, &decoded, 0, 14);
        image::imageops::replace(&mut canvas, &tile, (index * 200) as i64, 0);
    }

    let resized = DynamicImage::ImageRgb8(canvas)
        .resize_exact(800, 128, FilterType::Lanczos3)
        .to_rgb8();
    let rotated = image::imageops::rotate180(&resized);
    let jpeg = encode_jpeg(&rotated, DEVICE_JPEG_QUALITY)?;
    Ok(Estimate {
        bytes: jpeg.len(),
        jpeg,
    })
}

/// The device JPEG for a raw 800x128 frame: AKP05 rotates it 180 degrees and
/// `mirajazz` encodes it at quality 90. Nothing else touches the pixels.
pub fn estimate_raw(frame: &RgbImage) -> Result<Estimate> {
    if frame.dimensions() != (800, 128) {
        bail!(
            "a raw frame must be 800x128, got {}x{}",
            frame.width(),
            frame.height()
        );
    }
    let rotated = image::imageops::rotate180(frame);
    let jpeg = encode_jpeg(&rotated, DEVICE_JPEG_QUALITY)?;
    Ok(Estimate {
        bytes: jpeg.len(),
        jpeg,
    })
}

pub fn blank_floor() -> Result<usize> {
    let slices: Vec<RgbImage> = (0..4).map(|_| RgbImage::new(200, 100)).collect();
    Ok(estimate_strip(&slices)?.bytes)
}

pub fn jpeg_data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", STANDARD.encode(jpeg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn estimate_matches_the_final_encode() {
        let mut slices: Vec<RgbImage> = (0..4).map(|_| RgbImage::new(200, 100)).collect();
        for (index, slice) in slices.iter_mut().enumerate() {
            for (x, y, pixel) in slice.enumerate_pixels_mut() {
                let v = ((x * 7 + y * 13 + index as u32 * 31) % 256) as u8;
                *pixel = Rgb([v, v / 2, 255 - v]);
            }
        }
        let estimate = estimate_strip(&slices).unwrap();
        assert_eq!(estimate.bytes, estimate.jpeg.len());
        assert!(estimate.bytes > 5000, "{}", estimate.bytes);
        let decoded = image::load_from_memory_with_format(&estimate.jpeg, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8();
        assert_eq!(decoded.dimensions(), (800, 128));
        let mut band_sum = 0u64;
        for y in 0..12 {
            for x in 0..800 {
                let [r, g, b] = decoded.get_pixel(x, y).0;
                band_sum += u64::from(r) + u64::from(g) + u64::from(b);
            }
        }
        let band_mean = band_sum as f64 / (12.0 * 800.0 * 3.0);
        assert!(band_mean < 4.0, "letterbox band mean {band_mean}");
    }

    #[test]
    fn blank_floor_is_about_the_expected_size() {
        let floor = blank_floor().unwrap();
        assert!(floor > 1000, "{floor}");
        assert!(floor < 8000, "{floor}");
    }

    #[test]
    fn noise_is_larger_than_blank() {
        let mut slices: Vec<RgbImage> = (0..4).map(|_| RgbImage::new(200, 100)).collect();
        for slice in &mut slices {
            for (x, y, pixel) in slice.enumerate_pixels_mut() {
                let v = ((x * 73 + y * 151) % 256) as u8;
                *pixel = Rgb([v, v.wrapping_mul(3), v.wrapping_add(77)]);
            }
        }
        assert!(estimate_strip(&slices).unwrap().bytes > blank_floor().unwrap());
    }

    #[test]
    fn rejects_wrong_slice_count_and_size() {
        assert!(estimate_strip(&[]).is_err());
        assert!(estimate_strip(&[RgbImage::new(200, 100)]).is_err());
        assert!(
            estimate_strip(&(0..4).map(|_| RgbImage::new(10, 10)).collect::<Vec<_>>()).is_err()
        );
    }

    #[test]
    fn raw_estimate_needs_exactly_800x128() {
        assert!(estimate_raw(&RgbImage::new(800, 100)).is_err());
        let blank = estimate_raw(&RgbImage::new(800, 128)).unwrap();
        assert_eq!(blank.bytes, blank.jpeg.len());
        let decoded =
            image::load_from_memory_with_format(&blank.jpeg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!(decoded.width(), 800);
        assert_eq!(decoded.height(), 128);
    }

    #[test]
    fn png_roundtrip() {
        let image = RgbImage::from_pixel(4, 3, Rgb([1, 2, 3]));
        let bytes = png_bytes(&image).unwrap();
        assert_eq!(decode_png(&bytes).unwrap().dimensions(), (4, 3));
        assert!(
            png_data_url(&image)
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}
