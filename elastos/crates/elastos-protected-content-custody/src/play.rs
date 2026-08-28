//! Play-path CENC decrypt that runs only after a PQ-hybrid decrypt-session CEK
//! unwrap. This is not a second wrap path and it never returns CEK bytes.

use elastos_protected_content_provider_contracts::{
    ValidatedCencFmp4MediaSessionLayoutV1, ValidatedCencFmp4SegmentLayoutV1,
};
use zeroize::Zeroizing;

use crate::{
    cenc::{ctr_xor, ctr_xor_subsamples, derive_cenc_aes128_key, pad_iv},
    unwrap_content_key_in_decrypt_session, CustodyError, DecryptSessionSecretKeyV1,
    DecryptSessionWrappedContentKeyV1,
};

pub fn rewrite_validated_cenc_fmp4_init_to_clear_v1(
    layout: &ValidatedCencFmp4MediaSessionLayoutV1,
    protected_init_segment: &[u8],
) -> Result<Vec<u8>, CustodyError> {
    if protected_init_segment.is_empty() {
        return Err(CustodyError::InvalidPayload("protected_init_segment"));
    }
    layout
        .rewrite_clear_init(protected_init_segment)
        .map_err(Into::into)
}

pub fn decrypt_validated_cenc_fmp4_segment_to_clear_v1(
    layout: &ValidatedCencFmp4MediaSessionLayoutV1,
    encrypted_segment: &[u8],
    segment_index: u32,
    session_secret: &DecryptSessionSecretKeyV1,
    wrapped: &DecryptSessionWrappedContentKeyV1,
    transcript: &[u8],
) -> Result<Vec<u8>, CustodyError> {
    if encrypted_segment.is_empty() {
        return Err(CustodyError::InvalidPayload("encrypted_segment"));
    }
    let segment_layout = layout
        .validate_indexed_segment(
            usize::try_from(segment_index)
                .map_err(|_| CustodyError::InvalidPayload("segment_index"))?,
            encrypted_segment,
        )
        .map_err(|_| CustodyError::InvalidPayload("segment_index"))?;
    let encrypted_mdat = segment_layout.exact_source_mdat_payload(encrypted_segment)?;
    let cenc_key = {
        let content_key =
            unwrap_content_key_in_decrypt_session(session_secret, wrapped, transcript)?;
        Zeroizing::new(content_key.with_bytes(derive_cenc_aes128_key))
    };
    let clear_mdat = decrypt_validated_segment_mdat_v1(&segment_layout, encrypted_mdat, &cenc_key)?;
    rewrite_validated_cenc_fmp4_segment_from_clear_mdat_v1(
        &segment_layout,
        encrypted_segment,
        &clear_mdat,
    )
}

fn rewrite_validated_cenc_fmp4_segment_from_clear_mdat_v1(
    segment_layout: &ValidatedCencFmp4SegmentLayoutV1,
    encrypted_segment: &[u8],
    clear_mdat: &[u8],
) -> Result<Vec<u8>, CustodyError> {
    segment_layout
        .rewrite_clear_segment(encrypted_segment, clear_mdat)
        .map_err(Into::into)
}

