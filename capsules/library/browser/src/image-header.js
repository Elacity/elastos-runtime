// Header-only image size checks for Library thumbnails.
//
// A browser decodes an image to about four bytes per pixel, so a small file can
// still need gigabytes once decoded. Library reads only the container header
// and keeps the file icon when the canvas is too large or the header cannot be
// read. The limits are Runtime's (IMAGE_MAX_SIDE and IMAGE_MAX_PIXELS in
// elastos/crates/elastos-server/src/image_bounds.rs, the one source of truth);
// these are copies, and image-header.test.mjs fails when they differ. One
// decode of a picture at the pixel limit is about 96 MB as RGBA.

export const IMAGE_MAX_SIDE = 8192;
export const IMAGE_MAX_PIXELS = 24_000_000;

/** Bytes one decode of `header` takes as RGBA (4 bytes per pixel). */
export function decodedImageBytes(header) {
  return header.width * header.height * 4;
}

const be16 = (b, at) => (at + 2 <= b.length ? (b[at] << 8) | b[at + 1] : null);
const be32 = (b, at) => (at + 4 <= b.length ? ((b[at] << 24) >>> 0) + ((b[at + 1] << 16) | (b[at + 2] << 8) | b[at + 3]) : null);
const le16 = (b, at) => (at + 2 <= b.length ? b[at] | (b[at + 1] << 8) : null);
const le24 = (b, at) => (at + 3 <= b.length ? b[at] | (b[at + 1] << 8) | (b[at + 2] << 16) : null);
const le32 = (b, at) => (at + 4 <= b.length ? (b[at] | (b[at + 1] << 8) | (b[at + 2] << 16) | (b[at + 3] << 24)) >>> 0 : null);
const ascii = (b, at, n) => (at + n <= b.length ? String.fromCharCode(...b.subarray(at, at + n)) : "");

export function sniffImageFormat(bytes) {
  if (bytes.length >= 8 && bytes[0] === 0x89 && ascii(bytes, 1, 3) === "PNG" && bytes[4] === 0x0d && bytes[5] === 0x0a && bytes[6] === 0x1a && bytes[7] === 0x0a) return "png";
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return "jpeg";
  if (ascii(bytes, 0, 6) === "GIF87a" || ascii(bytes, 0, 6) === "GIF89a") return "gif";
  if (ascii(bytes, 0, 4) === "RIFF" && ascii(bytes, 8, 4) === "WEBP") return "webp";
  return null;
}

