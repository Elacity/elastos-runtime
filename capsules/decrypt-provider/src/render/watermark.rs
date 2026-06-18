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
    let raw = watermark
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("refusing to emit a protected page without a forensic watermark")?;
    // The wire mark is `<display>` or `<display>\u{1F}gd:<32 hex>`: the display half is the human
    // `wallet · content · time` stamp; the optional trailing token is the AUTHENTICATED grant-digest
    // anchor (`ddrm_envelope::grant_watermark_digest16`) for the invisible layer only.
    let (display, grant_digest) = split_mark(raw);
    if display.is_empty() {
        return Err("refusing to emit a protected page without a forensic watermark".into());
    }
    // Visible (quiet) human mark first, then the INVISIBLE forensic layer — which carries the
    // grant-digest anchor when present (verifiable, non-repudiable) else the compact wallet — so a
    // leaked frame stays attributable even if the visible mark is cropped/painted out.
    apply_watermark(&mut img, display);
    super::invisible::embed(&mut img, display, grant_digest);

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

/// Separator between the human display stamp and the invisible-only grant-digest token. US (0x1F)
/// is a non-printable control char that never occurs in a `wallet · content · time` stamp.
const MARK_SEP: char = '\u{1F}';

/// Split a wire watermark into `(visible display text, optional 16-byte grant digest)`. The
/// media-authority appends `\u{1F}gd:<32 hex>` when a wallet-signed grant is present; a plain string
/// (no separator) carries no digest (local-dev / no-grant opens — back-compat).
fn split_mark(s: &str) -> (&str, Option<[u8; 16]>) {
    match s.split_once(MARK_SEP) {
        Some((display, tail)) => {
            let digest = tail.trim().strip_prefix("gd:").and_then(parse_hex16);
            (display.trim(), digest)
        }
        None => (s.trim(), None),
    }
}

/// Parse exactly 32 hex chars into 16 bytes, or `None` (the mark then falls back to the unauthenticated
/// compact wallet rather than failing the whole render).
fn parse_hex16(hex: &str) -> Option<[u8; 16]> {
    let hex = hex.trim();
    if hex.len() != 32 {
        return None;
    }
    let b = hex.as_bytes();
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (nibble(b[2 * i])? << 4) | nibble(b[2 * i + 1])?;
    }
    Some(out)
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
    fn split_mark_separates_display_from_grant_digest() {
        // Plain stamp: all display, no digest (back-compat).
        let (d, g) = split_mark("0xabc \u{00b7} c0ffee \u{00b7} 2026-06-18 09:40Z");
        assert_eq!(d, "0xabc \u{00b7} c0ffee \u{00b7} 2026-06-18 09:40Z");
        assert!(g.is_none());

        // Stamp + grant-digest token: display is clean (no token), digest parsed.
        let hex = "00112233445566778899aabbccddeeff";
        let wire = format!("0xabc \u{00b7} c0ffee \u{00b7} t\u{1F}gd:{hex}");
        let (d, g) = split_mark(&wire);
        assert_eq!(d, "0xabc \u{00b7} c0ffee \u{00b7} t");
        assert_eq!(
            g,
            Some([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ])
        );

        // Malformed token degrades gracefully to no-digest (render still succeeds, unauthenticated).
        let (_, g) = split_mark("0xabc\u{1F}gd:nothex");
        assert!(g.is_none());
    }

    #[test]
    fn finalize_strips_the_grant_token_from_visible_output() {
        // The 0x1F token must never reach the visible compositor; finalize must still emit a JPEG.
        let jpeg = finalize(
            white(400, 300),
            Some("0x1a2b..9f0e\u{1F}gd:00112233445566778899aabbccddeeff"),
        )
        .expect("jpeg");
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
