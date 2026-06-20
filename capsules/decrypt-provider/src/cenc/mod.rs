//! Vendored CENC (Common Encryption, ISO 23001-7) AES-128-CTR fMP4 decrypt engine.
//!
//! Provenance: ported from PC2 `pc2-node/crates/cenc-decrypt`
//! (`Elacity/pc2.net` @ `a0a910158`). This is the `decrypt-provider`'s internal
//! decrypt/render backend. The CEK is held only inside this boundary and zeroized
//! after use; it is never returned to callers, logged, or surfaced to app capsules.
//!
//! Day 1 status: vendored in-tree and characterization-tested. It is wired into
//! `open_session`/`render` behind the fail-closed provider contract in a later step.

pub mod cenc;
pub mod mp4box;
pub mod strip;

use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DecryptCommand {
    /// Base64-encoded 16-byte AES-128-CTR Content Encryption Key.
    pub cek_b64: String,
    /// Per-sample IV size in bytes (typically 8 or 16). Default: 8.
    pub iv_size: Option<u8>,
    /// Default sample size (from tfhd) if trun doesn't include per-sample sizes.
    pub default_sample_size: Option<u32>,
    /// If true, the input is an init segment — extract tenc and pass through unchanged.
    #[serde(default)]
    pub is_init: bool,
    /// If true, strip encryption signaling/metadata boxes from the output.
    #[serde(default)]
    pub strip: bool,
    /// If "strip_init", only strip encryption signaling from init segment (no decrypt).
    pub mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DecryptResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iv_size: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_protected: Option<bool>,
}

/// Process a segment decryption request.
///
/// The CEK arrives base64-encoded inside `command_json`, is used only within this
/// function, and is zeroized on every return path. Only the decrypted segment bytes
/// (or a passthrough copy) and a metadata `DecryptResult` are returned.
pub fn process(
    command_json: &str,
    segment_data: &[u8],
    init_data: Option<&[u8]>,
) -> (String, Option<Vec<u8>>) {
    let cmd: DecryptCommand = match serde_json::from_str(command_json) {
        Ok(c) => c,
        Err(e) => return (error_result(&format!("invalid command: {e}")), None),
    };

    // strip_init mode: strip encryption signaling from init segment, no decrypt
    if cmd.mode.as_deref() == Some("strip_init") {
        let stripped = strip::strip_encryption_signaling(segment_data);
        let result = DecryptResult {
            success: true,
            error: None,
            sample_count: None,
            iv_size: None,
            is_protected: None,
        };
        return (serde_json::to_string(&result).unwrap(), Some(stripped));
    }

    if cmd.is_init {
        return process_init(segment_data);
    }

    let b64 = base64::engine::general_purpose::STANDARD;
    let mut cek_bytes = match b64.decode(&cmd.cek_b64) {
        Ok(k) => k,
        Err(e) => return (error_result(&format!("cek decode: {e}")), None),
    };

    if cek_bytes.len() != 16 {
        cek_bytes.iter_mut().for_each(|b| *b = 0);
        return (
            error_result(&format!("cek length {} (expected 16)", cek_bytes.len())),
            None,
        );
    }

    let iv_size = cmd.iv_size.unwrap_or(8);
    let default_sample_size = cmd.default_sample_size.unwrap_or(0);

    // Determine IV size from init segment's tenc if available
    let effective_iv_size = if let Some(init) = init_data {
        if let Some(tenc) = mp4box::parse_init_for_tenc(init) {
            if tenc.default_per_sample_iv_size > 0 {
                tenc.default_per_sample_iv_size
            } else {
                iv_size
            }
        } else {
            iv_size
        }
    } else {
        iv_size
    };

    let parsed = match mp4box::parse_segment(segment_data, effective_iv_size) {
        Ok(p) => p,
        Err(e) => {
            cek_bytes.iter_mut().for_each(|b| *b = 0);
            return (error_result(&format!("parse segment: {e}")), None);
        }
    };

    let traf = match &parsed.traf {
        Some(t) => t,
        None => {
            // No moof/traf — might be an unencrypted segment, pass through
            cek_bytes.iter_mut().for_each(|b| *b = 0);
            let result = DecryptResult {
                success: true,
                error: None,
                sample_count: Some(0),
                iv_size: Some(effective_iv_size),
                is_protected: Some(false),
            };
            return (
                serde_json::to_string(&result).unwrap(),
                Some(segment_data.to_vec()),
            );
        }
    };

    let senc = match &traf.senc {
        Some(s) => s,
        None => {
            // No senc box — segment is not encrypted, pass through
            cek_bytes.iter_mut().for_each(|b| *b = 0);
            let result = DecryptResult {
                success: true,
                error: None,
                sample_count: Some(0),
                iv_size: Some(effective_iv_size),
                is_protected: Some(false),
            };
            return (
                serde_json::to_string(&result).unwrap(),
                Some(segment_data.to_vec()),
            );
        }
    };

    let trun_entries = traf.trun.as_ref().map(|t| &t.entries[..]).unwrap_or(&[]);

    let cek_arr: [u8; 16] = cek_bytes[..16].try_into().unwrap();

    let mdat_start = parsed.mdat_offset;
    let mdat_end = parsed.mdat_offset + parsed.mdat_size;

    // Copy the segment ONCE, then decrypt the mdat content range in place. Bytes
    // outside the mdat content (moof, the mdat box header, any trailing bytes) are
    // preserved verbatim, so the output is byte-identical to the previous
    // decrypt-then-reconstruct path — only the redundant whole-mdat copy is
    // removed. The CEK is still zeroized on every return path, as before.
    let mut output = segment_data.to_vec();
    if let Err(e) = cenc::decrypt_samples_in_place(
        &mut output[mdat_start..mdat_end],
        &cek_arr,
        trun_entries,
        &senc.samples,
        default_sample_size,
    ) {
        cek_bytes.iter_mut().for_each(|b| *b = 0);
        return (error_result(&format!("decrypt: {e}")), None);
    }

    // Zero CEK
    cek_bytes.iter_mut().for_each(|b| *b = 0);

    // Strip encryption metadata boxes if requested
    let final_output = if cmd.strip {
        strip::strip_segment_encryption_boxes(&output)
    } else {
        output
    };

    let result = DecryptResult {
        success: true,
        error: None,
        sample_count: Some(senc.samples.len()),
        iv_size: Some(effective_iv_size),
        is_protected: Some(true),
    };

    (serde_json::to_string(&result).unwrap(), Some(final_output))
}

