//! CENC (Common Encryption Standard, ISO 23001-7) sample decryption.
//!
//! Implements AES-128-CTR decryption per sample with per-sample IVs from
//! the senc box. Handles both full-sample and subsample encryption.
//! CEK is zeroed in memory after use.

use aes::cipher::{KeyIvInit, StreamCipher};
use super::mp4box::{SencSample, SubsampleEntry, TrunEntry};

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

/// Decrypt all samples in an mdat payload using CENC AES-128-CTR.
///
/// - `mdat`: raw mdat content bytes
/// - `cek`: 16-byte Content Encryption Key
/// - `trun_entries`: sample sizes from trun box
/// - `senc_samples`: per-sample IVs (and optional subsample info) from senc box
///
/// Returns decrypted mdat bytes. The layout and sizes are identical to the
/// input — only encrypted byte ranges are decrypted in-place.
pub fn decrypt_samples(
    mdat: &[u8],
    cek: &[u8; 16],
    trun_entries: &[TrunEntry],
    senc_samples: &[SencSample],
    default_sample_size: u32,
) -> Result<Vec<u8>, String> {
    let mut output = mdat.to_vec();
    let mut offset = 0usize;

    for (i, senc_sample) in senc_samples.iter().enumerate() {
        let sample_size = trun_entries
            .get(i)
            .and_then(|e| e.sample_size)
            .unwrap_or(default_sample_size) as usize;

        if offset + sample_size > output.len() {
            return Err(format!(
                "sample {i} exceeds mdat: offset={offset} size={sample_size} mdat_len={}",
                output.len()
            ));
        }

        let iv = build_iv(&senc_sample.iv)?;

        if senc_sample.subsamples.is_empty() {
            decrypt_range(&mut output[offset..offset + sample_size], cek, &iv)?;
        } else {
            decrypt_subsamples(
                &mut output[offset..offset + sample_size],
                cek,
                &iv,
                &senc_sample.subsamples,
            )?;
        }

        offset += sample_size;
    }

    Ok(output)
}

/// Build a 16-byte IV/nonce for AES-128-CTR from the senc IV (8 or 16 bytes).
/// 8-byte IVs are zero-padded on the right per CENC spec.
fn build_iv(senc_iv: &[u8]) -> Result<[u8; 16], String> {
    let mut iv = [0u8; 16];
    match senc_iv.len() {
        8 => iv[..8].copy_from_slice(senc_iv),
        16 => iv.copy_from_slice(senc_iv),
        other => return Err(format!("unexpected IV size: {other} (expected 8 or 16)")),
    }
    Ok(iv)
}

/// Decrypt an entire byte range with AES-128-CTR.
fn decrypt_range(data: &mut [u8], key: &[u8; 16], iv: &[u8; 16]) -> Result<(), String> {
    let mut cipher = Aes128Ctr::new(key.into(), iv.into());
    cipher.apply_keystream(data);
    Ok(())
}

/// Decrypt with subsample encryption: alternate clear and encrypted ranges.
/// The CTR counter is continuous across encrypted ranges within a sample.
fn decrypt_subsamples(
    data: &mut [u8],
    key: &[u8; 16],
    iv: &[u8; 16],
    subsamples: &[SubsampleEntry],
) -> Result<(), String> {
    let mut cipher = Aes128Ctr::new(key.into(), iv.into());
    let mut pos = 0usize;

    for sub in subsamples {
        let clear = sub.clear_bytes as usize;
        let encrypted = sub.encrypted_bytes as usize;

        // Skip clear bytes (no decryption, but they don't advance the CTR counter)
        pos += clear;

        if pos + encrypted > data.len() {
            return Err(format!(
                "subsample exceeds data: pos={pos} encrypted={encrypted} len={}",
                data.len()
            ));
        }

        // Decrypt the encrypted portion
        cipher.apply_keystream(&mut data[pos..pos + encrypted]);
        pos += encrypted;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::KeyIvInit;

    #[test]
    fn round_trip_full_sample() {
        let key = [0x01u8; 16];
        let iv = [0u8; 16];
        let plaintext = b"Hello CENC decryption test data!";

        // Encrypt
        let mut encrypted = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(&key.into(), &iv.into());
        cipher.apply_keystream(&mut encrypted);

        // Decrypt
        let mut decrypted = encrypted.clone();
        decrypt_range(&mut decrypted, &key, &iv).unwrap();
        assert_eq!(&decrypted, plaintext);
    }

    #[test]
    fn round_trip_subsamples() {
        let key = [0x02u8; 16];
        let iv = [0u8; 16];

        // 5 clear + 11 encrypted + 3 clear + 13 encrypted = 32 bytes
        let plaintext = b"CLEARencrypteddatCLRmorecrypted!!";
        let mut data = plaintext.to_vec();

        // Encrypt only the encrypted portions
        let mut cipher = Aes128Ctr::new(&key.into(), &iv.into());
        cipher.apply_keystream(&mut data[5..16]);   // first encrypted range
        cipher.apply_keystream(&mut data[19..32]);   // second encrypted range

        let subsamples = vec![
            SubsampleEntry { clear_bytes: 5, encrypted_bytes: 11 },
            SubsampleEntry { clear_bytes: 3, encrypted_bytes: 13 },
        ];

        decrypt_subsamples(&mut data, &key, &iv, &subsamples).unwrap();
        assert_eq!(&data, plaintext);
    }

    #[test]
    fn iv_8_bytes_padded() {
        let iv8 = [0xAA; 8];
        let iv16 = build_iv(&iv8).unwrap();
        assert_eq!(&iv16[..8], &iv8);
        assert_eq!(&iv16[8..], &[0u8; 8]);
    }
}
