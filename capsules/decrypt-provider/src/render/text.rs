//! Text/code renderer — multi-page pixel-lock for `text/*` and source-code mimes.
//!
//! A protected text/code asset is decoded in-boundary, wrapped to a fixed column width, and
//! rasterised page-by-page to a watermarked image (monospace `font8x8`). The browser receives
//! flattened, buyer-stamped page images — never the source text — so the text cannot be copied
//! out and every page carries the forensic watermark. Mirrors PC2's `CodeRenderer`/`TextRenderer`
//! (rasterise-don't-ship), staying in the pure-Rust boundary (no new dependency).

use super::watermark;

/// Glyph scale (8x8 → 16px cells): legible without being huge.
const SCALE: u32 = 2;
/// Wrapped columns per line and lines per page (the page geometry).
const COLS: usize = 100;
const ROWS: usize = 50;
/// Page margin in pixels.
const MARGIN: u32 = 24;
/// Tabs expand to this many spaces.
const TAB: usize = 4;
/// Defense against a hostile/huge file pinning memory as wrapped lines.
const MAX_LINES: usize = 200_000;

/// The wrapped display lines, computed once at parse. Each render slices `ROWS` of them.
pub struct ParsedText {
    lines: Vec<String>,
}

/// Decode the already-decrypted bytes (UTF-8, lossy) and wrap into display lines. Fails closed
/// only if the wrapped form would exceed `MAX_LINES` (otherwise renders best-effort text).
pub fn parse(object: &[u8]) -> Result<ParsedText, String> {
    let text = String::from_utf8_lossy(object);
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        // Drop a trailing CR, expand tabs, strip other control chars (keep printable + space).
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let expanded = raw.replace('\t', &" ".repeat(TAB));
        let cleaned: String = expanded
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let chars: Vec<char> = cleaned.chars().collect();
        if chars.is_empty() {
            lines.push(String::new());
        } else {
            let mut i = 0;
            while i < chars.len() {
                let end = (i + COLS).min(chars.len());
                lines.push(chars[i..end].iter().collect());
                i = end;
            }
        }
        if lines.len() > MAX_LINES {
            return Err(format!(
                "text too large to rasterise (> {MAX_LINES} wrapped lines)"
            ));
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    Ok(ParsedText { lines })
}

impl ParsedText {
    pub fn total_pages(&self) -> u32 {
        (self.lines.len().div_ceil(ROWS)).max(1) as u32
    }

    /// Rasterise one page. `max_width` is intentionally ignored: text must stay legible, so the
    /// page is rendered at native size and the browser scales it down to fit (downscaling here
    /// would blur the glyphs).
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
        let cell = 8 * SCALE;
        let width = COLS as u32 * cell + 2 * MARGIN;
        let height = ROWS as u32 * cell + 2 * MARGIN;
        let mut img =
            image::RgbaImage::from_pixel(width, height, image::Rgba([250, 250, 250, 255]));

        let start = page as usize * ROWS;
        let end = (start + ROWS).min(self.lines.len());
        for (row, line) in self.lines[start..end].iter().enumerate() {
            let y = MARGIN + row as u32 * cell;
            watermark::draw_solid(&mut img, line, MARGIN as i64, y as i64, SCALE, [30, 30, 30]);
        }
        watermark::finalize(img, watermark_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginates_and_renders_a_jpeg() {
        // 120 short lines -> spans multiple pages at ROWS=50.
        let body: String = (0..120).map(|i| format!("line {i}\n")).collect();
        let parsed = parse(body.as_bytes()).expect("parse");
        assert_eq!(parsed.total_pages(), 3, "120 lines / 50 per page = 3 pages");
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
    fn long_lines_wrap() {
        let long = "x".repeat(COLS * 3 + 5);
        let parsed = parse(long.as_bytes()).expect("parse");
        // One 305-char line wraps into 4 display lines (100+100+100+5).
        assert_eq!(parsed.lines.len(), 4);
    }

    #[test]
    fn empty_input_is_one_page() {
        let parsed = parse(b"").expect("parse empty");
        assert_eq!(parsed.total_pages(), 1);
        assert!(parsed.render_page(0, None, None).is_ok());
    }
}
