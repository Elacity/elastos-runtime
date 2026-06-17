//! Raster-image renderer — single-page pixel-lock for `image/*` assets.
//!
//! A protected image is decoded INSIDE the decrypt boundary, downscaled to a bounded width,
//! buyer-watermarked, and re-encoded to JPEG. The browser therefore receives a flattened,
//! stamped image — never the original file (so EXIF/GPS/source metadata is stripped and the
//! source bytes never egress). One "page" (page 0).
//!
//! SVG is deliberately NOT handled here (it is XML, not a raster format, and carries a script/
//! XSS surface) — `is_pixel_lock` excludes it so it fails closed pending a vector rasteriser.

use super::watermark;

/// A decoded image held warm for the session (decoded once; each render is a downscale +
/// watermark + encode). Owns its pixels, so the plaintext lives only inside this sandbox.
pub struct ParsedImage {
    img: image::RgbaImage,
}

/// Decode the already-decrypted image bytes once. Fails closed (no plaintext echoed) on a
/// format we cannot decode or a zero-dimension image.
pub fn parse(object: &[u8]) -> Result<ParsedImage, String> {
    let decoded = image::load_from_memory(object).map_err(|e| format!("image decode: {e}"))?;
    let img = decoded.to_rgba8();
    if img.width() == 0 || img.height() == 0 {
        return Err("image has zero dimensions".to_string());
    }
    Ok(ParsedImage { img })
}

impl ParsedImage {
    /// Single page; any page index other than 0 fails closed.
    pub fn total_pages(&self) -> u32 {
        1
    }

    pub fn render_page(
        &self,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        if page != 0 {
            return Err(format!(
                "image asset has a single page (requested {})",
                page + 1
            ));
        }
        let img = watermark::fit_width(self.img.clone(), max_width.or(Some(1600)));
        watermark::finalize(img, watermark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 RGBA PNG, encoded in-memory, so the test needs no fixture file.
    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn renders_an_image_to_a_watermarked_jpeg() {
        let parsed = parse(&tiny_png()).expect("decode the PNG");
        assert_eq!(parsed.total_pages(), 1);
        let bytes = parsed
            .render_page(0, Some(400), Some("0xBUYER"))
            .expect("render the image");
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "output must be a JPEG");
    }

    #[test]
    fn second_page_fails_closed() {
        let parsed = parse(&tiny_png()).expect("decode the PNG");
        assert!(parsed.render_page(1, None, None).is_err());
    }

    #[test]
    fn garbage_input_fails_closed() {
        assert!(parse(b"definitely not an image").is_err());
    }
}
