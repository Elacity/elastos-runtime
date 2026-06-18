//! Text renderer — multi-page pixel-lock for `text/plain` and `text/markdown`.
//!
//! A protected text asset is decoded in-boundary, word-wrapped to a fixed page width, and
//! rasterised page-by-page to a watermarked image. The browser receives flattened, buyer-stamped
//! page images — never the source text — so it cannot be copied out and every page carries the
//! forensic watermark. Mirrors PC2's `TextRenderer` (rasterise-don't-ship), staying in the
//! pure-Rust boundary.
//!
//! Presentation parity with a normal reader (friendly + easy to read): the body is set in a REAL
//! anti-aliased proportional typeface (DejaVu Sans, via `super::font`/`ab_glyph`) on a warm
//! off-white page with comfortable line leading + margins — not the old blocky 8x8 bitmap. Text is
//! wrapped by measured pixel width (true word-wrap, no mid-word breaks) and rendered with full
//! Unicode coverage, so smart quotes, dashes, accents and bullets show as themselves.

use super::{font, watermark};

/// Body text size in pixels (rendered large; the browser scales the page image to fit).
const FONT_PX: f32 = 30.0;
/// Baseline-to-baseline pitch. ~1.5× the font size reads far easier than packed rows.
const LINE_H: f32 = FONT_PX * 1.5;
/// Page margin in pixels.
const MARGIN: f32 = 64.0;
/// Text column width in pixels (word-wrap target). Page width is this plus both margins.
const CONTENT_W: f32 = 1200.0;
/// Wrapped display lines per page (the page geometry).
const ROWS: usize = 40;
/// Tabs expand to this many spaces.
const TAB: usize = 4;
/// Warm off-white page (a cream reader feel) and a soft near-black ink — high contrast but not
/// harsh.
const PAGE_BG: [u8; 4] = [251, 250, 246, 255];
const INK: [u8; 3] = [34, 34, 38];
/// Defense against a hostile/huge file pinning memory as wrapped lines.
const MAX_LINES: usize = 200_000;

/// The wrapped display lines, computed once at parse. Each render slices `ROWS` of them.
pub struct ParsedText {
    lines: Vec<String>,
}

