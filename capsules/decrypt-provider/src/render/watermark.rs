//! Watermark compositor — embeds buyer identity into rendered output.
//!
//! Applied to pixel-lock render output. The watermark is a faint, anti-aliased TEXT overlay
//! rasterised in the SAME real vector face (DejaVu Sans Mono via `ab_glyph`, see [`super::font`])
//! as the document body — so the forensic stamp reads as quiet, professional text rather than the
//! old blocky 1-bit bitmap. It is tiled on a diagonal lattice so the identity cannot be cropped
//! out, and alpha-blended at low opacity so it never destroys the underlying content.
//!
//! FAIL CLOSED: [`finalize`] is the single egress path for EVERY pixel-lock renderer (pdf / image
//! / comic / text / code / svg). A protected page is NEVER emitted without a non-empty forensic
//! stamp — no identity, no image.

use super::font;
use image::RgbaImage;

/// Apply the forensic watermark and encode to JPEG. The single egress path for EVERY pixel-lock
/// renderer, so the boundary always emits a flattened, buyer-stamped JPEG and never the source
/// bytes. Fails closed if no (non-empty) watermark is supplied — protected pixel output without a
/// traceable stamp must not exist.
pub fn finalize(mut img: RgbaImage, watermark: Option<&str>) -> Result<Vec<u8>, String> {
    let stamp = watermark
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("refusing to emit a protected page without a forensic watermark")?;
    // Visible (quiet) mark first, then the INVISIBLE forensic layer carrying the identical identity
    // string — so a leaked frame stays attributable even if the visible mark is cropped/painted out.
    apply_watermark(&mut img, stamp);
    super::invisible::embed(&mut img, stamp);

    let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
    let mut buf = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    encoder
        .encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("jpeg encode: {e}"))?;
    Ok(buf)
}

/// Downscale `img` so its width is at most `max_width` (preserving aspect ratio); never
/// upscales. Bounds the egress image (and re-encoding strips the source file + its metadata).
pub fn fit_width(img: RgbaImage, max_width: Option<u32>) -> RgbaImage {
    let max_w = match max_width {
        Some(w) if w > 0 => w,
        _ => return img,
    };
    let (w, h) = img.dimensions();
    if w <= max_w || w == 0 {
        return img;
    }
    let new_h = ((h as u64 * max_w as u64) / w as u64).max(1) as u32;
    image::imageops::resize(&img, max_w, new_h, image::imageops::FilterType::Triangle)
}

/// Blend strength toward the ink colour (0..1). ~0.07 — a very quiet, legible-on-close-inspection
/// notice. It can be this faint because the INVISIBLE DCT layer ([`super::invisible`]) carries the
/// same identity for attribution; the visible mark is now mostly a deterrent/notice, not the sole
/// forensic record. (Was 0.42 with the old bitmap overlay.)
const OPACITY: f32 = 0.07;
/// Ink colour the stamp blends toward (dark grey, legible over typical light document pages).
const INK: [u8; 3] = [38, 38, 38];

/// Overlay the forensic stamp TILED diagonally across the WHOLE page, so the buyer's identity
/// cannot be cropped out (every region carries it). Rendered in the DejaVu Mono vector face at low
/// opacity (anti-aliased, professional), repeated on a lattice with alternate rows half-offset.
/// No-op for tiny images or empty text.
pub fn apply_watermark(img: &mut RgbaImage, text: &str) {
    let (w, h) = img.dimensions();
    if w < 120 || h < 60 || text.is_empty() {
        return;
    }

    let face = font::mono();
    // Stamp size scales gently with page width; clamped so it stays a quiet caption on any page.
    let px = (w as f32 / 56.0).clamp(13.0, 22.0);
    let text_w = font::measure(&face, text, px);

    // Generous gaps so the page stays readable; diagonal half-offset resists cropping.
    let tile_w = text_w + px * 9.0;
    let tile_h = px * 5.0;

    let mut row = 0u32;
    let mut baseline = px; // first row baseline drops below the top edge
    while baseline < h as f32 + tile_h {
        let row_offset = if row % 2 == 0 { 0.0 } else { tile_w / 2.0 };
        let mut x = -row_offset;
        while x < w as f32 {
            font::draw_line_opacity(img, &face, text, x, baseline, px, INK, OPACITY);
            x += tile_w;
        }
        baseline += tile_h;
        row += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn white(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]))
    }

    #[test]
    fn finalize_fails_closed_without_a_watermark() {
        assert!(
            finalize(white(400, 300), None).is_err(),
            "a protected page must never egress without a forensic stamp"
        );
        assert!(
            finalize(white(400, 300), Some("   ")).is_err(),
            "a blank/whitespace stamp must fail closed like an absent one"
        );
    }

    #[test]
    fn finalize_emits_jpeg_with_a_watermark() {
        let jpeg = finalize(white(400, 300), Some("0x1a2b..9f0e")).expect("jpeg");
        // JPEG SOI marker.
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    }

    #[test]
    fn watermark_marks_the_page() {
        let mut img = white(640, 480);
        apply_watermark(
            &mut img,
            "0x1a2b..9f0e \u{00b7} c0ffee12 \u{00b7} 2026-06-18 09:40Z",
        );
        // At least some pixels must have been darkened by the stamp.
        let darkened = img.pixels().filter(|p| p[0] < 250).count();
        assert!(darkened > 0, "watermark must leave a visible (faint) mark");
    }
}
