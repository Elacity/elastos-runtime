//! EPUB renderer — multi-"page" (one per spine chapter) HTML-LOCK for `application/epub+zip`.
//!
//! Unlike the pixel-lock renderers (which rasterise to a JPEG), a reflowable EPUB is served as a
//! sanitised, self-contained HTML document per chapter — the "html-lock" tier, mirroring PC2's
//! `EpubRenderer`. The raw EPUB (a ZIP of XHTML+CSS+images) NEVER leaves the decrypt boundary;
//! only inert, script-free chapter HTML egresses, displayed by the viewer inside a locked,
//! script-less sandbox iframe (no `allow-scripts`, no `allow-same-origin`) with a forensic
//! watermark overlay. Author CSS/JS is stripped and replaced with our readable reader CSS, so a
//! hostile book cannot script, navigate, or phone home.
//!
//! Containment layers (defense in depth):
//!   1. ZIP/XHTML parsed + sanitised in THIS boundary; raw bytes never egress.
//!   2. Output is script-free HTML (scripts/styles/handlers/dangerous tags removed here).
//!   3. The viewer renders it in a `sandbox` iframe with NO `allow-scripts` (true containment).
//!   4. A tiled buyer watermark + `user-select:none` deter casual copy/redistribution.

use std::io::{Cursor, Read};

use roxmltree::Document;

/// Cap on inlined images per chapter and per-image bytes, so a hostile book cannot blow up the
/// chapter HTML (data-URI inlining is ~1.33× the raw image).
const MAX_INLINE_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Cap on a single chapter's raw XHTML, defending against a decompression-bomb chapter.
const MAX_CHAPTER_BYTES: usize = 8 * 1024 * 1024;
/// Cap on spine length (chapters), so `total_pages` and per-page reads stay bounded.
const MAX_SPINE: usize = 5_000;

/// A parsed EPUB ready to serve chapters on demand. Holds the raw container bytes (re-read
/// per chapter so only the requested chapter's plaintext is materialised) plus the resolved
/// reading order. The bytes never leave this boundary.
pub struct ParsedEpub {
    zip: Vec<u8>,
    /// Absolute (zip-internal) paths of the spine documents, in reading order.
    spine: Vec<String>,
    title: Option<String>,
}

/// Parse an EPUB container: locate the OPF via `META-INF/container.xml`, then read the OPF
/// manifest + spine to resolve the chapter reading order. Fails closed on a malformed package.
pub fn parse(object: &[u8]) -> Result<ParsedEpub, String> {
    // Validate it's a zip + locate the OPF path.
    let opf_path = {
        let container = read_zip_entry(object, "META-INF/container.xml")
            .ok_or("not an EPUB: missing META-INF/container.xml")?;
        let container = String::from_utf8_lossy(&container);
        let doc = Document::parse(&container).map_err(|e| format!("container.xml parse: {e}"))?;
        doc.descendants()
            .find(|n| n.has_tag_name("rootfile"))
            .and_then(|n| n.attribute("full-path"))
            .map(|s| s.to_string())
            .ok_or("container.xml has no rootfile full-path")?
    };

    let opf_bytes =
        read_zip_entry(object, &opf_path).ok_or_else(|| format!("OPF not found: {opf_path}"))?;
    let opf = String::from_utf8_lossy(&opf_bytes);
    let doc = Document::parse(&opf).map_err(|e| format!("OPF parse: {e}"))?;

    // OPF directory: hrefs in the manifest are relative to it.
    let opf_dir = parent_dir(&opf_path);

    // manifest: id -> href (zip-internal absolute path).
    let mut manifest: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        if let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) {
            manifest.insert(id.to_string(), join_zip_path(&opf_dir, href));
        }
    }

    let title = doc
        .descendants()
        .find(|n| n.has_tag_name("title"))
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // spine: ordered idrefs -> chapter paths.
    let mut spine: Vec<String> = Vec::new();
    for itemref in doc.descendants().filter(|n| n.has_tag_name("itemref")) {
        if let Some(idref) = itemref.attribute("idref") {
            if let Some(path) = manifest.get(idref) {
                spine.push(path.clone());
                if spine.len() >= MAX_SPINE {
                    break;
                }
            }
        }
    }
    if spine.is_empty() {
        return Err("EPUB spine is empty (no readable chapters)".into());
    }
    Ok(ParsedEpub {
        zip: object.to_vec(),
        spine,
        title,
    })
}

