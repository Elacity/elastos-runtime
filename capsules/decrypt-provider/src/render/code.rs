//! Code renderer — multi-page pixel-lock for SOURCE-CODE mimes (JSON / JS / XML / YAML / TOML /
//! shell). A dark, IDE-style view with a line-number gutter and conservative, per-language token
//! colouring (comments, strings, numbers, and XML tags), rasterised page-by-page to a watermarked
//! image so the source never leaves the boundary. Mirrors PC2's `render::code` intent (line
//! numbers, dark theme, syntax colour) but stays dependency-free + wasm-lean: the colouring is a
//! small, robust per-mime tokeniser rather than a full grammar set, so it never mis-renders code.
//!
//! Prose (`text/plain`, `text/markdown`) is intentionally NOT routed here — it reads better on the
//! light reflow reader in `text.rs`. This renderer is for code, where a dark gutter view helps.

use super::{font, watermark};
use image::{Rgba, RgbaImage};

// Body is set in a real fixed-pitch face (DejaVu Sans Mono, via `super::font`/`ab_glyph`) so the
// gutter/column layout stays aligned while the glyphs read as a friendly IDE font (not the blocky
// 8x8 bitmap). All geometry is in pixels at this font size.
const FONT_PX: f32 = 26.0;
/// Baseline-to-baseline pitch (~1.45× the font size: comfortable for code).
const LINE_H: f32 = FONT_PX * 1.45;
const COLS: usize = 104;
const ROWS: usize = 46;
const MARGIN: f32 = 40.0;
const TAB: usize = 4;
const MAX_LINES: usize = 200_000;
/// Columns reserved for the line-number gutter (digits + a separating space).
const GUTTER: usize = 6;

// base16-ocean.dark–inspired palette (PC2 parity feel).
const BG: [u8; 4] = [40, 44, 52, 255];
const GUTTER_BG: [u8; 4] = [33, 37, 43, 255];
const FG: [u8; 3] = [197, 200, 198];
const LINENO: [u8; 3] = [99, 109, 122];
const COMMENT: [u8; 3] = [101, 123, 131];
const STRING: [u8; 3] = [166, 200, 122];
const NUMBER: [u8; 3] = [208, 135, 112];
const TAG: [u8; 3] = [129, 162, 190];

/// Fold common Unicode typography to the closest printable ASCII so the FIXED-PITCH code view
/// keeps its column alignment (a wide/CJK glyph would advance differently and skew the gutter).
/// Code is overwhelmingly ASCII; only the occasional smart-quote/dash in a string or comment is
/// folded. Anything already ASCII passes through unchanged.
fn ascii_fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' | '\u{00B4}' | '\u{0060}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' | '\u{00AB}' | '\u{00BB}' => '"',
        '\u{2013}' | '\u{2014}' | '\u{2012}' | '\u{2212}' | '\u{2010}' | '\u{2011}' => '-',
        '\u{2026}' => '.', // ellipsis → a single dot (kept to one cell; '...' would re-wrap)
        '\u{00A0}' | '\u{2009}' | '\u{200A}' | '\u{2002}' | '\u{2003}' => ' ',
        '\u{2022}' | '\u{00B7}' | '\u{2027}' => '*', // bullets
        other => other,
    }
}

/// Per-language token rules. Kept tiny + conservative so colouring is always correct-or-plain.
struct Lang {
    line_comment: Option<&'static str>,
    hash_comment: bool,
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [char],
    xml: bool,
}

fn lang_for(mime: &str) -> Lang {
    match mime.trim().to_ascii_lowercase().as_str() {
        "application/json" => Lang {
            line_comment: None,
            hash_comment: false,
            block: None,
            quotes: &['"'],
            xml: false,
        },
        "application/javascript" => Lang {
            line_comment: Some("//"),
            hash_comment: false,
            block: Some(("/*", "*/")),
            quotes: &['"', '\'', '`'],
            xml: false,
        },
        "application/xml" => Lang {
            line_comment: None,
            hash_comment: false,
            block: Some(("<!--", "-->")),
            quotes: &['"', '\''],
            xml: true,
        },
        // yaml / toml / shell — `#` line comments, simple quotes.
        _ => Lang {
            line_comment: None,
            hash_comment: true,
            block: None,
            quotes: &['"', '\''],
            xml: false,
        },
    }
}

/// A coloured run of characters on a display row.
struct Span {
    text: String,
    ink: [u8; 3],
}

/// One display row: an optional source line number (only on the first wrapped row) + coloured runs.
struct Row {
    line_no: Option<usize>,
    spans: Vec<Span>,
}

pub struct ParsedCode {
    rows: Vec<Row>,
}