fn decrypt_validated_segment_mdat_v1(
    segment_layout: &ValidatedCencFmp4SegmentLayoutV1,
    encrypted_mdat: &[u8],
    cenc_key: &[u8; 16],
) -> Result<Vec<u8>, CustodyError> {
    if encrypted_mdat.is_empty() {
        return Err(CustodyError::InvalidPayload("segment_mdat"));
    }
    let mut cursor = 0usize;
    let mut clear_mdat = Vec::with_capacity(encrypted_mdat.len());
    for sample in segment_layout.samples() {
        let start = usize::try_from(sample.ciphertext_offset())
            .map_err(|_| CustodyError::InvalidPayload("segment_mdat"))?;
        if start != cursor {
            return Err(CustodyError::InvalidPayload("segment_mdat"));
        }
        let end = start
            .checked_add(sample.sample_size() as usize)
            .ok_or(CustodyError::InvalidPayload("segment_mdat"))?;
        let mut sample_bytes = encrypted_mdat
            .get(start..end)
            .ok_or(CustodyError::InvalidPayload("segment_mdat"))?
            .to_vec();
        let iv16 = pad_iv(&sample.iv())?;
        if sample.subsamples().is_empty() {
            ctr_xor(&mut sample_bytes, cenc_key, &iv16);
        } else {
            let subsamples: Vec<(u32, u32)> = sample
                .subsamples()
                .iter()
                .map(|subsample| {
                    (
                        u32::from(subsample.clear_bytes()),
                        subsample.encrypted_bytes(),
                    )
                })
                .collect();
            ctr_xor_subsamples(&mut sample_bytes, cenc_key, &iv16, &subsamples)?;
        }
        clear_mdat.extend_from_slice(&sample_bytes);
        cursor = end;
    }
    if cursor != encrypted_mdat.len() || clear_mdat.len() != encrypted_mdat.len() {
        return Err(CustodyError::InvalidPayload("segment_mdat"));
    }
    Ok(clear_mdat)
}

#[cfg(test)]
mod tests {
    use elastos_protected_content_contracts::CanonicalContract;
    use elastos_protected_content_provider_contracts::{
        CencFmp4MediaIdentityV1, ValidatedCencFmp4MediaSessionLayoutV1,
    };
    use rand09::{rngs::StdRng as HpkeStdRng, SeedableRng as _};

    use super::*;
    use crate::{
        cenc::derive_cenc_aes128_key, mint_decrypt_session_from_seed,
        wrap_content_key_to_decrypt_session, ContentEncryptionKeyV1, CONTENT_KEY_BYTES,
    };

    const TRANSCRIPT: &[u8] = b"elastos.protected-content.test-cenc-transcript/v1";
    const TFHD_FLAGS_PRODUCER_V1: u32 = 0x020038;
    const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
    const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;
    const SENC_FLAG_SUBSAMPLES: u32 = 0x000002;
    const VISUAL_SAMPLE_ENTRY_FIXED_BYTES: usize = 78;
    const AUDIO_SAMPLE_ENTRY_FIXED_BYTES: usize = 28;

    #[derive(Clone, Copy)]
    struct TestBoxHeader {
        box_type: [u8; 4],
        size: usize,
        header_size: usize,
    }

