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
