//! dDRM CEK-sealing envelope — characterization spec vendored from PC2's
//! `ddrm-decrypt/src/envelope.rs` (`unwrapECDHEnvelope`).
//!
//! This captures, as executable + tested code, the exact wire contract by which
//! a Content Encryption Key (CEK) is sealed to this provider's session keypair
//! and unwrapped *inside* the provider. It is the concrete shape of the dDRM
//! decrypt "rail" (Option A in `docs/dkms/history/DDRM_DECRYPT_RAIL.md`): the
//! decrypt boundary RECEIVES VM/session-sealed material rather than reaching out
//! for it.
//!
//! IMPORTANT: this module is intentionally NOT wired into the live `OpenSession`
//! / `Render` dispatch yet. It is a contract-first characterization of PC2's
//! proven scheme, pending Anders' confirmation of (a) Option A and (b) the
//! session-key provisioning path. Keeping it as a tested island lets the live
//! wiring land in one reviewed step once the rail is confirmed, without
//! re-discovering the byte layout.
//!
//! Containment invariants preserved from PC2:
//!   - the CEK only ever exists inside this provider's linear memory;
//!   - it is held in `Zeroizing` storage and scrubbed on drop;
//!   - it never appears in any response surface (see `scoped_session_response`).
//!
//! Envelope layout (binary, big-endian length prefixes):
//!
//!   offset 0..3   : header (3 bytes format + 1 byte version)
//!                   version byte at offset 3: 0x02 = legacy fixed-IV, 0x03 = random IV
//!   offset 4..6   : ephPubKeyLen (u16)
//!   offset 6..6+N : ephPubKey (compressed P-256, typically 33 bytes)
//!   (v=0x03 only)  AES-CBC IV (16 bytes)
//!   (v=0x02 only)  IV derived from first 16 bytes of ephPubKey
//!   next 2 bytes  : sigLen (u16)
//!   next sigLen   : signature (skipped — verified at the policy/rights layer)
//!   next 33 bytes : compressed signer public key (skipped)
//!   next 4 bytes  : encCekLen (u32)
//!   next encCekLen: AES-CBC-256 encrypted CEK blob
//!
//! Unwrap (matching WebCrypto `deriveKey({name:'ECDH'}, ..., 'AES-CBC', 256)`):
//!   1. ECDH(session SK, eph PK) -> 32-byte X-coordinate Z
//!   2. AES-256-CBC key = Z (full 32 bytes, no KDF)
//!   3. PKCS#7 unpad
//!
//! Inner plaintext: metaSize(u32 BE) | metadata | keyCount(u32 BE) | keys...

#![allow(dead_code)] // rail-candidate: tested island, not yet wired into dispatch

use aes::Aes256;
use cbc::Decryptor as CbcDecryptor;
use cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use elliptic_curve::sec1::FromEncodedPoint;
use p256::{ecdh::diffie_hellman, EncodedPoint, PublicKey, SecretKey};
use zeroize::Zeroizing;

type Aes256CbcDec = CbcDecryptor<Aes256>;

/// Fail-closed error surface for envelope handling. Messages are deliberately
/// coarse so a malformed/forged envelope cannot probe internal state.
#[derive(Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    BadEnvelope,
    DecryptFailed,
}

#[derive(Debug)]
pub struct ParsedEnvelope<'a> {
    pub version: u8,
    pub eph_pub_key: &'a [u8],
    pub iv: [u8; 16],
    pub encrypted_cek: &'a [u8],
}