/// Decode + tokenise + wrap the source into coloured display rows. Block-comment state carries
/// across lines; everything else is line-local (robust against pathological input).
pub fn parse(object: &[u8], mime: &str) -> Result<ParsedCode, String> {
    let lang = lang_for(mime);
    let text = String::from_utf8_lossy(object);
    let mut rows: Vec<Row> = Vec::new();
    let mut in_block = false;
    for (idx, raw) in text.split('\n').enumerate() {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let expanded = raw.replace('\t', &" ".repeat(TAB));
        let cleaned: String = expanded
            .chars()
            .map(ascii_fold)
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let (spans, block_after) = tokenize_line(&cleaned, &lang, in_block);
        in_block = block_after;
        // Wrap the coloured spans into COLS-wide display rows, tagging the first with the line no.
        let wrapped = wrap_spans(spans, COLS);
        if wrapped.is_empty() {
            rows.push(Row {
                line_no: Some(idx + 1),
                spans: Vec::new(),
            });
        } else {
            for (w, row_spans) in wrapped.into_iter().enumerate() {
                rows.push(Row {
                    line_no: if w == 0 { Some(idx + 1) } else { None },
                    spans: row_spans,
                });
            }
        }
        if rows.len() > MAX_LINES {
            return Err(format!("code too large to rasterise (> {MAX_LINES} rows)"));
        }
    }
    if rows.is_empty() {
        rows.push(Row {
            line_no: Some(1),
            spans: Vec::new(),
        });
    }
    Ok(ParsedCode { rows })
}

/// Tokenise one already-cleaned line into coloured spans. `in_block` carries an open block comment
/// from a previous line; returns whether a block comment is still open after this line.
fn tokenize_line(line: &str, lang: &Lang, mut in_block: bool) -> (Vec<Span>, bool) {
    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = String::new();
    let mut buf_ink = FG;
    let mut i = 0usize;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                spans.push(Span {
                    text: std::mem::take(&mut buf),
                    ink: buf_ink,
                });
            }
        };
    }
    macro_rules! push_ink {
        ($s:expr, $ink:expr) => {{
            flush!();
            spans.push(Span {
                text: $s,
                ink: $ink,
            });
        }};
    }

    while i < chars.len() {
        // Inside an open block comment: consume to the closer (or to EOL).
        if in_block {
            if let Some((_, close)) = lang.block {
                let rest: String = chars[i..].iter().collect();
                if let Some(pos) = rest.find(close) {
                    let end = i + rest[..pos].chars().count() + close.chars().count();
                    let seg: String = chars[i..end].iter().collect();
                    push_ink!(seg, COMMENT);
                    i = end;
                    in_block = false;
                    continue;
                } else {
                    let seg: String = chars[i..].iter().collect();
                    push_ink!(seg, COMMENT);
                    return (spans, true);
                }
            }
            in_block = false;
        }

        let rest: String = chars[i..].iter().collect();

        // Block comment open.
        if let Some((open, _)) = lang.block {
            if rest.starts_with(open) {
                buf_ink = FG;
                in_block = true;
                continue;
            }
        }
        // Line comment to EOL.
        if let Some(lc) = lang.line_comment {
            if rest.starts_with(lc) {
                push_ink!(rest, COMMENT);
                return (spans, false);
            }
        }
        if lang.hash_comment && chars[i] == '#' {
            push_ink!(rest, COMMENT);
            return (spans, false);
        }
        // String literal.
        if lang.quotes.contains(&chars[i]) {
            let q = chars[i];
            let mut j = i + 1;
            let mut s = String::new();
            s.push(q);
            while j < chars.len() {
                s.push(chars[j]);
                if chars[j] == '\\' && j + 1 < chars.len() {
                    s.push(chars[j + 1]);
                    j += 2;
                    continue;
                }
                if chars[j] == q {
                    j += 1;
                    break;
                }
                j += 1;
            }
            push_ink!(s, STRING);
            i = j;
            continue;
        }
        // XML tag name colour: `<name` / `</name`.
        if lang.xml && chars[i] == '<' {
            let mut j = i + 1;
            let mut s = String::from("<");
            if j < chars.len() && chars[j] == '/' {
                s.push('/');
                j += 1;
            }
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == ':' || chars[j] == '-')
            {
                s.push(chars[j]);
                j += 1;
            }
            push_ink!(s, TAG);
            i = j;
            continue;
        }
        // Number literal.
        if chars[i].is_ascii_digit()
            && (buf.is_empty() || !buf.chars().last().unwrap().is_ascii_alphanumeric())
        {
            let mut j = i;
            let mut s = String::new();
            while j < chars.len()
                && (chars[j].is_ascii_hexdigit()
                    || chars[j] == '.'
                    || chars[j] == 'x'
                    || chars[j] == '_')
            {
                s.push(chars[j]);
                j += 1;
            }
            push_ink!(s, NUMBER);
            i = j;
            continue;
        }
        // Default character.
        buf_ink = FG;
        buf.push(chars[i]);
        i += 1;
    }
    flush!();
    (spans, in_block)
}

