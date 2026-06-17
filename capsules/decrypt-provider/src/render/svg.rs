//! SVG renderer — single-page pixel-lock for `image/svg+xml`.
//!
//! SVG is XML (a script/external-reference surface), not a raster format, so it must NEVER be
//! handed to the browser raw. We rasterise it in-boundary onto a white canvas via the pure-Rust
//! `resvg`/`tiny-skia`/`usvg` stack, watermark it, and serve a single flattened JPEG — the source
//! markup never egresses. Text rendering is best-effort (font support is off to keep the boundary
//! lean); vector shapes and embedded raster images render fully.

use super::watermark;

/// Maximum rasterised dimension (px). A hostile SVG can declare an enormous viewport; we scale
/// the render down to fit this bound so one asset cannot exhaust boundary memory.
const MAX_DIM: u32 = 4000;

/// A parsed SVG tree held warm for the session. Rendering is deterministic, so we keep the tree
/// and rasterise on demand.
pub struct ParsedSvg {
    tree: resvg::usvg::Tree,
}

/// Parse the already-decrypted SVG bytes once. Fails closed (no markup echoed) on malformed XML.
pub fn parse(object: &[u8]) -> Result<ParsedSvg, String> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(object, &opt).map_err(|e| format!("svg parse: {e}"))?;
    Ok(ParsedSvg { tree })
}

impl ParsedSvg {
    pub fn total_pages(&self) -> u32 {
        1
    }

    pub fn render_page(
        &self,
        page: u32,
        _max_width: Option<u32>,
        watermark_text: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        if page != 0 {
            return Err(format!(
                "svg asset has a single page (requested {})",
                page + 1
            ));
        }
        let size = self.tree.size();
        let (sw, sh) = (size.width(), size.height());
        if !(sw.is_finite() && sh.is_finite()) || sw <= 0.0 || sh <= 0.0 {
            return Err("svg has no usable dimensions".to_string());
        }
        // Scale down (never up) so neither dimension exceeds MAX_DIM.
        let scale = (MAX_DIM as f32 / sw)
            .min(MAX_DIM as f32 / sh)
            .clamp(f32::MIN_POSITIVE, 1.0);
        let w = (sw * scale).ceil().max(1.0) as u32;
        let h = (sh * scale).ceil().max(1.0) as u32;

        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
            .ok_or_else(|| format!("svg pixmap alloc failed ({w}x{h})"))?;
        // Flatten onto white so transparency reads like a document page (and JPEG has no alpha).
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&self.tree, transform, &mut pixmap.as_mut());

        // tiny-skia is premultiplied RGBA; flattened over opaque white the alpha is 255, so the
        // premultiplied and straight values coincide — safe to hand straight to `image`.
        let img = image::RgbaImage::from_raw(w, h, pixmap.take())
            .ok_or_else(|| "svg pixmap -> image buffer size mismatch".to_string())?;
        watermark::finalize(img, watermark_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80">
        <rect width="120" height="80" fill="#3366cc"/>
        <circle cx="60" cy="40" r="25" fill="#ffcc00"/>
    </svg>"##;

    #[test]
    fn rasterises_svg_to_a_watermarked_jpeg() {
        let parsed = parse(SAMPLE).expect("parse the SVG");
        assert_eq!(parsed.total_pages(), 1);
        let bytes = parsed
            .render_page(0, Some(800), Some("0xBUYER"))
            .expect("render the SVG");
        assert_eq!(
            &bytes[0..2],
            &[0xFF, 0xD8],
            "output must be a JPEG, not the SVG markup"
        );
    }

    #[test]
    fn second_page_fails_closed() {
        let parsed = parse(SAMPLE).expect("parse the SVG");
        assert!(parsed.render_page(1, None, None).is_err());
    }

    #[test]
    fn garbage_input_fails_closed() {
        assert!(parse(b"<not-svg>").is_err());
    }
}