    fn wrapped_cek(
        cek: &ContentEncryptionKeyV1,
    ) -> (DecryptSessionSecretKeyV1, DecryptSessionWrappedContentKeyV1) {
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

    fn make_sinf(original_fourcc: &[u8; 4]) -> Vec<u8> {
        let frma = make_box(b"frma", original_fourcc);
        let mut schm_payload = Vec::new();
        schm_payload.extend_from_slice(b"cenc");
        schm_payload.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        let schm = make_fullbox(b"schm", 0, 0, &schm_payload);
        let mut tenc_payload = vec![0, 0, 1, 8];
        tenc_payload.extend_from_slice(&[0x44; 16]);
        let tenc = make_fullbox(b"tenc", 0, 0, &tenc_payload);
        let schi = make_box(b"schi", &tenc);
        let mut sinf_content = Vec::new();
        sinf_content.extend_from_slice(&frma);
        sinf_content.extend_from_slice(&schm);
        sinf_content.extend_from_slice(&schi);
        make_box(b"sinf", &sinf_content)
    }

    fn make_sample_entry(handler_type: &[u8; 4]) -> Vec<u8> {
        let (protected_fourcc, original_fourcc, fixed) = match handler_type {
            b"vide" => (b"encv", b"avc1", VISUAL_SAMPLE_ENTRY_FIXED_BYTES),
            b"soun" => (b"enca", b"mp4a", AUDIO_SAMPLE_ENTRY_FIXED_BYTES),
            _ => panic!("unsupported handler"),
        };
        let mut content = vec![0u8; fixed];
        content.extend_from_slice(&make_sinf(original_fourcc));
        make_box(protected_fourcc, &content)
    }

    fn make_trak(track_id: u32, handler_type: &[u8; 4]) -> Vec<u8> {
        let mut tkhd_payload = vec![0u8; 12];
        tkhd_payload[8..12].copy_from_slice(&track_id.to_be_bytes());
        let tkhd = make_fullbox(b"tkhd", 0, 0, &tkhd_payload);

        let mut hdlr_payload = vec![0u8; 4];
        hdlr_payload.extend_from_slice(handler_type);
        let hdlr = make_fullbox(b"hdlr", 0, 0, &hdlr_payload);

        let entry = make_sample_entry(handler_type);
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

    fn valid_init_segment() -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
        let mvhd = make_box(b"mvhd", &[0u8; 4]);
        let video_trak = make_trak(1, b"vide");
        let audio_trak = make_trak(2, b"soun");
        let mut mvex_content = Vec::new();
        mvex_content.extend_from_slice(&make_trex(1));
        mvex_content.extend_from_slice(&make_trex(2));
        let mvex = make_box(b"mvex", &mvex_content);

        let mut moov_content = Vec::new();
        moov_content.extend_from_slice(&mvhd);
        moov_content.extend_from_slice(&video_trak);
        moov_content.extend_from_slice(&audio_trak);
        moov_content.extend_from_slice(&mvex);
        let moov = make_box(b"moov", &moov_content);

        let mut init = Vec::new();
        init.extend_from_slice(&ftyp);
        init.extend_from_slice(&moov);
        init
    }

    fn make_segment_with(
        track_id: u32,
        payload: &[u8],
        subsamples: Option<&[(u16, u32)]>,
    ) -> Vec<u8> {
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

        let iv = (u64::from(track_id) + 0x10).to_be_bytes();
        let mut senc_payload = Vec::new();
        senc_payload.extend_from_slice(&1u32.to_be_bytes());
        senc_payload.extend_from_slice(&iv);
        let senc_flags = if subsamples.is_some() {
            SENC_FLAG_SUBSAMPLES
        } else {
            0
        };
        if let Some(subsamples) = subsamples {
            senc_payload.extend_from_slice(&(subsamples.len() as u16).to_be_bytes());
            for (clear, encrypted) in subsamples {
                senc_payload.extend_from_slice(&clear.to_be_bytes());
                senc_payload.extend_from_slice(&encrypted.to_be_bytes());
            }
        }
        let senc = make_fullbox(b"senc", 0, senc_flags, &senc_payload);

        let mut traf_content = Vec::new();
        traf_content.extend_from_slice(&tfhd);
        traf_content.extend_from_slice(&make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()));
        traf_content.extend_from_slice(&trun);
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let mut moof_content = Vec::new();
        moof_content.extend_from_slice(&mfhd);
        moof_content.extend_from_slice(&traf);
        let mut moof = make_box(b"moof", &moof_content);

        let moof_children = scan_boxes(&moof, 8, moof.len()).unwrap();
        let traf_box = moof_children
            .iter()
            .copied()
            .find(|(_, header)| header.box_type == *b"traf")
            .unwrap();
        let traf_children = scan_boxes(
            &moof,
            traf_box.0 + traf_box.1.header_size,
            traf_box.0 + traf_box.1.size,
        )
        .unwrap();
        let trun_box = traf_children
            .iter()
            .copied()
            .find(|(_, header)| header.box_type == *b"trun")
            .unwrap();
        let data_offset_at = trun_box.0 + trun_box.1.header_size + 8;
        let sample_offset = (moof.len() + 8) as i32;
        moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());

