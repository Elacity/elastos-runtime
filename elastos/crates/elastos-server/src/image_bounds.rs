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

/// CRC-32 (ISO 3309, as PNG uses it) of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// The first chunk must be a complete IHDR: length 13, type, 13 data bytes
// with valid field values, and a matching CRC. 8 + 4 + 4 + 13 + 4 = 33 bytes.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 33 || be_u32(bytes, 8)? != 13 || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = be_u32(bytes, 16)?;
    let height = be_u32(bytes, 20)?;
    // PNG limits each side to 2^31 - 1.
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    let (bit_depth, color_type) = (bytes[24], bytes[25]);
    let depth_is_valid = match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        _ => false,
    };
    // Compression and filter method 0; interlace 0 or 1.
    if !depth_is_valid || bytes[26] != 0 || bytes[27] != 0 || bytes[28] > 1 {
        return None;
    }
    if crc32(&bytes[12..29]) != be_u32(bytes, 29)? {
        return None;
    }
    Some((width, height))
}

/// Most marker segments read before the frame header. Real files carry a few
/// APPn, DQT and DHT segments; an ICC profile split into 64 KB pieces stays
/// far below this.
const JPEG_MAX_SEGMENTS_BEFORE_FRAME: usize = 1024;

// Walks the marker segments to the first start-of-frame. Each segment must
// fit in the file, the walk stops after a bounded number of segments, and
// the frame header must have the length its component count needs.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    for _ in 0..JPEG_MAX_SEGMENTS_BEFORE_FRAME {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        // Fill bytes; bounded by the file length.
        while *bytes.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *bytes.get(at)?;
        at += 1;
        match marker {
            // TEM carries no length.
            0x01 => continue,
            // A stuffed byte, a restart marker, a second start of image, the
            // end of image, or the scan, all before any frame header.
            0x00 | 0xD0..=0xD9 | 0xDA => return None,
            _ => {}
        }
        let length = usize::from(be_u16(bytes, at)?);
        if length < 2 {
            return None;
        }
        let end = at.checked_add(length)?;
        if end > bytes.len() {
            return None;
        }
        let is_frame = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_frame {
            // Length (2), precision (1), height (2), width (2), component
            // count (1), then three bytes per component.
            let precision = *bytes.get(at + 2)?;
            let components = usize::from(*bytes.get(at + 7)?);
            if !(2..=16).contains(&precision)
                || !(1..=4).contains(&components)
                || length != 8 + 3 * components
            {
                return None;
            }
            let height = be_u16(bytes, at + 3)?;
            let width = be_u16(bytes, at + 5)?;
            return Some((u32::from(width), u32::from(height)));
        }
        at = end;
    }
    None
}

// The logical screen bounds every frame. A frame that claims more than the
// screen is refused, because some decoders size buffers from the frame. The
// file must hold at least one frame and reach its trailer.
fn gif_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 13 {
        return None;
    }
    let width = u32::from(le_u16(bytes, 6)?);
    let height = u32::from(le_u16(bytes, 8)?);
    let flags = bytes[10];
    let mut at: usize = 13;
    if flags & 0x80 != 0 {
        at = at.checked_add(3 << ((flags & 0x07) + 1))?;
    }
    let mut frames = 0usize;
    loop {
        match *bytes.get(at)? {
            // Trailer.
            0x3B => return (frames > 0).then_some((width, height)),
            // Extension: label, then data sub-blocks.
            0x21 => {
                bytes.get(at + 1)?;
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
                    at = at.checked_add(3 << ((frame_flags & 0x07) + 1))?;
                }
                // LZW minimum code size, then image data sub-blocks.
                bytes.get(at)?;
                at = skip_gif_sub_blocks(bytes, at + 1)?;
                frames += 1;
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
        if at > bytes.len() {
            return None;
        }
    }
}

