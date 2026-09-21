// Reads the archive containers this viewer opens: comic books and books are
// both plain zip files. Everything here runs on bytes an author chose, so every
// field is checked before it is used and every read is bounded by the buffer.

export const MAX_ENTRY_BYTES = 64 * 1024 * 1024;
export const METHOD_STORED = 0;
export const METHOD_DEFLATE = 8;

const LOCAL_SIGNATURE = 0x04034b50;
const CENTRAL_SIGNATURE = 0x02014b50;
const EOCD_SIGNATURE = 0x06054b50;
const ZIP64_LOCATOR_SIGNATURE = 0x07064b50;
const EOCD_SIZE = 22;
const CENTRAL_HEADER_SIZE = 46;
const LOCAL_HEADER_SIZE = 30;
const ZIP64_LOCATOR_SIZE = 20;
const MAX_COMMENT_BYTES = 0xffff;
const MAX_NAME_BYTES = 1024;
const ZIP64_EXTRA_ID = 0x0001;
const ZIP64_SENTINEL_32 = 0xffffffff;
const ZIP64_SENTINEL_16 = 0xffff;
const FLAG_ENCRYPTED = 0x0001;
const FLAG_STRONG_ENCRYPTION = 0x0040;
const FLAG_MASKED_DIRECTORY = 0x2000;

const NOT_AN_ARCHIVE = "This file is not a readable archive.";
const DAMAGED = "This archive is damaged.";
const ENCRYPTED = "This archive is password protected.";
const UNSUPPORTED_FORMAT = "This archive uses a layout this reader cannot open.";
const UNSUPPORTED_METHOD = "This archive uses a compression method this reader cannot open.";
const UNSAFE_PATH = "This archive contains an unsafe file path.";
const TOO_LARGE = "This archive contains an item that is too large to open.";
const UNREADABLE_NAME = "This archive uses file names this reader cannot read.";

function fail(message) {
  throw new Error(message);
}

function bufferOf(bytes) {
  if (!(bytes instanceof Uint8Array)) fail(NOT_AN_ARCHIVE);
  return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
}

function inBounds(view, at, length) {
  return (
    Number.isSafeInteger(at) &&
    Number.isSafeInteger(length) &&
    at >= 0 &&
    length >= 0 &&
    at + length <= view.byteLength
  );
}

function requireBounds(view, at, length, message) {
  if (!inBounds(view, at, length)) fail(message);
}

function u16(view, at, message) {
  requireBounds(view, at, 2, message);
  return view.getUint16(at, true);
}

function u32(view, at, message) {
  requireBounds(view, at, 4, message);
  return view.getUint32(at, true);
}

function requireOpenableFlags(flags) {
  if (flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION | FLAG_MASKED_DIRECTORY)) fail(ENCRYPTED);
}

function requireOpenableMethod(method) {
  if (method !== METHOD_STORED && method !== METHOD_DEFLATE) fail(UNSUPPORTED_METHOD);
}

function decodeName(bytes) {
  if (bytes.length === 0 || bytes.length > MAX_NAME_BYTES) fail(UNSAFE_PATH);
  let name = "";
  try {
    name = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    fail(UNREADABLE_NAME);
  }
  return name;
}

// A name is only ever used to pick an entry out of the archive, but it also
// reaches link resolution inside a book, so anything that could point outside
// the archive is refused rather than normalised away.
function requireSafeName(name) {
  if (name.includes("\0") || name.includes("\\")) fail(UNSAFE_PATH);
  if (name.startsWith("/")) fail(UNSAFE_PATH);
  if (/^[A-Za-z]:/.test(name)) fail(UNSAFE_PATH);
  const segments = name.split("/");
  for (let index = 0; index < segments.length; index += 1) {
    const segment = segments[index];
    if (segment === "." || segment === "..") fail(UNSAFE_PATH);
    // A single trailing empty segment is the directory marker; nothing else.
    if (segment === "" && index !== segments.length - 1) fail(UNSAFE_PATH);
  }
  if (segments.length === 1 && segments[0] === "") fail(UNSAFE_PATH);
  return name;
}

// Zip64 stores the real sizes in an extra field. This reader does not read
// them, so an archive that carries one is refused instead of being read with
// the placeholder values.
function requireNoZip64Extra(bytes, view, at, length) {
  requireBounds(view, at, length, DAMAGED);
  let cursor = at;
  const end = at + length;
  while (cursor + 4 <= end) {
    const id = view.getUint16(cursor, true);
    const size = view.getUint16(cursor + 2, true);
    if (id === ZIP64_EXTRA_ID) fail(UNSUPPORTED_FORMAT);
    cursor += 4 + size;
  }
  if (cursor !== end) fail(DAMAGED);
}

