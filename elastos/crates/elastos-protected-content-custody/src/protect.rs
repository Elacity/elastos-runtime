use std::collections::BTreeSet;

use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, ValidatedClearFmp4MediaSessionLayoutV1,
    ValidatedClearFmp4SegmentLayoutV1,
};
use zeroize::Zeroizing;

use crate::{
    cenc::{ctr_xor, derive_cenc_aes128_key, pad_iv},
    ContentEncryptionKeyV1, CustodyError,
};

pub fn protect_validated_clear_fmp4_init_to_cenc_v1(
    layout: &ValidatedClearFmp4MediaSessionLayoutV1,
    clear_init_segment: &[u8],
    key_id: [u8; 16],
) -> Result<Vec<u8>, CustodyError> {
    if clear_init_segment.is_empty() {
        return Err(CustodyError::InvalidPayload("clear_init_segment"));
    }
    layout
        .rewrite_protected_init(clear_init_segment, key_id)
        .map_err(Into::into)
}

pub fn protect_validated_clear_fmp4_segment_to_cenc_v1(
    layout: &ValidatedClearFmp4SegmentLayoutV1,
    clear_segment: &[u8],
    content_key: &ContentEncryptionKeyV1,
    sample_ivs: &[[u8; 8]],
) -> Result<Vec<u8>, CustodyError> {
    if clear_segment.is_empty() {
        return Err(CustodyError::InvalidPayload("clear_segment"));
    }
    if sample_ivs.len() != layout.samples().len() {
        return Err(CustodyError::InvalidPayload("sample_ivs"));
    }
    let cenc_key = Zeroizing::new(content_key.with_bytes(derive_cenc_aes128_key));
    let encrypted_samples =
        encrypt_validated_clear_segment_samples_v1(layout, clear_segment, &cenc_key, sample_ivs)?;
    layout
        .rewrite_protected_segment(clear_segment, &encrypted_samples, sample_ivs)
        .map_err(Into::into)
}

#[allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    reason = "the protect boundary binds the exact CENC inputs and returns the exact protected parts without an intermediate authority struct"
)]
pub fn protect_validated_clear_cenc_fmp4_media_v1(
    layout: &ValidatedClearFmp4MediaSessionLayoutV1,
    clear_init_segment: &[u8],
    clear_segments: &[Vec<u8>],
    mime_type: &str,
    codecs: &str,
    content_key: &ContentEncryptionKeyV1,
    key_id: [u8; 16],
    segment_sample_ivs: &[Vec<[u8; 8]>],
) -> Result<(Vec<u8>, Vec<Vec<u8>>, CencFmp4MediaIdentityV1), CustodyError> {
    if clear_segments.is_empty() || clear_segments.len() != segment_sample_ivs.len() {
        return Err(CustodyError::InvalidPayload("clear_segments"));
    }
    let protected_init =
        protect_validated_clear_fmp4_init_to_cenc_v1(layout, clear_init_segment, key_id)?;
    let mut protected_segments = Vec::with_capacity(clear_segments.len());
    for (clear_segment, sample_ivs) in clear_segments.iter().zip(segment_sample_ivs) {
        let segment_layout = layout
            .validate_segment(clear_segment)
            .map_err(|_| CustodyError::InvalidPayload("clear_segment"))?;
        protected_segments.push(protect_validated_clear_fmp4_segment_to_cenc_v1(
            &segment_layout,
            clear_segment,
            content_key,
            sample_ivs.as_slice(),
        )?);
    }
    let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
        &protected_init,
        &protected_segments,
        mime_type,
        codecs,
    )?;
    Ok((protected_init, protected_segments, media_identity))
}