        let mdat = make_box(b"mdat", payload);
        let mut out = moof;
        out.extend_from_slice(&mdat);
        out
    }

    fn session_layout_for(
        init_segment: &[u8],
        encrypted_segments: &[Vec<u8>],
    ) -> ValidatedCencFmp4MediaSessionLayoutV1 {
        let media_identity = CencFmp4MediaIdentityV1::new_from_bytes(
            init_segment,
            encrypted_segments,
            "video/mp4",
            "avc1,mp4a",
        )
        .unwrap();
        ValidatedCencFmp4MediaSessionLayoutV1::new(&media_identity, init_segment).unwrap()
    }

    fn encrypt_sample_bytes(
        plaintext: &[u8],
        track_id: u32,
        cek_bytes: &[u8; CONTENT_KEY_BYTES],
        subsamples: &[(u16, u32)],
    ) -> Vec<u8> {
        let mut ciphertext = plaintext.to_vec();
        let key = derive_cenc_aes128_key(cek_bytes);
        let iv = pad_iv(&(u64::from(track_id) + 0x10).to_be_bytes()).unwrap();
        if subsamples.is_empty() {
            ctr_xor(&mut ciphertext, &key, &iv);
        } else {
            let pairs: Vec<(u32, u32)> = subsamples
                .iter()
                .map(|(clear, encrypted)| (u32::from(*clear), *encrypted))
                .collect();
            ctr_xor_subsamples(&mut ciphertext, &key, &iv, &pairs).unwrap();
        }
        ciphertext
    }

    fn read_u32_at(bytes: &[u8], start: usize) -> u32 {
        u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap())
    }

    fn read_u64_at(bytes: &[u8], start: usize) -> u64 {
        u64::from_be_bytes(bytes[start..start + 8].try_into().unwrap())
    }

    fn read_box_header(bytes: &[u8], offset: usize) -> TestBoxHeader {
        let size32 = read_u32_at(bytes, offset);
        let box_type: [u8; 4] = bytes[offset + 4..offset + 8].try_into().unwrap();
        if size32 == 1 {
            TestBoxHeader {
                box_type,
                size: usize::try_from(read_u64_at(bytes, offset + 8)).unwrap(),
                header_size: 16,
            }
        } else {
            TestBoxHeader {
                box_type,
                size: size32 as usize,
                header_size: 8,
            }
        }
    }

    fn scan_boxes(
        bytes: &[u8],
        start: usize,
        end: usize,
    ) -> Result<Vec<(usize, TestBoxHeader)>, ()> {
        if start > end || end > bytes.len() {
            return Err(());
        }
        let mut out = Vec::new();
        let mut offset = start;
        while offset < end {
            let header = read_box_header(bytes, offset);
            let next = offset.checked_add(header.size).ok_or(())?;
            if header.size < header.header_size || next > end {
                return Err(());
            }
            out.push((offset, header));
            offset = next;
        }
        if offset != end {
            return Err(());
        }
        Ok(out)
    }

    fn child_box(
        bytes: &[u8],
        start: usize,
        end: usize,
        box_type: &[u8; 4],
    ) -> (usize, TestBoxHeader) {
        scan_boxes(bytes, start, end)
            .unwrap()
            .into_iter()
            .find(|(_, header)| header.box_type == *box_type)
            .unwrap()
    }

    fn mdat_payload(bytes: &[u8]) -> &[u8] {
        let (mdat_off, mdat_h) = child_box(bytes, 0, bytes.len(), b"mdat");
        &bytes[mdat_off + mdat_h.header_size..mdat_off + mdat_h.size]
    }

    #[test]
    fn playback_helper_rewrites_multiplexed_init_for_all_tracks() {
        let init_segment = valid_init_segment();
        let encrypted_segment = make_segment_with(1, b"video-segment", None);
        let layout = session_layout_for(&init_segment, &[encrypted_segment]);

        let clear_init =
            rewrite_validated_cenc_fmp4_init_to_clear_v1(&layout, &init_segment).unwrap();
        let top = scan_boxes(&clear_init, 0, clear_init.len()).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1.box_type, *b"ftyp");
        assert_eq!(top[1].1.box_type, *b"moov");
        assert!(clear_init.windows(4).any(|window| window == b"avc1"));
        assert!(clear_init.windows(4).any(|window| window == b"mp4a"));
        assert!(!clear_init.windows(4).any(|window| window == b"encv"));
        assert!(!clear_init.windows(4).any(|window| window == b"enca"));
        assert!(!clear_init.windows(4).any(|window| window == b"sinf"));
    }

    #[test]
    fn playback_helper_decrypts_exact_segment_to_playable_clear_fmp4() {
        let cek = ContentEncryptionKeyV1::from_test_bytes([0x42; 32]);
        let cek_bytes = cek.with_bytes(|bytes| *bytes);
        let subsamples = [(2u16, 4u32)];
        let plaintext_sample = b"abCDEF".to_vec();
        let encrypted_sample = encrypt_sample_bytes(&plaintext_sample, 1, &cek_bytes, &subsamples);
        let init_segment = valid_init_segment();
        let encrypted_segment = make_segment_with(1, &encrypted_sample, Some(&subsamples));
        let layout = session_layout_for(&init_segment, std::slice::from_ref(&encrypted_segment));
        let (session_secret, wrapped) = wrapped_cek(&cek);

        let clear_init =
            rewrite_validated_cenc_fmp4_init_to_clear_v1(&layout, &init_segment).unwrap();
        let clear_segment = decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &encrypted_segment,
            0,
            &session_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .unwrap();

        let top = scan_boxes(&clear_segment, 0, clear_segment.len()).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1.box_type, *b"moof");
        assert_eq!(top[1].1.box_type, *b"mdat");
        assert!(clear_segment.len() > plaintext_sample.len());
        assert_eq!(mdat_payload(&clear_segment), plaintext_sample.as_slice());
        assert!(!clear_segment.windows(4).any(|window| window == b"senc"));
        assert!(!clear_init
            .windows(CONTENT_KEY_BYTES)
            .any(|window| window == cek_bytes));
        let sealed = wrapped.sealed_share().canonical_bytes().unwrap();
        if clear_segment.len() >= sealed.len() {
            assert!(!clear_segment
                .windows(sealed.len())
                .any(|window| window == sealed.as_slice()));
        }
    }

    #[test]
    fn playback_helper_fails_closed_for_changed_source_wrong_session_and_bad_lengths() {
        let cek = ContentEncryptionKeyV1::from_test_bytes([0x42; 32]);
        let cek_bytes = cek.with_bytes(|bytes| *bytes);
        let subsamples = [(2u16, 4u32)];
        let plaintext_sample = b"abCDEF".to_vec();
        let encrypted_sample = encrypt_sample_bytes(&plaintext_sample, 1, &cek_bytes, &subsamples);
        let init_segment = valid_init_segment();
        let encrypted_segment = make_segment_with(1, &encrypted_sample, Some(&subsamples));
        let layout = session_layout_for(&init_segment, std::slice::from_ref(&encrypted_segment));
        let (session_secret, wrapped) = wrapped_cek(&cek);
        let wrong_secret = mint_decrypt_session_from_seed([0x52; 32]).unwrap().0;

        let mut changed_segment = encrypted_segment.clone();
        let last = changed_segment.len() - 1;
        changed_segment[last] ^= 1;
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &changed_segment,
            0,
            &session_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .is_err());
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &encrypted_segment,
            1,
            &session_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .is_err());
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &encrypted_segment,
            0,
            &wrong_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .is_err());
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &encrypted_segment,
            0,
            &session_secret,
            &wrapped,
            b"wrong-transcript",
        )
        .is_err());
        assert!(rewrite_validated_cenc_fmp4_segment_from_clear_mdat_v1(
            &layout
                .validate_indexed_segment(0, &encrypted_segment)
                .unwrap(),
            &encrypted_segment,
            &plaintext_sample[..plaintext_sample.len() - 1],
        )
        .is_err());
        assert!(rewrite_validated_cenc_fmp4_init_to_clear_v1(&layout, &[]).is_err());
        assert!(decrypt_validated_cenc_fmp4_segment_to_clear_v1(
            &layout,
            &[],
            0,
            &session_secret,
            &wrapped,
            TRANSCRIPT,
        )
        .is_err());
    }
}