function findEndRecord(bytes, view) {
  const last = bytes.length - EOCD_SIZE;
  const first = Math.max(0, bytes.length - EOCD_SIZE - MAX_COMMENT_BYTES);
  for (let at = last; at >= first; at -= 1) {
    if (view.getUint32(at, true) !== EOCD_SIGNATURE) continue;
    const commentLength = view.getUint16(at + 20, true);
    if (at + EOCD_SIZE + commentLength === bytes.length) return at;
  }
  return -1;
}

let crcTable = null;

function crc32(bytes) {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let index = 0; index < 256; index += 1) {
      let value = index;
      for (let bit = 0; bit < 8; bit += 1) {
        value = value & 1 ? (value >>> 1) ^ 0xedb88320 : value >>> 1;
      }
      crcTable[index] = value >>> 0;
    }
  }
  let crc = 0xffffffff;
  for (let index = 0; index < bytes.length; index += 1) {
    crc = crcTable[(crc ^ bytes[index]) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/**
 * Reads the directory at the end of the archive and returns one description per
 * item, in the order the archive lists them.
 *
 * @param {Uint8Array} bytes
 * @returns {{
 *   name: string,
 *   method: number,
 *   flags: number,
 *   crc: number,
 *   compressedSize: number,
 *   uncompressedSize: number,
 *   localHeaderOffset: number,
 *   isDirectory: boolean,
 * }[]}
 */
export function listEntries(bytes) {
  const view = bufferOf(bytes);
  if (bytes.length < EOCD_SIZE) fail(NOT_AN_ARCHIVE);

  const endAt = findEndRecord(bytes, view);
  if (endAt < 0) fail(NOT_AN_ARCHIVE);

  if (
    endAt >= ZIP64_LOCATOR_SIZE &&
    view.getUint32(endAt - ZIP64_LOCATOR_SIZE, true) === ZIP64_LOCATOR_SIGNATURE
  ) {
    fail(UNSUPPORTED_FORMAT);
  }

  // An archive split across several files only holds part of its own content,
  // so the disk numbers are read and refused rather than passed over.
  const thisDisk = u16(view, endAt + 4, DAMAGED);
  const directoryDisk = u16(view, endAt + 6, DAMAGED);
  const entriesOnThisDisk = u16(view, endAt + 8, DAMAGED);
  const entryCount = u16(view, endAt + 10, DAMAGED);
  if (thisDisk !== 0 || directoryDisk !== 0 || entriesOnThisDisk !== entryCount) {
    fail(UNSUPPORTED_FORMAT);
  }
  const directorySize = u32(view, endAt + 12, DAMAGED);
  const directoryOffset = u32(view, endAt + 16, DAMAGED);
  if (
    entryCount === ZIP64_SENTINEL_16 ||
    directorySize === ZIP64_SENTINEL_32 ||
    directoryOffset === ZIP64_SENTINEL_32
  ) {
    fail(UNSUPPORTED_FORMAT);
  }
  requireBounds(view, directoryOffset, directorySize, DAMAGED);
  if (directoryOffset + directorySize > endAt) fail(DAMAGED);

  const entries = [];
  let cursor = directoryOffset;
  const directoryEnd = directoryOffset + directorySize;
  for (let index = 0; index < entryCount; index += 1) {
    if (cursor + CENTRAL_HEADER_SIZE > directoryEnd) fail(DAMAGED);
    if (u32(view, cursor, DAMAGED) !== CENTRAL_SIGNATURE) fail(DAMAGED);
    const flags = u16(view, cursor + 8, DAMAGED);
    requireOpenableFlags(flags);
    const method = u16(view, cursor + 10, DAMAGED);
    requireOpenableMethod(method);
    const crc = u32(view, cursor + 16, DAMAGED);
    const compressedSize = u32(view, cursor + 20, DAMAGED);
    const uncompressedSize = u32(view, cursor + 24, DAMAGED);
    const nameLength = u16(view, cursor + 28, DAMAGED);
    const extraLength = u16(view, cursor + 30, DAMAGED);
    const commentLength = u16(view, cursor + 32, DAMAGED);
    if (u16(view, cursor + 34, DAMAGED) !== 0) fail(UNSUPPORTED_FORMAT);
    const localHeaderOffset = u32(view, cursor + 42, DAMAGED);
    if (
      compressedSize === ZIP64_SENTINEL_32 ||
      uncompressedSize === ZIP64_SENTINEL_32 ||
      localHeaderOffset === ZIP64_SENTINEL_32
    ) {
      fail(UNSUPPORTED_FORMAT);
    }
    if (compressedSize > MAX_ENTRY_BYTES || uncompressedSize > MAX_ENTRY_BYTES) fail(TOO_LARGE);

    const nameAt = cursor + CENTRAL_HEADER_SIZE;
    const extraAt = nameAt + nameLength;
    const recordEnd = extraAt + extraLength + commentLength;
    if (recordEnd > directoryEnd) fail(DAMAGED);
    requireNoZip64Extra(bytes, view, extraAt, extraLength);

    const name = requireSafeName(decodeName(bytes.subarray(nameAt, extraAt)));
    if (localHeaderOffset >= directoryOffset) fail(DAMAGED);
    entries.push({
      name,
      method,
      flags,
      crc,
      compressedSize,
      uncompressedSize,
      localHeaderOffset,
      isDirectory: name.endsWith("/"),
    });
    cursor = recordEnd;
  }
  if (cursor !== directoryEnd) fail(DAMAGED);
  return entries;
}

async function inflateRaw(compressed, expectedSize) {
  const stream = new DecompressionStream("deflate-raw");
  const writer = stream.writable.getWriter();
  writer.write(compressed).catch(() => {});
  writer.close().catch(() => {});
  const reader = stream.readable.getReader();
  const out = new Uint8Array(expectedSize);
  let filled = 0;
  let overflowed = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (filled + value.byteLength > expectedSize) {
        overflowed = true;
        break;
      }
      out.set(value, filled);
      filled += value.byteLength;
    }
  } catch {
    fail(DAMAGED);
  }
  if (overflowed) {
    reader.cancel().catch(() => {});
    fail(DAMAGED);
  }
  if (filled !== expectedSize) fail(DAMAGED);
  return out;
}

