// Ground-truth oracle for the runtime content plane's chunked-UnixFS support.
//
// Mirrors EXACTLY what PC2 does to content-address bytes (pc2-node/src/storage/ipfs.ts:
// `fs = unixfs(helia); fs.addBytes(data)`), using the real @helia/unixfs with its
// DEFAULTS (cidVersion 1, rawLeaves true, fixedSize chunker, balanced layout). For each
// fixed test vector it prints:
//   - the root CID (single-chunk => raw leaf `bafkrei…`; multi-chunk => dag-pb `bafybei…`)
//   - the total size and per-leaf sizes
//   - the raw leaf CIDs (in order)
//   - the root BLOCK bytes as hex (so the Rust dag-pb encoder can match byte-for-byte)
//
// Output is JSON on stdout, one object per vector under `vectors`. Deterministic.

import { unixfs } from '@helia/unixfs'
import { fixedSize } from 'ipfs-unixfs-importer/chunker'
import { balanced } from 'ipfs-unixfs-importer/layout'
import { MemoryBlockstore } from 'blockstore-core'
import { CID } from 'multiformats/cid'

// Deterministic bytes via a trivial counter pattern so the goldens are reproducible AND the
// EXACT same generator ports byte-for-byte to Rust (`(i + seed) & 0xff`), with no committed
// binary fixtures. Not crypto — just a stable, cross-language byte stream.
function detBytes (len, seed) {
  const out = new Uint8Array(len)
  for (let i = 0; i < len; i++) {
    out[i] = (i + seed) & 0xff
  }
  return out
}

const MiB = 1024 * 1024

// Test vectors: cover single-chunk (collapses to a raw leaf), exactly-on-boundary, and
// multi-chunk (2 and 3 leaves) so the dag-pb root + blocksizes are exercised.
const inputs = [
  { name: 'empty', bytes: new Uint8Array(0) },
  { name: 'abc', bytes: new TextEncoder().encode('abc') },
  { name: 'one_mib_exact', bytes: detBytes(MiB, 0x100) },
  { name: 'one_mib_plus_one', bytes: detBytes(MiB + 1, 0x101) },
  { name: 'two_and_half_mib', bytes: detBytes(2 * MiB + MiB / 2, 0x222) },
  { name: 'three_mib_exact', bytes: detBytes(3 * MiB, 0x333) }
]

const blockstore = new MemoryBlockstore()
const fs = unixfs({ blockstore })

const vectors = []
for (const { name, bytes } of inputs) {
  // EXACTLY PC2's call: fs.addBytes(data) with Helia defaults.
  const rootCid = await fs.addBytes(bytes)

  const rootBlock = await blockstore.get(rootCid)
  const rootCidStr = rootCid.toString()
  const rootCodec = rootCid.code // 0x55 raw (single-chunk), 0x70 dag-pb (multi-chunk)

  // Collect the raw leaf CIDs by walking the block: for a dag-pb root we re-parse links via
  // the exporter is overkill; instead, list every block in the store except the root, in the
  // order @helia/unixfs created them. The importer adds leaves before the root, so the store
  // iteration order (insertion) gives leaves first. We filter to raw-codec blocks.
  const leaves = []
  for await (const { cid } of blockstore.getAll()) {
    if (cid.toString() === rootCidStr) continue
    if (cid.code === 0x55) leaves.push(cid.toString())
  }

  vectors.push({
    name,
    total_size: bytes.length,
    root_cid: rootCidStr,
    root_codec: rootCodec,
    root_block_hex: Buffer.from(rootBlock).toString('hex'),
    root_block_len: rootBlock.length
  })

  // Reset the store between vectors so leaf enumeration stays clean per-vector.
  for await (const { cid } of blockstore.getAll()) {
    await blockstore.delete(cid)
  }
}

// Balanced-TREE vectors: above one root's fan-out, @helia/unixfs builds a BALANCED tree of
// intermediate dag-pb nodes (it batches the leaf stream into groups of `maxChildrenPerNode`,
// reduces each to a parent node, and recurses until one root remains). At Helia's real defaults
// (1 MiB leaves, 1024-child fan-out) the first tree needs > 1 GiB of input — impractical to pin.
// The dag-pb block encoding is INDEPENDENT of the chunk size and the fan-out, so we exercise the
// EXACT same multi-level tree-building code byte-for-byte with a REDUCED chunk size + fan-out and
// tiny inputs. Equality of the tree ROOT CID (a Merkle root) transitively proves every
// intermediate node block is byte-identical to Helia's.
const treeChunkSize = 256
const treeFanOut = 4
const treeImporterSettings = {
  chunker: fixedSize({ chunkSize: treeChunkSize }),
  layout: balanced({ maxChildrenPerNode: treeFanOut })
}
const treeInputs = [
  // 5 leaves > fan-out 4 -> 2 levels: root links [node(4 leaves), node(1 leaf)].
  { name: 'tree_5_leaves', leaves: 5, seed: 0x511 },
  // 16 leaves -> 2 levels, fully balanced: root links 4 nodes of 4 leaves each.
  { name: 'tree_16_leaves', leaves: 16, seed: 0x16a },
  // 17 leaves -> 3 levels: 5 first-level nodes -> batched [4,1] -> 2 second-level nodes -> root.
  { name: 'tree_17_leaves', leaves: 17, seed: 0x173 },
  // A non-multiple, partially-filled deep tree.
  { name: 'tree_21_leaves', leaves: 21, seed: 0x215 }
]
const treeVectors = []
for (const { name, leaves, seed } of treeInputs) {
  const totalSize = leaves * treeChunkSize - 7 // last leaf partial, so blocksizes vary
  const bytes = detBytes(totalSize, seed)
  const rootCid = await fs.addBytes(bytes, treeImporterSettings)
  treeVectors.push({
    name,
    total_size: totalSize,
    seed,
    chunk_size: treeChunkSize,
    max_children: treeFanOut,
    leaves,
    root_cid: rootCid.toString(),
    root_codec: rootCid.code
  })
  for await (const { cid } of blockstore.getAll()) {
    await blockstore.delete(cid)
  }
}

process.stdout.write(JSON.stringify({ chunk_size: MiB, vectors, tree_vectors: treeVectors }, null, 2) + '\n')
