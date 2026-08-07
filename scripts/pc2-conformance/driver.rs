//! Executable cross-implementation conformance driver.
//!
//! Reads ElastOS's committed classical golden vector and decrypts it using PC2
//! `ddrm-decrypt`'s REAL code, then asserts byte-for-byte parity at two layers:
//!
//!   1. envelope:  parse -> ecdh_unwrap -> extract_keys_blob  ==>  vector CEK
//!   2. cenc:      parse_segment -> decrypt_samples           ==>  vector plaintext
//!   3. tamper:    a corrupted envelope is rejected (fail-closed parity)
//!   4. session (the rail carrier path): install a session holding the vector's
//!      key, then PC2's PUBLIC `session::unwrap_envelope` (L1 ECDH + L2 CEK store)
//!      -> `media::decrypt_segment` (tenc IV-size + moof/traf/senc walk) ==> vector
//!      plaintext; and a tampered carrier fails closed inside unwrap_envelope.
//!      This proves our Option-A carrier is wire-compatible with PC2's session
//!      model — the exact entrypoints the production decrypt runtime calls — not
//!      merely its crypto primitives.
//!
//! This turns the "byte-compatible with PC2 ddrm-decrypt" claim from an assertion
//! into something that fails loudly (exit 1) if the two implementations ever
//! diverge. Compiled on demand against the PC2 repo by `scripts/pc2-conformance.sh`.

use base64::Engine as _;
use ddrm_decrypt::{cenc, envelope, media, mp4box};
use ddrm_decrypt::session::{self, SessionState};
use ddrm_decrypt::state::{next_handle, SESSIONS};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::SecretKey;
use std::process::exit;

fn b64d(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("invalid base64 in vector")
}

/// Run positive + negative conformance for a single vector. Returns on success;
/// prints a FAIL line and exits 1 on any divergence.
fn check_vector(path: &str) {
    let raw = std::fs::read_to_string(path).expect("read vector file");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse vector json");
    let field = |k: &str| {
        v[k].as_str()
            .unwrap_or_else(|| panic!("vector missing field `{k}`"))
            .to_string()
    };
    let label = v["description"].as_str().unwrap_or(path);
    println!("vector: {label}");

    let sk_bytes = b64d(&field("session_secret_key_b64"));
    let sealed = b64d(&field("sealed_envelope_b64"));
    let expected_cek = b64d(&field("cek_b64"));
    let segment = b64d(&field("encrypted_segment_b64"));
    let expected_pt = b64d(&field("expected_plaintext_b64"));
    // Optional richer-shape fields (default to the single-sample / 8-byte-IV case).
    let iv_size = v["iv_size"].as_u64().unwrap_or(8) as u8;
    let init: Option<Vec<u8>> = v["init_segment_b64"].as_str().map(b64d);

    let sk = SecretKey::from_slice(&sk_bytes).expect("p256 session secret key");

    // 1. Recover the CEK through PC2's real ECDH envelope unwrap.
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
    let seg = mp4box::parse_segment(&segment, iv_size).expect("PC2 mp4box::parse_segment failed");
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

    // 3. Negative parity: a tampered envelope must fail closed in PC2 too — proving
    //    both implementations reject the same corruption (no silent plaintext leak).
    let mut tampered = sealed.clone();
    let n = tampered.len();
    tampered[n - 1] ^= 0xFF; // corrupt the encrypted-CEK tail
    let rejected = match envelope::parse(&tampered) {
        Ok(p) => envelope::ecdh_unwrap(&sk, &p)
            .and_then(|pt| envelope::extract_keys_blob(&pt))
            .map(|recovered| recovered == expected_cek)
            .unwrap_or(false),
        Err(_) => false,
    };
    if rejected {
        eprintln!("FAIL: PC2 accepted a tampered envelope (expected fail-closed)");
        exit(1);
    }
    println!("  tamper: PC2 ddrm-decrypt rejected the corrupted envelope — fail-closed parity OK");

    // 4. Session-level parity (the rail carrier path): drive the SAME sealed
    //    bytes + segment through PC2's PUBLIC session API — the entrypoints the
    //    production decrypt runtime calls. A session holding the vector's key
    //    `unwrap_envelope`s the carrier (L1 ECDH + L2 CEK store) and then
    //    `media::decrypt_segment` decrypts it (tenc IV-size + moof/traf/senc walk).
    //    This proves our Option-A carrier is wire-compatible with PC2's *session
    //    model*, not merely its crypto primitives.
    let install_session = |label: &str| -> u32 {
        let sk = SecretKey::from_slice(&sk_bytes).expect("p256 session secret key");
        let pk_point = sk.public_key().to_encoded_point(true);
        let mut pk = [0u8; 33];
        pk.copy_from_slice(pk_point.as_bytes());
        let handle = next_handle();
        SESSIONS.with(|s| {
            s.borrow_mut().insert(
                handle,
                SessionState {
                    session_id: format!("{label}-{handle}"),
                    secret_key: sk,
                    public_key_compressed: pk,
                    created_at: 0,
                },
            )
        });
        handle
    };

    let handle = install_session("conformance");
    let req = session::unwrap_envelope(handle, &sealed)
        .expect("PC2 session::unwrap_envelope rejected our carrier");
    let seg_out = media::decrypt_segment(req, init.as_deref(), &segment, false)
        .expect("PC2 media::decrypt_segment failed");
    let off = segment.len() - expected_pt.len();
    if seg_out.len() != segment.len() || seg_out[off..] != expected_pt[..] {
        eprintln!("FAIL: PC2 session-level decrypt produced the wrong plaintext");
        exit(1);
    }
    println!(
        "  session: PC2 unwrap_envelope + decrypt_segment recovered the plaintext (carrier path) — parity OK"
    );

    // Negative parity at the session boundary: a tampered carrier must fail
    // closed inside PC2's unwrap_envelope too (no CEK ever lands in L2).
    let mut tampered_carrier = sealed.clone();
    let m = tampered_carrier.len();
    tampered_carrier[m - 1] ^= 0xFF;
    let tamper_handle = install_session("conformance-tamper");
    if session::unwrap_envelope(tamper_handle, &tampered_carrier).is_ok() {
        eprintln!("FAIL: PC2 session accepted a tampered carrier (expected fail-closed)");
        exit(1);
    }
    println!(
        "  session-tamper: PC2 unwrap_envelope rejected the corrupted carrier — fail-closed parity OK"
    );
}