impl ParsedEpub {
    pub fn total_pages(&self) -> u32 {
        self.spine.len().min(MAX_SPINE) as u32
    }

    /// Build the sanitised, self-contained HTML document for chapter `page`. `max_width` is
    /// ignored (reflowable text). `watermark` is tiled across the chapter as a forensic overlay.
    pub fn render_page(
        &self,
        page: u32,
        _max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        // Fail closed: a protected chapter must never egress without a traceable forensic stamp,
        // exactly like the pixel-lock egress path (`watermark::finalize`).
        let watermark = watermark
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("refusing to emit a protected chapter without a forensic watermark")?;
        let watermark = Some(watermark);

        let total = self.total_pages();
        if page >= total {
            return Err(format!(
                "chapter {} out of range (total: {total})",
                page + 1
            ));
        }
        let chapter_path = &self.spine[page as usize];
        let raw = read_zip_entry(&self.zip, chapter_path)
            .ok_or_else(|| format!("chapter not found in EPUB: {chapter_path}"))?;
        if raw.len() > MAX_CHAPTER_BYTES {
            return Err("EPUB chapter too large to render".into());
        }
        let xhtml = String::from_utf8_lossy(&raw);
        let body = extract_body(&xhtml);
        let sanitized = sanitize_html(&body);
        let chapter_dir = parent_dir(chapter_path);
        let inlined = self.inline_images(&sanitized, &chapter_dir);
        let html = wrap_document(
            self.title.as_deref().unwrap_or("Protected book"),
            &inlined,
            watermark,
            page + 1,
            total,
        );
        Ok(html.into_bytes())
    }

    /// Replace `<img src="relative">` references with inlined `data:` URIs read from the ZIP, so
    /// the chapter is fully self-contained (the sandbox iframe has no network + no same-origin).
    /// Images that can't be resolved/encoded are dropped (their `src` is blanked).
    fn inline_images(&self, html: &str, chapter_dir: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut count = 0usize;
        let mut rest = html;
        while let Some(pos) = find_ci(rest, "src=") {
            out.push_str(&rest[..pos + 4]);
            rest = &rest[pos + 4..];
            // Parse the quoted value.
            let quote = rest.chars().next().unwrap_or('"');
            if quote != '"' && quote != '\'' {
                continue;
            }
            let after = &rest[1..];
            let Some(end) = after.find(quote) else {
                out.push_str(rest);
                rest = "";
                break;
            };
            let value = &after[..end];
            rest = &after[end..]; // points at the closing quote
            let replacement = if value.starts_with("data:") {
                value.to_string()
            } else if count < 64 {
                match self.image_data_uri(chapter_dir, value) {
                    Some(uri) => {
                        count += 1;
                        uri
                    }
                    None => String::new(),
                }
            } else {
                String::new()
            };
            out.push(quote);
            out.push_str(&replacement);
            // leave the closing quote in `rest` to be copied on the next loop / final push
        }
        out.push_str(rest);
        out
    }

    fn image_data_uri(&self, chapter_dir: &str, href: &str) -> Option<String> {
        let path = join_zip_path(chapter_dir, strip_fragment(href));
        let bytes = read_zip_entry(&self.zip, &path)?;
        if bytes.is_empty() || bytes.len() > MAX_INLINE_IMAGE_BYTES {
            return None;
        }
        let mime = image_mime_for(&path)?;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(format!("data:{mime};base64,{b64}"))
    }
}

/// MSE-style `addSourceBuffer` page content type for the html-lock tier.
pub const PAGE_CONTENT_TYPE: &str = "text/html; charset=utf-8";

