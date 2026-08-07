//! AES-128-CTR CENC (ISO/IEC 23001-7) content cipher primitive.
//!
//! Container-agnostic: this crate knows nothing about fMP4, `senc`, `mdat`, or
//! metadata. It is the single home of the CTR crypto shared by `encrypt-provider`
//! (seal), `decrypt-provider` (open), and the ddrm-reader wasm (non-media open).
//! CTR is symmetric, so `ctr_xor` both encrypts and decrypts.

use aes::cipher::{KeyIvInit, StreamCipher};
use core::fmt;

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

#[derive(Debug, PartialEq, Eq)]
pub enum CencError {
    UnexpectedIvSize(usize),
    SubsampleOutOfRange { pos: usize, encrypted: usize, len: usize },
}

impl fmt::Display for CencError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CencError::UnexpectedIvSize(n) => write!(f, "unexpected IV size: {n} (expected 8 or 16)"),
            CencError::SubsampleOutOfRange { pos, encrypted, len } => {
                write!(f, "subsample exceeds data: pos={pos} encrypted={encrypted} len={len}")
            }
        }
    }
}

/// Build the 16-byte AES-128-CTR IV/counter block from a senc IV (8 or 16 bytes).
/// An 8-byte IV is left-justified and right-zero-padded per CENC.
pub fn pad_iv(iv: &[u8]) -> Result<[u8; 16], CencError> {
    let mut out = [0u8; 16];
    match iv.len() {
        8 => out[..8].copy_from_slice(iv),
        16 => out.copy_from_slice(iv),
        other => return Err(CencError::UnexpectedIvSize(other)),
    }
    Ok(out)
}

/// One full-range AES-128-CTR pass in place. Encrypt and decrypt are identical.
pub fn ctr_xor(buf: &mut [u8], cek: &[u8; 16], iv16: &[u8; 16]) {
    let mut cipher = Aes128Ctr::new(cek.into(), iv16.into());
    cipher.apply_keystream(buf);
}

/// Continuous-counter subsample pass. Each `(clear, encrypted)` pair advances past
/// `clear` cleartext bytes (the CTR counter does NOT advance over them) then
/// enciphers `encrypted` bytes. The counter is continuous across encrypted ranges
/// within the buffer, per ISO/IEC 23001-7 §9.
pub fn ctr_xor_subsamples(
    buf: &mut [u8],
    cek: &[u8; 16],
    iv16: &[u8; 16],
    subs: &[(u32, u32)],
) -> Result<(), CencError> {
    let mut cipher = Aes128Ctr::new(cek.into(), iv16.into());
    let mut pos = 0usize;
    for &(clear, encrypted) in subs {
        pos += clear as usize;
        let end = pos + encrypted as usize;
        if end > buf.len() {
            return Err(CencError::SubsampleOutOfRange { pos, encrypted: encrypted as usize, len: buf.len() });
        }
        cipher.apply_keystream(&mut buf[pos..end]);
        pos = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_iv_8_bytes_right_pads_with_zeros() {
        let iv16 = pad_iv(&[0xAA; 8]).unwrap();
        assert_eq!(&iv16[..8], &[0xAA; 8]);
        assert_eq!(&iv16[8..], &[0u8; 8]);
    }

    #[test]
    fn pad_iv_16_bytes_passthrough() {
        let iv = [1u8; 16];
        assert_eq!(pad_iv(&iv).unwrap(), iv);
    }

    #[test]
    fn pad_iv_rejects_other_sizes() {
        assert_eq!(pad_iv(&[0u8; 12]), Err(CencError::UnexpectedIvSize(12)));
    }

    #[test]
    fn ctr_xor_round_trips() {
        let key = [0x01u8; 16];
        let iv = pad_iv(&[0x07u8; 8]).unwrap();
        let plaintext = b"Hello CENC decryption test data!".to_vec();
        let mut buf = plaintext.clone();
        ctr_xor(&mut buf, &key, &iv); // encrypt
        assert_ne!(buf, plaintext);
        ctr_xor(&mut buf, &key, &iv); // decrypt (symmetric)
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn ctr_xor_subsamples_matches_manual_continuous_counter() {
        let key = [0x02u8; 16];
        let iv = [0u8; 16];
        let plaintext = b"CLEARencrypteddatCLRmorecrypted!!".to_vec();

        // Reference: encrypt only the two protected ranges with a continuous counter.
        let mut expected_ct = plaintext.clone();
        {
            use aes::cipher::{KeyIvInit, StreamCipher};
            let mut c = ctr::Ctr128BE::<aes::Aes128>::new(&key.into(), &iv.into());
            c.apply_keystream(&mut expected_ct[5..16]);
            c.apply_keystream(&mut expected_ct[19..32]);
        }

        let subs = [(5u32, 11u32), (3u32, 13u32)];
        let mut buf = expected_ct.clone();
        ctr_xor_subsamples(&mut buf, &key, &iv, &subs).unwrap();
        assert_eq!(buf, plaintext, "subsample decrypt must recover plaintext");
    }

    #[test]
    fn ctr_xor_subsamples_out_of_range_fails_closed() {
        let mut buf = vec![0u8; 8];
        let err = ctr_xor_subsamples(&mut buf, &[0u8; 16], &[0u8; 16], &[(4, 99)]);
        assert!(matches!(err, Err(CencError::SubsampleOutOfRange { .. })));
    }
}
