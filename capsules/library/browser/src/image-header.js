// Header-only image size checks for Library thumbnails.
//
// A browser decodes an image to about four bytes per pixel, so a small file can
// still need gigabytes once decoded. Library reads only the container header
// and keeps the file icon when the canvas is too large or the header cannot be
// read. Runtime applies the same limits to the desktop background
// (elastos/crates/elastos-server/src/image_bounds.rs).

export const IMAGE_MAX_SIDE = 16384;
export const IMAGE_MAX_PIXELS = 50_000_000;

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

function pngSize(b) {
  if (be32(b, 8) !== 13 || ascii(b, 12, 4) !== "IHDR") return null;
  const width = be32(b, 16);
  const height = be32(b, 20);
  return width === null || height === null ? null : { width, height };
}

function jpegSize(b) {
  let at = 2;
  for (;;) {
    if (at >= b.length || b[at] !== 0xff) return null;
    while (at < b.length && b[at] === 0xff) at += 1;
    if (at >= b.length) return null;
    const marker = b[at];
    at += 1;
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
    if (marker === 0xd9 || marker === 0xda) return null;
    const length = be16(b, at);
    if (length === null || length < 2) return null;
    const isFrame = marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
    if (isFrame) {
      if (length < 7) return null;
      const height = be16(b, at + 3);
      const width = be16(b, at + 5);
      return width === null || height === null ? null : { width, height };
    }
    at += length;
  }
}

function skipGifSubBlocks(b, at) {
  for (;;) {
    if (at >= b.length) return null;
    const size = b[at];
    at += 1;
    if (size === 0) return at;
    at += size;
  }
}

function gifSize(b) {
  const width = le16(b, 6);
  const height = le16(b, 8);
  if (width === null || height === null || b.length < 13) return null;
  let at = 13;
  if (b[10] & 0x80) at += 3 << ((b[10] & 0x07) + 1);
  for (;;) {
    if (at >= b.length) return null;
    const block = b[at];
    if (block === 0x3b) return { width, height };
    if (block === 0x21) {
      at = skipGifSubBlocks(b, at + 2);
    } else if (block === 0x2c) {
      const left = le16(b, at + 1);
      const top = le16(b, at + 3);
      const frameWidth = le16(b, at + 5);
      const frameHeight = le16(b, at + 7);
      if ([left, top, frameWidth, frameHeight].includes(null) || at + 9 >= b.length) return null;
      if (left + frameWidth > width || top + frameHeight > height) return null;
      const flags = b[at + 9];
      at += 10;
      if (flags & 0x80) at += 3 << ((flags & 0x07) + 1);
      at = skipGifSubBlocks(b, at + 1);
    } else {
      return null;
    }
    if (at === null) return null;
  }
}

function webpSize(b) {
  const chunk = ascii(b, 12, 4);
  const data = 20;
  if (chunk === "VP8 ") {
    if (b[data + 3] !== 0x9d || b[data + 4] !== 0x01 || b[data + 5] !== 0x2a) return null;
    const width = le16(b, data + 6);
    const height = le16(b, data + 8);
    return width === null || height === null ? null : { width: width & 0x3fff, height: height & 0x3fff };
  }
  if (chunk === "VP8L") {
    if (b[data] !== 0x2f) return null;
    const bits = le32(b, data + 1);
    return bits === null ? null : { width: (bits & 0x3fff) + 1, height: ((bits >>> 14) & 0x3fff) + 1 };
  }
  if (chunk === "VP8X") {
    const width = le24(b, data + 4);
    const height = le24(b, data + 7);
    return width === null || height === null ? null : { width: width + 1, height: height + 1 };
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