// ---------------------------------------------------------------------------
// ZIP + path helpers
// ---------------------------------------------------------------------------

/// Read one entry from a ZIP archive in `bytes`. Returns None if the archive or entry is absent.
fn read_zip_entry(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut file = zip.by_name(name).ok()?;
    let mut out = Vec::new();
    file.read_to_end(&mut out).ok()?;
    Some(out)
}

/// The directory portion of a zip-internal path (`"OEBPS/text/c1.xhtml"` -> `"OEBPS/text"`).
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// Resolve `href` relative to zip-internal `base` dir, collapsing `.`/`..` and leading `/`.
fn join_zip_path(base: &str, href: &str) -> String {
    let href = href.trim();
    let combined = if href.starts_with('/') {
        href.trim_start_matches('/').to_string()
    } else if base.is_empty() {
        href.to_string()
    } else {
        format!("{base}/{href}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn strip_fragment(href: &str) -> &str {
    href.split(['#', '?']).next().unwrap_or(href)
}

fn image_mime_for(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit('.').next()?;
    Some(match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// HTML sanitisation (defense-in-depth; the script-less sandbox iframe is the true containment)
// ---------------------------------------------------------------------------

/// Case-insensitive substring search.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.to_ascii_lowercase();
    h.find(&needle.to_ascii_lowercase())
}

/// Extract the `<body>…</body>` inner HTML if present; otherwise return the whole document
/// (after dropping any `<head>…</head>`). Keeps the renderer resilient to imperfect markup.
fn extract_body(doc: &str) -> String {
    let no_head = strip_block(doc, "head");
    if let Some(start) = find_ci(&no_head, "<body") {
        if let Some(gt) = no_head[start..].find('>') {
            let after = &no_head[start + gt + 1..];
            if let Some(end) = find_ci(after, "</body>") {
                return after[..end].to_string();
            }
            return after.to_string();
        }
    }
    no_head
}

/// Remove every `<tag …>…</tag>` block (case-insensitive), including its contents.
fn strip_block(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let Some(start) = find_ci(rest, &open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match find_ci(tail, &close) {
            Some(end) => rest = &tail[end + close.len()..],
            None => {
                // Unterminated: drop the rest (fail safe).
                break;
            }
        }
    }
    out
}

/// Remove standalone (void/unknown) tags by name, keeping any inner text (`<tag …>` -> "").
fn strip_tags_named(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        let Some(start) = find_ci(rest, &open) else {
            out.push_str(rest);
            break;
        };
        // Only treat as a tag if followed by whitespace, '>', or '/'.
        let next = rest[start + open.len()..].chars().next().unwrap_or(' ');
        if !(next.is_whitespace() || next == '>' || next == '/') {
            // Not actually this tag (e.g. <linkewise>); copy one char and continue.
            out.push_str(&rest[..start + 1]);
            rest = &rest[start + 1..];
            continue;
        }
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('>') {
            Some(end) => rest = &tail[end + 1..],
            None => break,
        }
    }
    out
}

/// Remove inline event-handler attributes (`on*="…"`/`on*='…'`), wherever they appear.
fn strip_event_handlers(html: &str) -> String {
    // Build a byte buffer (not by casting bytes to `char`, which would mangle every multi-byte
    // UTF-8 sequence — e.g. NBSP `C2 A0` → "Â\u{a0}"). `to_ascii_lowercase` keeps byte length and
    // positions, so indices into `lb` align with `bytes` and stay on UTF-8 char boundaries.
    let mut out: Vec<u8> = Vec::with_capacity(html.len());
    let bytes = html.as_bytes();
    let lower = html.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Match a space/quote boundary, then "on", letters, optional ws, '='.
        let boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if boundary && i + 2 < lb.len() && &lb[i..i + 2] == b"on" {
            let mut j = i + 2;
            while j < lb.len() && lb[j].is_ascii_alphabetic() {
                j += 1;
            }
            let mut k = j;
            while k < lb.len() && lb[k].is_ascii_whitespace() {
                k += 1;
            }
            if j > i + 2 && k < lb.len() && lb[k] == b'=' {
                // Skip the attribute and its (optionally quoted) value.
                let mut v = k + 1;
                while v < lb.len() && lb[v].is_ascii_whitespace() {
                    v += 1;
                }
                if v < lb.len() && (lb[v] == b'"' || lb[v] == b'\'') {
                    let q = lb[v];
                    v += 1;
                    while v < lb.len() && lb[v] != q {
                        v += 1;
                    }
                    v += 1; // closing quote
                } else {
                    while v < lb.len() && !lb[v].is_ascii_whitespace() && lb[v] != b'>' {
                        v += 1;
                    }
                }
                i = v;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| html.to_string())
}

/// Neutralise `javascript:` URLs in href/src by blanking the scheme.
fn neutralize_js_urls(html: &str) -> String {
    let mut out = html.to_string();
    // Cheap, case-insensitive replacements of the dangerous scheme.
    for variant in ["javascript:", "JAVASCRIPT:", "JavaScript:", "Javascript:"] {
        out = out.replace(variant, "about:blank#");
    }
    out
}

/// Extract the (unescaped) value of attribute `name` from a single start-tag string. Handles
/// quoted (`"`/`'`) and bare values, and skips false matches where `name` is a suffix of a
/// longer attribute (e.g. `href` inside `xlink:href`) by requiring a boundary before it.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = name.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(&key) {
        let idx = from + rel;
        let before_ok = idx == 0 || !{ lb[idx - 1].is_ascii_alphanumeric() || lb[idx - 1] == b':' };
        let after = idx + key.len();
        let after_trim = lower[after..].trim_start();
        if before_ok && after_trim.starts_with('=') {
            let eq = after + (lower[after..].len() - after_trim.len());
            let val = tag[eq + 1..].trim_start();
            let mut chars = val.chars();
            return match chars.next() {
                Some(q @ ('"' | '\'')) => {
                    let rest = &val[q.len_utf8()..];
                    let end = rest.find(q)?;
                    Some(rest[..end].to_string())
                }
                Some(_) => {
                    let end = val
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .unwrap_or(val.len());
                    Some(val[..end].to_string())
                }
                None => None,
            };
        }
        from = idx + key.len();
    }
    None
}

/// Rewrite SVG `<image … xlink:href|href="…"/>` (the canonical EPUB cover-page wrapper) into a
/// plain `<img src="…">`, so the cover inlines like any other image. The surrounding `<svg>` is
/// stripped by the sanitiser; an SVG `<image>` (not HTML `<img>`, and keyed on `xlink:href`, not
/// `src`) would otherwise render as a blank box — exactly the "empty first page" symptom.
fn rewrite_svg_image_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(pos) = find_ci(rest, "<image") {
        let after_name = rest[pos + 6..].chars().next().unwrap_or(' ');
        if !(after_name.is_whitespace() || after_name == '>' || after_name == '/') {
            out.push_str(&rest[..pos + 6]);
            rest = &rest[pos + 6..];
            continue;
        }
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(gt) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let tag = &tail[..=gt];
        if let Some(href) = attr_value(tag, "xlink:href").or_else(|| attr_value(tag, "href")) {
            if !href.trim().is_empty() {
                out.push_str("<img src=\"");
                out.push_str(href.trim());
                out.push_str("\"/>");
            }
        }
        rest = &tail[gt + 1..];
    }
    out.push_str(rest);
    out
}