function requireEntryShape(entry) {
  const shaped =
    entry &&
    typeof entry === "object" &&
    typeof entry.name === "string" &&
    Number.isSafeInteger(entry.method) &&
    Number.isSafeInteger(entry.crc) &&
    Number.isSafeInteger(entry.compressedSize) &&
    Number.isSafeInteger(entry.uncompressedSize) &&
    Number.isSafeInteger(entry.localHeaderOffset) &&
    entry.compressedSize >= 0 &&
    entry.uncompressedSize >= 0 &&
    entry.localHeaderOffset >= 0;
  if (!shaped) fail(DAMAGED);
}

/**
 * Reads one item out of the archive. The directory description is checked
 * against the item's own header before any byte is read, and the result is
 * checked against the declared size and checksum before it is returned.
 *
 * @param {Uint8Array} bytes
 * @param {ReturnType<typeof listEntries>[number]} entry
 * @returns {Promise<Uint8Array>}
 */
export async function readEntry(bytes, entry) {
  const view = bufferOf(bytes);
  requireEntryShape(entry);
  requireOpenableMethod(entry.method);
  if (entry.compressedSize > MAX_ENTRY_BYTES || entry.uncompressedSize > MAX_ENTRY_BYTES) {
    fail(TOO_LARGE);
  }

  const headerAt = entry.localHeaderOffset;
  requireBounds(view, headerAt, LOCAL_HEADER_SIZE, DAMAGED);
  if (u32(view, headerAt, DAMAGED) !== LOCAL_SIGNATURE) fail(DAMAGED);
  requireOpenableFlags(u16(view, headerAt + 6, DAMAGED));
  if (u16(view, headerAt + 8, DAMAGED) !== entry.method) fail(DAMAGED);
  const nameLength = u16(view, headerAt + 26, DAMAGED);
  const extraLength = u16(view, headerAt + 28, DAMAGED);
  const nameAt = headerAt + LOCAL_HEADER_SIZE;
  requireBounds(view, nameAt, nameLength + extraLength, DAMAGED);
  const localName = decodeName(bytes.subarray(nameAt, nameAt + nameLength));
  if (localName !== entry.name) fail(DAMAGED);
  requireNoZip64Extra(bytes, view, nameAt + nameLength, extraLength);

  const dataAt = nameAt + nameLength + extraLength;
  requireBounds(view, dataAt, entry.compressedSize, DAMAGED);
  const data = bytes.subarray(dataAt, dataAt + entry.compressedSize);

  let plain = null;
  if (entry.method === METHOD_STORED) {
    if (entry.compressedSize !== entry.uncompressedSize) fail(DAMAGED);
    plain = data.slice();
  } else {
    plain = await inflateRaw(data, entry.uncompressedSize);
  }
  if (crc32(plain) !== entry.crc) fail(DAMAGED);
  return plain;
}

/**
 * Picks one item out of a listing by exact name.
 *
 * @param {ReturnType<typeof listEntries>} entries
 * @param {string} name
 * @returns {ReturnType<typeof listEntries>[number] | null}
 */
export function findEntry(entries, name) {
  return entries.find((entry) => entry.name === name && !entry.isDirectory) || null;
}
