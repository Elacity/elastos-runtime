import assert from "node:assert/strict";
import test from "node:test";

import { MAX_ENTRY_BYTES, listEntries, readEntry } from "./zip.js";
import {
  DEFLATE,
  buildZip,
  deflateRaw,
  text,
} from "./zip-fixture.test-helper.mjs";

async function rejects(run, expected) {
  await assert.rejects(async () => run(), (error) => {
    assert.ok(error instanceof Error, "a plain Error must reach the reader");
    assert.match(error.message, expected);
    return true;
  });
}

function throws(run, expected) {
  assert.throws(run, (error) => {
    assert.ok(error instanceof Error, "a plain Error must reach the reader");
    assert.match(error.message, expected);
    return true;
  });
}

test("listEntries reports the names and sizes of a stored archive", () => {
  const bytes = buildZip([
    { name: "page1.txt", data: text("first page") },
    { name: "images/", data: new Uint8Array(0) },
    { name: "images/cover.bin", data: text("cover bytes here") },
  ]);
  const entries = listEntries(bytes);
  assert.deepEqual(
    entries.map((entry) => entry.name),
    ["page1.txt", "images/", "images/cover.bin"],
  );
  assert.deepEqual(
    entries.map((entry) => entry.uncompressedSize),
    [10, 0, 16],
  );
  assert.deepEqual(
    entries.map((entry) => entry.isDirectory),
    [false, true, false],
  );
});

test("readEntry returns the bytes of a stored entry", async () => {
  const payload = text("stored payload \u00e9\u00e8");
  const bytes = buildZip([{ name: "a.txt", data: payload }]);
  const [entry] = listEntries(bytes);
  const read = await readEntry(bytes, entry);
  assert.ok(read instanceof Uint8Array);
  assert.deepEqual([...read], [...payload]);
});

test("readEntry inflates a deflate entry", async () => {
  const payload = text("compress me ".repeat(64));
  const compressed = await deflateRaw(payload);
  const bytes = buildZip([
    { name: "big.txt", data: payload, stored: compressed, method: DEFLATE },
  ]);
  const [entry] = listEntries(bytes);
  assert.equal(entry.method, DEFLATE);
  const read = await readEntry(bytes, entry);
  assert.deepEqual([...read], [...payload]);
});

test("listEntries refuses an encrypted archive", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("secret"), flags: 0x0001 }]);
  throws(() => listEntries(bytes), /password protected/i);
});

test("listEntries refuses strong encryption and a masked directory", () => {
  throws(
    () => listEntries(buildZip([{ name: "a.txt", data: text("s"), flags: 0x0040 }])),
    /password protected/i,
  );
  throws(
    () => listEntries(buildZip([{ name: "a.txt", data: text("s"), flags: 0x2000 }])),
    /password protected/i,
  );
});

test("listEntries refuses a zip64 archive", () => {
  throws(
    () => listEntries(buildZip([{ name: "a.txt", data: text("s") }], { zip64Locator: true })),
    /cannot open/i,
  );
  throws(
    () =>
      listEntries(
        buildZip([
          {
            name: "a.txt",
            data: text("s"),
            compressedSize: 0xffffffff,
            uncompressedSize: 0xffffffff,
          },
        ]),
      ),
    /cannot open/i,
  );
  throws(
    () =>
      listEntries(
        buildZip([{ name: "a.txt", data: text("s"), centralExtra: new Uint8Array([1, 0, 0, 0]) }]),
      ),
    /cannot open/i,
  );
});

test("listEntries refuses an entry larger than the size limit", () => {
  assert.equal(MAX_ENTRY_BYTES, 64 * 1024 * 1024);
  const bytes = buildZip([
    { name: "huge.bin", data: text("tiny"), uncompressedSize: MAX_ENTRY_BYTES + 1 },
  ]);
  throws(() => listEntries(bytes), /too large/i);
});

test("listEntries refuses a compression method it cannot read", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body"), method: 12 }]);
  throws(() => listEntries(bytes), /compression/i);
});

test("listEntries refuses a path that escapes the archive", () => {
  for (const name of [
    "../../etc/passwd",
    "a/../../b.txt",
    "/etc/passwd",
    "C:\\Windows\\win.ini",
    "dir\\file.txt",
    "a//b.txt",
    ".",
  ]) {
    throws(() => listEntries(buildZip([{ name, data: text("x") }])), /file path/i);
  }
});

test("listEntries refuses a name that is not valid text", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("x") }]);
  const patched = Uint8Array.from(bytes);
  // The single central directory name sits just before the 22-byte end record.
  const nameStart = patched.length - 22 - "a.txt".length;
  patched[nameStart] = 0xff;
  throws(() => listEntries(patched), /file names|damaged/i);
});

