//! End-to-end AV forensic-variant pipeline (chunks 3/4/5) on a REAL ffmpeg fragmented MP4 + real
//! CENC — mint variants → build manifest → per-buyer serve selection → AAD weld → extract.
//!
//! This proves the MECHANISM the production wiring plugs into (creator mint / `ddrm-media-authority`
//! serve / `decrypt-provider` AAD rebuild) without the server IPC: every function here is the exact
//! one the pipeline calls. Run: `cargo test -p ddrm-media --features av-variants`.
#![cfg(feature = "av-variants")]

use ddrm_envelope::av;
use ddrm_envelope::transcript::DecryptTranscriptV1;
use ddrm_media::mp4;
use sha2::{Digest, Sha256};

const FIXTURE_B64: &str = include_str!("vectors/tiny_av_fragmented.mp4.b64");

fn fixture() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(FIXTURE_B64.trim())
        .expect("decode av fixture")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// A minimal transcript — only the welded bindings (segment digests, variant-set commitment) vary
/// between buyers; the identity fields are fixed.
fn transcript<'a>(
    decrypt_session_pub: &'a [u8],
    nonce: &'a [u8],
    content_hash: &'a [u8],
) -> DecryptTranscriptV1<'a> {
    DecryptTranscriptV1 {
        suite_id: "x25519-aes256gcm",
        provider_id: "decrypt-provider",
        principal_id: "person:test:av",
        session_id: "sess-av",
        object_cid: "owned:av-asset",
        content_hash,
        action: "view",
        viewer_interface: "media",
        output_kind: "stream",
        expires_at: 0,
        release_receipt_hash: [0u8; 32],
        decrypt_session_pub,
        nonce,
        node_set_id: None,
    }
}

/// The mint side: produce per-marked-segment {A,B} encrypted variants + the manifest. Returns the
/// manifest, the asset secret, and a map uri -> encrypted variant bytes (the "published DASH dir").
struct Minted {
    manifest: av::VariantManifestV1,
    asset_secret: [u8; 32],
    by_uri: std::collections::HashMap<String, Vec<u8>>,
    content_hash: [u8; 32],
}

fn mint(fragments: &[Vec<u8>]) -> Minted {
    let cek = [0x42u8; 16];
    let content_hash: [u8; 32] = Sha256::digest(b"av-asset-content").into();
    let asset_secret = av::asset_secret_from_master(b"node-master-secret", &content_hash);

    let mut by_uri = std::collections::HashMap::new();
    let mut marked: Vec<(u32, Vec<av::VariantRef>)> = Vec::new();

    for (i, frag) in fragments.iter().enumerate() {
        let mut variants = Vec::with_capacity(2);
        for symbol in 0u8..2 {
            // Both variants of a timeline slot share the slot's IV counter — only one is ever served.
            let mut ctr = (i as u64) << 20;
            let plain = mp4::embed_placeholder_variant(frag, symbol);
            let enc = mp4::encrypt_fragment(&plain, &cek, &mut ctr).expect("CENC variant");
            let uri = format!("seg-{i}.{}.m4s", if symbol == 0 { "A" } else { "B" });
            let digest_hex = hex(&Sha256::digest(&enc));
            by_uri.insert(uri.clone(), enc);
            variants.push(av::VariantRef {
                symbol,
                uri,
                digest_hex,
            });
        }
        marked.push((i as u32, variants));
    }

    let manifest = av::build_manifest(2, 2.0, &asset_secret, &marked).expect("valid manifest");
    Minted {
        manifest,
        asset_secret,
        by_uri,
        content_hash,
    }
}

/// The serve side for one buyer: select per-marked-segment variant from the grant, return the ordered
/// selected ciphertexts, the welded AAD, and the chosen symbols (what the production selector does).
fn serve(minted: &Minted, grant_digest: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>, Vec<u8>) {
    let m = minted.manifest.codeword.length;
    let bias = av::asset_bias_vector(&minted.asset_secret, m);
    let symbols = av::select_symbols(&minted.manifest, &bias, grant_digest).expect("selection");

    let selected: Vec<Vec<u8>> = minted
        .manifest
        .marked_segments
        .iter()
        .zip(&symbols)
        .map(|(seg, &sym)| {
            let uri = &seg.variants[sym as usize].uri;
            minted
                .by_uri
                .get(uri)
                .expect("variant ciphertext present")
                .clone()
        })
        .collect();

    let refs: Vec<&[u8]> = selected.iter().map(|s| s.as_slice()).collect();
    let seg_digests = ddrm_envelope::segment_digests(&refs);
    let vsc = av::variant_set_commitment(&minted.manifest);
    let aad = transcript(b"dsp", b"nonce", &minted.content_hash).to_aad_with_all_bindings(
        Some(&seg_digests),
        None,
        Some(&vsc),
    );
    (selected, aad, symbols)
}

