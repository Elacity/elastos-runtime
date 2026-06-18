//! PDF renderer — full-fidelity rasterisation via `hayro`.
//!
//! `hayro` is a pure-Rust, `#![forbid(unsafe_code)]` PDF rasteriser that compiles
//! cleanly to `wasm32-wasip1`. It handles fonts, vector graphics, images and text
//! layout natively — no text-extraction fallback needed.
//!
//! Flow:
//!   PDF bytes ──► hayro_syntax (parse) ──► hayro (rasterise page)
//!     ──► Pixmap (RGBA8) ──► watermark ──► JPEG encode ──► output
//!
//! Ported from PC2 `wasm-renderer/src/render/pdf.rs` to keep secure-view parity.

use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::RenderSettings;
use image::{ImageBuffer, Rgba, RgbaImage};

use super::watermark;

/// A parsed PDF held WARM in the decrypt boundary for the lifetime of an open session, so
/// the document is decrypted + parsed ONCE and every page is a fast rasterise (no re-decrypt,
/// no re-parse). Owns its bytes via `Arc`, so it is fully self-contained; the plaintext it
/// holds lives only inside this sandbox process and is dropped when the session ends.
pub struct ParsedPdf {
    pdf: Pdf,
    pub total_pages: u32,
}

/// Parse decrypted PDF bytes once. Fails closed (no plaintext echoed) on a malformed file.
pub fn parse(object: &[u8]) -> Result<ParsedPdf, String> {
    let data = Arc::new(object.to_vec());
    let pdf = Pdf::new(data).map_err(|e| format!("PDF parse: {e:?}"))?;
    let total_pages = pdf.pages().len() as u32;
    if total_pages == 0 {
        return Err("PDF has no pages".to_string());
    }
    Ok(ParsedPdf { pdf, total_pages })
}

impl ParsedPdf {
    /// Rasterise one page to a watermarked JPEG from the already-parsed document. Fails
    /// closed on an out-of-range page or an encode error.
    pub fn render_page(
        &self,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let pages = self.pdf.pages();
        let page_idx = page as usize;
        if page_idx >= pages.len() {
            return Err(format!(
                "page {} out of range (total: {})",
                page_idx + 1,
                self.total_pages
            ));
        }
        let page = &pages[page_idx];

        let max_w = max_width.unwrap_or(800);
        let (native_w, native_h) = page.render_dimensions();
        // Pixel-bomb defense: bound the scale on BOTH axes and by total area, not width alone — an
        // extreme aspect (tiny width × huge height) would otherwise rasterise to a gigapixel pixmap.
        let scale = bounded_scale(native_w, native_h, max_w);
        // Defensive backstop: even after scaling, refuse a page whose predicted raster would exceed
        // the per-side or area budget (guards against float-rounding at the extremes). Fails closed.
        let pred_w = (native_w * scale).ceil().max(1.0) as u64;
        let pred_h = (native_h * scale).ceil().max(1.0) as u64;
        if pred_w > super::MAX_DIM as u64
            || pred_h > super::MAX_DIM as u64
            || pred_w.saturating_mul(pred_h) > super::MAX_PIXELS
        {
            return Err(format!(
                "pdf page exceeds max render size ({pred_w}x{pred_h})"
            ));
        }

        let interpreter_settings = InterpreterSettings::default();
        let render_settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: None,
            height: None,
            bg_color: WHITE,
        };

        let pixmap = hayro::render(page, &interpreter_settings, &render_settings);
        let w = pixmap.width() as u32;
        let h = pixmap.height() as u32;
        let rgba_bytes = pixmap.data_as_u8_slice();

        let img: RgbaImage = ImageBuffer::from_raw(w, h, rgba_bytes.to_vec())
            .unwrap_or_else(|| RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255])));

        watermark::finalize(img, watermark)
    }
}

