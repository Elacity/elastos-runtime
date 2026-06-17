//! Pixel-lock secure renderer (feature `pdf-render`) — PC2 `ddrm-renderer` parity.
//!
//! Rasterises a protected, ALREADY-DECRYPTED asset to a flattened, buyer-watermarked
//! page image INSIDE this decrypt boundary, so the plaintext bytes never leave the
//! sandbox — only an opaque image egresses (the "pixel-lock" tier). The decrypt
//! boundary may *see* the plaintext (its job) but the browser/gateway only ever
//! receive the rendered image, closing the "raw plaintext reaches the client" gap
//! and side-stepping browser PDF-viewer quirks (an image always renders).
//!
//! Mirrors PC2 `wasm-renderer/src/render/*` (same `hayro` rasteriser, same bitmap
//! watermark), so the secure-view behaviour stays aligned across the two runtimes.

#[cfg(feature = "pdf-render")]
pub mod cbz;
#[cfg(feature = "pdf-render")]
pub mod image_page;
#[cfg(feature = "pdf-render")]
pub mod pdf;
#[cfg(feature = "pdf-render")]
pub mod svg;
#[cfg(feature = "pdf-render")]
pub mod text;
#[cfg(feature = "pdf-render")]
pub mod watermark;

/// Whether a (lowercased) mime is text/code we rasterise as paged document text. Source-code
/// `application/*` mimes are enumerated; everything `text/*` is included.
fn is_text_like(m: &str) -> bool {
    m.starts_with("text/")
        || matches!(
            m,
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

/// Whether a mime is served as a flattened, watermarked page image ("pixel-lock") rather than
/// as its raw bytes. Pixel-lock content NEVER egresses this boundary in plaintext — only the
/// rendered image does. Covers: PDF (multi-page), CBZ comics (multi-page), text/code
/// (multi-page), SVG (rasterised, single page), and raster images (single page). SVG is
/// rasterised rather than shipped raw precisely because it is scriptable XML.
///
/// IMPORTANT (Principle 12): the helper keeps a mirror of this predicate in
/// `scripts/dev/ddrm-media-authority/src/quorum.rs::is_pixel_lock`. Keep the two in sync.
pub fn is_pixel_lock(mime: &str) -> bool {
    let m = mime.trim().to_ascii_lowercase();
    m == "application/pdf"
        || m == "application/vnd.comicbook+zip"
        || m == "application/x-cbz"
        || is_text_like(&m)
        || m.starts_with("image/")
}

/// A WARM render session: the decrypted asset parsed ONCE plus an in-memory cache of the
/// page images already rendered. It lives entirely inside the decrypt boundary for the
/// duration of one open, so the quorum is contacted once, the object is decrypted +
/// parsed once, and every page is a fast rasterise (and re-visits are an instant cache
/// hit). The plaintext + parsed document never leave this sandbox — only JPEGs egress.
#[cfg(feature = "pdf-render")]
pub struct RenderSession {
    /// The open session this warm state belongs to; a mismatched id is refused (fail closed).
    pub session_id: String,
    pub mime: String,
    pub total_pages: u32,
    /// Cap so a hostile/huge document cannot pin unbounded memory via page caching.
    max_cached_pages: usize,
    parsed: Renderable,
    /// page index ⇒ encoded JPEG (insertion-ordered so we can evict the oldest).
    cache: std::collections::HashMap<u32, Vec<u8>>,
    order: std::collections::VecDeque<u32>,
}

/// The parsed, warm form of a pixel-lock asset. Each variant decodes/parses ONCE at `open`
/// and rasterises a page on demand; the plaintext it holds never leaves the sandbox.
#[cfg(feature = "pdf-render")]
enum Renderable {
    // The PDF and SVG parses hold large inline trees; box them so the enum stays small.
    Pdf(Box<pdf::ParsedPdf>),
    Image(image_page::ParsedImage),
    Cbz(cbz::ParsedCbz),
    Text(text::ParsedText),
    Svg(Box<svg::ParsedSvg>),
}

#[cfg(feature = "pdf-render")]
impl Renderable {
    fn total_pages(&self) -> u32 {
        match self {
            Renderable::Pdf(p) => p.total_pages,
            Renderable::Image(p) => p.total_pages(),
            Renderable::Cbz(p) => p.total_pages(),
            Renderable::Text(p) => p.total_pages(),
            Renderable::Svg(p) => p.total_pages(),
        }
    }

    fn render_page(
        &self,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        match self {
            Renderable::Pdf(p) => p.render_page(page, max_width, watermark),
            Renderable::Image(p) => p.render_page(page, max_width, watermark),
            Renderable::Cbz(p) => p.render_page(page, max_width, watermark),
            Renderable::Text(p) => p.render_page(page, max_width, watermark),
            Renderable::Svg(p) => p.render_page(page, max_width, watermark),
        }
    }
}

#[cfg(feature = "pdf-render")]
impl RenderSession {
    /// Decrypt-boundary entry: parse the already-decrypted object ONCE for `session_id`.
    /// Fails closed for non-pixel-lock mimes and malformed documents.
    pub fn open(session_id: String, mime: &str, object: &[u8]) -> Result<Self, String> {
        let mime_norm = mime.trim().to_ascii_lowercase();
        let parsed = match mime_norm.as_str() {
            "application/pdf" => Renderable::Pdf(Box::new(pdf::parse(object)?)),
            "application/vnd.comicbook+zip" | "application/x-cbz" => {
                Renderable::Cbz(cbz::parse(object)?)
            }
            m if is_text_like(m) => Renderable::Text(text::parse(object)?),
            "image/svg+xml" => Renderable::Svg(Box::new(svg::parse(object)?)),
            m if m.starts_with("image/") => Renderable::Image(image_page::parse(object)?),
            other => return Err(format!("no pixel-lock renderer for mime: {other}")),
        };
        Ok(Self {
            session_id,
            mime: mime_norm,
            total_pages: parsed.total_pages(),
            max_cached_pages: 24,
            parsed,
            cache: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        })
    }

    /// Render `page` from the warm document, serving a cached JPEG when present. Only the
    /// rasterise runs on a cache miss — never a re-decrypt or re-parse.
    pub fn page(
        &mut self,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        if let Some(bytes) = self.cache.get(&page) {
            return Ok(bytes.clone());
        }
        let bytes = self.parsed.render_page(page, max_width, watermark)?;
        if self.cache.len() >= self.max_cached_pages {
            if let Some(evict) = self.order.pop_front() {
                self.cache.remove(&evict);
            }
        }
        self.cache.insert(page, bytes.clone());
        self.order.push_back(page);
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a payload in a minimal 32-bit `mdat` box (the single-sample fragment shape
    /// the object path uses), so we can prove the boundary recovers + renders the object
    /// from a decrypted segment without the plaintext ever leaving.
    #[cfg(feature = "pdf-render")]
    fn in_mdat(payload: &[u8]) -> Vec<u8> {
        let size = (payload.len() + 8) as u32;
        let mut seg = Vec::with_capacity(payload.len() + 8);
        seg.extend_from_slice(&size.to_be_bytes());
        seg.extend_from_slice(b"mdat");
        seg.extend_from_slice(payload);
        seg
    }

    #[cfg(feature = "pdf-render")]
    #[test]
    fn extract_then_render_recovers_a_page_from_a_segment() {
        let pdf = pdf::minimal_pdf();
        let segment = in_mdat(&pdf);
        let object = extract_object_mdat(&segment).expect("mdat extracts");
        assert_eq!(object, pdf, "extracted object must equal the wrapped PDF");

        let mut session =
            RenderSession::open("session:test".to_string(), "application/pdf", &object)
                .expect("warm session opens");
        let bytes = session
            .page(0, Some(400), Some("0xBUYER"))
            .expect("rendered bytes");
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "must emit a JPEG, not the PDF");

        // A re-visit is served from the page cache (byte-identical, no re-render).
        let again = session
            .page(0, Some(400), Some("0xBUYER"))
            .expect("cache hit");
        assert_eq!(again, bytes, "cached page must match the first render");
    }

    #[cfg(feature = "pdf-render")]
    #[test]
    fn unknown_renderer_fails_closed() {
        // A pixel-lock mime with bytes that don't parse must still fail closed (no plaintext).
        assert!(
            RenderSession::open("s".to_string(), "image/png", b"not an image").is_err(),
            "a pixel-lock mime with garbage bytes must fail closed"
        );
        // A non-pixel-lock mime has no renderer.
        assert!(
            RenderSession::open("s".to_string(), "application/zip", b"not used").is_err(),
            "a non-pixel-lock mime must fail closed"
        );
    }

    #[test]
    fn pixel_lock_set_covers_pdf_image_comics_and_text() {
        assert!(is_pixel_lock("application/pdf"));
        assert!(is_pixel_lock("APPLICATION/PDF"));
        assert!(is_pixel_lock("image/png"));
        assert!(is_pixel_lock("image/jpeg"));
        assert!(is_pixel_lock("application/vnd.comicbook+zip"));
        assert!(is_pixel_lock("application/x-cbz"));
        assert!(is_pixel_lock("text/plain"));
        assert!(is_pixel_lock("text/markdown"));
        assert!(is_pixel_lock("application/json"));
        assert!(is_pixel_lock("application/javascript"));
        // SVG is pixel-lock too (rasterised, never shipped raw).
        assert!(is_pixel_lock("image/svg+xml"));
        // Excluded: non-renderable / streamed types.
        assert!(!is_pixel_lock("application/zip"));
        assert!(!is_pixel_lock("video/mp4"));
        assert!(!is_pixel_lock("audio/mpeg"));
    }

    #[test]
    fn extract_mdat_fails_closed_on_garbage() {
        assert!(extract_object_mdat(b"no boxes here").is_err());
    }
}

/// Extract the `mdat` payload (the object bytes) from a decrypted single-fragment MP4
/// segment. The protected object is carried as one CENC sample inside `mdat`; this is
/// the in-boundary analogue of the helper's extractor, so the plaintext object is
/// recovered for rendering WITHOUT ever leaving this sandbox. Fails closed on a
/// malformed/oversized box rather than reading out of bounds.
pub fn extract_object_mdat(segment: &[u8]) -> Result<Vec<u8>, String> {
    let mut off = 0usize;
    while off + 8 <= segment.len() {
        let size = u32::from_be_bytes([
            segment[off],
            segment[off + 1],
            segment[off + 2],
            segment[off + 3],
        ]) as usize;
        let typ = &segment[off + 4..off + 8];
        let (payload_start, box_end) = if size == 1 {
            if off + 16 > segment.len() {
                return Err("truncated 64-bit box header".into());
            }
            let large = u64::from_be_bytes(segment[off + 8..off + 16].try_into().unwrap()) as usize;
            (off + 16, off.checked_add(large).ok_or("box size overflow")?)
        } else if size == 0 {
            (off + 8, segment.len())
        } else {
            (off + 8, off.checked_add(size).ok_or("box size overflow")?)
        };
        if box_end > segment.len() || box_end < payload_start {
            return Err("box exceeds segment bounds".into());
        }
        if typ == b"mdat" {
            return Ok(segment[payload_start..box_end].to_vec());
        }
        off = box_end;
    }
    Err("no mdat box found in the decrypted segment".into())
}