fn encrypt_validated_clear_segment_samples_v1(
    layout: &ValidatedClearFmp4SegmentLayoutV1,
    clear_segment: &[u8],
    cenc_key: &[u8; 16],
    sample_ivs: &[[u8; 8]],
) -> Result<Vec<Vec<u8>>, CustodyError> {
    let clear_mdat = layout.exact_source_mdat_payload(clear_segment)?;
    let mut seen_ivs = BTreeSet::new();
    let mut encrypted_samples = Vec::with_capacity(layout.samples().len());
    for (sample, iv8) in layout.samples().iter().zip(sample_ivs) {
        if !seen_ivs.insert(*iv8) {
            return Err(CustodyError::InvalidPayload("sample_ivs"));
        }
        let start = usize::try_from(sample.mdat_offset())
            .map_err(|_| CustodyError::InvalidPayload("clear_segment"))?;
        let end = start
            .checked_add(sample.sample_size() as usize)
            .ok_or(CustodyError::InvalidPayload("clear_segment"))?;
        let mut sample_bytes = clear_mdat
            .get(start..end)
            .ok_or(CustodyError::InvalidPayload("clear_segment"))?
            .to_vec();
        let iv16 = pad_iv(iv8)?;
        ctr_xor(&mut sample_bytes, cenc_key, &iv16);
        encrypted_samples.push(sample_bytes);
    }
    Ok(encrypted_samples)
}

