import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { crc32 } from "node:zlib";

import { IMAGE_MAX_PIXELS, IMAGE_MAX_SIDE, JPEG_MAX_SEGMENTS_BEFORE_FRAME, isBoundedImage, readImageHeader } from "./image-header.js";
import { THUMBNAIL_CACHE_MAX_BYTES, createLibraryThumbnails, isThumbnailCandidate } from "./thumbnails.js";

function pngHeader(width, height) {
  const bytes = new Uint8Array(33);
  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  const view = new DataView(bytes.buffer);
  view.setUint32(16, width);
  view.setUint32(20, height);
  bytes.set([8, 6, 0, 0, 0], 24);
  fixPngCrc(bytes);
  return bytes;
}

// Rewrites the IHDR CRC with Node's own CRC-32, after a test changed fields.
function fixPngCrc(bytes) {
  new DataView(bytes.buffer, bytes.byteOffset).setUint32(29, crc32(bytes.subarray(12, 29)));
}

function jpegHeader(width, height) {
  return new Uint8Array([
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00,
    0xff, 0xc2, 0x00, 0x0b, 0x08, height >> 8, height & 0xff, width >> 8, width & 0xff, 0x01, 0x01, 0x11, 0x00,
    0xff, 0xda, 0x00, 0x02, 0xff, 0xd9,
  ]);
}

function gif(screenWidth, screenHeight, frameWidth, frameHeight) {
  const le = (n) => [n & 0xff, n >> 8];
  return new Uint8Array([
    ...[0x47, 0x49, 0x46, 0x38, 0x39, 0x61], ...le(screenWidth), ...le(screenHeight), 0, 0, 0,
    0x21, 0xf9, 0x04, 0, 0, 0, 0, 0,
    0x2c, ...le(0), ...le(0), ...le(frameWidth), ...le(frameHeight), 0, 0x02, 0x02, 0x44, 0x01, 0x00,
    0x3b,
  ]);
}

function webp(chunk, body) {
  const bytes = new Uint8Array(20 + body.length);
  bytes.set([0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50]);
  bytes.set([...chunk].map((c) => c.charCodeAt(0)), 12);
  const view = new DataView(bytes.buffer);
  view.setUint32(4, body.length + 12, true);
  view.setUint32(16, body.length, true);
  bytes.set(body, 20);
  return bytes;
}

// A VP8X header for a (w + 1) x (h + 1) canvas, then a small VP8L image chunk.
function webpExtended(widthMinusOne, heightMinusOne) {
  const le24 = (n) => [n & 0xff, (n >> 8) & 0xff, (n >> 16) & 0xff];
  const body = [0, 0, 0, 0, ...le24(widthMinusOne), ...le24(heightMinusOne),
    0x56, 0x50, 0x38, 0x4c, 6, 0, 0, 0, 0x2f, 0, 0, 0, 0, 0];
  const bytes = webp("VP8X", body);
  new DataView(bytes.buffer).setUint32(16, 10, true);
  return bytes;
}

const lossyWebp = () => webp("VP8 ", [0, 0, 0, 0x9d, 0x01, 0x2a, 0x80, 0x07, 0x38, 0x04]);

function concat(...parts) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

function edited(bytes, at, value) {
  const copy = bytes.slice();
  copy[at] = value;
  return copy;
}

test("reads the declared canvas of each format Library thumbnails", () => {
  assert.deepEqual(readImageHeader(pngHeader(1920, 1080)), { format: "png", width: 1920, height: 1080 });
  assert.deepEqual(readImageHeader(jpegHeader(4000, 3000)), { format: "jpeg", width: 4000, height: 3000 });
  assert.deepEqual(readImageHeader(gif(640, 480, 640, 480)), { format: "gif", width: 640, height: 480 });
  assert.deepEqual(readImageHeader(lossyWebp()), { format: "webp", width: 1920, height: 1080 });
  assert.deepEqual(readImageHeader(webpExtended(1919, 1079)), { format: "webp", width: 1920, height: 1080 });
});

test("a tiny file that declares a huge canvas is not decoded", () => {
  const bomb = pngHeader(100_000, 100_000);
  assert.ok(bomb.length < 64);
  assert.equal(isBoundedImage(bomb), false);
  assert.equal(isBoundedImage(pngHeader(8_000, 8_000)), false, "over the pixel limit");
  assert.equal(isBoundedImage(pngHeader(8_193, 10)), false, "over the side limit");
  assert.equal(isBoundedImage(pngHeader(10, 8_193)), false, "over the side limit");
  assert.equal(isBoundedImage(pngHeader(8_192, 10)), true, "at the side limit");
  assert.equal(isBoundedImage(jpegHeader(65_535, 65_535)), false);
  assert.equal(isBoundedImage(gif(65_535, 65_535, 1, 1)), false);
  assert.equal(isBoundedImage(webpExtended(0xffffff, 0xffffff)), false);
  assert.equal(isBoundedImage(pngHeader(6000, 4000)), true, "24 MP");
  assert.equal(isBoundedImage(pngHeader(6000, 4001)), false, "one row past 24 MP");
});