/// Largest render scale that keeps the page within `max_w`, `MAX_DIM` on BOTH axes, and `MAX_PIXELS`
/// of area, never upscaling past the caller's 3.0 request cap. Pure → unit-testable without `hayro`.
/// Mirrors `svg.rs`'s scale-down-never-(over)-up clamp, extended with an area bound.
fn bounded_scale(native_w: f32, native_h: f32, max_w: u32) -> f32 {
    if !(native_w.is_finite() && native_h.is_finite()) || native_w <= 0.0 || native_h <= 0.0 {
        return 1.0;
    }
    let by_req = max_w as f32 / native_w;
    let by_dim_w = super::MAX_DIM as f32 / native_w;
    let by_dim_h = super::MAX_DIM as f32 / native_h;
    let by_area = (super::MAX_PIXELS as f32 / (native_w * native_h)).sqrt();
    by_req
        .min(by_dim_w)
        .min(by_dim_h)
        .min(by_area)
        .min(3.0)
        .clamp(f32::MIN_POSITIVE, 3.0)
}

/// Build a structurally-valid, single-page PDF with an accurate xref table at runtime
/// (offsets computed from the actual bytes), so the fixture is correct regardless of
/// hand-counting. Draws a filled blue rectangle so the page is non-blank. Shared across
/// the render module's tests.
#[cfg(test)]
pub(crate) fn minimal_pdf() -> Vec<u8> {
    let mut pdf: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();

    pdf.extend_from_slice(b"%PDF-1.4\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    offsets.push(pdf.len());
    pdf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] \
          /Contents 4 0 R /Resources << >> >>\nendobj\n",
    );

    let content: &[u8] = b"0 0 1 rg 20 20 160 160 re f";
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("4 0 obj\n<< /Length {} >>\nstream\n", content.len()).as_bytes());
    pdf.extend_from_slice(content);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_pos = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n");
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF").as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_pdf_page_to_a_nonempty_jpeg_and_reports_page_count() {
        let parsed = parse(&minimal_pdf()).expect("parse the fixture PDF");
        assert_eq!(parsed.total_pages, 1);
        let bytes = parsed
            .render_page(0, Some(400), Some("0xBUYER"))
            .expect("render the page");
        assert!(!bytes.is_empty(), "rendered JPEG must be non-empty");
        // JPEG SOI marker — confirms we emit an image, never the raw PDF.
        assert_eq!(
            &bytes[0..2],
            &[0xFF, 0xD8],
            "output must be a JPEG, not the source PDF"
        );
    }

    #[test]
    fn garbage_input_fails_closed() {
        assert!(
            parse(b"this is not a pdf at all").is_err(),
            "a malformed document must fail closed at parse"
        );
    }

    #[test]
    fn page_out_of_range_fails_closed() {
        let parsed = parse(&minimal_pdf()).expect("parse the fixture PDF");
        assert!(
            parsed.render_page(99, None, None).is_err(),
            "an out-of-range page must fail closed"
        );
    }

    #[test]
    fn bounded_scale_keeps_every_predicted_page_within_budget() {
        // Adversarial native sizes (incl. extreme aspect ratios) must all yield predicted dims
        // within MAX_DIM per side AND MAX_PIXELS of area — never a gigapixel pixmap.
        let cases = [
            (1.0_f32, 1_000_000.0_f32), // tiny width, huge height
            (1_000_000.0, 1.0),         // huge width, tiny height
            (100_000.0, 100_000.0),     // huge square (10 gigapixel)
            (612.0, 792.0),             // a normal US-Letter PDF point size
            (2384.0, 3370.0),           // A0-ish
        ];
        for (nw, nh) in cases {
            let scale = bounded_scale(nw, nh, 800);
            let pw = (nw * scale).ceil().max(1.0) as u64;
            let ph = (nh * scale).ceil().max(1.0) as u64;
            assert!(
                pw <= super::super::MAX_DIM as u64 && ph <= super::super::MAX_DIM as u64,
                "native {nw}x{nh} -> predicted {pw}x{ph} exceeds MAX_DIM",
            );
            assert!(
                pw.saturating_mul(ph) <= super::super::MAX_PIXELS,
                "native {nw}x{nh} -> predicted area {} exceeds MAX_PIXELS",
                pw.saturating_mul(ph),
            );
        }
    }

    #[test]
    fn bounded_scale_is_defensive_on_degenerate_dimensions() {
        // Non-finite / non-positive native sizes fall back to 1.0 rather than producing NaN/inf.
        assert_eq!(bounded_scale(f32::NAN, 100.0, 800), 1.0);
        assert_eq!(bounded_scale(0.0, 100.0, 800), 1.0);
        assert_eq!(bounded_scale(-5.0, 100.0, 800), 1.0);
        assert_eq!(bounded_scale(100.0, f32::INFINITY, 800), 1.0);
    }
}
