// Test-only archive builder. It writes local headers, a central directory and
// an end-of-central-directory record by hand so a fixture never inherits the
// parser's own idea of a well-formed archive. Not loaded by the reader.

export const LOCAL_SIGNATURE = 0x04034b50;
export const CENTRAL_SIGNATURE = 0x02014b50;
export const EOCD_SIGNATURE = 0x06054b50;
export const ZIP64_LOCATOR_SIGNATURE = 0x07064b50;
export const STORED = 0;
export const DEFLATE = 8;

const encoder = new TextEncoder();

export function text(value) {
  return encoder.encode(value);
}

// Written out independently of the reader so a fixture never inherits the
// reader's idea of a correct checksum.
export function fixtureCrc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

export async function deflateRaw(bytes) {
  const stream = new CompressionStream("deflate-raw");
  const writer = stream.writable.getWriter();
  void writer.write(bytes);
  void writer.close();
  const chunks = [];
  const reader = stream.readable.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  return concat(chunks);
}

function concat(chunks) {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}

class ByteWriter {
  constructor() {
    this.parts = [];
    this.length = 0;
  }

  u16(value) {
    const part = new Uint8Array(2);
    new DataView(part.buffer).setUint16(0, value, true);
    return this.raw(part);
  }

  u32(value) {
    const part = new Uint8Array(4);
    new DataView(part.buffer).setUint32(0, value >>> 0, true);
    return this.raw(part);
  }

  raw(bytes) {
    this.parts.push(bytes);
    this.length += bytes.length;
    return this;
  }

  bytes() {
    return concat(this.parts);
  }
}

/**
 * Builds an archive from explicit field values. Every field a fixture needs to
 * lie about is an explicit override, so a malformed fixture is malformed on
 * purpose and never by accident.
 */
export function buildZip(files, options = {}) {
  const out = new ByteWriter();
  const placed = [];
  for (const file of files) {
    const name = encoder.encode(file.name);
    const stored = file.stored ?? file.data ?? new Uint8Array(0);
    const plain = file.data ?? stored;
    const method = file.method ?? STORED;
    const flags = file.flags ?? 0;
    const crc = file.crc ?? fixtureCrc32(plain);
    const compressedSize = file.compressedSize ?? stored.length;
    const uncompressedSize = file.uncompressedSize ?? plain.length;
    const localOffset = out.length;
    out
      .u32(file.localSignature ?? LOCAL_SIGNATURE)
      .u16(20)
      .u16(flags)
      .u16(file.localMethod ?? method)
      .u16(0)
      .u16(0)
      .u32(crc)
      .u32(compressedSize)
      .u32(uncompressedSize)
      .u16(name.length)
      .u16(0)
      .raw(name)
      .raw(stored);
    placed.push({
      name,
      method,
      flags,
      crc,
      compressedSize,
      uncompressedSize,
      localOffset: file.centralLocalOffset ?? localOffset,
      extra: file.centralExtra ?? new Uint8Array(0),
      diskStart: file.centralDiskStart ?? 0,
    });
  }

  const centralOffset = out.length;
  const central = new ByteWriter();
  for (const entry of placed) {
    central
      .u32(CENTRAL_SIGNATURE)
      .u16(20)
      .u16(20)
      .u16(entry.flags)
      .u16(entry.method)
      .u16(0)
      .u16(0)
      .u32(entry.crc)
      .u32(entry.compressedSize)
      .u32(entry.uncompressedSize)
      .u16(entry.name.length)
      .u16(entry.extra.length)
      .u16(0)
      .u16(entry.diskStart)
      .u16(0)
      .u32(0)
      .u32(entry.localOffset)
      .raw(entry.name)
      .raw(entry.extra);
  }
  let centralBytes = central.bytes();
  if (typeof options.truncateCentralDirectory === "number") {
    centralBytes = centralBytes.subarray(0, centralBytes.length - options.truncateCentralDirectory);
  }
  out.raw(centralBytes);

  if (options.zip64Locator) {
    out.u32(ZIP64_LOCATOR_SIGNATURE).u32(0).u32(0).u32(0).u32(1);
  }

  out
    .u32(options.eocdSignature ?? EOCD_SIGNATURE)
    .u16(options.diskNumber ?? 0)
    .u16(options.directoryDisk ?? 0)
    .u16(options.entriesOnThisDisk ?? options.entryCount ?? placed.length)
    .u16(options.entryCount ?? placed.length)
    .u32(options.centralSize ?? centralBytes.length)
    .u32(options.centralOffset ?? centralOffset)
    .u16(0);
  return out.bytes();
}

/** Builds a stored-mode archive from a plain map of name to string or bytes. */
export function buildStoredZip(files) {
  return buildZip(
    Object.entries(files).map(([name, value]) => ({
      name,
      data: typeof value === "string" ? text(value) : value,
    })),
  );
}