// The RIFF size must not claim more than the file holds, and the first chunk
// must fit inside it with the size its format needs. Bytes after the RIFF
// payload are ignored, as decoders ignore them.
fn webp_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let riff_size = usize::try_from(le_u32(bytes, 4)?).ok()?;
    let riff_end = riff_size.checked_add(8)?;
    if riff_end > bytes.len() {
        return None;
    }
    let chunk = bytes.get(12..16)?;
    let chunk_size = usize::try_from(le_u32(bytes, 16)?).ok()?;
    let data: usize = 20;
    let chunk_end = data.checked_add(chunk_size)?;
    if chunk_end > riff_end {
        return None;
    }
    match chunk {
        b"VP8 " => {
            // Frame tag (3 bytes): a key frame, version 0 to 3, and a first
            // partition inside the chunk. Then the start code and 14-bit sizes.
            if chunk_size < 10 {
                return None;
            }
            let tag = le_u24(bytes, data)?;
            let key_frame = tag & 1 == 0;
            let version = (tag >> 1) & 0x07;
            let first_partition = usize::try_from(tag >> 5).ok()?;
            if !key_frame || version > 3 || first_partition >= chunk_size {
                return None;
            }
            if bytes.get(data + 3..data + 6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            let width = u32::from(le_u16(bytes, data + 6)? & 0x3FFF);
            let height = u32::from(le_u16(bytes, data + 8)? & 0x3FFF);
            Some((width, height))
        }
        b"VP8L" => {
            // Signature, then 14-bit sizes, an alpha bit and a 3-bit version 0.
            if chunk_size < 5 || *bytes.get(data)? != 0x2F {
                return None;
            }
            let bits = le_u32(bytes, data + 1)?;
            if bits >> 29 != 0 {
                return None;
            }
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            // A 10-byte header, followed by at least one more chunk.
            let padded_end = chunk_end + (chunk_size & 1);
            if chunk_size != 10 || padded_end.checked_add(8)? > riff_end {
                return None;
            }
            Some((le_u24(bytes, data + 4)? + 1, le_u24(bytes, data + 7)? + 1))
        }
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
        let crc = crc32(&bytes[12..29]);
        bytes.extend_from_slice(&crc.to_be_bytes());
        bytes
    }

    /// Rewrites the IHDR CRC after a test changed IHDR fields on purpose.
    fn fix_png_crc(bytes: &mut [u8]) {
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
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
        let extended = webp_extended(0x07_7F, 0x04_37);
        assert_eq!(
            read_image_header(&extended).map(|h| (h.width, h.height)),
            Some((1920, 1080))
        );
    }

    /// A VP8X header for a `width_minus_one + 1` x `height_minus_one + 1`
    /// canvas, followed by a small VP8L image chunk.
    fn webp_extended(width_minus_one: u32, height_minus_one: u32) -> Vec<u8> {
        let mut body = vec![0x00, 0x00, 0x00, 0x00];
        body.extend_from_slice(&width_minus_one.to_le_bytes()[..3]);
        body.extend_from_slice(&height_minus_one.to_le_bytes()[..3]);
        body.extend_from_slice(b"VP8L");
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(&[0x2F, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let mut bytes = webp(b"VP8X", &body);
        // The VP8X chunk itself is 10 bytes; the rest is the next chunk.
        bytes[16..20].copy_from_slice(&10u32.to_le_bytes());
        bytes
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
        assert_eq!(
            require_bounded_image(&webp_extended(0xFF_FFFF, 0xFF_FFFF)),
            Err(ImageBoundError::TooLarge)
        );
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

    #[test]
    fn truncated_png_headers_are_refused() {
        let png = png_header(10, 10);
        assert!(require_bounded_image(&png).is_ok());
        // Cut inside the sizes, inside the IHDR fields, and inside the CRC.
        for len in [16, 20, 24, 28, 29, 32] {
            assert_eq!(
                require_bounded_image(&png[..len]),
                Err(ImageBoundError::Unreadable),
                "{len} bytes"
            );
        }
    }

    #[test]
    fn forged_png_headers_are_refused() {
        // A size changed after the CRC was computed.
        let mut resized = png_header(10, 10);
        resized[16..20].copy_from_slice(&20u32.to_be_bytes());
        assert_eq!(
            require_bounded_image(&resized),
            Err(ImageBoundError::Unreadable)
        );
        // IHDR length other than 13, even with the rest intact.
        let mut long_ihdr = png_header(10, 10);
        long_ihdr[8..12].copy_from_slice(&14u32.to_be_bytes());
        assert!(require_bounded_image(&long_ihdr).is_err());
        // Field values PNG does not define, each with a correct CRC.
        for (at, value) in [(24, 3), (25, 5), (26, 1), (27, 1), (28, 2)] {
            let mut forged = png_header(10, 10);
            forged[at] = value;
            fix_png_crc(&mut forged);
            assert_eq!(
                require_bounded_image(&forged),
                Err(ImageBoundError::Unreadable),
                "byte {at} = {value}"
            );
        }
        // A side above 2^31 - 1.
        let mut wide = png_header(10, 10);
        wide[16..20].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        fix_png_crc(&mut wide);
        assert_eq!(
            require_bounded_image(&wide),
            Err(ImageBoundError::Unreadable)
        );
    }

    #[test]
    fn truncated_and_forged_jpeg_headers_are_refused() {
        let jpeg = jpeg_header(40, 30);
        assert!(require_bounded_image(&jpeg).is_ok());
        // Cut inside the APP0 segment and inside the frame header.
        for len in [5, 7, 12, 16, 18] {
            assert_eq!(
                require_bounded_image(&jpeg[..len]),
                Err(ImageBoundError::Unreadable),
                "{len} bytes"
            );
        }
        // A segment whose length runs past the end of the file.
        let mut overlong = jpeg.clone();
        overlong[4..6].copy_from_slice(&0x4000u16.to_be_bytes());
        assert!(require_bounded_image(&overlong).is_err());
        // A frame header whose length does not match its component count.
        let mut three_components = jpeg.clone();
        three_components[18] = 3;
        assert!(require_bounded_image(&three_components).is_err());
        // No components, and a precision of 0.
        let mut no_components = jpeg.clone();
        no_components[18] = 0;
        assert!(require_bounded_image(&no_components).is_err());
        let mut no_precision = jpeg.clone();
        no_precision[13] = 0;
        assert!(require_bounded_image(&no_precision).is_err());
        // A restart marker or a second start of image before the frame.
        for marker in [0xD0, 0xD8] {
            let mut stray = vec![0xFF, 0xD8, 0xFF, marker];
            stray.extend_from_slice(&jpeg[2..]);
            assert!(require_bounded_image(&stray).is_err(), "marker {marker:#x}");
        }
    }

    #[test]
    fn jpeg_segment_walk_is_bounded() {
        let with_comments = |count: usize| {
            let mut bytes = vec![0xFF, 0xD8];
            for _ in 0..count {
                bytes.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x02]);
            }
            bytes.extend_from_slice(&jpeg_header(40, 30)[2..]);
            bytes
        };
        // The APP0 segment in the fixture counts too.
        assert!(require_bounded_image(&with_comments(JPEG_MAX_SEGMENTS_BEFORE_FRAME - 2)).is_ok());
        assert_eq!(
            require_bounded_image(&with_comments(JPEG_MAX_SEGMENTS_BEFORE_FRAME)),
            Err(ImageBoundError::Unreadable)
        );
    }

    #[test]
    fn truncated_and_forged_webp_headers_are_refused() {
        let lossy = webp(
            b"VP8 ",
            &[0x00, 0x00, 0x00, 0x9D, 0x01, 0x2A, 0x80, 0x07, 0x38, 0x04],
        );
        assert!(require_bounded_image(&lossy).is_ok());
        // The file is shorter than its RIFF size says.
        for len in [12, 16, 20, 25, lossy.len() - 1] {
            assert_eq!(
                require_bounded_image(&lossy[..len]),
                Err(ImageBoundError::Unreadable),
                "{len} bytes"
            );
        }
        // A RIFF size larger than the file.
        let mut riff_too_large = lossy.clone();
        riff_too_large[4..8].copy_from_slice(&1_000u32.to_le_bytes());
        assert!(require_bounded_image(&riff_too_large).is_err());
        // A chunk size larger than the RIFF payload.
        let mut chunk_too_large = lossy.clone();
        chunk_too_large[16..20].copy_from_slice(&64u32.to_le_bytes());
        assert!(require_bounded_image(&chunk_too_large).is_err());
        // A VP8 frame that is not a key frame, and one with an unknown version.
        let mut inter_frame = lossy.clone();
        inter_frame[20] |= 0x01;
        assert!(require_bounded_image(&inter_frame).is_err());
        let mut version = lossy.clone();
        version[20] |= 0x0E;
        assert!(require_bounded_image(&version).is_err());
        // A VP8L header with a non-zero version.
        let bits: u32 = 9 | (9 << 14) | (1 << 29);
        let mut lossless = vec![0x2F];
        lossless.extend_from_slice(&bits.to_le_bytes());
        assert!(require_bounded_image(&webp(b"VP8L", &lossless)).is_err());
        // A VP8X header of the wrong size, and one with no image after it.
        let mut short_extended = webp_extended(9, 9);
        short_extended[16..20].copy_from_slice(&9u32.to_le_bytes());
        assert!(require_bounded_image(&short_extended).is_err());
        let alone = webp(b"VP8X", &[0, 0, 0, 0, 9, 0, 0, 9, 0, 0]);
        assert!(require_bounded_image(&alone).is_err());
        // Bytes after the RIFF payload are ignored.
        let mut trailing = lossy.clone();
        trailing.extend_from_slice(b"trailing");
        assert!(require_bounded_image(&trailing).is_ok());
    }

    #[test]
    fn truncated_and_forged_gif_headers_are_refused() {
        let good = gif((10, 10), (10, 10));
        assert!(require_bounded_image(&good).is_ok());
        for len in [6, 10, 12, 13, 20, 25, good.len() - 1] {
            assert_eq!(
                require_bounded_image(&good[..len]),
                Err(ImageBoundError::Unreadable),
                "{len} bytes"
            );
        }
        // A logical screen with no frame at all.
        let mut no_frame = good[..13].to_vec();
        no_frame.push(0x3B);
        assert!(require_bounded_image(&no_frame).is_err());
        // A global color table the file does not hold.
        let mut missing_table = good.clone();
        missing_table[10] = 0x87;
        assert!(require_bounded_image(&missing_table).is_err());
        // A data sub-block longer than the rest of the file.
        let mut long_block = good.clone();
        let at = long_block.len() - 5;
        long_block[at] = 0x40;
        assert!(require_bounded_image(&long_block).is_err());
    }
}