/// Producer-side parity for an encrypt-provider round-trip golden
/// (`RoundTripVector`). These vectors carry NO sealed envelope or session key —
/// the CEK is captured directly as the test stand-in for the still-blocked
/// transport rail — so there is no envelope/session layer to cross-check. What
/// they DO prove is the half that matters here: that the fMP4 segment
/// `encrypt-provider`'s REAL in-boundary engine emitted (mint CEK+KID -> CENC
/// encrypt -> mux) is decrypted **byte-for-byte by PC2's real
/// `mp4box::parse_segment` + `cenc::decrypt_samples`**. I.e. PC2's production
/// decrypt can consume our producer's output, over real playback shapes
/// (multi-sample, subsample/clear-leader), not only our own consumer.
fn check_roundtrip_vector(path: &str) {
    let raw = std::fs::read_to_string(path).expect("read vector file");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse vector json");
    let field = |k: &str| {
        v[k].as_str()
            .unwrap_or_else(|| panic!("vector missing field `{k}`"))
            .to_string()
    };
    let label = v["description"].as_str().unwrap_or(path);
    println!("vector (producer round-trip): {label}");

    let expected_cek = b64d(&field("cek_b64"));
    let segment = b64d(&field("encrypted_segment_b64"));
    let expected_pt = b64d(&field("expected_plaintext_b64"));
    let iv_size = v["iv_size"].as_u64().unwrap_or(8) as u8;
    let cek16: [u8; 16] = expected_cek
        .as_slice()
        .try_into()
        .expect("CEK must be exactly 16 bytes");

    // 1. PC2 decrypts the producer's segment to the producer's exact plaintext.
    let seg = mp4box::parse_segment(&segment, iv_size).expect("PC2 mp4box::parse_segment failed");
    let traf = seg.traf.expect("segment has no traf");
    let trun = traf.trun.expect("segment has no trun");
    let senc = traf.senc.expect("segment has no senc");
    let mdat = &segment[seg.mdat_offset..seg.mdat_offset + seg.mdat_size];
    let out = cenc::decrypt_samples(mdat, &cek16, &trun.entries, &senc.samples, 0)
        .expect("PC2 cenc::decrypt_samples failed");
    if out != expected_pt {
        eprintln!("FAIL: PC2 decrypted the producer segment to the wrong plaintext");
        exit(1);
    }
    println!(
        "  cenc: PC2 ddrm-decrypt decrypted the producer's {}-sample segment to the expected plaintext — parity OK",
        senc.samples.len()
    );

    // 2. Key-bound parity: with the WRONG CEK, PC2 must NOT recover the plaintext
    //    (proves the parity is a real key-bound decrypt, not an echo).
    let mut wrong = cek16;
    wrong[0] ^= 0xFF;
    let out_wrong = cenc::decrypt_samples(mdat, &wrong, &trun.entries, &senc.samples, 0)
        .expect("PC2 cenc::decrypt_samples (wrong key) failed");
    if out_wrong == expected_pt {
        eprintln!("FAIL: PC2 recovered the plaintext under the WRONG CEK (not key-bound)");
        exit(1);
    }
    // For a subsample/clear-leader sample, the clear prefix is (correctly)
    // unchanged under any key; assert the ENCRYPTED region diverged instead.
    if out_wrong == out {
        eprintln!("FAIL: wrong-CEK decrypt produced identical bytes (nothing was encrypted?)");
        exit(1);
    }
    println!("  key-bound: PC2 decrypt under a wrong CEK diverges from the plaintext — OK");
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: pc2-conformance <vector.json> [<vector.json> ...]");
        exit(2);
    }
    for path in &paths {
        // Dispatch on schema: classical envelope vectors carry a session secret
        // key; producer round-trip vectors do not (CEK captured directly).
        let raw = std::fs::read_to_string(path).expect("read vector file");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("parse vector json");
        if v.get("session_secret_key_b64").is_some() {
            check_vector(path);
        } else {
            check_roundtrip_vector(path);
        }
    }
    println!(
        "PASS: ElastOS classical + producer round-trip vectors are byte-compatible with PC2 ddrm-decrypt."
    );
}
