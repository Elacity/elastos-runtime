//! Vector-font text rasteriser for the pixel-lock document renderers.
//!
//! Replaces the blocky 8x8 bitmap (`font8x8`) for document BODY text with a REAL, anti-aliased
//! TrueType typeface (DejaVu) rasterised in-boundary via the pure-Rust, unsafe-free `ab_glyph`
//! crate (no system fonts; builds to wasm32-wasip1). The prose reader (`text.rs`) uses the
//! proportional Sans face and the code reader (`code.rs`) uses the fixed-pitch Mono face so its
//! column alignment is preserved. The faint tiled FORENSIC watermark still uses `font8x8`
//! (`watermark.rs`) — only the legible content text moves to the vector face.

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use image::RgbaImage;

/// DejaVu Sans — clean humanist proportional face for prose (`text/plain`, `text/markdown`).
static SANS_TTF: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
/// DejaVu Sans Mono — fixed-pitch face for source code (keeps the gutter/column layout aligned).
static MONO_TTF: &[u8] = include_bytes!("../../assets/fonts/DejaVuSansMono.ttf");

/// The proportional reading face (lazily parsed from the embedded bytes; the parse is cheap and
/// the bytes are `'static`, so callers just grab a fresh handle per page render).
pub fn sans() -> FontRef<'static> {
    FontRef::try_from_slice(SANS_TTF).expect("embedded DejaVuSans.ttf is a valid TrueType font")
}

/// The fixed-pitch code face.
pub fn mono() -> FontRef<'static> {
    FontRef::try_from_slice(MONO_TTF).expect("embedded DejaVuSansMono.ttf is a valid TrueType font")
}

/// Scaled ascent (px above the baseline) for `font` at pixel height `px` — used to drop the first
/// row's baseline below the top margin so glyphs sit fully on the page.
pub fn ascent<F: Font>(font: &F, px: f32) -> f32 {
    font.as_scaled(PxScale::from(px)).ascent()
}

/// Total advance width (in px) of `text` rendered with `font` at `px`, including kerning.
/// Used by the prose word-wrapper to fill lines to a target column width.
pub fn measure<F: Font>(font: &F, text: &str, px: f32) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    let mut w = 0.0f32;
    let mut prev: Option<GlyphId> = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Draw one line of `text` with `font` at pixel height `px`, with the baseline at `(x, baseline_y)`,
/// in solid `ink`. Glyph coverage is alpha-blended over the existing pixels so the edges
/// anti-alias (the readability win over the 1-bit bitmap). Pixels outside the image are clipped.
/// Returns the advance width consumed (so callers can lay out runs left-to-right).
pub fn draw_line<F: Font>(
    img: &mut RgbaImage,
    font: &F,
    text: &str,
    x: f32,
    baseline_y: f32,
    px: f32,
    ink: [u8; 3],
) -> f32 {
    draw_line_opacity(img, font, text, x, baseline_y, px, ink, 1.0)
}

/// Like [`draw_line`] but every glyph's coverage is scaled by `opacity` in `[0,1]`. Used by the
/// faint tiled forensic watermark ([`super::watermark`]) so the stamp is rendered in the SAME
/// anti-aliased vector face as the body text — legible but non-destructive — instead of the old
/// 1-bit bitmap. `opacity = 1.0` is identical to [`draw_line`].
// Mirrors `draw_line`'s positional layout (image, face, text, x, baseline, px, ink) plus the
// opacity scalar — a params struct would just obscure this hot per-glyph call.
#[allow(clippy::too_many_arguments)]
pub fn draw_line_opacity<F: Font>(
    img: &mut RgbaImage,
    font: &F,
    text: &str,
    x: f32,
    baseline_y: f32,
    px: f32,
    ink: [u8; 3],
    opacity: f32,
) -> f32 {
    let opacity = opacity.clamp(0.0, 1.0);
    let sf = font.as_scaled(PxScale::from(px));
    let (w, h) = img.dimensions();
    let mut caret = x;
    let mut prev: Option<GlyphId> = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let mut glyph = sf.scaled_glyph(ch);
        glyph.position = point(caret, baseline_y);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                if cov <= 0.0 {
                    return;
                }
                let px_x = bb.min.x as i32 + gx as i32;
                let px_y = bb.min.y as i32 + gy as i32;
                if px_x < 0 || px_y < 0 || px_x >= w as i32 || px_y >= h as i32 {
                    return;
                }
                let p = img.get_pixel_mut(px_x as u32, px_y as u32);
                let a = (cov * opacity).clamp(0.0, 1.0);
                p[0] = blend(p[0], ink[0], a);
                p[1] = blend(p[1], ink[1], a);
                p[2] = blend(p[2], ink[2], a);
            });
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    caret - x
}

/// Blend `src` toward `target` by coverage `a` in [0,1].
#[inline]
fn blend(src: u8, target: u8, a: f32) -> u8 {
    (src as f32 * (1.0 - a) + target as f32 * a)
        .round()
        .clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse() {
        let _ = sans();
        let _ = mono();
    }

    #[test]
    fn mono_is_fixed_pitch() {
        // Every glyph in a monospace face advances by the same width — the property the code
        // renderer's column layout relies on.
        let m = mono();
        let wi = measure(&m, "i", 20.0);
        let ww = measure(&m, "W", 20.0);
        assert!(
            (wi - ww).abs() < 0.01,
            "mono advances must match: {wi} vs {ww}"
        );
    }

    #[test]
    fn measure_grows_with_text() {
        let s = sans();
        assert!(measure(&s, "hello world", 20.0) > measure(&s, "hi", 20.0));
    }
}
