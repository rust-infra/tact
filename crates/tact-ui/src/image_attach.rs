//! Downscale and re-encode user-attached images before sending to vision models.

use std::io::Cursor;

use image::{DynamicImage, GenericImageView, codecs::jpeg::JpegEncoder, imageops::FilterType};
use tact::config::VisionImageSettings;

pub struct PreparedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

/// Prepare bytes for a vision attachment using installed config.
pub fn prepare_image_attachment(
    bytes: &[u8],
    ext: &str,
    settings: &VisionImageSettings,
) -> Option<PreparedImage> {
    if settings.compress {
        prepare_image_for_vision(bytes, settings.max_edge, settings.jpeg_quality)
    } else {
        prepare_raw_image(bytes, ext)
    }
}

/// Decode, optionally downscale, and re-encode as JPEG to cut vision token usage.
pub fn prepare_image_for_vision(
    bytes: &[u8],
    max_edge: u32,
    jpeg_quality: u8,
) -> Option<PreparedImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = downscale_if_needed(img, max_edge);
    encode_jpeg(&img, jpeg_quality)
}

/// Attach original file bytes without re-encoding (higher vision token cost).
pub fn prepare_raw_image(bytes: &[u8], ext: &str) -> Option<PreparedImage> {
    let media_type = media_type_from_ext(ext)?.to_string();
    Some(PreparedImage {
        bytes: bytes.to_vec(),
        media_type,
    })
}

fn downscale_if_needed(img: DynamicImage, max_edge: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let long_edge = w.max(h);
    if long_edge <= max_edge {
        return img;
    }
    let scale = max_edge as f32 / long_edge as f32;
    let new_w = ((w as f32 * scale).round() as u32).max(1);
    let new_h = ((h as f32 * scale).round() as u32).max(1);
    img.resize(new_w, new_h, FilterType::Triangle)
}

fn encode_jpeg(img: &DynamicImage, jpeg_quality: u8) -> Option<PreparedImage> {
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let mut cursor = Cursor::new(&mut out);
    let mut encoder = JpegEncoder::new_with_quality(&mut cursor, jpeg_quality);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(PreparedImage {
        bytes: out,
        media_type: "image/jpeg".to_string(),
    })
}

fn media_type_from_ext(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tiff" | "tif" => Some("image/tiff"),
        _ => None,
    }
}

/// Whether the file extension is a supported raster image type.
pub fn is_supported_image_ext(ext: &str) -> bool {
    media_type_from_ext(ext).is_some()
}

/// Read vision image settings from installed tact config.
pub fn vision_settings_from_config() -> VisionImageSettings {
    tact::config::settings().ui.vision_image
}

#[cfg(test)]
mod tests {
    use image::{ImageBuffer, ImageFormat, Rgb};

    use super::*;

    fn solid_png(w: u32, h: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
            ImageBuffer::from_fn(w, h, |_, _| Rgb([120, 80, 200]));
        let mut bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn default_settings() -> VisionImageSettings {
        VisionImageSettings {
            compress: true,
            max_edge: VisionImageSettings::DEFAULT_MAX_EDGE,
            jpeg_quality: VisionImageSettings::DEFAULT_JPEG_QUALITY,
        }
    }

    #[test]
    fn downscales_large_image() {
        let raw = solid_png(3000, 2000);
        let prepared = prepare_image_for_vision(
            &raw,
            VisionImageSettings::DEFAULT_MAX_EDGE,
            VisionImageSettings::DEFAULT_JPEG_QUALITY,
        )
        .expect("valid png");
        assert_eq!(prepared.media_type, "image/jpeg");
        let decoded = image::load_from_memory(&prepared.bytes).unwrap();
        assert!(decoded.width() <= VisionImageSettings::DEFAULT_MAX_EDGE);
        assert!(decoded.height() <= VisionImageSettings::DEFAULT_MAX_EDGE);
        assert!(prepared.bytes.len() < raw.len());
    }

    #[test]
    fn raw_path_preserves_png_bytes() {
        let raw = solid_png(400, 300);
        let prepared = prepare_raw_image(&raw, "png").expect("png");
        assert_eq!(prepared.media_type, "image/png");
        assert_eq!(prepared.bytes, raw);
    }

    #[test]
    fn compress_off_skips_reencode() {
        let raw = solid_png(3000, 2000);
        let settings = VisionImageSettings {
            compress: false,
            ..default_settings()
        };
        let prepared = prepare_image_attachment(&raw, "png", &settings).expect("png");
        assert_eq!(prepared.media_type, "image/png");
        assert_eq!(prepared.bytes, raw);
    }

    #[test]
    fn compress_on_uses_jpeg() {
        let raw = solid_png(3000, 2000);
        let prepared = prepare_image_attachment(&raw, "png", &default_settings()).expect("png");
        assert_eq!(prepared.media_type, "image/jpeg");
    }

    #[test]
    fn respects_custom_max_edge() {
        let raw = solid_png(2000, 1000);
        let prepared = prepare_image_for_vision(&raw, 640, 70).expect("valid png");
        let decoded = image::load_from_memory(&prepared.bytes).unwrap();
        assert!(decoded.width() <= 640);
        assert!(decoded.height() <= 640);
    }

    #[test]
    fn keeps_small_image_under_cap() {
        let raw = solid_png(400, 300);
        let prepared = prepare_image_for_vision(&raw, 1280, 80).expect("valid png");
        let decoded = image::load_from_memory(&prepared.bytes).unwrap();
        assert_eq!(decoded.width(), 400);
        assert_eq!(decoded.height(), 300);
    }

    #[test]
    fn rejects_invalid_bytes() {
        assert!(prepare_image_for_vision(b"not-an-image", 1280, 80).is_none());
    }
}
