# unixfs-oracle — Helia ground-truth for the runtime content plane

This is a **dev oracle**, not part of the runtime. It computes the **exact** content
addresses the real IPFS importer produces, so the Rust content plane in
`scripts/dev/ddrm-runtime-open` can be pinned **byte-for-byte** against them.

It uses `@helia/unixfs`'s `addBytes` with its defaults — the same call PC2 uses to
content-address bytes (`pc2-node/src/storage/ipfs.ts`: `fs = unixfs(helia); fs.addBytes(data)`):
CIDv1, raw leaves, 1 MiB fixed-size chunks, balanced layout, single-chunk collapse to the
raw leaf. For each fixed test vector it prints the root CID, codec, and the dag-pb root
**block bytes** (hex), so a hand-rolled encoder can be matched exactly.

## Run

```sh
cd scripts/dev/unixfs-oracle
npm install
node index.mjs
```

The test inputs use a trivial, cross-language deterministic byte stream
(`out[i] = (i + seed) & 0xff`) so the Rust tests reproduce the identical bytes without
committing large binary fixtures.

## Where the goldens are pinned

`scripts/dev/ddrm-runtime-open/src/main.rs`, test
`content_plane_tests::unixfs_root_cid_matches_helia_oracle`. If `@helia/unixfs` ever
changes its defaults, regenerate here and update that test in lockstep.