#[cfg(test)]
mod tests {
    use elastos_protected_content_provider_contracts::ValidatedCencFmp4MediaSessionLayoutV1;
    use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};

    use super::*;
    use crate::{
        decrypt_validated_cenc_fmp4_segment_to_clear_v1, mint_decrypt_session_from_seed,
        rewrite_validated_cenc_fmp4_init_to_clear_v1, wrap_content_key_to_decrypt_session,
    };

    const TRANSCRIPT: &[u8] = b"elastos.protected-content.test-protect-transcript/v1";
    const TFHD_FLAGS_PRODUCER_V1: u32 = 0x020038;
    const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
    const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;
    const VISUAL_SAMPLE_ENTRY_FIXED_BYTES: usize = 78;
    const AUDIO_SAMPLE_ENTRY_FIXED_BYTES: usize = 28;

    fn wrapped_cek(
        cek: &ContentEncryptionKeyV1,
    ) -> (
        crate::DecryptSessionSecretKeyV1,
        crate::DecryptSessionWrappedContentKeyV1,
    ) {
        let (session_secret, session_public) = mint_decrypt_session_from_seed([0x51; 32]).unwrap();
        let wrapped = wrap_content_key_to_decrypt_session(
            cek,
            &session_public,
            TRANSCRIPT,
            &mut HpkeStdRng::from_seed([0x71; 32]),
        )
        .unwrap();
        (session_secret, wrapped)
    }

    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(box_type);
        out.extend_from_slice(content);
        out
    }

    fn make_fullbox(box_type: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut content = Vec::with_capacity(4 + payload.len());
        content.push(version);
        content.extend_from_slice(&flags.to_be_bytes()[1..]);
        content.extend_from_slice(payload);
        make_box(box_type, &content)
    }

    fn make_clear_sample_entry(handler_type: &[u8; 4]) -> Vec<u8> {
        let (fourcc, fixed) = match handler_type {
            b"vide" => (b"avc1", VISUAL_SAMPLE_ENTRY_FIXED_BYTES),
            b"soun" => (b"mp4a", AUDIO_SAMPLE_ENTRY_FIXED_BYTES),
            _ => panic!("unsupported handler"),
        };
        let content = vec![0u8; fixed];
        make_box(fourcc, &content)
    }

    fn make_clear_trak(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
        let mut tkhd_payload = vec![0u8; 12];
        tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
        let tkhd = make_fullbox(b"tkhd", 0, 0, &tkhd_payload);

        let mut hdlr_payload = vec![0u8; 4];
        hdlr_payload.extend_from_slice(handler_type);
        let hdlr = make_fullbox(b"hdlr", 0, 0, &hdlr_payload);

        let entry = make_clear_sample_entry(handler_type);
        let mut stsd_payload = vec![0u8; 4];
        stsd_payload.extend_from_slice(&1u32.to_be_bytes());
        stsd_payload.extend_from_slice(&entry);
        let stsd = make_box(b"stsd", &stsd_payload);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);

        let mut mdia_content = Vec::new();
        mdia_content.extend_from_slice(&hdlr);
        mdia_content.extend_from_slice(&minf);
        let mdia = make_box(b"mdia", &mdia_content);

        let mut trak_content = Vec::new();
        trak_content.extend_from_slice(&tkhd);
        trak_content.extend_from_slice(&mdia);
        make_box(b"trak", &trak_content)
    }

    fn make_trex(track_id: u32) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&track_id.to_be_bytes());
        payload.extend_from_slice(&[0u8; 16]);
        make_fullbox(b"trex", 0, 0, &payload)
    }

    fn valid_clear_init_segment() -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
        let traks = [make_clear_trak(1, b"vide"), make_clear_trak(2, b"soun")];
        let mut mvex_content = Vec::new();
        mvex_content.extend_from_slice(&make_trex(1));
        mvex_content.extend_from_slice(&make_trex(2));
        let mvex = make_box(b"mvex", &mvex_content);
        let mvhd = make_box(b"mvhd", &[0u8; 4]);

        let mut moov_content = Vec::new();
        moov_content.extend_from_slice(&mvhd);
        for trak in traks {
            moov_content.extend_from_slice(&trak);
        }
        moov_content.extend_from_slice(&mvex);
        let moov = make_box(b"moov", &moov_content);

        let mut init = Vec::new();
        init.extend_from_slice(&ftyp);
        init.extend_from_slice(&moov);
        init
    }

    fn make_clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut tfhd_payload = Vec::new();
        tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
        tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
        tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
        let tfhd = make_fullbox(b"tfhd", 0, TFHD_FLAGS_PRODUCER_V1, &tfhd_payload);

        let mut trun_payload = Vec::new();
        trun_payload.extend_from_slice(&1u32.to_be_bytes());
        trun_payload.extend_from_slice(&0i32.to_be_bytes());
        trun_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        let trun = make_fullbox(
            b"trun",
            0,
            TRUN_FLAG_DATA_OFFSET | TRUN_FLAG_SAMPLE_SIZE,
            &trun_payload,
        );

        let traf = {
            let mut traf_content = Vec::new();
            traf_content.extend_from_slice(&tfhd);
            traf_content.extend_from_slice(&make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()));
            traf_content.extend_from_slice(&trun);
            make_box(b"traf", &traf_content)
        };
        let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let mut moof_content = Vec::new();
        moof_content.extend_from_slice(&mfhd);
        moof_content.extend_from_slice(&traf);
        let mut moof = make_box(b"moof", &moof_content);
        let sample_offset = (moof.len() + 8) as i32;
        let data_offset_at = moof.len() - trun.len() + 16;
        moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());

        let mdat = make_box(b"mdat", payload);
        let mut out = moof;
        out.extend_from_slice(&mdat);
        out
    }

    #[test]
    fn clear_fixture_protect_round_trips_through_validator_and_play_decrypt() {
        let clear_init = valid_clear_init_segment();
        let clear_segment = make_clear_segment(1, b"clear-video");
        let clear_layout = ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init).unwrap();
        let cek = ContentEncryptionKeyV1::from_test_bytes([0x42; 32]);
        let (protected_init, protected_segments, media_identity) =
            protect_validated_clear_cenc_fmp4_media_v1(
                &clear_layout,
                &clear_init,
                std::slice::from_ref(&clear_segment),
                "video/mp4",
                "avc1.64001f,mp4a.40.2",
                &cek,
                [0x55; 16],
                &[vec![[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]]],
            )
            .unwrap();

        let protected_layout =
            ValidatedCencFmp4MediaSessionLayoutV1::new(&media_identity, &protected_init).unwrap();
        let clear_rewritten =
            rewrite_validated_cenc_fmp4_init_to_clear_v1(&protected_layout, &protected_init)
                .unwrap();
        assert_eq!(clear_rewritten, clear_init);

        let (session_secret, wrapped) = wrapped_cek(&cek);
        let clear_segment_rewritten = decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &protected_layout,
            &protected_segments[0],
            0,
            &session_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .unwrap();
        assert_eq!(clear_segment_rewritten, clear_segment);
    }

    #[test]
    fn protect_rejects_wrong_iv_count_duplicate_iv_and_mutated_protected_boundary() {
        let clear_init = valid_clear_init_segment();
        let clear_segment = make_clear_segment(1, b"clear-video");
        let clear_layout = ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init).unwrap();
        let segment_layout = clear_layout.validate_segment(&clear_segment).unwrap();
        let cek = ContentEncryptionKeyV1::from_test_bytes([0x24; 32]);

        assert!(protect_validated_clear_fmp4_segment_to_cenc_v1(
            &segment_layout,
            &clear_segment,
            &cek,
            &[]
        )
        .is_err());

        let two_sample_segment = {
            let mut tfhd_payload = Vec::new();
            tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
            tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
            tfhd_payload.extend_from_slice(&5u32.to_be_bytes());
            tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
            let tfhd = make_fullbox(b"tfhd", 0, TFHD_FLAGS_PRODUCER_V1, &tfhd_payload);
            let mut trun_payload = Vec::new();
            trun_payload.extend_from_slice(&2u32.to_be_bytes());
            trun_payload.extend_from_slice(&0i32.to_be_bytes());
            trun_payload.extend_from_slice(&5u32.to_be_bytes());
            trun_payload.extend_from_slice(&6u32.to_be_bytes());
            let trun = make_fullbox(
                b"trun",
                0,
                TRUN_FLAG_DATA_OFFSET | TRUN_FLAG_SAMPLE_SIZE,
                &trun_payload,
            );
            let traf = {
                let mut traf_content = Vec::new();
                traf_content.extend_from_slice(&tfhd);
                traf_content.extend_from_slice(&make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()));
                traf_content.extend_from_slice(&trun);
                make_box(b"traf", &traf_content)
            };
            let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
            let mut moof_content = Vec::new();
            moof_content.extend_from_slice(&mfhd);
            moof_content.extend_from_slice(&traf);
            let mut moof = make_box(b"moof", &moof_content);
            let sample_offset = (moof.len() + 8) as i32;
            let data_offset_at = moof.len() - trun.len() + 16;
            moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());
            let mut out = moof;
            out.extend_from_slice(&make_box(b"mdat", b"hello-world"));
            out
        };
        let two_sample_layout = clear_layout.validate_segment(&two_sample_segment).unwrap();
        assert!(protect_validated_clear_fmp4_segment_to_cenc_v1(
            &two_sample_layout,
            &two_sample_segment,
            &cek,
            &[
                [0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22],
                [0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22]
            ]
        )
        .is_err());

        let (protected_init, protected_segments, media_identity) =
            protect_validated_clear_cenc_fmp4_media_v1(
                &clear_layout,
                &clear_init,
                std::slice::from_ref(&clear_segment),
                "video/mp4",
                "avc1.64001f,mp4a.40.2",
                &cek,
                [0x55; 16],
                &[vec![[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]]],
            )
            .unwrap();
        let protected_layout =
            ValidatedCencFmp4MediaSessionLayoutV1::new(&media_identity, &protected_init).unwrap();
        let mut tampered_segment = protected_segments[0].clone();
        tampered_segment[0] ^= 1;
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &protected_layout,
            &tampered_segment,
            0,
            &wrapped_cek(&cek).0,
            &wrapped_cek(&cek).1,
            TRANSCRIPT,
        )
        .is_err());
    }

    #[test]
    fn content_key_debug_is_redacted() {
        let cek = ContentEncryptionKeyV1::from_test_bytes([0xAB; 32]);
        let debug = format!("{cek:?}");
        assert!(!debug.contains("abab"));
        assert!(debug.contains("redacted"));
    }
}