/// Parse the envelope binary layout without performing any crypto.
pub fn parse(envelope: &[u8]) -> Result<ParsedEnvelope<'_>, EnvelopeError> {
    if envelope.len() < 4 {
        return Err(EnvelopeError::BadEnvelope);
    }
    let version = envelope[3];
    let mut offset = 4usize;

    if offset + 2 > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let eph_len = u16::from_be_bytes([envelope[offset], envelope[offset + 1]]) as usize;
    offset += 2;
    if offset + eph_len > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let eph_pub_key = &envelope[offset..offset + eph_len];
    offset += eph_len;

    let iv: [u8; 16] = if version == 0x03 {
        if offset + 16 > envelope.len() {
            return Err(EnvelopeError::BadEnvelope);
        }
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&envelope[offset..offset + 16]);
        offset += 16;
        iv
    } else {
        if eph_pub_key.len() < 16 {
            return Err(EnvelopeError::BadEnvelope);
        }
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&eph_pub_key[..16]);
        iv
    };

    // Signature + signer pubkey are verified at the rights/policy layer upstream.
    if offset + 2 > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let sig_len = u16::from_be_bytes([envelope[offset], envelope[offset + 1]]) as usize;
    offset += 2;
    if offset + sig_len + 33 > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    offset += sig_len;
    offset += 33;

    if offset + 4 > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let enc_cek_len = u32::from_be_bytes([
        envelope[offset],
        envelope[offset + 1],
        envelope[offset + 2],
        envelope[offset + 3],
    ]) as usize;
    offset += 4;
    if offset + enc_cek_len > envelope.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let encrypted_cek = &envelope[offset..offset + enc_cek_len];

    Ok(ParsedEnvelope {
        version,
        eph_pub_key,
        iv,
        encrypted_cek,
    })
}

/// ECDH + AES-256-CBC unwrap. Returns the inner plaintext (still framed) held in
/// `Zeroizing` so it is scrubbed from linear memory on drop.
pub fn ecdh_unwrap(
    secret_key: &SecretKey,
    parsed: &ParsedEnvelope,
) -> Result<Zeroizing<Vec<u8>>, EnvelopeError> {
    let eph_pk = parse_p256_public(parsed.eph_pub_key)?;

    let shared = diffie_hellman(secret_key.to_nonzero_scalar(), eph_pk.as_affine());
    let key_bytes = Zeroizing::new(shared.raw_secret_bytes().to_vec());

    let cipher = Aes256CbcDec::new(key_bytes.as_slice().into(), (&parsed.iv).into());
    let mut buf = Zeroizing::new(parsed.encrypted_cek.to_vec());
    let pt_len = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| EnvelopeError::DecryptFailed)?
        .len();
    buf.truncate(pt_len);
    Ok(buf)
}

/// Extract the CEK bytes from the unwrapped inner plaintext.
/// Inner format: `metaSize(u32 BE) | metadata | keyCount(u32 BE) | keys...`
pub fn extract_cek(plaintext: &[u8]) -> Result<Zeroizing<Vec<u8>>, EnvelopeError> {
    if plaintext.len() < 4 {
        return Err(EnvelopeError::BadEnvelope);
    }
    let meta_size =
        u32::from_be_bytes([plaintext[0], plaintext[1], plaintext[2], plaintext[3]]) as usize;
    let body_offset = 4 + meta_size;
    if body_offset + 4 > plaintext.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    let key_start = body_offset + 4;
    if key_start > plaintext.len() {
        return Err(EnvelopeError::BadEnvelope);
    }
    Ok(Zeroizing::new(plaintext[key_start..].to_vec()))
}

