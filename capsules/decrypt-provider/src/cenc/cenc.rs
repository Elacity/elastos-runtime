//! CENC (Common Encryption Standard, ISO 23001-7) sample decryption.
//!
//! Implements AES-128-CTR decryption per sample with per-sample IVs from
//! the senc box. Handles both full-sample and subsample encryption.
//! CEK is zeroed in memory after use.
//!
//! The CTR crypto primitive itself (IV padding + full-range/subsample CTR xor)
//! is delegated to the shared, container-agnostic `cenc-core` crate; this
//! module owns only the fMP4 framing (`senc`/`trun` sample layout).

use super::mp4box::{SencSample, SubsampleEntry, TrunEntry};

/// Decrypt all samples of an mdat payload **in place** using CENC AES-128-CTR.
///
/// - `mdat`: the mutable mdat content bytes (decrypted in place)
/// - `cek`: 16-byte Content Encryption Key
/// - `trun_entries`: sample sizes from trun box
/// - `senc_samples`: per-sample IVs (and optional subsample info) from senc box
///
/// Only encrypted byte ranges are touched; the layout and sizes are unchanged.
/// Decrypting in place lets the caller hold a single segment buffer instead of
/// allocating a separate decrypted-mdat copy and then reconstructing the segment.
pub fn decrypt_samples_in_place(
    mdat: &mut [u8],
    cek: &[u8; 16],
    trun_entries: &[TrunEntry],
    senc_samples: &[SencSample],
    default_sample_size: u32,
) -> Result<(), String> {
    let mut offset = 0usize;

    for (i, senc_sample) in senc_samples.iter().enumerate() {
        let sample_size = trun_entries
            .get(i)
            .and_then(|e| e.sample_size)
            .unwrap_or(default_sample_size) as usize;

        if offset + sample_size > mdat.len() {
            return Err(format!(
                "sample {i} exceeds mdat: offset={offset} size={sample_size} mdat_len={}",
                mdat.len()
            ));
        }

        let iv = cenc_core::pad_iv(&senc_sample.iv).map_err(|e| e.to_string())?;

        if senc_sample.subsamples.is_empty() {
            cenc_core::ctr_xor(&mut mdat[offset..offset + sample_size], cek, &iv);
        } else {
            let pairs = subsample_pairs(&senc_sample.subsamples);
            cenc_core::ctr_xor_subsamples(&mut mdat[offset..offset + sample_size], cek, &iv, &pairs)
                .map_err(|e| e.to_string())?;
        }

        offset += sample_size;
    }

    Ok(())
}

/// Vec-returning wrapper preserved for the PC2 conformance driver
/// (`scripts/pc2-conformance/driver.rs`), which pins this exact signature.
/// Allocates one copy and decrypts it in place — byte-identical output.
pub fn decrypt_samples(
    mdat: &[u8],
    cek: &[u8; 16],
    trun_entries: &[TrunEntry],
    senc_samples: &[SencSample],
    default_sample_size: u32,
) -> Result<Vec<u8>, String> {
    let mut output = mdat.to_vec();
    decrypt_samples_in_place(&mut output, cek, trun_entries, senc_samples, default_sample_size)?;
    Ok(output)
}

/// Map fMP4 `SubsampleEntry` framing to the `(clear, encrypted)` byte-count
/// pairs `cenc_core::ctr_xor_subsamples` expects.
fn subsample_pairs(subsamples: &[SubsampleEntry]) -> Vec<(u32, u32)> {
    subsamples
        .iter()
        .map(|s| (s.clear_bytes as u32, s.encrypted_bytes))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_full_sample() {
        let key = [0x01u8; 16];
        let iv = [0u8; 16];
        let plaintext = b"Hello CENC decryption test data!";

        // Encrypt
        let mut encrypted = plaintext.to_vec();
        cenc_core::ctr_xor(&mut encrypted, &key, &iv);

        // Decrypt
        let mut decrypted = encrypted.clone();
        cenc_core::ctr_xor(&mut decrypted, &key, &iv);
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
        cenc_core::ctr_xor_subsamples(&mut data, &key, &iv, &[(5, 11), (3, 13)]).unwrap();

        let subsamples = vec![
            SubsampleEntry {
                clear_bytes: 5,
                encrypted_bytes: 11,
            },
            SubsampleEntry {
                clear_bytes: 3,
                encrypted_bytes: 13,
            },
        ];

        let pairs = subsample_pairs(&subsamples);
        cenc_core::ctr_xor_subsamples(&mut data, &key, &iv, &pairs).unwrap();
        assert_eq!(&data, plaintext);
    }

    #[test]
    fn iv_8_bytes_padded() {
        let iv8 = [0xAA; 8];
        let iv16 = cenc_core::pad_iv(&iv8).unwrap();
        assert_eq!(&iv16[..8], &iv8);
        assert_eq!(&iv16[8..], &[0u8; 8]);
    }
}
