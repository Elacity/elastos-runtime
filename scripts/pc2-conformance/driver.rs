//! Executable cross-implementation conformance driver.
//!
//! Reads ElastOS's committed classical golden vector and decrypts it using PC2
//! `ddrm-decrypt`'s REAL code — the same envelope unwrap and CENC sample-decrypt
//! the production decrypt runtime uses — then asserts byte-for-byte parity:
//!
//!   1. envelope:  parse -> ecdh_unwrap -> extract_keys_blob  ==>  vector CEK
//!   2. cenc:      parse_segment -> decrypt_samples           ==>  vector plaintext
//!
//! This turns the "byte-compatible with PC2 ddrm-decrypt" claim from an assertion
//! into something that fails loudly (exit 1) if the two implementations ever
//! diverge. Compiled on demand against the PC2 repo by `scripts/pc2-conformance.sh`.

use base64::Engine as _;
use ddrm_decrypt::{cenc, envelope, mp4box};
use p256::SecretKey;
use std::process::exit;

fn b64d(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("invalid base64 in vector")
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: pc2-conformance <classical_cenc.json>");
    let raw = std::fs::read_to_string(&path).expect("read vector file");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse vector json");
    let field = |k: &str| {
        v[k].as_str()
            .unwrap_or_else(|| panic!("vector missing field `{k}`"))
            .to_string()
    };

    let sk_bytes = b64d(&field("session_secret_key_b64"));
    let sealed = b64d(&field("sealed_envelope_b64"));
    let expected_cek = b64d(&field("cek_b64"));
    let segment = b64d(&field("encrypted_segment_b64"));
    let expected_pt = b64d(&field("expected_plaintext_b64"));

    // 1. Recover the CEK through PC2's real ECDH envelope unwrap.
    let sk = SecretKey::from_slice(&sk_bytes).expect("p256 session secret key");
    let parsed = envelope::parse(&sealed).expect("PC2 envelope::parse rejected our envelope");
    let pt = envelope::ecdh_unwrap(&sk, &parsed).expect("PC2 envelope::ecdh_unwrap failed");
    let cek = envelope::extract_keys_blob(&pt).expect("PC2 envelope::extract_keys_blob failed");
    if cek != expected_cek {
        eprintln!("FAIL: PC2-recovered CEK does not match the vector CEK");
        exit(1);
    }
    println!("  envelope: PC2 ddrm-decrypt recovered the CEK (16 bytes) — parity OK");

    // 2. Decrypt the segment through PC2's real MP4 parser + AES-128-CTR cenc.
    let cek16: [u8; 16] = cek.as_slice().try_into().expect("CEK must be exactly 16 bytes");
    let seg = mp4box::parse_segment(&segment, 8).expect("PC2 mp4box::parse_segment failed");
    let traf = seg.traf.expect("segment has no traf");
    let trun = traf.trun.expect("segment has no trun");
    let senc = traf.senc.expect("segment has no senc");
    let mdat = &segment[seg.mdat_offset..seg.mdat_offset + seg.mdat_size];
    let out = cenc::decrypt_samples(mdat, &cek16, &trun.entries, &senc.samples, 0)
        .expect("PC2 cenc::decrypt_samples failed");
    if out != expected_pt {
        eprintln!("FAIL: PC2-decrypted plaintext does not match the vector plaintext");
        exit(1);
    }
    println!("  cenc: PC2 ddrm-decrypt decrypted the segment to the expected plaintext — parity OK");

    println!("PASS: ElastOS classical golden vector is byte-compatible with PC2 ddrm-decrypt.");
}