/// Process an init segment — extract tenc info and pass through unchanged.
fn process_init(data: &[u8]) -> (String, Option<Vec<u8>>) {
    let tenc = mp4box::parse_init_for_tenc(data);
    let result = DecryptResult {
        success: true,
        error: None,
        sample_count: None,
        iv_size: tenc.as_ref().map(|t| t.default_per_sample_iv_size),
        is_protected: tenc.as_ref().map(|t| t.default_is_protected != 0),
    };
    (serde_json::to_string(&result).unwrap(), Some(data.to_vec()))
}

fn error_result(msg: &str) -> String {
    serde_json::to_string(&DecryptResult {
        success: false,
        error: Some(msg.to_string()),
        sample_count: None,
        iv_size: None,
        is_protected: None,
    })
    .unwrap()
}

#[cfg(test)]
mod golden_tests {
    use super::*;
    use aes::cipher::{KeyIvInit, StreamCipher};

    type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Build a minimal single-sample encrypted fMP4 segment:
    /// moof { traf { trun(sample_size), senc(iv) } } + mdat { ciphertext }
    fn build_encrypted_segment(plaintext: &[u8], cek: &[u8; 16], iv8: &[u8; 8]) -> Vec<u8> {
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut ciphertext);

        // trun: version(1)=0, flags(3)=0x000200 (sample-size-present),
        //       sample_count(4)=1, sample_size(4)
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        // senc: version(1)=0, flags(3)=0 (no subsamples), sample_count(4)=1, iv(8)
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

    #[test]
    fn process_decrypts_encrypted_segment_end_to_end() {
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);
        let command = format!(r#"{{"cek_b64":"{cek_b64}","iv_size":8}}"#);

        let (result_json, output) = process(&command, &segment, None);

        // The decrypted mdat content must equal the original plaintext.
        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_content_off = moof_len + 8;
        let mut expected = segment.clone();
        expected[mdat_content_off..mdat_content_off + plaintext.len()].copy_from_slice(plaintext);

        let output = output.expect("expected decrypted output");
        assert_eq!(
            output, expected,
            "decrypted segment must match plaintext mdat"
        );

        // Metadata reports a protected, single-sample segment.
        assert!(result_json.contains("\"is_protected\":true"));
        assert!(result_json.contains("\"sample_count\":1"));

        // CEK-containment smoke check: the key must not leak into the result metadata.
        assert!(
            !result_json.contains(&cek_b64),
            "CEK must never appear in the decrypt result"
        );
    }

    #[test]
    fn process_rejects_wrong_length_cek() {
        let short_cek = base64::engine::general_purpose::STANDARD.encode([0u8; 8]);
        let command = format!(r#"{{"cek_b64":"{short_cek}"}}"#);
        let (result_json, output) = process(&command, &[], None);
        assert!(output.is_none());
        assert!(result_json.contains("\"success\":false"));
    }

    #[test]
    fn process_passes_through_unencrypted_segment() {
        // A bare mdat with no moof/traf is treated as unencrypted passthrough.
        let mdat = make_box(b"mdat", b"plain media bytes");
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode([0x33u8; 16]);
        let command = format!(r#"{{"cek_b64":"{cek_b64}","iv_size":8}}"#);
        let (result_json, output) = process(&command, &mdat, None);
        assert_eq!(output.unwrap(), mdat);
        assert!(result_json.contains("\"is_protected\":false"));
    }
}
