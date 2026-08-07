//! Guards that `cenc_core::ctr_xor` produces the identical AES-128-CTR
//! keystream as an independently, directly-instantiated `ctr::Ctr128BE<aes::Aes128>`
//! cipher, for the CENC 8-byte-IV shape. Enciphers with the reference cipher,
//! then decrypts that reference ciphertext with `cenc_core::ctr_xor` — a
//! self-symmetry check alone would only prove CTR round-trips with itself,
//! not that it matches an independently-built reference.
use aes::cipher::{KeyIvInit, StreamCipher};
use cenc_core::{ctr_xor, pad_iv};

#[test]
fn ctr_xor_matches_independent_reference_cipher() {
    let cek = [0x33u8; 16];
    let iv16 = pad_iv(&[0x5Au8; 8]).unwrap();
    let plaintext = b"\x89PNG\r\n\x1a\n...pretend non-media asset body...".to_vec();

    // Reference ciphertext from a directly-instantiated AES-128-CTR (not via cenc_core).
    let mut reference_ct = plaintext.clone();
    let mut reference_cipher = ctr::Ctr128BE::<aes::Aes128>::new(&cek.into(), &iv16.into());
    reference_cipher.apply_keystream(&mut reference_ct);
    assert_ne!(reference_ct, plaintext);

    // cenc_core::ctr_xor must recover the plaintext from the reference ciphertext,
    // proving its keystream is byte-identical to the independent reference cipher's.
    let mut recovered = reference_ct.clone();
    ctr_xor(&mut recovered, &cek, &iv16);
    assert_eq!(recovered, plaintext);
}