/// Accept either a 33-byte compressed or 65-byte uncompressed P-256 point.
fn parse_p256_public(raw: &[u8]) -> Result<PublicKey, EnvelopeError> {
    let point = EncodedPoint::from_bytes(raw).map_err(|_| EnvelopeError::BadEnvelope)?;
    Option::<PublicKey>::from(PublicKey::from_encoded_point(&point))
        .ok_or(EnvelopeError::BadEnvelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cbc::Encryptor as CbcEncryptor;
    use cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use elliptic_curve::sec1::ToEncodedPoint;
    use p256::ecdh::EphemeralSecret;
    use rand_core::OsRng;

    type Aes256CbcEnc = CbcEncryptor<Aes256>;

    /// Build an envelope exactly as the upstream sealer (Lit/key-provider) would,
    /// so the round-trip pins the wire contract end to end.
    fn make_envelope(session_sk: &SecretKey, cek: &[u8], version: u8) -> Vec<u8> {
        let eph = EphemeralSecret::random(&mut OsRng);
        let eph_pk = eph.public_key();
        let eph_compressed = eph_pk.to_encoded_point(true);
        let eph_bytes = eph_compressed.as_bytes();
        assert_eq!(eph_bytes.len(), 33);

        let shared = eph.diffie_hellman(&session_sk.public_key());
        let key_bytes = shared.raw_secret_bytes();

        let mut inner = Vec::new();
        inner.extend_from_slice(&0u32.to_be_bytes()); // metaSize
        inner.extend_from_slice(&1u32.to_be_bytes()); // keyCount
        inner.extend_from_slice(cek);

        let iv: [u8; 16] = if version == 0x03 {
            let mut iv = [0u8; 16];
            getrandom::getrandom(&mut iv).unwrap();
            iv
        } else {
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&eph_bytes[..16]);
            iv
        };

        let cipher = Aes256CbcEnc::new(key_bytes.as_slice().into(), (&iv).into());
        let mut buf = vec![0u8; inner.len() + 16];
        buf[..inner.len()].copy_from_slice(&inner);
        let ct_len = cipher
            .encrypt_padded_mut::<Pkcs7>(&mut buf, inner.len())
            .unwrap()
            .len();
        let ciphertext = &buf[..ct_len];

        let mut env = Vec::new();
        env.extend_from_slice(&[0, 0, 0, version]);
        env.extend_from_slice(&(eph_bytes.len() as u16).to_be_bytes());
        env.extend_from_slice(eph_bytes);
        if version == 0x03 {
            env.extend_from_slice(&iv);
        }
        env.extend_from_slice(&0u16.to_be_bytes()); // empty signature
        env.extend_from_slice(&[0u8; 33]); // signer pubkey (skipped)
        env.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        env.extend_from_slice(ciphertext);
        env
    }

    #[test]
    fn round_trip_v3_random_iv() {
        let sk = SecretKey::random(&mut OsRng);
        let cek = [0x42u8; 16];
        let env = make_envelope(&sk, &cek, 0x03);
        let parsed = parse(&env).unwrap();
        let pt = ecdh_unwrap(&sk, &parsed).unwrap();
        let recovered = extract_cek(&pt).unwrap();
        assert_eq!(recovered.as_slice(), &cek);
    }

    #[test]
    fn round_trip_v2_fixed_iv() {
        let sk = SecretKey::random(&mut OsRng);
        let cek = [0x99u8; 16];
        let env = make_envelope(&sk, &cek, 0x02);
        let parsed = parse(&env).unwrap();
        let pt = ecdh_unwrap(&sk, &parsed).unwrap();
        let recovered = extract_cek(&pt).unwrap();
        assert_eq!(recovered.as_slice(), &cek);
    }

    #[test]
    fn truncated_envelope_rejected() {
        assert_eq!(
            parse(&[0, 0, 0, 0x03]).unwrap_err(),
            EnvelopeError::BadEnvelope
        );
    }

    #[test]
    fn wrong_session_key_fails_closed() {
        let sk = SecretKey::random(&mut OsRng);
        let other_sk = SecretKey::random(&mut OsRng);
        let cek = [0xAA; 16];
        let env = make_envelope(&sk, &cek, 0x03);
        let parsed = parse(&env).unwrap();
        // CBC carries no integrity tag: a wrong key almost always dies at PKCS#7
        // unpadding, but random garbage unpads "successfully" ~1/256 of the time.
        // The fail-closed invariant is therefore not "unwrap errors" but "the CEK
        // never materializes" — an accidental unpad must still not yield the CEK.
        let recovered = ecdh_unwrap(&other_sk, &parsed)
            .ok()
            .and_then(|pt| extract_cek(&pt).ok());
        assert!(
            !recovered.is_some_and(|key| key.as_slice() == cek),
            "wrong session key must never recover the sealed CEK"
        );
    }

    /// CEK-containment: the sealed envelope bytes must never contain the raw CEK
    /// in cleartext — the key only materializes after a correct ECDH unwrap.
    #[test]
    fn sealed_envelope_does_not_contain_raw_cek() {
        let sk = SecretKey::random(&mut OsRng);
        let cek = [0x7Eu8; 16];
        let env = make_envelope(&sk, &cek, 0x03);
        assert!(
            !env.windows(cek.len()).any(|w| w == cek),
            "raw CEK must not appear in the sealed envelope"
        );
    }

    // --- portable golden vectors (classical envelope + cenc) ------------------

    /// Minimal single-sample encrypted fMP4 segment, matching the decrypt-step
    /// golden, for emitting a portable vector.
    #[cfg(feature = "gen-vectors")]
    fn build_encrypted_segment_for_vector(
        plaintext: &[u8],
        cek: &[u8; 16],
        iv8: &[u8; 8],
    ) -> Vec<u8> {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

        fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
            let size = (8 + content.len()) as u32;
            let mut b = size.to_be_bytes().to_vec();
            b.extend_from_slice(box_type);
            b.extend_from_slice(content);
            b
        }

        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut ciphertext);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);
        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);
        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", &ciphertext);
        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    /// Emit a classical vector for the given envelope `version` (0x03 random-IV or
    /// 0x02 fixed-IV — both PC2-supported wire shapes).
    #[cfg(feature = "gen-vectors")]
    fn write_classical_vector(version: u8, file: &str, description: &str) {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let sk = SecretKey::random(&mut OsRng);
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let plaintext = b"the quick brown fox jumps over!!";
        let sealed = make_envelope(&sk, &cek, version);
        let segment = build_encrypted_segment_for_vector(plaintext, &cek, &iv8);

        let v = crate::vector_format::ClassicalVector {
            description: description.to_string(),
            session_secret_key_b64: b64.encode(sk.to_bytes()),
            sealed_envelope_b64: b64.encode(&sealed),
            cek_b64: b64.encode(cek),
            encrypted_segment_b64: b64.encode(&segment),
            expected_plaintext_b64: b64.encode(plaintext),
            init_segment_b64: None,
            iv_size: None,
        };
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/{file}");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Regenerate the committed classical vectors. Run:
    /// `cargo test --features gen-vectors emit_classical`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_classical_vector() {
        write_classical_vector(
            0x03,
            "classical_cenc.json",
            "P-256 ECDH envelope (v3, random IV) -> CENC AES-128-CTR; byte-compatible with PC2 ddrm-decrypt",
        );
    }

    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_classical_v2_vector() {
        write_classical_vector(
            0x02,
            "classical_cenc_v2.json",
            "P-256 ECDH envelope (v2, IV derived from eph pubkey) -> CENC AES-128-CTR; byte-compatible with PC2 ddrm-decrypt",
        );
    }

    // --- richer cenc shapes (multi-sample, subsample, non-default IV size) -----
    //
    // Real fMP4 segments are not single-sample/single-subsample/default-IV. These
    // builders + vectors pin the shapes most likely to bite at wire-up time, by
    // executable parity against PC2 (classical). Box layouts validated against
    // PC2 `mp4box.rs` / `cenc.rs`.

    #[cfg(feature = "gen-vectors")]
    fn mk_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Multi-sample segment: N samples, each with its own 8-byte IV + fresh CTR.
    /// `trun` carries per-sample sizes; `senc` has no subsamples.
    #[cfg(feature = "gen-vectors")]
    fn build_multisample_segment(
        samples: &[(&[u8], [u8; 8])],
        cek: &[u8; 16],
    ) -> (Vec<u8>, Vec<u8>) {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00]; // v0, flags=sample-size-present
        trun_content.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        for (pt, _) in samples {
            trun_content.extend_from_slice(&(pt.len() as u32).to_be_bytes());
        }
        let trun = mk_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0]; // v0, flags=0 (no subsamples)
        senc_content.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        let mut mdat_payload = Vec::new();
        let mut expected = Vec::new();
        for (pt, iv8) in samples {
            senc_content.extend_from_slice(iv8);
            let mut iv16 = [0u8; 16];
            iv16[..8].copy_from_slice(iv8);
            let mut ct = pt.to_vec();
            Aes128Ctr::new(cek.into(), (&iv16).into()).apply_keystream(&mut ct);
            mdat_payload.extend_from_slice(&ct);
            expected.extend_from_slice(pt);
        }
        let senc = mk_box(b"senc", &senc_content);

        let mut traf = trun;
        traf.extend_from_slice(&senc);
        let traf = mk_box(b"traf", &traf);
        let moof = mk_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&mk_box(b"mdat", &mdat_payload));
        (segment, expected)
    }

    /// Single-sample segment with subsample (clear+encrypted) ranges. The CTR
    /// keystream is continuous across encrypted ranges only (clear bytes are
    /// skipped), matching CENC + PC2 `decrypt_subsamples`.
    #[cfg(feature = "gen-vectors")]
    fn build_subsample_segment(
        plaintext: &[u8],
        subs: &[(u16, u32)],
        cek: &[u8; 16],
        iv8: &[u8; 8],
    ) -> Vec<u8> {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

        let mut data = plaintext.to_vec();
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        let mut pos = 0usize;
        for (clear, enc) in subs {
            pos += *clear as usize;
            let e = *enc as usize;
            cipher.apply_keystream(&mut data[pos..pos + e]);
            pos += e;
        }

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = mk_box(b"trun", &trun_content);

        // senc: v0, flags=0x000002 (subsamples present), count=1, iv8, then
        // subsample_count(u16) + per-subsample clear(u16)+encrypted(u32).
        let mut senc_content = vec![0u8, 0x00, 0x00, 0x02, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        senc_content.extend_from_slice(&(subs.len() as u16).to_be_bytes());
        for (clear, enc) in subs {
            senc_content.extend_from_slice(&clear.to_be_bytes());
            senc_content.extend_from_slice(&enc.to_be_bytes());
        }
        let senc = mk_box(b"senc", &senc_content);

        let mut traf = trun;
        traf.extend_from_slice(&senc);
        let traf = mk_box(b"traf", &traf);
        let moof = mk_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&mk_box(b"mdat", &data));
        segment
    }

    /// Single-sample segment using a 16-byte IV (the `senc` carries the full 16).
    #[cfg(feature = "gen-vectors")]
    fn build_segment_iv16(plaintext: &[u8], cek: &[u8; 16], iv16: &[u8; 16]) -> Vec<u8> {
        use aes::cipher::{KeyIvInit, StreamCipher};
        type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

        let mut ct = plaintext.to_vec();
        Aes128Ctr::new(cek.into(), iv16.into()).apply_keystream(&mut ct);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = mk_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv16); // 16-byte IV
        let senc = mk_box(b"senc", &senc_content);

        let mut traf = trun;
        traf.extend_from_slice(&senc);
        let traf = mk_box(b"traf", &traf);
        let moof = mk_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&mk_box(b"mdat", &ct));
        segment
    }

    /// Build an init segment whose `tenc.default_per_sample_iv_size = iv_size`.
    /// Path: moov→trak→mdia→minf→stbl→stsd→encv→sinf→schi→tenc (matches both our
    /// and PC2's `parse_init_for_tenc`: encv skips 78 sample-entry bytes).
    #[cfg(feature = "gen-vectors")]
    fn build_init_segment(iv_size: u8) -> Vec<u8> {
        // tenc: v0 flags(3) + reserved(1) + reserved(1) + is_protected(1) +
        //       iv_size(1) + default_kid(16)
        let mut tenc_content = vec![0u8, 0, 0, 0, 0, 0, 1, iv_size];
        tenc_content.extend_from_slice(&[0u8; 16]);
        let tenc = mk_box(b"tenc", &tenc_content);
        let schi = mk_box(b"schi", &tenc);
        let sinf = mk_box(b"sinf", &schi);

        let mut encv_content = vec![0u8; 78]; // SampleEntry + VisualSampleEntry header
        encv_content.extend_from_slice(&sinf);
        let encv = mk_box(b"encv", &encv_content);

        let mut stsd_content = vec![0u8, 0, 0, 0]; // version+flags
        stsd_content.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        stsd_content.extend_from_slice(&encv);
        let stsd = mk_box(b"stsd", &stsd_content);

        let stbl = mk_box(b"stbl", &stsd);
        let minf = mk_box(b"minf", &stbl);
        let mdia = mk_box(b"mdia", &minf);
        let trak = mk_box(b"trak", &mdia);
        mk_box(b"moov", &trak)
    }

    /// Write a richer classical vector (shared by the three emit tests below).
    #[cfg(feature = "gen-vectors")]
    fn write_rich_vector(
        file: &str,
        description: &str,
        segment: &[u8],
        expected_plaintext: &[u8],
        cek: &[u8; 16],
        init: Option<&[u8]>,
        iv_size: Option<u8>,
    ) {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let sk = SecretKey::random(&mut OsRng);
        let sealed = make_envelope(&sk, cek, 0x03);
        let v = crate::vector_format::ClassicalVector {
            description: description.to_string(),
            session_secret_key_b64: b64.encode(sk.to_bytes()),
            sealed_envelope_b64: b64.encode(&sealed),
            cek_b64: b64.encode(cek),
            encrypted_segment_b64: b64.encode(segment),
            expected_plaintext_b64: b64.encode(expected_plaintext),
            init_segment_b64: init.map(|i| b64.encode(i)),
            iv_size,
        };
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/{file}");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// `cargo test --features gen-vectors emit_classical_multisample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_classical_multisample_vector() {
        let cek = [0x11u8; 16];
        let samples: &[(&[u8], [u8; 8])] = &[
            (b"first sample plaintext block .01", [0x10; 8]),
            (b"second sample plaintext block 02", [0x20; 8]),
            (b"third sample plaintext block .03", [0x30; 8]),
        ];
        let (segment, expected) = build_multisample_segment(samples, &cek);
        write_rich_vector(
            "classical_cenc_multisample.json",
            "P-256 ECDH envelope -> CENC AES-128-CTR, 3 samples (per-sample IV); byte-compatible with PC2 ddrm-decrypt",
            &segment,
            &expected,
            &cek,
            None,
            None,
        );
    }

    /// `cargo test --features gen-vectors emit_classical_subsample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_classical_subsample_vector() {
        let cek = [0x11u8; 16];
        // 5 clear + 11 enc + 3 clear + 13 enc = 32 bytes
        let plaintext = b"CLEARencrypteddatCLRmorecrypted!!";
        let subs: &[(u16, u32)] = &[(5, 11), (3, 13)];
        let segment = build_subsample_segment(plaintext, subs, &cek, &[0x22u8; 8]);
        write_rich_vector(
            "classical_cenc_subsample.json",
            "P-256 ECDH envelope -> CENC AES-128-CTR subsample (clear+encrypted ranges); byte-compatible with PC2 ddrm-decrypt",
            &segment,
            plaintext,
            &cek,
            None,
            None,
        );
    }

    /// `cargo test --features gen-vectors emit_classical_initseg_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_classical_initseg_vector() {
        let cek = [0x11u8; 16];
        let plaintext = b"sixteen-byte IV sample plaintext";
        let iv16 = [0x33u8; 16];
        let segment = build_segment_iv16(plaintext, &cek, &iv16);
        let init = build_init_segment(16);
        write_rich_vector(
            "classical_cenc_initseg.json",
            "P-256 ECDH envelope -> CENC AES-128-CTR, 16-byte IV via init-segment tenc; byte-compatible with PC2 ddrm-decrypt",
            &segment,
            plaintext,
            &cek,
            Some(&init),
            Some(16),
        );
    }

    /// Emit the rail carrier golden (rail Option A) by repackaging the committed
    /// classical vector as a `RailCarrierVector`. Deriving from
    /// `classical_cenc.json` guarantees the carrier's `sealed_cek` is
    /// byte-identical to the PC2-conformant fixture, so the carrier golden and the
    /// cross-impl conformance check exercise the same bytes. Run AFTER
    /// `emit_classical_vector`:
    /// `cargo test --features gen-vectors emit_classical_vector && \
    ///  cargo test --features gen-vectors emit_rail_carrier_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_rail_carrier_vector() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors");
        let classical: crate::vector_format::ClassicalVector = serde_json::from_str(
            &std::fs::read_to_string(format!("{dir}/classical_cenc.json")).unwrap(),
        )
        .unwrap();
        let carrier = crate::vector_format::RailCarrierVector {
            description: "rail Option A carrier (classical P-256): sealed CEK + segment -> \
                 decrypt_from_carrier; sealed_cek byte-identical to classical_cenc.json \
                 (PC2-conformant via session unwrap_envelope + decrypt_segment)"
                .to_string(),
            profile: "ClassicalP256".to_string(),
            session_secret_key_b64: classical.session_secret_key_b64,
            mlkem_dk_b64: None,
            sealed_cek_b64: classical.sealed_envelope_b64,
            ciphertext_segment_b64: classical.encrypted_segment_b64,
            init_segment_b64: None,
            expected_plaintext_b64: classical.expected_plaintext_b64,
            mldsa_vk_b64: None,
        };
        let path = format!("{dir}/rail_carrier_classical.json");
        std::fs::write(&path, serde_json::to_string_pretty(&carrier).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Replay a committed classical vector through the engines (no in-test
    /// sealing): proves the portable bytes still decrypt after any refactor.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    fn replay_classical_vector(json: &str) {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let v: crate::vector_format::ClassicalVector = serde_json::from_str(json).unwrap();
        let sk = SecretKey::from_slice(&b64.decode(&v.session_secret_key_b64).unwrap()).unwrap();
        let sealed = b64.decode(&v.sealed_envelope_b64).unwrap();
        let parsed = parse(&sealed).unwrap();
        let recovered = extract_cek(&ecdh_unwrap(&sk, &parsed).unwrap()).unwrap();
        assert_eq!(
            b64.encode(recovered.as_slice()),
            v.cek_b64,
            "vector CEK recovered via ECDH"
        );

        let cek_b64 = b64.encode(recovered.as_slice());
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let init = v.init_segment_b64.as_ref().map(|s| b64.decode(s).unwrap());
        let (output, meta) =
            crate::decrypt_session_segment(&cek_b64, &segment, init.as_deref()).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "vector plaintext recovered via cenc"
        );
        assert_eq!(meta["is_protected"], serde_json::json!(true));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_golden_vector_replays() {
        replay_classical_vector(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc.json"
        )));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_v2_golden_vector_replays() {
        replay_classical_vector(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc_v2.json"
        )));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_multisample_golden_vector_replays() {
        replay_classical_vector(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc_multisample.json"
        )));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_subsample_golden_vector_replays() {
        replay_classical_vector(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc_subsample.json"
        )));
    }

    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_initseg_golden_vector_replays() {
        replay_classical_vector(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc_initseg.json"
        )));
    }

    /// A corrupted vector must fail closed (no plaintext on tampered input).
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn classical_golden_vector_corrupted_fails_closed() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;

        let v: crate::vector_format::ClassicalVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/classical_cenc.json"
        )))
        .unwrap();
        let sk = SecretKey::from_slice(&b64.decode(&v.session_secret_key_b64).unwrap()).unwrap();
        let mut sealed = b64.decode(&v.sealed_envelope_b64).unwrap();
        let n = sealed.len();
        sealed[n - 1] ^= 0xFF; // corrupt the encrypted-CEK tail

        let result = parse(&sealed).and_then(|p| ecdh_unwrap(&sk, &p));
        assert!(
            result.is_err(),
            "a corrupted classical vector must fail closed"
        );
    }

    // --- adversarial negative-space sweep (feature = "harden") ----------------
    //
    // `parse` is the untrusted-input boundary the rail exposes (it ingests
    // attacker-controlled envelope bytes). These sweeps prove it fails closed on
    // every malformed shape and NEVER panics — a panic in a wasm capsule is a
    // denial-of-service. Mirrors the careful bounds-checks PC2's `unwrapECDHEnvelope`
    // performs; coarse `EnvelopeError` carries no bytes, so errors leak nothing.

    #[cfg(feature = "harden")]
    fn valid_classical_envelope() -> (SecretKey, Vec<u8>) {
        let sk = SecretKey::random(&mut OsRng);
        let env = make_envelope(&sk, &[0xABu8; 16], 0x03);
        (sk, env)
    }

    /// Every proper prefix (truncation at every length) must fail closed.
    #[cfg(feature = "harden")]
    #[test]
    fn harden_parse_truncations_fail_closed() {
        let (_sk, env) = valid_classical_envelope();
        assert!(parse(&env).is_ok(), "the full envelope must parse");
        for t in 0..env.len() {
            assert!(
                parse(&env[..t]).is_err(),
                "truncation to {t} bytes must fail closed (got Ok)"
            );
        }
    }

    /// Arbitrary single-byte corruption at every position must never panic
    /// (Ok or Err are both acceptable; the invariant is "no panic").
    #[cfg(feature = "harden")]
    #[test]
    fn harden_parse_never_panics_on_byte_flips() {
        let (_sk, env) = valid_classical_envelope();
        for i in 0..env.len() {
            let mut e = env.clone();
            e[i] ^= 0xFF;
            let _ = parse(&e); // must not panic
        }
    }

    /// Oversized length prefixes (claim more bytes than present) fail closed.
    #[cfg(feature = "harden")]
    #[test]
    fn harden_parse_oversized_length_prefixes_fail_closed() {
        let (_sk, env) = valid_classical_envelope();

        // eph_len lives at bytes 4..6 and is always present in a valid envelope.
        let mut big_eph = env.clone();
        big_eph[4] = 0xFF;
        big_eph[5] = 0xFF;
        assert!(
            parse(&big_eph).is_err(),
            "oversized eph_len must fail closed"
        );

        // sig_len lives right after eph_pub_key + IV (v=0x03): 4 + 2 + 33 + 16 = 55.
        let mut big_sig = env.clone();
        big_sig[55] = 0xFF;
        big_sig[56] = 0xFF;
        assert!(
            parse(&big_sig).is_err(),
            "oversized sig_len must fail closed"
        );
    }

    /// Tampering anywhere in the envelope yields only a coarse `EnvelopeError`
    /// (no field-level probe, no bytes) when carried through unwrap.
    #[cfg(feature = "harden")]
    #[test]
    fn harden_classical_tamper_errors_are_coarse() {
        let (sk, env) = valid_classical_envelope();
        for i in 0..env.len() {
            let mut e = env.clone();
            e[i] ^= 0xFF;
            if let Ok(parsed) = parse(&e) {
                if let Err(err) = ecdh_unwrap(&sk, &parsed) {
                    // The only surfaces are the two coarse variants.
                    assert!(
                        matches!(
                            err,
                            EnvelopeError::BadEnvelope | EnvelopeError::DecryptFailed
                        ),
                        "error surface must stay coarse"
                    );
                }
            }
        }
    }
}
