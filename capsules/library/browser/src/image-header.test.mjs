import assert from "node:assert/strict";
import test from "node:test";

import { IMAGE_MAX_PIXELS, isBoundedImage, readImageHeader } from "./image-header.js";
import { createLibraryThumbnails, isThumbnailCandidate } from "./thumbnails.js";

function pngHeader(width, height) {
  const bytes = new Uint8Array(33);
  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  const view = new DataView(bytes.buffer);
  view.setUint32(16, width);
  view.setUint32(20, height);
  bytes.set([8, 6, 0, 0, 0], 24);
  return bytes;
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
  new DataView(bytes.buffer).setUint32(16, body.length, true);
  bytes.set(body, 20);
  return bytes;
}

test("reads the declared canvas of each format Library thumbnails", () => {
  assert.deepEqual(readImageHeader(pngHeader(1920, 1080)), { format: "png", width: 1920, height: 1080 });
  assert.deepEqual(readImageHeader(jpegHeader(4000, 3000)), { format: "jpeg", width: 4000, height: 3000 });
  assert.deepEqual(readImageHeader(gif(640, 480, 640, 480)), { format: "gif", width: 640, height: 480 });
  const lossy = webp("VP8 ", [0, 0, 0, 0x9d, 0x01, 0x2a, 0x80, 0x07, 0x38, 0x04]);
  assert.deepEqual(readImageHeader(lossy), { format: "webp", width: 1920, height: 1080 });
  const extended = webp("VP8X", [0, 0, 0, 0, 0x7f, 0x07, 0x00, 0x37, 0x04, 0x00]);
  assert.deepEqual(readImageHeader(extended), { format: "webp", width: 1920, height: 1080 });
});

test("a tiny file that declares a huge canvas is not decoded", () => {
  const bomb = pngHeader(100_000, 100_000);
  assert.ok(bomb.length < 64);
  assert.equal(isBoundedImage(bomb), false);
  assert.equal(isBoundedImage(pngHeader(16_000, 16_000)), false, "over the pixel limit");
  assert.equal(isBoundedImage(pngHeader(16_385, 10)), false, "over the side limit");
  assert.equal(isBoundedImage(jpegHeader(65_535, 65_535)), false);
  assert.equal(isBoundedImage(gif(65_535, 65_535, 1, 1)), false);
  assert.equal(isBoundedImage(webp("VP8X", [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff])), false);
  assert.equal(isBoundedImage(pngHeader(8000, 6000)), true);
  assert.ok(8000 * 6000 <= IMAGE_MAX_PIXELS);
});

test("unreadable or inconsistent headers fail closed", () => {
  assert.equal(isBoundedImage(new Uint8Array()), false);
  assert.equal(isBoundedImage(pngHeader(0, 10)), false);
  assert.equal(isBoundedImage(new Uint8Array([0xff, 0xd8, 0xff, 0xda, 0x00, 0x02])), false);
  assert.equal(isBoundedImage(gif(10, 10, 60_000, 60_000)), false, "frame larger than its screen");
  assert.equal(isBoundedImage(gif(10, 10, 10, 10).subarray(0, 30)), false, "no trailer");
  assert.equal(isBoundedImage(new TextEncoder().encode("<svg xmlns='http://www.w3.org/2000/svg'/>")), false);
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
