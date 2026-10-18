//! The CID Library's plain publish gives a file, computed locally.
//!
//! Library's plain publish sends the file's bytes to the content provider,
//! which adds them to the IPFS node with Kubo's `/api/v0/add` and no import
//! options (`capsules/ipfs-provider/src/main.rs` `kubo_add_bytes`). With
//! Kubo's default import settings that is a CIDv0: SHA-256, dag-pb UnixFS
//! file nodes (no raw leaves), 256 KiB chunks, and a balanced DAG of at most
//! 174 links per node. This module computes that CID without the IPFS node,
//! so Runtime can tell whether a file still holds the bytes a record says
//! were published.
//!
//! Only DAGs of one level are built (at most 174 chunks, about 43 MiB). For
//! more, the answer is `None` and the caller treats the bytes as unknown.

use sha2::{Digest as _, Sha256};

/// Kubo's default chunk size (`size-262144`).
const CHUNK_BYTES: usize = 256 * 1024;
/// Kubo's default most links per file node.
const MAX_FILE_LINKS: usize = 174;
/// UnixFS `Data.DataType.File`.
const UNIXFS_FILE: u64 = 2;
/// Multihash code for SHA-256.
const SHA2_256: u64 = 0x12;

/// The CIDv0 (`Qm...`) Kubo's default `add` gives `bytes`, or `None` when
/// the file is too large for this one-level computation.
pub(crate) fn plain_publish_file_cid(bytes: &[u8]) -> Option<String> {
    if bytes.len() <= CHUNK_BYTES {
        return cid_v0(&leaf_node(bytes));
    }
    let chunks: Vec<&[u8]> = bytes.chunks(CHUNK_BYTES).collect();
    if chunks.len() > MAX_FILE_LINKS {
        return None;
    }
    let mut links = Vec::new();
    let mut unixfs = Vec::new();
    put_varint_field(&mut unixfs, 1, UNIXFS_FILE);
    put_varint_field(&mut unixfs, 3, bytes.len() as u64);
    for chunk in &chunks {
        let leaf = leaf_node(chunk);
        let mut link = Vec::new();
        put_bytes_field(&mut link, 1, &multihash_bytes(&leaf));
        // Kubo writes an empty name on file links.
        put_bytes_field(&mut link, 2, b"");
        put_varint_field(&mut link, 3, leaf.len() as u64);
        put_bytes_field(&mut links, 2, &link);
        put_varint_field(&mut unixfs, 4, chunk.len() as u64);
    }
    // dag-pb writes the links before the data.
    let mut root = links;
    put_bytes_field(&mut root, 1, &unixfs);
    cid_v0(&root)
}

/// A dag-pb node that holds one chunk of the file.
fn leaf_node(chunk: &[u8]) -> Vec<u8> {
    let mut unixfs = Vec::new();
    put_varint_field(&mut unixfs, 1, UNIXFS_FILE);
    if !chunk.is_empty() {
        put_bytes_field(&mut unixfs, 2, chunk);
    }
    put_varint_field(&mut unixfs, 3, chunk.len() as u64);
    let mut node = Vec::new();
    put_bytes_field(&mut node, 1, &unixfs);
    node
}

fn multihash_bytes(node: &[u8]) -> Vec<u8> {
    let mut out = vec![SHA2_256 as u8, 32];
    out.extend_from_slice(&Sha256::digest(node));
    out
}

fn cid_v0(node: &[u8]) -> Option<String> {
    let hash = cid::multihash::Multihash::<64>::wrap(SHA2_256, &Sha256::digest(node)).ok()?;
    cid::Cid::new_v0(hash).ok().map(|cid| cid.to_string())
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    put_varint(out, field << 3);
    put_varint(out, value);
}

fn put_bytes_field(out: &mut Vec<u8>, field: u64, value: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bytes that do not repeat, for a file of several chunks.
    pub(crate) fn varied_bytes(len: usize) -> Vec<u8> {
        (0u32..)
            .flat_map(|index| Sha256::digest(index.to_string().as_bytes()))
            .take(len)
            .collect()
    }

    /// Expected values are from Kubo v0.40.1 (the version Runtime ships),
    /// `ipfs add -Q --only-hash` on a fresh `ipfs init` repository.
    #[test]
    fn matches_kubo_default_add() {
        assert_eq!(
            plain_publish_file_cid(b"").as_deref(),
            Some("QmbFMke1KXqnYyBBWxB74N4c5SBnJMVAiMNRcGu6x1AwQH")
        );
        assert_eq!(
            plain_publish_file_cid(b"hello world\n").as_deref(),
            Some("QmT78zSuBmuS4z925WZfrqQ1qHaJ56DQaTfyMUF7F8ff5o")
        );
        // Two chunks: 262144 + 37856 bytes.
        assert_eq!(
            plain_publish_file_cid(&[b'a'; 300_000]).as_deref(),
            Some("QmYCTciJdFNMNUPCHSNS6dKMmUAqkGQ9tQQeGgbELhQQcn")
        );
        // Four chunks of bytes that do not repeat.
        assert_eq!(
            plain_publish_file_cid(&varied_bytes(1_000_000)).as_deref(),
            Some("Qmc3wpHopLhmsnXWn5tiiiCbzh4hH8neryb4C6Fh5W29Mk")
        );
    }

    #[test]
    fn a_file_too_large_for_one_level_is_unknown() {
        assert!(plain_publish_file_cid(&vec![0; CHUNK_BYTES * MAX_FILE_LINKS]).is_some());
        assert_eq!(
            plain_publish_file_cid(&vec![0; CHUNK_BYTES * MAX_FILE_LINKS + 1]),
            None
        );
    }
}