// CRC-32 (ISO 3309, as PNG uses it).
function crc32(b, start, end) {
  let crc = 0xffffffff;
  for (let at = start; at < end; at += 1) {
    crc ^= b[at];
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
}

// The first chunk must be a complete IHDR: length 13, type, 13 data bytes
// with valid field values, and a matching CRC. 8 + 4 + 4 + 13 + 4 = 33 bytes.
const PNG_DEPTHS = { 0: [1, 2, 4, 8, 16], 2: [8, 16], 3: [1, 2, 4, 8], 4: [8, 16], 6: [8, 16] };

function pngSize(b) {
  if (b.length < 33 || be32(b, 8) !== 13 || ascii(b, 12, 4) !== "IHDR") return null;
  const width = be32(b, 16);
  const height = be32(b, 20);
  if (width > 0x7fffffff || height > 0x7fffffff) return null;
  const depths = PNG_DEPTHS[b[25]];
  if (!depths || !depths.includes(b[24]) || b[26] !== 0 || b[27] !== 0 || b[28] > 1) return null;
  if (crc32(b, 12, 29) !== be32(b, 29)) return null;
  return { width, height };
}

// Most marker segments read before the frame header (same as Runtime).
export const JPEG_MAX_SEGMENTS_BEFORE_FRAME = 1024;

// Walks the marker segments to the first start-of-frame. Each segment must fit
// in the file, the walk is bounded, and the frame header must have the length
// its component count needs.
function jpegSize(b) {
  let at = 2;
  for (let segment = 0; segment < JPEG_MAX_SEGMENTS_BEFORE_FRAME; segment += 1) {
    if (at >= b.length || b[at] !== 0xff) return null;
    while (at < b.length && b[at] === 0xff) at += 1;
    if (at >= b.length) return null;
    const marker = b[at];
    at += 1;
    if (marker === 0x01) continue;
    // A stuffed byte, a restart marker, a second start of image, the end of
    // image, or the scan, all before any frame header.
    if (marker === 0x00 || (marker >= 0xd0 && marker <= 0xda)) return null;
    const length = be16(b, at);
    if (length === null || length < 2 || at + length > b.length) return null;
    const isFrame = marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
    if (isFrame) {
      const precision = b[at + 2];
      const components = b[at + 7];
      if (!(precision >= 2 && precision <= 16) || !(components >= 1 && components <= 4) || length !== 8 + 3 * components) {
        return null;
      }
      return { width: be16(b, at + 5), height: be16(b, at + 3) };
    }
    at += length;
  }
  return null;
}

function skipGifSubBlocks(b, at) {
  for (;;) {
    if (at >= b.length) return null;
    const size = b[at];
    at += 1;
    if (size === 0) return at;
    at += size;
    if (at > b.length) return null;
  }
}

// The logical screen bounds every frame. The file must hold at least one
// frame and reach its trailer.
function gifSize(b) {
  if (b.length < 13) return null;
  const width = le16(b, 6);
  const height = le16(b, 8);
  let at = 13;
  if (b[10] & 0x80) at += 3 << ((b[10] & 0x07) + 1);
  let frames = 0;
  for (;;) {
    if (at >= b.length) return null;
    const block = b[at];
    if (block === 0x3b) return frames > 0 ? { width, height } : null;
    if (block === 0x21) {
      if (at + 1 >= b.length) return null;
      at = skipGifSubBlocks(b, at + 2);
    } else if (block === 0x2c) {
      if (at + 9 >= b.length) return null;
      const left = le16(b, at + 1);
      const top = le16(b, at + 3);
      const frameWidth = le16(b, at + 5);
      const frameHeight = le16(b, at + 7);
      if (left + frameWidth > width || top + frameHeight > height) return null;
      const flags = b[at + 9];
      at += 10;
      if (flags & 0x80) at += 3 << ((flags & 0x07) + 1);
      if (at >= b.length) return null;
      at = skipGifSubBlocks(b, at + 1);
      frames += 1;
    } else {
      return null;
    }
    if (at === null) return null;
  }
}

// The RIFF size must not claim more than the file holds, and the first chunk
// must fit inside it with the size its format needs. Bytes after the RIFF
// payload are ignored, as decoders ignore them.
function webpSize(b) {
  const riffSize = le32(b, 4);
  if (riffSize === null || riffSize + 8 > b.length) return null;
  const riffEnd = riffSize + 8;
  const chunk = ascii(b, 12, 4);
  const chunkSize = le32(b, 16);
  const data = 20;
  if (chunkSize === null || data + chunkSize > riffEnd) return null;
  if (chunk === "VP8 ") {
    if (chunkSize < 10) return null;
    const tag = le24(b, data);
    const keyFrame = (tag & 1) === 0;
    const version = (tag >>> 1) & 0x07;
    if (!keyFrame || version > 3 || (tag >>> 5) >= chunkSize) return null;
    if (b[data + 3] !== 0x9d || b[data + 4] !== 0x01 || b[data + 5] !== 0x2a) return null;
    return { width: le16(b, data + 6) & 0x3fff, height: le16(b, data + 8) & 0x3fff };
  }
  if (chunk === "VP8L") {
    if (chunkSize < 5 || b[data] !== 0x2f) return null;
    const bits = le32(b, data + 1);
    if (bits >>> 29 !== 0) return null;
    return { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 };
  }
  if (chunk === "VP8X") {
    // A 10-byte header, followed by at least one more chunk.
    if (chunkSize !== 10 || data + chunkSize + 8 > riffEnd) return null;
    return { width: le24(b, data + 4) + 1, height: le24(b, data + 7) + 1 };
  }
  return null;
}

/** Declared canvas of a PNG, JPEG, GIF, or WebP, or null when unreadable. */
export function readImageHeader(bytes) {
  const format = sniffImageFormat(bytes);
  const size = format === "png" ? pngSize(bytes)
    : format === "jpeg" ? jpegSize(bytes)
      : format === "gif" ? gifSize(bytes)
        : format === "webp" ? webpSize(bytes)
          : null;
  return size ? { format, ...size } : null;
}

/** True only for a readable header whose decoded size is within the limits. */
export function isBoundedImage(bytes) {
  const header = readImageHeader(bytes);
  if (!header || header.width === 0 || header.height === 0) return false;
  return header.width <= IMAGE_MAX_SIDE && header.height <= IMAGE_MAX_SIDE &&
    header.width * header.height <= IMAGE_MAX_PIXELS;
}