test("the limits are the ones Runtime applies", () => {
  const rust = readFileSync(new URL("../../../../elastos/crates/elastos-server/src/image_bounds.rs", import.meta.url), "utf8");
  const constant = (name) => Number(rust.match(new RegExp(`const ${name}: u\\d+ = ([\\d_]+);`))[1].replaceAll("_", ""));
  assert.equal(IMAGE_MAX_SIDE, constant("IMAGE_MAX_SIDE"));
  assert.equal(IMAGE_MAX_PIXELS, constant("IMAGE_MAX_PIXELS"));
  assert.equal(IMAGE_MAX_SIDE, 8192);
  assert.equal(IMAGE_MAX_PIXELS, 24_000_000);
});

test("unreadable or inconsistent headers fail closed", () => {
  assert.equal(isBoundedImage(new Uint8Array()), false);
  assert.equal(isBoundedImage(pngHeader(0, 10)), false);
  assert.equal(isBoundedImage(new Uint8Array([0xff, 0xd8, 0xff, 0xda, 0x00, 0x02])), false);
  assert.equal(isBoundedImage(gif(10, 10, 60_000, 60_000)), false, "frame larger than its screen");
  assert.equal(isBoundedImage(gif(10, 10, 10, 10).subarray(0, 30)), false, "no trailer");
  assert.equal(isBoundedImage(new TextEncoder().encode("<svg xmlns='http://www.w3.org/2000/svg'/>")), false);
});

test("truncated and forged PNG headers fail closed", () => {
  const png = pngHeader(10, 10);
  assert.equal(isBoundedImage(png), true);
  for (const length of [16, 20, 24, 28, 29, 32]) {
    assert.equal(isBoundedImage(png.subarray(0, length)), false, `${length} bytes`);
  }
  const resized = png.slice();
  new DataView(resized.buffer).setUint32(16, 20);
  assert.equal(isBoundedImage(resized), false, "size changed after the CRC");
  const longIhdr = png.slice();
  new DataView(longIhdr.buffer).setUint32(8, 14);
  assert.equal(isBoundedImage(longIhdr), false, "IHDR length 14");
  for (const [at, value] of [[24, 3], [25, 5], [26, 1], [27, 1], [28, 2]]) {
    const forged = edited(png, at, value);
    fixPngCrc(forged);
    assert.equal(isBoundedImage(forged), false, `byte ${at} = ${value}`);
  }
});

test("truncated and forged JPEG headers fail closed, and the segment walk is bounded", () => {
  const jpeg = jpegHeader(40, 30);
  assert.equal(isBoundedImage(jpeg), true);
  for (const length of [5, 7, 12, 16, 18]) {
    assert.equal(isBoundedImage(jpeg.subarray(0, length)), false, `${length} bytes`);
  }
  const overlong = jpeg.slice();
  overlong.set([0x40, 0x00], 4);
  assert.equal(isBoundedImage(overlong), false, "segment runs past the end");
  assert.equal(isBoundedImage(edited(jpeg, 17, 3)), false, "length does not match components");
  assert.equal(isBoundedImage(edited(jpeg, 17, 0)), false, "no components");
  assert.equal(isBoundedImage(edited(jpeg, 12, 0)), false, "precision 0");
  for (const marker of [0xd0, 0xd8]) {
    assert.equal(isBoundedImage(concat([0xff, 0xd8, 0xff, marker], jpeg.subarray(2))), false, `marker ${marker}`);
  }
  const withComments = (count) => concat(
    [0xff, 0xd8],
    ...Array.from({ length: count }, () => [0xff, 0xfe, 0x00, 0x02]),
    jpeg.subarray(2),
  );
  assert.equal(isBoundedImage(withComments(JPEG_MAX_SEGMENTS_BEFORE_FRAME - 2)), true);
  assert.equal(isBoundedImage(withComments(JPEG_MAX_SEGMENTS_BEFORE_FRAME)), false);
});

test("truncated and forged WebP headers fail closed", () => {
  const lossy = lossyWebp();
  assert.equal(isBoundedImage(lossy), true);
  for (const length of [12, 16, 20, 25, lossy.length - 1]) {
    assert.equal(isBoundedImage(lossy.subarray(0, length)), false, `${length} bytes`);
  }
  const riffTooLarge = lossy.slice();
  new DataView(riffTooLarge.buffer).setUint32(4, 1000, true);
  assert.equal(isBoundedImage(riffTooLarge), false, "RIFF size larger than the file");
  const chunkTooLarge = lossy.slice();
  new DataView(chunkTooLarge.buffer).setUint32(16, 64, true);
  assert.equal(isBoundedImage(chunkTooLarge), false, "chunk larger than the RIFF payload");
  assert.equal(isBoundedImage(edited(lossy, 20, 0x01)), false, "not a key frame");
  assert.equal(isBoundedImage(edited(lossy, 20, 0x0e)), false, "unknown VP8 version");
  const bits = (9 | (9 << 14) | (1 << 29)) >>> 0;
  assert.equal(isBoundedImage(webp("VP8L", [0x2f, bits & 0xff, (bits >> 8) & 0xff, (bits >> 16) & 0xff, bits >>> 24])), false, "VP8L version 1");
  const shortExtended = webpExtended(9, 9);
  new DataView(shortExtended.buffer).setUint32(16, 9, true);
  assert.equal(isBoundedImage(shortExtended), false, "VP8X of 9 bytes");
  assert.equal(isBoundedImage(webp("VP8X", [0, 0, 0, 0, 9, 0, 0, 9, 0, 0])), false, "VP8X with no image");
  assert.equal(isBoundedImage(concat(lossy, new TextEncoder().encode("trailing"))), true, "trailing bytes ignored");
});