/// Decode the already-decrypted bytes (UTF-8, lossy) and word-wrap into display lines at the page
/// width. Fails closed only if the wrapped form would exceed `MAX_LINES`.
pub fn parse(object: &[u8]) -> Result<ParsedText, String> {
    let text = String::from_utf8_lossy(object);
    let face = font::sans();
    let max_w = CONTENT_W;
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        // Drop a trailing CR, expand tabs, strip control chars (keep printable + space). Unicode
        // is preserved as-is — the vector face has broad coverage, so no ASCII folding is needed.
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let expanded = raw.replace('\t', &" ".repeat(TAB));
        let cleaned: String = expanded
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        for wrapped in wrap_line(&face, &cleaned, max_w) {
            lines.push(wrapped);
            if lines.len() > MAX_LINES {
                return Err(format!(
                    "text too large to rasterise (> {MAX_LINES} wrapped lines)"
                ));
            }
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    Ok(ParsedText { lines })
}

/// Greedy word-wrap one source line to `max_w` pixels, preserving the line's leading indent on
/// continuation rows so lists/tables stay readable. Words longer than `max_w` are hard-broken by
/// character so nothing overflows the page.
fn wrap_line<F: ab_glyph::Font>(face: &F, raw: &str, max_w: f32) -> Vec<String> {
    let indent: String = raw.chars().take_while(|&c| c == ' ').collect();
    let body = raw[indent.len()..].trim_end();
    if body.is_empty() {
        return vec![String::new()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = indent.clone();
    let mut have_word = false;
    for word in body.split(' ') {
        if word.is_empty() {
            continue;
        }
        for piece in break_word(face, word, max_w - font::measure(face, &indent, FONT_PX)) {
            let sep = if have_word { " " } else { "" };
            let candidate = format!("{cur}{sep}{piece}");
            if !have_word || font::measure(face, &candidate, FONT_PX) <= max_w {
                cur = candidate;
                have_word = true;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = format!("{indent}{piece}");
                have_word = true;
            }
        }
    }
    if have_word || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Split a single word that is wider than `max_w` into character chunks that each fit. A word that
/// already fits is returned unchanged.
fn break_word<F: ab_glyph::Font>(face: &F, word: &str, max_w: f32) -> Vec<String> {
    if max_w <= 0.0 || font::measure(face, word, FONT_PX) <= max_w {
        return vec![word.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in word.chars() {
        let candidate = format!("{cur}{ch}");
        if !cur.is_empty() && font::measure(face, &candidate, FONT_PX) > max_w {
            out.push(std::mem::take(&mut cur));
            cur = ch.to_string();
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl ParsedText {
    pub fn total_pages(&self) -> u32 {
        (self.lines.len().div_ceil(ROWS)).max(1) as u32
    }

    /// Rasterise one page. `max_width` is intentionally ignored: text must stay legible, so the
    /// page is rendered at a fixed high resolution and the browser scales it to fit. Body glyphs
    /// are anti-aliased by `ab_glyph` (coverage blending); the watermark is applied at final res.
    pub fn render_page(
        &self,
        page: u32,
        _max_width: Option<u32>,
        watermark_text: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let total = self.total_pages();
        if page >= total {
            return Err(format!("page {} out of range (total: {total})", page + 1));
        }
        let face = font::sans();
        let ascent = font::ascent(&face, FONT_PX);
        let width = (CONTENT_W + 2.0 * MARGIN).ceil() as u32;
        let height = (2.0 * MARGIN + ROWS as f32 * LINE_H).ceil() as u32;
        let mut img = image::RgbaImage::from_pixel(width, height, image::Rgba(PAGE_BG));

        let start = page as usize * ROWS;
        let end = (start + ROWS).min(self.lines.len());
        for (row, line) in self.lines[start..end].iter().enumerate() {
            // Baseline = top margin + ascent + row pitch (ascent lifts the glyph off the line top).
            let baseline = MARGIN + ascent + row as f32 * LINE_H;
            font::draw_line(&mut img, &face, line, MARGIN, baseline, FONT_PX, INK);
        }

        watermark::finalize(img, watermark_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginates_and_renders_a_jpeg() {
        // 120 short lines, each ending in '\n'; the trailing newline yields one final blank line
        // (121 wrapped lines), so at 40 lines/page that is 4 pages.
        let body: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let parsed = parse(body.as_bytes()).expect("parse");
        assert_eq!(parsed.total_pages(), 4, "121 wrapped lines / 40 per page = 4 pages");
        let bytes = parsed
            .render_page(0, Some(800), Some("0xBUYER"))
            .expect("render page 1");
        assert_eq!(
            &bytes[0..2],
            &[0xFF, 0xD8],
            "output must be a JPEG, not the source text"
        );
        assert!(
            parsed.render_page(9, None, None).is_err(),
            "OOB page fails closed"
        );
    }

    #[test]
    fn long_lines_wrap_on_word_boundaries() {
        // A long line of real words wraps into multiple display lines (no mid-word breaks).
        let long = "lorem ipsum dolor sit amet ".repeat(20);
        let parsed = parse(long.as_bytes()).expect("parse");
        assert!(parsed.lines.len() > 1, "long line must wrap");
        // No wrapped line should start or end with a stray space (clean word boundaries).
        for l in &parsed.lines {
            assert_eq!(l, l.trim_end(), "no trailing space on wrapped line");
        }
    }

    #[test]
    fn very_long_unbroken_token_is_hard_broken() {
        let token = "x".repeat(4000);
        let parsed = parse(token.as_bytes()).expect("parse");
        assert!(
            parsed.lines.len() > 1,
            "an unbroken token wider than the page must hard-break"
        );
    }

    #[test]
    fn preserves_unicode_typography() {
        // Smart quotes / em-dash / accent survive parse (the vector face renders them directly).
        let parsed =
            parse("\u{201c}H\u{00e9}llo\u{201d} \u{2014} world".as_bytes()).expect("parse");
        let joined = parsed.lines.join(" ");
        assert!(
            joined.contains('\u{201c}')
                && joined.contains('\u{2014}')
                && joined.contains('\u{00e9}')
        );
    }

    #[test]
    fn empty_input_is_one_page() {
        let parsed = parse(b"").expect("parse empty");
        assert_eq!(parsed.total_pages(), 1);
        assert!(parsed.render_page(0, None, None).is_ok());
    }

    /// Dev-only visual check: `ELASTOS_RENDER_SAMPLE=1 cargo test --features pdf-render
    /// render::text::emit_sample -- --nocapture` writes a JPEG to /tmp so the rendered
    /// text presentation can be eyeballed. No-op in normal runs.
    #[test]
    fn emit_sample() {
        if std::env::var("ELASTOS_RENDER_SAMPLE").is_err() {
            return;
        }
        let body = "The quick brown fox jumps over the lazy dog. 0123456789\n\n\
            \u{201c}Protected reading sample\u{201d} \u{2014} pixel-locked, watermarked, anti-aliased.\n\n\
            Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor \
            incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
            exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.\n\n\
            Table of Contents\n\n  1. Eos ex eius iusto delicata\n  2. Illum argumentum sed a\n  \
            3. In eum magna iusto integre\n\n\
            R\u{00e9}sum\u{00e9} caf\u{00e9} na\u{00ef}ve fa\u{00e7}ade \u{2022} bullet \u{2022} list.";
        let parsed = parse(body.as_bytes()).expect("parse");
        let jpeg = parsed
            .render_page(0, None, Some("0xBUYER..a1b2"))
            .expect("render");
        std::fs::write("/tmp/text-sample.jpg", &jpeg).expect("write sample");
        eprintln!("wrote /tmp/text-sample.jpg ({} bytes)", jpeg.len());
    }
}
