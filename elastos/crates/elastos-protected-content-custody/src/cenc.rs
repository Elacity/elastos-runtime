//! AES-128-CTR CENC (ISO/IEC 23001-7) primitive mined from PR #15 `cenc-core`.
//!
//! Container-agnostic. This is not a second key-wrap path. Callers must only
//! invoke it after a PQ-hybrid CEK exists inside the decrypt-session boundary.

use aes::cipher::{KeyIvInit, StreamCipher};
use sha2::{Digest as _, Sha256};

use crate::{CustodyError, CONTENT_KEY_BYTES};

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

const CENC_AES128_KEY_LABEL: &[u8] = b"elastos.protected-content.cenc-aes128-key/v1";

pub(crate) fn pad_iv(iv: &[u8]) -> Result<[u8; 16], CustodyError> {
    let mut out = [0u8; 16];
    match iv.len() {
        8 => out[..8].copy_from_slice(iv),
        16 => out.copy_from_slice(iv),
        _ => return Err(CustodyError::InvalidPayload("cenc_iv")),
    }
    Ok(out)
}

pub(crate) fn ctr_xor(buf: &mut [u8], cek: &[u8; 16], iv16: &[u8; 16]) {
    let mut cipher = Aes128Ctr::new(cek.into(), iv16.into());
    cipher.apply_keystream(buf);
}

pub(crate) fn ctr_xor_subsamples(
    buf: &mut [u8],
    cek: &[u8; 16],
    iv16: &[u8; 16],
    subs: &[(u32, u32)],
) -> Result<(), CustodyError> {
    let mut cipher = Aes128Ctr::new(cek.into(), iv16.into());
    let mut pos = 0usize;
    for &(clear, encrypted) in subs {
        pos = pos
            .checked_add(clear as usize)
            .ok_or(CustodyError::InvalidPayload("cenc_subsample"))?;
        let end = pos
            .checked_add(encrypted as usize)
            .ok_or(CustodyError::InvalidPayload("cenc_subsample"))?;
        if end > buf.len() {
            return Err(CustodyError::InvalidPayload("cenc_subsample"));
        }
        cipher.apply_keystream(&mut buf[pos..end]);
        pos = end;
    }
    Ok(())
}

pub(crate) fn derive_cenc_aes128_key(cek: &[u8; CONTENT_KEY_BYTES]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(CENC_AES128_KEY_LABEL);
    hasher.update(cek);
    let digest = hasher.finalize();
    digest[..16].try_into().expect("SHA-256 is 32 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_iv_accepts_8_or_16_and_rejects_other_sizes() {
        let iv16 = pad_iv(&[0xAA; 8]).unwrap();
        assert_eq!(&iv16[..8], &[0xAA; 8]);
        assert_eq!(&iv16[8..], &[0u8; 8]);
        assert_eq!(pad_iv(&[1u8; 16]).unwrap(), [1u8; 16]);
        assert!(pad_iv(&[0u8; 12]).is_err());
    }

    #[test]
    fn ctr_xor_round_trips() {
        let key = [0x01u8; 16];
        let iv = pad_iv(&[0x07u8; 8]).unwrap();
        let plaintext = b"Hello CENC decryption test data!".to_vec();
        let mut buf = plaintext.clone();
        ctr_xor(&mut buf, &key, &iv);
        assert_ne!(buf, plaintext);
        ctr_xor(&mut buf, &key, &iv);
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn derive_cenc_aes128_key_is_stable_and_not_identity() {
        let cek = [0xAB; CONTENT_KEY_BYTES];
        let key = derive_cenc_aes128_key(&cek);
        assert_ne!(key, cek[..16]);
        assert_eq!(key, derive_cenc_aes128_key(&cek));
    }
}
