//! Comic-book (CBZ) renderer — multi-page pixel-lock.
//!
//! A CBZ is a ZIP archive whose entries are page images (usually JPEG/PNG, in reading order).
//! We open the archive INSIDE the decrypt boundary, collect the image entries in natural page
//! order, and render each page on demand (decode → downscale → watermark → JPEG). The archive
//! and the page images never leave the sandbox — only the flattened, buyer-stamped JPEG does.
//!
//! CBR (RAR) is intentionally unsupported: RAR is a proprietary format with no safe pure-Rust
//! reader, so it fails closed (the creator should publish CBZ).

use std::io::Read;

use super::watermark;

/// Page-image entries (raw bytes, in reading order) held warm for the session. Each render
/// decodes one page on demand; the page-image cache upstream serves re-visits.
pub struct ParsedCbz {
    pages: Vec<Vec<u8>>,
}

/// Image entry extensions we treat as comic pages (lowercased). Anything else in the archive
/// (metadata XML, thumbnails dir, `__MACOSX`, …) is ignored.
fn is_page_name(name: &str) -> bool {
    // Skip directories, hidden/resource-fork entries.
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.is_empty() || base.starts_with('.') || name.contains("__MACOSX") {
        return false;
    }
    match base.rsplit_once('.') {
        Some((_, ext)) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tif" | "tiff"
        ),
        None => false,
    }
}

/// Natural (human) ordering so `2.jpg` sorts before `10.jpg` (not lexicographic). Compares
/// digit runs numerically and other runs byte-wise.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (sa, sb) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // Strip leading zeros, then compare by length, then lexically.
            let na = &a[sa..i];
            let nb = &b[sb..j];
            let ta = na.iter().skip_while(|&&c| c == b'0').count();
            let tb = nb.iter().skip_while(|&&c| c == b'0').count();
            let la = &na[na.len() - ta..];
            let lb = &nb[nb.len() - tb..];
            match la.len().cmp(&lb.len()).then_with(|| la.cmp(lb)) {
                Ordering::Equal => {}
                ord => return ord,
            }
        } else {
            match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                ord => return ord,
            }
        }
    }
    a.len().cmp(&b.len())
}

/// Maximum page entries we will index from one archive (defense against a hostile/huge CBZ).
const MAX_PAGES: usize = 5000;

/// Open the CBZ archive once and collect its page images in reading order. Fails closed on a
/// malformed archive or one with no image pages.
pub fn parse(object: &[u8]) -> Result<ParsedCbz, String> {
    let reader = std::io::Cursor::new(object.to_vec());
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("cbz open: {e}"))?;

    let mut indexed: Vec<(String, usize)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| format!("cbz entry {i}: {e}"))?;
        if entry.is_file() && is_page_name(entry.name()) {
            indexed.push((entry.name().to_string(), i));
        }
    }
    if indexed.is_empty() {
        return Err("cbz has no image pages".to_string());
    }
    if indexed.len() > MAX_PAGES {
        return Err(format!(
            "cbz has too many pages ({} > {MAX_PAGES})",
            indexed.len()
        ));
    }
    indexed.sort_by(|x, y| natural_cmp(&x.0, &y.0));

    let mut pages = Vec::with_capacity(indexed.len());
    for (_, i) in &indexed {
        let mut entry = archive
            .by_index(*i)
            .map_err(|e| format!("cbz read entry {i}: {e}"))?;
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("cbz read entry {i}: {e}"))?;
        pages.push(buf);
    }
    Ok(ParsedCbz { pages })
}

impl ParsedCbz {
    pub fn total_pages(&self) -> u32 {
        self.pages.len() as u32
    }

    pub fn render_page(
        &self,
        page: u32,
        max_width: Option<u32>,
        watermark: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let raw = self.pages.get(page as usize).ok_or_else(|| {
            format!(
                "page {} out of range (total: {})",
                page + 1,
                self.pages.len()
            )
        })?;
        let decoded = image::load_from_memory(raw)
            .map_err(|e| format!("cbz page {} decode: {e}", page + 1))?
            .to_rgba8();
        let img = watermark::fit_width(decoded, max_width.or(Some(1600)));
        watermark::finalize(img, watermark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn png_bytes(shade: u8) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([shade, shade, shade, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// Build a tiny CBZ (stored) with pages out of lexicographic order to exercise natural sort.
    fn tiny_cbz() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for name in ["10.png", "2.png", "1.png"] {
                zip.start_file(name, opts).unwrap();
                zip.write_all(&png_bytes(100)).unwrap();
            }
            // A non-image entry that must be ignored.
            zip.start_file("ComicInfo.xml", opts).unwrap();
            zip.write_all(b"<xml/>").unwrap();
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn renders_comic_pages_in_natural_order() {
        let parsed = parse(&tiny_cbz()).expect("open the CBZ");
        assert_eq!(parsed.total_pages(), 3, "only the 3 image entries count");
        let bytes = parsed
            .render_page(0, Some(400), Some("0xBUYER"))
            .expect("render page 1");
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "output must be a JPEG");
        assert!(
            parsed.render_page(3, None, None).is_err(),
            "OOB fails closed"
        );
    }

    #[test]
    fn natural_order_is_numeric() {
        use std::cmp::Ordering;
        // The property that matters for comics: numeric runs compare by VALUE, not lexically.
        assert_eq!(natural_cmp("2.png", "10.png"), Ordering::Less);
        assert_eq!(natural_cmp("page2.jpg", "page10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("009.png", "010.png"), Ordering::Less);
    }

    #[test]
    fn garbage_input_fails_closed() {
        assert!(parse(b"not a zip").is_err());
    }
}
