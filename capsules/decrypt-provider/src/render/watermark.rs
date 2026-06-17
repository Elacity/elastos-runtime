//! Watermark compositor — embeds buyer identity into rendered output.
//!
//! Applied to pixel-lock render output. The watermark is alpha-blended text overlaid on the
//! rendered pixels using the public-domain `font8x8` bitmap font, which covers the FULL
//! printable-ASCII set. The prior hand-rolled font only had `0-9 a-f x . : -`, so any other
//! character (e.g. the letters in a `did:…` principal) rendered as a garbled filler box — this
//! restores legibility for arbitrary principals/DIDs/wallets. The only image dependency stays
//! `image`; `font8x8` is pure const data (no font-file vendoring, no runtime).

use font8x8::legacy::BASIC_LEGACY;
use image::{Rgba, RgbaImage};

/// Apply the forensic watermark (when present) and encode to JPEG. The single egress path for
/// EVERY pixel-lock renderer (pdf / image / comic), so the boundary always emits a flattened,
/// buyer-stamped JPEG and never the source bytes.
pub fn finalize(mut img: RgbaImage, watermark: Option<&str>) -> Result<Vec<u8>, String> {
    if let Some(wm) = watermark {
        apply_watermark(&mut img, wm);
    }
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

/// One glyph cell is 8x8 in the source font.
const GLYPH: u32 = 8;
/// Blend strength toward the ink colour (out of 256). ~0.42 — clearly readable on light pages
/// without destroying the underlying content.
const ALPHA: u16 = 108;
/// Ink colour the stamp blends toward (dark grey, legible over both light and dark content).
const INK: u8 = 45;

/// Overlay a forensic watermark TILED diagonally across the WHOLE page, so the buyer's
/// identity cannot be cropped out (every region of the image carries the stamp). The text
/// is alpha-blended (legible but non-destructive), repeated on a diagonal lattice (alternate
/// rows offset by half a tile). No-op for tiny images or empty text.
pub fn apply_watermark(img: &mut RgbaImage, text: &str) {
    let (w, h) = img.dimensions();
    if w < 120 || h < 60 || text.is_empty() {
        return;
    }

    // Scale the 8x8 glyphs up for legibility (min 2× so the stamp is clearly readable; grows
    // with page width). Then tile with generous gaps so the page stays readable.
    let scale: u32 = (w / 420).clamp(2, 4);
    let cell = GLYPH * scale;
    let text_px = text.chars().count() as u32 * cell;

    let tile_w = text_px + 130 * scale;
    let tile_h = cell + 64 * scale;

    let mut row = 0u32;
    let mut y = 0i64;
    while y < h as i64 {
        // Offset every other row by half a tile → a diagonal lattice that resists cropping.
        let row_offset = if row % 2 == 0 {
            0i64
        } else {
            (tile_w / 2) as i64
        };
        let mut x = -row_offset;
        while x < w as i64 {
            draw_text(img, text, x, y, scale);
            x += tile_w as i64;
        }
        y += tile_h as i64;
        row += 1;
    }
}

/// Draw one text stamp at `(x, y)` (top-left), scaled, alpha-blended over the existing pixels.
/// Pixels outside the image are clipped. Negative coordinates are allowed so the lattice can
/// bleed off the top/left edge.
fn draw_text(img: &mut RgbaImage, text: &str, x: i64, y: i64, scale: u32) {
    let (w, h) = img.dimensions();
    for (i, ch) in text.chars().enumerate() {
        let code = ch as usize;
        // font8x8 BASIC_LEGACY covers ASCII 0x00..=0x7F; anything else falls back to '?'.
        let bitmap = BASIC_LEGACY[if code < 128 { code } else { '?' as usize }];
        let cell_x = x + (i as i64) * (GLYPH * scale) as i64;
        for (gy, bits) in bitmap.iter().enumerate() {
            for gx in 0..GLYPH {
                // font8x8 packs each row LSB-first: bit `gx` is the pixel at column `gx`.
                if bits & (1 << gx) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cell_x + (gx * scale + sx) as i64;
                        let py = y + (gy as u32 * scale + sy) as i64;
                        if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                            continue;
                        }
                        let pixel = img.get_pixel_mut(px as u32, py as u32);
                        *pixel = Rgba([
                            blend(pixel[0], INK, ALPHA),
                            blend(pixel[1], INK, ALPHA),
                            blend(pixel[2], INK, ALPHA),
                            pixel[3],
                        ]);
                    }
                }
            }
        }
    }
}

/// Blend `src` toward `target` by `alpha`/256.
#[inline]
fn blend(src: u8, target: u8, alpha: u16) -> u8 {
    let inv = 256 - alpha;
    ((src as u16 * inv + target as u16 * alpha) / 256) as u8
}

/// Draw one line of text in a SOLID `ink` colour (for rasterised document BODY text — e.g. the
/// text/code renderer — as opposed to the translucent tiled watermark). Monospace `font8x8`
/// glyphs, scaled, clipped to the image. Pixels outside the image are skipped.
pub fn draw_solid(img: &mut RgbaImage, text: &str, x: i64, y: i64, scale: u32, ink: [u8; 3]) {
    let (w, h) = img.dimensions();
    for (i, ch) in text.chars().enumerate() {
        let code = ch as usize;
        let bitmap = BASIC_LEGACY[if code < 128 { code } else { '?' as usize }];
        let cell_x = x + (i as i64) * (GLYPH * scale) as i64;
        // Cheap horizontal clip: stop once a glyph starts past the right edge.
        if cell_x >= w as i64 {
            break;
        }
        for (gy, bits) in bitmap.iter().enumerate() {
            for gx in 0..GLYPH {
                if bits & (1 << gx) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cell_x + (gx * scale + sx) as i64;
                        let py = y + (gy as u32 * scale + sy) as i64;
                        if px < 0 || py < 0 || px >= w as i64 || py >= h as i64 {
                            continue;
                        }
                        let p = img.get_pixel_mut(px as u32, py as u32);
                        *p = Rgba([ink[0], ink[1], ink[2], p[3]]);
                    }
                }
            }
        }
    }
}
