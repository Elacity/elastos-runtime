//! Header-only image size checks.
//!
//! A browser decodes an image to about four bytes per pixel. A small file can
//! declare a very large canvas, so a byte limit alone does not bound decode
//! memory. These checks read only the container headers, allocate nothing in
//! proportion to the declared size, and refuse anything they cannot read.

/// Image formats Runtime accepts for a stored picture such as the wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageFormat {
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// Longest side Runtime accepts for a decoded picture.
pub(crate) const IMAGE_MAX_SIDE: u32 = 16_384;
/// Most pixels Runtime accepts for a decoded picture (about 200 MB as RGBA).
pub(crate) const IMAGE_MAX_PIXELS: u64 = 50_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageHeader {
    pub(crate) format: ImageFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// The format named by the leading bytes, without reading further.
pub(crate) fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(ImageFormat::Png);
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    None
}

/// Reads the declared canvas size. `None` means the header is missing,
/// truncated, or in a form this reader does not know; callers refuse it.
pub(crate) fn read_image_header(bytes: &[u8]) -> Option<ImageHeader> {
    let format = sniff_image_format(bytes)?;
    let (width, height) = match format {
        ImageFormat::Png => png_size(bytes)?,
        ImageFormat::Jpeg => jpeg_size(bytes)?,
        ImageFormat::Gif => gif_size(bytes)?,
        ImageFormat::Webp => webp_size(bytes)?,
    };
    Some(ImageHeader {
        format,
        width,
        height,
    })
}

/// Why a picture was refused. Each reason has one fixed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageBoundError {
    /// The header is missing, truncated, or in a form this reader does not know.
    Unreadable,
    /// The header declares more pixels than a browser should decode.
    TooLarge,
}

impl std::fmt::Display for ImageBoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "the picture is not a readable PNG, JPEG, WebP, or GIF",
            Self::TooLarge => "the picture is too large to show",
        })
    }
}

impl std::error::Error for ImageBoundError {}

/// Refuses a picture whose header is unreadable, declares no pixels, or
/// declares more than [`IMAGE_MAX_SIDE`] per side or [`IMAGE_MAX_PIXELS`].
pub(crate) fn require_bounded_image(bytes: &[u8]) -> Result<ImageHeader, ImageBoundError> {
    let header = read_image_header(bytes).ok_or(ImageBoundError::Unreadable)?;
    if header.width == 0 || header.height == 0 {
        return Err(ImageBoundError::Unreadable);
    }
    if header.width > IMAGE_MAX_SIDE
        || header.height > IMAGE_MAX_SIDE
        || u64::from(header.width) * u64::from(header.height) > IMAGE_MAX_PIXELS
    {
        return Err(ImageBoundError::TooLarge);
    }
    Ok(header)
}

fn be_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le_u24(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 3)?;
    Some(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

// The first chunk must be a 13-byte IHDR.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if be_u32(bytes, 8)? != 13 || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be_u32(bytes, 16)?, be_u32(bytes, 20)?))
}

// The first start-of-frame marker before the scan carries the frame size.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        while *bytes.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            // Standalone markers carry no length.
            0x01 | 0xD0..=0xD7 => continue,
            // End of image or start of scan before any frame header.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(be_u16(bytes, at)?);
        if length < 2 {
            return None;
        }
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            if length < 7 {
                return None;
            }
            let height = be_u16(bytes, at + 3)?;
            let width = be_u16(bytes, at + 5)?;
            return Some((u32::from(width), u32::from(height)));
        }
        at = at.checked_add(length)?;
    }
}

// The logical screen bounds every frame. A frame that claims more than the
// screen is refused, because some decoders size buffers from the frame.
fn gif_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let width = u32::from(le_u16(bytes, 6)?);
    let height = u32::from(le_u16(bytes, 8)?);
    let flags = *bytes.get(10)?;
    let mut at = 13;
    if flags & 0x80 != 0 {
        at += 3 << ((flags & 0x07) + 1);
    }
    loop {
        match *bytes.get(at)? {
            // Trailer.
            0x3B => return Some((width, height)),
            // Extension: label, then data sub-blocks.
            0x21 => {
                at = skip_gif_sub_blocks(bytes, at + 2)?;
            }
            // Image descriptor.
            0x2C => {
                let left = u32::from(le_u16(bytes, at + 1)?);
                let top = u32::from(le_u16(bytes, at + 3)?);
                let frame_width = u32::from(le_u16(bytes, at + 5)?);
                let frame_height = u32::from(le_u16(bytes, at + 7)?);
                if left + frame_width > width || top + frame_height > height {
                    return None;
                }
                let frame_flags = *bytes.get(at + 9)?;
                at += 10;
                if frame_flags & 0x80 != 0 {
                    at += 3 << ((frame_flags & 0x07) + 1);
                }
                // LZW minimum code size, then image data sub-blocks.
                at = skip_gif_sub_blocks(bytes, at + 1)?;
            }
            _ => return None,
        }
    }
}

fn skip_gif_sub_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let size = usize::from(*bytes.get(at)?);
        at += 1;
        if size == 0 {
            return Some(at);
        }
        at = at.checked_add(size)?;
    }
}

fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let chunk = bytes.get(12..16)?;
    let data = 20;
    match chunk {
        b"VP8 " => {
            // Frame tag (3 bytes), start code, then 14-bit sizes.
            if bytes.get(data + 3..data + 6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u32::from(le_u16(bytes, data + 6)? & 0x3FFF);
            let height = u32::from(le_u16(bytes, data + 8)? & 0x3FFF);
            Some((width, height))
        }
        b"VP8L" => {
            if *bytes.get(data)? != 0x2F {
                return None;
            }
            let bits = le_u32(bytes, data + 1)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => Some((le_u24(bytes, data + 4)? + 1, le_u24(bytes, data + 7)? + 1)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG signature and IHDR that declare `width` x `height`; no pixel data.
    pub(crate) fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes
    }

    fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8];
        // APP0 with a short body, then a fill byte before SOF2.
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00]);
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xC2, 0x00, 0x0B, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        bytes.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        bytes
    }

    fn gif(screen: (u16, u16), frame: (u16, u16)) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&screen.0.to_le_bytes());
        bytes.extend_from_slice(&screen.1.to_le_bytes());
        bytes.extend_from_slice(&[0x00, 0x00, 0x00]);
        // Graphic control extension.
        bytes.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]);
        bytes.push(0x2C);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&frame.0.to_le_bytes());
        bytes.extend_from_slice(&frame.1.to_le_bytes());
        bytes.push(0x00);
        bytes.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
        bytes.push(0x3B);
        bytes
    }

    fn webp(chunk: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&((body.len() + 12) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WEBP");
        bytes.extend_from_slice(chunk);
        bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn reads_sizes_from_each_supported_header() {
        let png = read_image_header(&png_header(1920, 1080)).unwrap();
        assert_eq!(
            (png.format, png.width, png.height),
            (ImageFormat::Png, 1920, 1080)
        );
        let jpeg = read_image_header(&jpeg_header(4000, 3000)).unwrap();
        assert_eq!(
            (jpeg.format, jpeg.width, jpeg.height),
            (ImageFormat::Jpeg, 4000, 3000)
        );
        let gif = read_image_header(&gif((640, 480), (640, 480))).unwrap();
        assert_eq!(
            (gif.format, gif.width, gif.height),
            (ImageFormat::Gif, 640, 480)
        );
        let lossy = webp(
            b"VP8 ",
            &[0x00, 0x00, 0x00, 0x9D, 0x01, 0x2A, 0x80, 0x07, 0x38, 0x04],
        );
        assert_eq!(
            read_image_header(&lossy).map(|h| (h.width, h.height)),
            Some((1920, 1080))
        );
        let bits: u32 = (1919) | (1079 << 14);
        let mut lossless_body = vec![0x2F];
        lossless_body.extend_from_slice(&bits.to_le_bytes());
        let lossless = webp(b"VP8L", &lossless_body);
        assert_eq!(
            read_image_header(&lossless).map(|h| (h.width, h.height)),
            Some((1920, 1080))
        );
        let mut extended_body = vec![0x00, 0x00, 0x00, 0x00];
        extended_body.extend_from_slice(&[0x7F, 0x07, 0x00, 0x37, 0x04, 0x00]);
        let extended = webp(b"VP8X", &extended_body);
        assert_eq!(
            read_image_header(&extended).map(|h| (h.width, h.height)),
            Some((1920, 1080))
        );
    }

    #[test]
    fn small_file_with_huge_canvas_is_refused_before_decode() {
        let bomb = png_header(100_000, 100_000);
        assert!(bomb.len() < 64, "the bomb is a tiny file");
        assert_eq!(require_bounded_image(&bomb), Err(ImageBoundError::TooLarge));
        // Under the side limit, over the pixel limit.
        assert!(require_bounded_image(&png_header(16_000, 16_000)).is_err());
        assert!(require_bounded_image(&png_header(16_385, 10)).is_err());
        assert!(require_bounded_image(&jpeg_header(65_535, 65_535)).is_err());
        assert!(require_bounded_image(&gif((65_535, 65_535), (1, 1))).is_err());
        let mut huge_extended = vec![0x00, 0x00, 0x00, 0x00];
        huge_extended.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(require_bounded_image(&webp(b"VP8X", &huge_extended)).is_err());
        // An 8000x6000 camera picture is accepted.
        assert!(require_bounded_image(&png_header(8000, 6000)).is_ok());
    }

    #[test]
    fn unreadable_or_inconsistent_headers_fail_closed() {
        assert!(require_bounded_image(b"").is_err());
        assert!(require_bounded_image(b"\x89PNG\r\n\x1a\n").is_err());
        assert_eq!(
            require_bounded_image(&png_header(0, 10)),
            Err(ImageBoundError::Unreadable)
        );
        let mut wrong_chunk = png_header(10, 10);
        wrong_chunk[12..16].copy_from_slice(b"IDAT");
        assert!(require_bounded_image(&wrong_chunk).is_err());
        // JPEG with no frame header before the scan.
        assert!(require_bounded_image(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02]).is_err());
        // A GIF frame larger than its logical screen.
        assert!(require_bounded_image(&gif((10, 10), (60_000, 60_000))).is_err());
        // A GIF that never reaches its trailer.
        let mut truncated = gif((10, 10), (10, 10));
        truncated.pop();
        assert!(require_bounded_image(&truncated).is_err());
        assert!(require_bounded_image(&webp(b"ALPH", &[0; 10])).is_err());
        assert!(require_bounded_image(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_err());
    }
}