test("listEntries refuses a directory offset outside the archive", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("x") }], { centralOffset: 0xfffffff0 - 1 });
  throws(() => listEntries(bytes), /damaged/i);
});

test("listEntries refuses a truncated central directory record", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("x") }], {
    truncateCentralDirectory: 8,
  });
  throws(() => listEntries(bytes), /damaged/i);
});

test("listEntries refuses an archive split across several files", () => {
  const file = { name: "a.txt", data: text("x") };
  throws(() => listEntries(buildZip([file], { diskNumber: 1 })), /cannot open/i);
  throws(() => listEntries(buildZip([file], { directoryDisk: 2 })), /cannot open/i);
  throws(() => listEntries(buildZip([file], { entriesOnThisDisk: 0 })), /cannot open/i);
  throws(
    () => listEntries(buildZip([{ ...file, centralDiskStart: 3 }])),
    /cannot open/i,
  );
});

test("listEntries refuses an entry count that disagrees with the records", () => {
  const bytes = buildZip([{ name: "a.txt", data: text("x") }], { entryCount: 3 });
  throws(() => listEntries(bytes), /damaged/i);
});

test("listEntries refuses bytes with no end record", () => {
  throws(() => listEntries(text("not an archive at all")), /not a readable archive/i);
  throws(() => listEntries(new Uint8Array(0)), /not a readable archive/i);
});

test("readEntry refuses a stored entry whose sizes disagree", async () => {
  const bytes = buildZip([
    { name: "a.txt", data: text("0123456789"), uncompressedSize: 4 },
  ]);
  const [entry] = listEntries(bytes);
  await rejects(() => readEntry(bytes, entry), /damaged/i);
});

test("readEntry refuses a deflate entry that inflates to the wrong size", async () => {
  const payload = text("hello hello hello hello");
  const compressed = await deflateRaw(payload);
  const short = buildZip([
    {
      name: "a.txt",
      data: payload,
      stored: compressed,
      method: DEFLATE,
      uncompressedSize: payload.length - 3,
    },
  ]);
  await rejects(() => readEntry(short, listEntries(short)[0]), /damaged/i);
  const long = buildZip([
    {
      name: "a.txt",
      data: payload,
      stored: compressed,
      method: DEFLATE,
      uncompressedSize: payload.length + 3,
    },
  ]);
  await rejects(() => readEntry(long, listEntries(long)[0]), /damaged/i);
});

test("readEntry refuses deflate bytes that are not a valid stream", async () => {
  const payload = text("hello");
  const bytes = buildZip([
    {
      name: "a.txt",
      data: payload,
      stored: text("!!!not deflate!!!"),
      method: DEFLATE,
      uncompressedSize: payload.length,
    },
  ]);
  await rejects(() => readEntry(bytes, listEntries(bytes)[0]), /damaged/i);
});

test("readEntry refuses an entry whose checksum does not match", async () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body"), crc: 0x11223344 }]);
  const [entry] = listEntries(bytes);
  await rejects(() => readEntry(bytes, entry), /damaged/i);
});

test("readEntry refuses a local header that disagrees with the directory", async () => {
  const wrongSignature = buildZip([
    { name: "a.txt", data: text("body"), localSignature: 0x01020304 },
  ]);
  await rejects(
    () => readEntry(wrongSignature, listEntries(wrongSignature)[0]),
    /damaged/i,
  );
  const wrongMethod = buildZip([
    { name: "a.txt", data: text("body"), localMethod: DEFLATE },
  ]);
  await rejects(() => readEntry(wrongMethod, listEntries(wrongMethod)[0]), /damaged/i);
});

test("readEntry refuses a local header offset outside the archive", async () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body") }]);
  const [entry] = listEntries(bytes);
  await rejects(
    () => readEntry(bytes, { ...entry, localHeaderOffset: bytes.length - 4 }),
    /damaged/i,
  );
  await rejects(() => readEntry(bytes, { ...entry, localHeaderOffset: -8 }), /damaged/i);
});

test("readEntry refuses an entry whose data runs past the end of the archive", async () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body") }]);
  const [entry] = listEntries(bytes);
  await rejects(
    () =>
      readEntry(bytes, {
        ...entry,
        compressedSize: bytes.length,
        uncompressedSize: bytes.length,
      }),
    /damaged/i,
  );
});

test("readEntry refuses a forged entry that is over the size limit", async () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body") }]);
  const [entry] = listEntries(bytes);
  await rejects(
    () => readEntry(bytes, { ...entry, uncompressedSize: MAX_ENTRY_BYTES + 1 }),
    /too large/i,
  );
});

test("readEntry refuses an entry description it did not produce", async () => {
  const bytes = buildZip([{ name: "a.txt", data: text("body") }]);
  for (const forged of [null, "a.txt", {}, { name: "a.txt" }]) {
    await rejects(() => readEntry(bytes, forged), /damaged/i);
  }
});