/// Full chapter sanitisation: drop scripts/styles/dangerous blocks, void dangerous tags, strip
/// event handlers + JS URLs. The result is inert HTML; the sandbox iframe enforces the rest.
fn sanitize_html(body: &str) -> String {
    // Convert SVG cover `<image>` to `<img>` BEFORE the `svg` strip below, so the cover survives
    // (and gets inlined later) instead of vanishing with its `<svg>` wrapper.
    let body = rewrite_svg_image_tags(body);
    let mut s = strip_block(&body, "script");
    s = strip_block(&s, "style");
    s = strip_block(&s, "iframe");
    s = strip_block(&s, "object");
    s = strip_block(&s, "form");
    s = strip_block(&s, "noscript");
    for tag in ["link", "meta", "base", "embed", "input", "button", "svg"] {
        s = strip_tags_named(&s, tag);
    }
    s = strip_event_handlers(&s);
    s = neutralize_js_urls(&s);
    s
}

/// HTML-escape text for safe embedding in attributes / content.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Wrap sanitised chapter HTML in a self-contained reader document: our readable CSS, a tiled
/// forensic watermark overlay, `user-select:none`, and a strict inline CSP `<meta>` (belt; the
/// viewer also sandboxes the iframe). Fully offline (no external refs).
fn wrap_document(
    title: &str,
    chapter_html: &str,
    watermark: Option<&str>,
    page_no: u32,
    total: u32,
) -> String {
    let wm = watermark.unwrap_or("");
    let watermark_layer = if wm.is_empty() {
        String::new()
    } else {
        // A diagonal tiled, non-interactive watermark grid drawn with repeated stamps.
        let stamp = esc(wm);
        let cells: String = (0..240)
            .map(|_| format!("<span>{stamp}</span>"))
            .collect::<Vec<_>>()
            .join("");
        format!("<div class=\"wm\" aria-hidden=\"true\">{cells}</div>")
    };
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; \
style-src 'unsafe-inline'; font-src data:; base-uri 'none'; form-action 'none'\">\
<title>{title}</title><style>\
:root{{color-scheme:light}}\
*{{-webkit-user-select:none;user-select:none;-webkit-user-drag:none}}\
html,body{{margin:0;background:#fbfaf6}}\
.page{{max-width:42rem;margin:0 auto;padding:3.2rem 1.6rem 6rem;\
font:1.06rem/1.72 Georgia,'Iowan Old Style','Palatino Linotype',serif;color:#222428}}\
.page h1,.page h2,.page h3{{font-family:Georgia,serif;line-height:1.3;margin:1.6em 0 .6em}}\
.page p{{margin:0 0 1em}}\
.page img{{max-width:100%;height:auto;display:block;margin:1.2em auto}}\
.page a{{color:#3a5a9c;text-decoration:none;pointer-events:none}}\
.page pre,.page code{{font-family:ui-monospace,Menlo,Consolas,monospace;\
background:#f1efe8;border-radius:6px;padding:.1em .3em;white-space:pre-wrap}}\
.chrome{{position:fixed;left:0;right:0;bottom:0;font:0.72rem/1 ui-sans-serif,system-ui,sans-serif;\
color:#9a958a;text-align:center;padding:.5rem;background:#fbfaf6cc;backdrop-filter:blur(2px)}}\
.wm{{position:fixed;inset:0;z-index:9;pointer-events:none;overflow:hidden;\
display:flex;flex-wrap:wrap;gap:5.2rem 4.4rem;transform:rotate(-28deg) scale(1.6);\
transform-origin:center;opacity:.07}}\
.wm span{{font:700 .92rem ui-sans-serif,system-ui,sans-serif;color:#000;white-space:nowrap}}\
</style></head><body>{watermark_layer}\
<main class=\"page\">{chapter_html}</main>\
<div class=\"chrome\">Protected reading copy &middot; chapter {page_no} of {total}</div>\
</body></html>",
        title = esc(title),
        watermark_layer = watermark_layer,
        chapter_html = chapter_html,
        page_no = page_no,
        total = total,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal but valid EPUB (mimetype + container + OPF + 2 chapters) in memory.
    fn sample_epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("META-INF/container.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0"?><container version="1.0"
                xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                <rootfiles><rootfile full-path="OEBPS/content.opf"
                media-type="application/oebps-package+xml"/></rootfiles></container>"#,
            )
            .unwrap();
            zip.start_file("OEBPS/content.opf", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0"?>
                <package xmlns="http://www.idpf.org/2007/opf" version="3.0">
                <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Test Book</dc:title></metadata>
                <manifest>
                  <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
                  <item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/>
                </manifest>
                <spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#)
                .unwrap();
            zip.start_file("OEBPS/c1.xhtml", opts).unwrap();
            zip.write_all(
                br#"<html><head><title>x</title><style>body{color:red}</style></head>
                <body><h1>Chapter One</h1><p onclick="alert(1)">Hello <b>world</b>.</p>
                <script>alert('x')</script><a href="javascript:alert(2)">link</a></body></html>"#,
            )
            .unwrap();
            zip.start_file("OEBPS/c2.xhtml", opts).unwrap();
            zip.write_all(b"<html><body><h1>Chapter Two</h1><p>Second.</p></body></html>")
                .unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    #[test]
    fn parses_spine_in_order() {
        let epub = sample_epub();
        let parsed = parse(&epub).expect("parse epub");
        assert_eq!(parsed.total_pages(), 2);
        assert_eq!(parsed.title.as_deref(), Some("Test Book"));
    }

    #[test]
    fn renders_sanitised_chapter_html() {
        let epub = sample_epub();
        let parsed = parse(&epub).expect("parse");
        let html = parsed
            .render_page(0, None, Some("0xBUYER"))
            .expect("render");
        let html = String::from_utf8(html).unwrap();
        // Content survives.
        assert!(html.contains("Chapter One"));
        assert!(html.contains("world"));
        // Dangerous bits are gone.
        assert!(!html.to_lowercase().contains("<script"));
        assert!(!html.to_lowercase().contains("onclick"));
        assert!(!html.contains("javascript:"));
        assert!(!html.to_lowercase().contains("alert('x')"));
        // It's a self-contained, watermarked, CSP-bearing document.
        assert!(html.contains("Content-Security-Policy"));
        assert!(html.contains("0xBUYER"));
    }

    #[test]
    fn out_of_range_chapter_fails_closed() {
        let epub = sample_epub();
        let parsed = parse(&epub).expect("parse");
        assert!(parsed.render_page(9, None, None).is_err());
    }

    #[test]
    fn non_epub_fails_closed() {
        assert!(parse(b"not a zip at all").is_err());
    }

    #[test]
    fn preserves_multibyte_utf8_through_sanitiser() {
        // NBSP (U+00A0) + em-dash (U+2014): the byte-level event-handler stripper must NOT mangle
        // these into mojibake (the "Â Â Â" / "â€" bug from casting UTF-8 bytes to `char`).
        let body = "<p>\u{a0}\u{a0}Come, said my soul\u{2014}now.</p>";
        let out = sanitize_html(body);
        assert!(out.contains('\u{a0}'), "NBSP must survive sanitisation");
        assert!(
            out.contains('\u{2014}'),
            "em-dash must survive sanitisation"
        );
        assert!(
            !out.contains('\u{c2}'),
            "no Latin-1 mojibake 'Â' may appear"
        );
    }

    #[test]
    fn svg_cover_image_becomes_inlinable_img() {
        // The canonical EPUB cover wrapper uses an SVG <image xlink:href>, which must be rewritten
        // to an <img src> so it inlines instead of vanishing with the stripped <svg>.
        let body = r#"<div><svg viewBox="0 0 1 1"><image width="442" xlink:href="cover.jpeg"/></svg></div>"#;
        let out = sanitize_html(body);
        assert!(out.contains("<img src=\"cover.jpeg\""), "got: {out}");
        assert!(!out.to_lowercase().contains("<svg"), "svg wrapper stripped");
    }

    #[test]
    fn attr_value_skips_suffix_collisions() {
        let tag = r#"<image xlink:href="a.png" width="5"/>"#;
        assert_eq!(attr_value(tag, "xlink:href").as_deref(), Some("a.png"));
        // A bare `href` lookup must NOT match the `href` inside `xlink:href`.
        assert_eq!(attr_value(tag, "href").as_deref(), None);
        assert_eq!(attr_value(tag, "width").as_deref(), Some("5"));
    }
}