#[test]
fn av_pipeline_two_buyers_diverge_weld_and_extract() {
    let data = fixture();
    let split = mp4::split_fragmented(&data).expect("split");
    assert!(split.fragments.len() >= 2, "need a couple of fragments");
    let minted = mint(&split.fragments);

    let g_alice = ddrm_envelope::grant_watermark_digest16("0xalice");
    let g_bob = ddrm_envelope::grant_watermark_digest16("0xbob");

    let (sel_a, aad_a, sym_a) = serve(&minted, &g_alice);
    let (sel_b, aad_b, sym_b) = serve(&minted, &g_bob);

    // Distinct buyers get distinct codewords ⇒ distinct selections (with overwhelming probability
    // over enough segments) and therefore distinct served bytes + distinct welded AADs.
    assert_ne!(
        sym_a, sym_b,
        "distinct buyers must select distinct variant sets"
    );
    assert_ne!(
        sel_a, sel_b,
        "served ciphertext sets must diverge between buyers"
    );
    assert_ne!(
        aad_a, aad_b,
        "the welded AAD must differ (served selection is bound)"
    );

    // At every position the buyers chose differently, the served bytes differ; where they chose the
    // same symbol, the served bytes are identical (deterministic routing, no hidden divergence).
    for (i, (a, b)) in sel_a.iter().zip(&sel_b).enumerate() {
        if sym_a[i] == sym_b[i] {
            assert_eq!(a, b, "same symbol ⇒ identical served bytes at {i}");
        } else {
            assert_ne!(a, b, "different symbol ⇒ different served bytes at {i}");
        }
    }

    // Extract (placeholder stand-in for the offline forensic extractor): decrypting Alice's served
    // variant recovers Alice's chosen symbol per segment — attribution is carried in the bytes.
    let cek = [0x42u8; 16];
    let _ = cek; // documents the CEK; decrypt below only needs the senc strip
    for (i, seg) in sel_a.iter().enumerate() {
        let clean = mp4::strip_senc(seg).expect("decrypt served variant");
        assert_eq!(
            mp4::read_placeholder_variant(&clean),
            Some(sym_a[i]),
            "served variant at {i} must carry Alice's selected symbol"
        );
    }
}

#[test]
fn av_pipeline_aad_fails_closed_on_tampered_serve_and_manifest() {
    let data = fixture();
    let split = mp4::split_fragmented(&data).expect("split");
    let minted = mint(&split.fragments);
    let g = ddrm_envelope::grant_watermark_digest16("0xcarol");
    let (selected, aad_ok, symbols) = serve(&minted, &g);

    // (1) Served-bytes weld: swap one served segment for the OTHER variant of the same slot (an
    // out-of-selection substitution). The segment-digest AAD changes ⇒ the CEK unwrap would fail.
    let flip_at = symbols.iter().position(|_| true).unwrap();
    let other = if symbols[flip_at] == 0 { 1u8 } else { 0u8 };
    let other_uri = &minted.manifest.marked_segments[flip_at].variants[other as usize].uri;
    let mut tampered = selected.clone();
    tampered[flip_at] = minted.by_uri.get(other_uri).unwrap().clone();
    let refs: Vec<&[u8]> = tampered.iter().map(|s| s.as_slice()).collect();
    let seg_digests = ddrm_envelope::segment_digests(&refs);
    let vsc = av::variant_set_commitment(&minted.manifest);
    let aad_tampered = transcript(b"dsp", b"nonce", &minted.content_hash).to_aad_with_all_bindings(
        Some(&seg_digests),
        None,
        Some(&vsc),
    );
    assert_ne!(
        aad_ok, aad_tampered,
        "substituting a served variant must change the welded AAD"
    );

    // (2) Manifest-swap weld: a forged manifest (a variant digest altered) changes the variant-set
    // commitment, so even the SAME served bytes weld to a different AAD ⇒ fail closed.
    let mut forged = minted.manifest.clone();
    forged.marked_segments[0].variants[0].digest_hex.push('f');
    let refs_ok: Vec<&[u8]> = selected.iter().map(|s| s.as_slice()).collect();
    let seg_ok = ddrm_envelope::segment_digests(&refs_ok);
    let vsc_forged = av::variant_set_commitment(&forged);
    let aad_forged = transcript(b"dsp", b"nonce", &minted.content_hash).to_aad_with_all_bindings(
        Some(&seg_ok),
        None,
        Some(&vsc_forged),
    );
    assert_ne!(
        aad_ok, aad_forged,
        "a swapped/forged manifest must change the welded AAD"
    );
}

#[test]
fn av_pipeline_single_encode_is_honest_and_byte_identical() {
    // No manifest / single-encode ⇒ no selection, no variant binding; the AAD is byte-identical to
    // a plain segment-bound open (the fingerprinted layer is strictly additive).
    let honest = av::VariantManifestV1::single_encode();
    let empty_bias: Vec<u16> = Vec::new();
    let g = ddrm_envelope::grant_watermark_digest16("0xdave");
    assert!(av::select_symbols(&honest, &empty_bias, &g)
        .unwrap()
        .is_empty());

    let segs = ddrm_envelope::segment_digests(&[b"seg0".as_slice(), b"seg1".as_slice()]);
    let content_hash = [9u8; 32];
    let plain = transcript(b"dsp", b"nonce", &content_hash).to_aad_with_bindings(Some(&segs), None);
    let with_none = transcript(b"dsp", b"nonce", &content_hash).to_aad_with_all_bindings(
        Some(&segs),
        None,
        None,
    );
    assert_eq!(
        plain, with_none,
        "single-encode open is byte-identical (no variant binding)"
    );
}
