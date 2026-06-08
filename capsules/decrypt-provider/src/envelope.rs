//! dDRM CEK-sealing envelope — characterization spec vendored from PC2's
//! `ddrm-decrypt/src/envelope.rs` (`unwrapECDHEnvelope`).
//!
//! This captures, as executable + tested code, the exact wire contract by which
//! a Content Encryption Key (CEK) is sealed to this provider's session keypair
//! and unwrapped *inside* the provider. It is the concrete shape of the dDRM
//! decrypt "rail" (Option A in `docs/convergence/DDRM_DECRYPT_RAIL.md`): the
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
        assert_eq!(parse(&[0, 0, 0, 0x03]).unwrap_err(), EnvelopeError::BadEnvelope);
    }

    #[test]
    fn wrong_session_key_fails_closed() {
        let sk = SecretKey::random(&mut OsRng);
        let other_sk = SecretKey::random(&mut OsRng);
        let env = make_envelope(&sk, &[0xAA; 16], 0x03);
        let parsed = parse(&env).unwrap();
        assert!(ecdh_unwrap(&other_sk, &parsed).is_err());
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
}