test("truncated and forged GIF headers fail closed", () => {
  const good = gif(10, 10, 10, 10);
  assert.equal(isBoundedImage(good), true);
  for (const length of [6, 10, 12, 13, 20, 25, good.length - 1]) {
    assert.equal(isBoundedImage(good.subarray(0, length)), false, `${length} bytes`);
  }
  assert.equal(isBoundedImage(concat(good.subarray(0, 13), [0x3b])), false, "no frame");
  assert.equal(isBoundedImage(edited(good, 10, 0x87)), false, "missing global color table");
  assert.equal(isBoundedImage(edited(good, good.length - 5, 0x40)), false, "sub-block past the end");
});

test("thumbnails are offered only for formats whose header Library reads", () => {
  const object = (mime, size = 1024) => ({ uri: "localhost://Users/a/Pictures/x", kind: "file", mime, size });
  assert.equal(isThumbnailCandidate(object("image/png")), true);
  assert.equal(isThumbnailCandidate(object("image/svg+xml")), false);
  assert.equal(isThumbnailCandidate(object("image/bmp")), false);
  assert.equal(isThumbnailCandidate(object("image/png", 5 * 1024 * 1024)), false);
});

test("a bomb keeps the file icon and is never turned into an object URL", async () => {
  const created = [];
  const originalCreate = URL.createObjectURL;
  URL.createObjectURL = (blob) => {
    created.push(blob);
    return `blob:test/${created.length}`;
  };
  try {
    const img = { dataset: {}, src: "" };
    const node = {
      isConnected: true,
      classList: { add() {} },
      querySelector: () => img,
    };
    const blobs = {
      "localhost://Users/a/Pictures/bomb.png": new Blob([pngHeader(100_000, 100_000)]),
      "localhost://Users/a/Pictures/ok.png": new Blob([pngHeader(800, 600)]),
    };
    const thumbnails = createLibraryThumbnails({ readObjectBlob: async (uri) => blobs[uri], root: null });
    const object = (name) => ({ uri: `localhost://Users/a/Pictures/${name}`, kind: "file", mime: "image/png", size: 40 });
    thumbnails.observe(node, object("bomb.png"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(created.length, 0);
    assert.equal(img.src, "");
    const okImg = { dataset: {}, src: "" };
    thumbnails.observe({ ...node, querySelector: () => okImg }, object("ok.png"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(created.length, 1);
    assert.equal(okImg.src, "blob:test/1");
  } finally {
    URL.createObjectURL = originalCreate;
  }
});

test("the thumbnail cache evicts once its pictures pass the byte limit", async () => {
  const created = [];
  const revoked = [];
  const originalCreate = URL.createObjectURL;
  const originalRevoke = URL.revokeObjectURL;
  URL.createObjectURL = (blob) => {
    created.push(blob);
    return `blob:test/${created.length}`;
  };
  URL.revokeObjectURL = (url) => revoked.push(url);
  try {
    // 6000 x 4000 decodes to 96,000,000 bytes: two fit in 256 MiB, three do not.
    const big = pngHeader(6000, 4000);
    const perPicture = 6000 * 4000 * 4 + big.length;
    assert.ok(2 * perPicture <= THUMBNAIL_CACHE_MAX_BYTES && 3 * perPicture > THUMBNAIL_CACHE_MAX_BYTES);
    const thumbnails = createLibraryThumbnails({ readObjectBlob: async () => new Blob([big]), root: null });
    const show = async (name) => {
      const img = { dataset: {}, src: "" };
      thumbnails.observe({ isConnected: true, classList: { add() {} }, querySelector: () => img },
        { uri: `localhost://Users/a/Pictures/${name}`, kind: "file", mime: "image/png", size: big.length });
      await new Promise((resolve) => setTimeout(resolve, 20));
    };
    await show("one.png");
    await show("two.png");
    assert.deepEqual(revoked, []);
    assert.equal(thumbnails.cachedBytes(), 2 * perPicture);
    await show("three.png");
    assert.deepEqual(revoked, ["blob:test/1"], "the oldest picture is released");
    assert.equal(thumbnails.cachedCount(), 2);
    assert.equal(thumbnails.cachedBytes(), 2 * perPicture);
    assert.ok(thumbnails.cachedBytes() <= THUMBNAIL_CACHE_MAX_BYTES);
  } finally {
    URL.createObjectURL = originalCreate;
    URL.revokeObjectURL = originalRevoke;
  }
});