/// Wrap coloured spans into rows of at most `cols` characters, preserving colour at the split.
fn wrap_spans(spans: Vec<Span>, cols: usize) -> Vec<Vec<Span>> {
    let mut rows: Vec<Vec<Span>> = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut width = 0usize;
    for span in spans {
        let mut chars = span.text.chars().peekable();
        let mut chunk = String::new();
        while let Some(c) = chars.next() {
            chunk.push(c);
            width += 1;
            let last = chars.peek().is_none();
            if width >= cols || last {
                if !chunk.is_empty() {
                    cur.push(Span {
                        text: std::mem::take(&mut chunk),
                        ink: span.ink,
                    });
                }
                if width >= cols {
                    rows.push(std::mem::take(&mut cur));
                    width = 0;
                }
            }
        }
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

impl ParsedCode {
    pub fn total_pages(&self) -> u32 {
        (self.rows.len().div_ceil(ROWS)).max(1) as u32
    }

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
        let mono = font::mono();
        let ascent = font::ascent(&mono, FONT_PX);
        // Fixed-pitch advance: every glyph in a monospace face is the same width, so one cell ==
        // one column. Measuring 'M' gives that advance.
        let cell = font::measure(&mono, "M", FONT_PX);
        let gutter_px = GUTTER as f32 * cell;
        let gutter_w = MARGIN + gutter_px;
        let width = (gutter_w + COLS as f32 * cell + MARGIN).ceil() as u32;
        let height = (2.0 * MARGIN + ROWS as f32 * LINE_H).ceil() as u32;
        let mut img = RgbaImage::from_pixel(width, height, Rgba(BG));

        // Gutter background band.
        let gutter_band = (gutter_w.ceil() as u32).min(width);
        for y in 0..height {
            for x in 0..gutter_band {
                img.put_pixel(x, y, Rgba(GUTTER_BG));
            }
        }

        let start = page as usize * ROWS;
        let end = (start + ROWS).min(self.rows.len());
        for (r, row) in self.rows[start..end].iter().enumerate() {
            // Baseline = top margin + ascent + row pitch.
            let baseline = MARGIN + ascent + r as f32 * LINE_H;
            if let Some(no) = row.line_no {
                // Right-align the line number within the gutter (mono → exact column placement).
                let label = format!("{no:>width$}", width = GUTTER - 1);
                font::draw_line(&mut img, &mono, &label, MARGIN, baseline, FONT_PX, LINENO);
            }
            // Code starts after the gutter; columns advance by `cell` per character.
            let mut col = 0usize;
            for span in &row.spans {
                let x = gutter_w + col as f32 * cell;
                font::draw_line(&mut img, &mono, &span.text, x, baseline, FONT_PX, span.ink);
                col += span.text.chars().count();
            }
        }

        watermark::finalize(img, watermark_text)
    }
}

/// Whether this mime should render through the code view (dark + gutter + colour) rather than the
/// light prose reader. Source-code mimes only; `text/plain` and `text/markdown` stay on `text.rs`.
pub fn is_code_mime(mime: &str) -> bool {
    matches!(
        mime.trim().to_ascii_lowercase().as_str(),
        "application/json"
            | "application/javascript"
            | "application/xml"
            | "application/x-yaml"
            | "application/yaml"
            | "application/toml"
            | "application/x-sh"
            | "application/x-shellscript"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_json_with_line_numbers() {
        let src = b"{\n  \"name\": \"elastos\",\n  \"n\": 42\n}\n";
        let parsed = parse(src, "application/json").expect("parse");
        assert!(parsed.total_pages() >= 1);
        let jpeg = parsed
            .render_page(0, None, Some("0xBUYER"))
            .expect("render");
        assert_eq!(&jpeg[0..2], &[0xFF, 0xD8], "must emit a JPEG");
    }

    #[test]
    fn javascript_block_comment_spans_lines() {
        let src = b"/* a\n b */ var x = 1; // tail\n";
        let parsed = parse(src, "application/javascript").expect("parse");
        // First two source lines are inside the block comment; coloured as COMMENT.
        assert!(parsed.rows[0].spans.iter().all(|s| s.ink == COMMENT));
    }

    #[test]
    fn is_code_mime_excludes_prose() {
        assert!(is_code_mime("application/json"));
        assert!(is_code_mime("application/x-yaml"));
        assert!(!is_code_mime("text/plain"));
        assert!(!is_code_mime("text/markdown"));
    }

    #[test]
    fn out_of_range_page_fails_closed() {
        let parsed = parse(b"x", "application/json").expect("parse");
        assert!(parsed.render_page(9, None, None).is_err());
    }

    #[test]
    fn emit_sample() {
        if std::env::var("ELASTOS_RENDER_SAMPLE").is_err() {
            return;
        }
        let src = b"// elastos config loader\nimport fs from \"fs\";\n\nconst PORT = 8080; /* default */\nfunction load(path) {\n  const raw = fs.readFileSync(path, \"utf8\");\n  return JSON.parse(raw); // may throw\n}\n\nexport default { PORT, load };\n";
        let parsed = parse(src, "application/javascript").expect("parse");
        let jpeg = parsed
            .render_page(0, None, Some("0xBUYER..a1b2"))
            .expect("render");
        std::fs::write("/tmp/code-sample.jpg", &jpeg).expect("write");
        eprintln!("wrote /tmp/code-sample.jpg ({} bytes)", jpeg.len());
    }
}
