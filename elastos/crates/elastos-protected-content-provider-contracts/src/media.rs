use elastos_protected_content_contracts::{
    CanonicalContract, ContractError, Digest32, EncryptedContentIdentityV1,
    MAX_ENCRYPTED_CONTENT_BYTES,
};
use sha2::{Digest as _, Sha256};

const CENC_FMP4_SEGMENT_IDENTITY_DOMAIN_V1: &str = "elastos.protected-content.cenc-fmp4-segment/v1";
const CENC_FMP4_MEDIA_IDENTITY_DOMAIN_V1: &str =
    "elastos.protected-content.cenc-fmp4-media-identity/v1";
pub const CENC_FMP4_MEDIA_SUITE_ID_V1: &str = "cenc-fmp4-aes128ctr/v1";
pub const MAX_CENC_FMP4_MEDIA_IDENTITY_BYTES_V1: usize = 64 * 1024;
const CIPHERTEXT_STREAM_IDENTITY_DOMAIN_V1: &[u8] =
    b"elastos.protected-content.cenc-fmp4-ciphertext-stream/v1";
const MEDIA_MANIFEST_ROOT_DOMAIN_V1: &[u8] =
    b"elastos.protected-content.cenc-fmp4-manifest-root/v1";
pub(crate) const MAX_MEDIA_DECLARATION_BYTES_V1: usize = 255;
const MAX_MEDIA_SEGMENTS_V1: usize = 512;
const MAX_MEDIA_TRACKS_V1: usize = 8;
const MAX_TRACK_SAMPLES_V1: usize = 1 << 20;
const MAX_CHILD_BOXES_V1: usize = 64;
const SUPPORTED_COMMON_IV_SIZE_V1: u8 = 8;
const VISUAL_SAMPLE_ENTRY_FIXED_BYTES: usize = 78;
const AUDIO_SAMPLE_ENTRY_FIXED_BYTES: usize = 28;
const TFHD_FLAG_BASE_DATA_OFFSET: u32 = 0x000001;
const TFHD_FLAG_DEFAULT_SAMPLE_DURATION: u32 = 0x000008;
const TFHD_FLAG_DEFAULT_SAMPLE_SIZE: u32 = 0x000010;
const TFHD_FLAG_DEFAULT_SAMPLE_FLAGS: u32 = 0x000020;
const TFHD_FLAG_DEFAULT_BASE_IS_MOOF: u32 = 0x020000;
const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
const TRUN_FLAG_FIRST_SAMPLE_FLAGS: u32 = 0x000004;
const TRUN_FLAG_SAMPLE_DURATION: u32 = 0x000100;
const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;
const TRUN_FLAG_SAMPLE_FLAGS: u32 = 0x000400;
const TRUN_FLAG_SAMPLE_COMPOSITION_TIME_OFFSET: u32 = 0x000800;
const SENC_FLAG_SUBSAMPLES: u32 = 0x000002;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    box_type: [u8; 4],
    size: usize,
    header_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoxResizeTargetV1 {
    box_off: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedCencFmp4TrackRewriteV1 {
    track_id: u32,
    sample_entry_off: usize,
    original_fourcc: [u8; 4],
    sinf_off: usize,
    sinf_size: usize,
    resize_targets: Vec<BoxResizeTargetV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedCencFmp4SegmentRewriteV1 {
    moof_off: usize,
    traf_off: usize,
    trun_off: usize,
    senc_off: usize,
    senc_size: usize,
    trun_data_offset: i32,
    mdat_content_start: usize,
    mdat_content_end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedClearFmp4TrackLayoutV1 {
    track_id: u32,
    sample_entry_off: usize,
    original_fourcc: [u8; 4],
    resize_targets: Vec<BoxResizeTargetV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedClearFmp4MediaSessionLayoutV1 {
    init_segment_sha256: Digest32,
    init_segment_bytes: u64,
    track_ids: Vec<u32>,
    tracks: Vec<ValidatedClearFmp4TrackLayoutV1>,
}

impl ValidatedClearFmp4MediaSessionLayoutV1 {
    pub fn new(clear_init_segment: &[u8]) -> Result<Self, ContractError> {
        let init = validate_clear_init_segment_v1(clear_init_segment)?;
        Ok(Self {
            init_segment_sha256: Digest32::new(Sha256::digest(clear_init_segment).into()),
            init_segment_bytes: clear_init_segment.len() as u64,
            track_ids: init.track_ids,
            tracks: init.tracks,
        })
    }

    pub fn track_ids(&self) -> &[u32] {
        &self.track_ids
    }

    pub fn validate_segment(
        &self,
        clear_segment: &[u8],
    ) -> Result<ValidatedClearFmp4SegmentLayoutV1, ContractError> {
        validate_clear_media_segment_v1(clear_segment, self.track_ids.as_slice())
    }

    pub fn rewrite_protected_init(
        &self,
        clear_init_segment: &[u8],
        key_id: [u8; 16],
    ) -> Result<Vec<u8>, ContractError> {
        verify_exact_source_bytes(
            clear_init_segment,
            self.init_segment_sha256,
            self.init_segment_bytes,
            "init_segment_bytes",
        )?;
        rewrite_protected_init_v1(clear_init_segment, &self.tracks, key_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedClearFmp4SegmentLayoutV1 {
    source_sha256: Digest32,
    source_bytes: u64,
    track_id: u32,
    samples: Vec<ValidatedClearFmp4SampleLayoutV1>,
    rewrite: ValidatedClearFmp4SegmentRewriteV1,
}

impl ValidatedClearFmp4SegmentLayoutV1 {
    pub const fn source_sha256(&self) -> Digest32 {
        self.source_sha256
    }

    pub const fn source_bytes(&self) -> u64 {
        self.source_bytes
    }

    pub const fn track_id(&self) -> u32 {
        self.track_id
    }

    pub fn samples(&self) -> &[ValidatedClearFmp4SampleLayoutV1] {
        &self.samples
    }

    pub fn exact_source_mdat_payload<'a>(
        &self,
        clear_segment: &'a [u8],
    ) -> Result<&'a [u8], ContractError> {
        verify_exact_source_bytes(
            clear_segment,
            self.source_sha256,
            self.source_bytes,
            "clear_segment_bytes",
        )?;
        clear_segment
            .get(self.rewrite.mdat_content_start..self.rewrite.mdat_content_end)
            .ok_or(ContractError::InvalidField("clear_segment_bytes"))
    }

    pub fn rewrite_protected_segment(
        &self,
        clear_segment: &[u8],
        encrypted_samples: &[Vec<u8>],
        sample_ivs: &[[u8; 8]],
    ) -> Result<Vec<u8>, ContractError> {
        verify_exact_source_bytes(
            clear_segment,
            self.source_sha256,
            self.source_bytes,
            "clear_segment_bytes",
        )?;
        if encrypted_samples.len() != self.samples.len() || sample_ivs.len() != self.samples.len() {
            return Err(ContractError::InvalidField("clear_segment_bytes"));
        }
        let mut seen_ivs = std::collections::BTreeSet::new();
        let mut protected_mdat = Vec::with_capacity(
            self.rewrite
                .mdat_content_end
                .checked_sub(self.rewrite.mdat_content_start)
                .ok_or(ContractError::InvalidField("clear_segment_bytes"))?,
        );
        for ((sample, encrypted_sample), iv) in
            self.samples.iter().zip(encrypted_samples).zip(sample_ivs)
        {
            if encrypted_sample.len() != sample.sample_size as usize || !seen_ivs.insert(*iv) {
                return Err(ContractError::InvalidField("clear_segment_bytes"));
            }
            protected_mdat.extend_from_slice(encrypted_sample);
        }
        let senc = make_fullsample_senc_box_v1(sample_ivs)?;
        let mut rewritten = clear_segment.to_vec();
        rewritten[self.rewrite.mdat_content_start..self.rewrite.mdat_content_end]
            .copy_from_slice(&protected_mdat);
        grow_box_size_in_place(
            &mut rewritten,
            self.rewrite.traf_off,
            senc.len(),
            "clear_segment_bytes",
        )?;
        grow_box_size_in_place(
            &mut rewritten,
            self.rewrite.moof_off,
            senc.len(),
            "clear_segment_bytes",
        )?;
        let new_data_offset = self
            .rewrite
            .trun_data_offset
            .checked_add(
                i32::try_from(senc.len())
                    .map_err(|_| ContractError::InvalidField("clear_segment_bytes"))?,
            )
            .ok_or(ContractError::InvalidField("clear_segment_bytes"))?;
        let data_offset_at = self
            .rewrite
            .trun_off
            .checked_add(read_box_header(&rewritten, self.rewrite.trun_off)?.header_size)
            .and_then(|value| value.checked_add(8))
            .ok_or(ContractError::InvalidField("clear_segment_bytes"))?;
        rewritten[data_offset_at..data_offset_at + 4]
            .copy_from_slice(&new_data_offset.to_be_bytes());
        rewritten.splice(
            self.rewrite.senc_insert_off..self.rewrite.senc_insert_off,
            senc,
        );
        Ok(rewritten)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedClearFmp4SampleLayoutV1 {
    mdat_offset: u64,
    sample_size: u32,
}

impl ValidatedClearFmp4SampleLayoutV1 {
    pub const fn mdat_offset(&self) -> u64 {
        self.mdat_offset
    }

    pub const fn sample_size(&self) -> u32 {
        self.sample_size
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedClearFmp4SegmentRewriteV1 {
    moof_off: usize,
    traf_off: usize,
    trun_off: usize,
    trun_data_offset: i32,
    mdat_content_start: usize,
    mdat_content_end: usize,
    senc_insert_off: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCencFmp4MediaLayoutV1 {
    init_segment_sha256: Digest32,
    init_segment_bytes: u64,
    protected_track_ids: Vec<u32>,
    track_rewrites: Vec<ValidatedCencFmp4TrackRewriteV1>,
    segments: Vec<ValidatedCencFmp4SegmentLayoutV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCencFmp4MediaSessionLayoutV1 {
    media_identity: CencFmp4MediaIdentityV1,
    protected_track_ids: Vec<u32>,
    track_rewrites: Vec<ValidatedCencFmp4TrackRewriteV1>,
}

impl ValidatedCencFmp4MediaSessionLayoutV1 {
    pub fn new(
        media_identity: &CencFmp4MediaIdentityV1,
        protected_init_segment: &[u8],
    ) -> Result<Self, ContractError> {
        media_identity.validate()?;
        let init = validate_staged_init_segment_v1(
            protected_init_segment,
            media_identity.init_segment_sha256(),
            media_identity.init_segment_bytes(),
        )?;
        Ok(Self {
            media_identity: media_identity.clone(),
            protected_track_ids: init.protected_track_ids,
            track_rewrites: init.track_rewrites,
        })
    }

    pub fn media_identity(&self) -> &CencFmp4MediaIdentityV1 {
        &self.media_identity
    }

    pub fn protected_track_ids(&self) -> &[u32] {
        &self.protected_track_ids
    }

    pub fn rewrite_clear_init(
        &self,
        protected_init_segment: &[u8],
    ) -> Result<Vec<u8>, ContractError> {
        rewrite_clear_init_v1(
            protected_init_segment,
            self.media_identity.init_segment_sha256(),
            self.media_identity.init_segment_bytes(),
            &self.track_rewrites,
        )
    }

    pub fn validate_indexed_segment(
        &self,
        segment_index: usize,
        encrypted_segment: &[u8],
    ) -> Result<ValidatedCencFmp4SegmentLayoutV1, ContractError> {
        let expected_segment = self
            .media_identity
            .encrypted_segments()
            .get(segment_index)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        validate_staged_segment_v1(
            expected_segment,
            encrypted_segment,
            self.protected_track_ids.as_slice(),
        )
    }
}

impl ValidatedCencFmp4MediaLayoutV1 {
    pub const fn init_segment_sha256(&self) -> Digest32 {
        self.init_segment_sha256
    }

    pub const fn init_segment_bytes(&self) -> u64 {
        self.init_segment_bytes
    }

    pub fn protected_track_ids(&self) -> &[u32] {
        &self.protected_track_ids
    }

    pub fn segments(&self) -> &[ValidatedCencFmp4SegmentLayoutV1] {
        &self.segments
    }

    pub fn rewrite_clear_init(
        &self,
        protected_init_segment: &[u8],
    ) -> Result<Vec<u8>, ContractError> {
        rewrite_clear_init_v1(
            protected_init_segment,
            self.init_segment_sha256,
            self.init_segment_bytes,
            &self.track_rewrites,
        )
    }

    pub fn rewrite_clear_segment(
        &self,
        segment_index: usize,
        encrypted_segment: &[u8],
        clear_mdat: &[u8],
    ) -> Result<Vec<u8>, ContractError> {
        self.segments
            .get(segment_index)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?
            .rewrite_clear_segment(encrypted_segment, clear_mdat)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCencFmp4SegmentLayoutV1 {
    source_sha256: Digest32,
    source_bytes: u64,
    track_id: u32,
    samples: Vec<ValidatedCencFmp4SampleLayoutV1>,
    rewrite: ValidatedCencFmp4SegmentRewriteV1,
}

impl ValidatedCencFmp4SegmentLayoutV1 {
    pub const fn source_sha256(&self) -> Digest32 {
        self.source_sha256
    }

    pub const fn source_bytes(&self) -> u64 {
        self.source_bytes
    }

    pub const fn track_id(&self) -> u32 {
        self.track_id
    }

    pub fn samples(&self) -> &[ValidatedCencFmp4SampleLayoutV1] {
        &self.samples
    }

    pub fn exact_source_mdat_payload<'a>(
        &self,
        encrypted_segment: &'a [u8],
    ) -> Result<&'a [u8], ContractError> {
        verify_exact_source_bytes(
            encrypted_segment,
            self.source_sha256,
            self.source_bytes,
            "encrypted_segments",
        )?;
        encrypted_segment
            .get(self.rewrite.mdat_content_start..self.rewrite.mdat_content_end)
            .ok_or(ContractError::InvalidField("encrypted_segments"))
    }

    pub fn rewrite_clear_segment(
        &self,
        encrypted_segment: &[u8],
        clear_mdat: &[u8],
    ) -> Result<Vec<u8>, ContractError> {
        verify_exact_source_bytes(
            encrypted_segment,
            self.source_sha256,
            self.source_bytes,
            "encrypted_segments",
        )?;
        rewrite_clear_segment_unchecked_v1(&self.rewrite, encrypted_segment, clear_mdat)
    }
}

fn rewrite_clear_segment_unchecked_v1(
    rewrite: &ValidatedCencFmp4SegmentRewriteV1,
    encrypted_segment: &[u8],
    clear_mdat: &[u8],
) -> Result<Vec<u8>, ContractError> {
    let expected_mdat_len = rewrite
        .mdat_content_end
        .checked_sub(rewrite.mdat_content_start)
        .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    if clear_mdat.len() != expected_mdat_len {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let mut rewritten = encrypted_segment.to_vec();
    rewritten[rewrite.mdat_content_start..rewrite.mdat_content_end].copy_from_slice(clear_mdat);
    shrink_box_size_in_place(
        &mut rewritten,
        rewrite.traf_off,
        rewrite.senc_size,
        "encrypted_segments",
    )?;
    shrink_box_size_in_place(
        &mut rewritten,
        rewrite.moof_off,
        rewrite.senc_size,
        "encrypted_segments",
    )?;
    let new_data_offset = rewrite
        .trun_data_offset
        .checked_sub(
            i32::try_from(rewrite.senc_size)
                .map_err(|_| ContractError::InvalidField("encrypted_segments"))?,
        )
        .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    if new_data_offset < 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let data_offset_at = rewrite
        .trun_off
        .checked_add(read_box_header(&rewritten, rewrite.trun_off)?.header_size)
        .and_then(|value| value.checked_add(8))
        .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    rewritten[data_offset_at..data_offset_at + 4].copy_from_slice(&new_data_offset.to_be_bytes());
    let senc_end = rewrite
        .senc_off
        .checked_add(rewrite.senc_size)
        .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    if senc_end > rewritten.len() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    rewritten.drain(rewrite.senc_off..senc_end);
    Ok(rewritten)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCencFmp4SampleLayoutV1 {
    ciphertext_offset: u64,
    sample_size: u32,
    iv: [u8; 8],
    subsamples: Vec<ValidatedCencFmp4SubsampleV1>,
}

impl ValidatedCencFmp4SampleLayoutV1 {
    pub const fn ciphertext_offset(&self) -> u64 {
        self.ciphertext_offset
    }

    pub const fn sample_size(&self) -> u32 {
        self.sample_size
    }

    pub const fn iv(&self) -> [u8; 8] {
        self.iv
    }

    pub fn subsamples(&self) -> &[ValidatedCencFmp4SubsampleV1] {
        &self.subsamples
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCencFmp4SubsampleV1 {
    clear_bytes: u16,
    encrypted_bytes: u32,
}

impl ValidatedCencFmp4SubsampleV1 {
    pub const fn clear_bytes(&self) -> u16 {
        self.clear_bytes
    }

    pub const fn encrypted_bytes(&self) -> u32 {
        self.encrypted_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CencFmp4SegmentIdentityV1 {
    ciphertext_sha256: Digest32,
    ciphertext_bytes: u64,
}

impl CencFmp4SegmentIdentityV1 {
    pub fn new_from_encrypted_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.is_empty() || (bytes.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(ContractError::InvalidField("ciphertext_bytes"));
        }
        Ok(Self {
            ciphertext_sha256: Digest32::new(Sha256::digest(bytes).into()),
            ciphertext_bytes: bytes.len() as u64,
        })
    }

    fn new_recorded(
        ciphertext_sha256: Digest32,
        ciphertext_bytes: u64,
    ) -> Result<Self, ContractError> {
        let value = Self {
            ciphertext_sha256,
            ciphertext_bytes,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn ciphertext_sha256(&self) -> Digest32 {
        self.ciphertext_sha256
    }

    pub const fn ciphertext_bytes(&self) -> u64 {
        self.ciphertext_bytes
    }
}

impl CencFmp4SegmentIdentityV1 {
    fn validate(&self) -> Result<(), ContractError> {
        if self.ciphertext_bytes == 0 || self.ciphertext_bytes > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(ContractError::InvalidField("ciphertext_bytes"));
        }
        Ok(())
    }
}

impl CanonicalContract for CencFmp4SegmentIdentityV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let mut bytes = canonical_prefix(CENC_FMP4_SEGMENT_IDENTITY_DOMAIN_V1);
        bytes.extend_from_slice(self.ciphertext_sha256.as_bytes());
        bytes.extend_from_slice(&self.ciphertext_bytes.to_be_bytes());
        Ok(bytes)
    }

    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let mut cursor = canonical_cursor(bytes, CENC_FMP4_SEGMENT_IDENTITY_DOMAIN_V1)?;
        let value = Self::new_recorded(
            Digest32::new(read_fixed_32(bytes, &mut cursor)?),
            read_u64(bytes, &mut cursor)?,
        )?;
        finish_canonical(bytes, cursor)?;
        if value.canonical_bytes()?.as_slice() != bytes {
            return Err(ContractError::InvalidField("non-canonical encoding"));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CencFmp4MediaIdentityV1 {
    encrypted_content: EncryptedContentIdentityV1,
    media_manifest_root: Digest32,
    init_segment_sha256: Digest32,
    init_segment_bytes: u64,
    mime_type: String,
    codecs: String,
    encrypted_segments: Vec<CencFmp4SegmentIdentityV1>,
}

impl CencFmp4MediaIdentityV1 {
    pub fn validate_structure(
        init_segment: &[u8],
        encrypted_segments: &[Vec<u8>],
    ) -> Result<ValidatedCencFmp4MediaLayoutV1, ContractError> {
        validate_cenc_fmp4_media_structure_v1(init_segment, encrypted_segments)
    }

    pub fn new_from_bytes(
        init_segment: &[u8],
        encrypted_segments: &[Vec<u8>],
        mime_type: impl Into<String>,
        codecs: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let _ = Self::validate_structure(init_segment, encrypted_segments)?;
        if init_segment.is_empty() || (init_segment.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
        if encrypted_segments.is_empty() || encrypted_segments.len() > MAX_MEDIA_SEGMENTS_V1 {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        let encrypted_segment_bytes = encrypted_segments;
        let encrypted_segments = encrypted_segment_bytes
            .iter()
            .map(|segment| CencFmp4SegmentIdentityV1::new_from_encrypted_bytes(segment))
            .collect::<Result<Vec<_>, _>>()?;
        let encrypted_content =
            Self::compute_encrypted_content_from_bytes(encrypted_segment_bytes)?;
        let mime_type = mime_type.into();
        let codecs = codecs.into();
        let init_segment_sha256 = Digest32::new(Sha256::digest(init_segment).into());
        let media_manifest_root = Self::compute_media_manifest_root(
            encrypted_content.clone(),
            init_segment_sha256,
            init_segment.len() as u64,
            &mime_type,
            &codecs,
            encrypted_segments.as_slice(),
        )?;
        let value = Self {
            encrypted_content,
            media_manifest_root,
            init_segment_sha256,
            init_segment_bytes: init_segment.len() as u64,
            mime_type,
            codecs,
            encrypted_segments,
        };
        value.validate()?;
        Ok(value)
    }

    fn new_recorded(
        encrypted_content: EncryptedContentIdentityV1,
        media_manifest_root: Digest32,
        init_segment_sha256: Digest32,
        init_segment_bytes: u64,
        mime_type: impl Into<String>,
        codecs: impl Into<String>,
        encrypted_segments: Vec<CencFmp4SegmentIdentityV1>,
    ) -> Result<Self, ContractError> {
        let value = Self {
            encrypted_content,
            media_manifest_root,
            init_segment_sha256,
            init_segment_bytes,
            mime_type: mime_type.into(),
            codecs: codecs.into(),
            encrypted_segments,
        };
        value.validate()?;
        Ok(value)
    }

    pub const fn encrypted_content(&self) -> &EncryptedContentIdentityV1 {
        &self.encrypted_content
    }

    pub const fn media_manifest_root(&self) -> Digest32 {
        self.media_manifest_root
    }

    pub const fn init_segment_sha256(&self) -> Digest32 {
        self.init_segment_sha256
    }

    pub const fn init_segment_bytes(&self) -> u64 {
        self.init_segment_bytes
    }

    pub fn mime_type(&self) -> &str {
        &self.mime_type
    }

    pub fn codecs(&self) -> &str {
        &self.codecs
    }

    pub fn encrypted_segments(&self) -> &[CencFmp4SegmentIdentityV1] {
        &self.encrypted_segments
    }

    pub const fn suite_id(&self) -> &'static str {
        CENC_FMP4_MEDIA_SUITE_ID_V1
    }

    fn validate_mime_and_codecs(&self) -> Result<(), ContractError> {
        validate_visible_ascii(&self.mime_type, "mime_type", MAX_MEDIA_DECLARATION_BYTES_V1)?;
        validate_visible_ascii(&self.codecs, "codecs", MAX_MEDIA_DECLARATION_BYTES_V1)
    }

    fn compute_encrypted_content_from_bytes(
        encrypted_segments: &[Vec<u8>],
    ) -> Result<EncryptedContentIdentityV1, ContractError> {
        let mut total_bytes = 0u64;
        let mut hasher = Sha256::new();
        hasher.update(CIPHERTEXT_STREAM_IDENTITY_DOMAIN_V1);
        hasher.update(
            u32::try_from(encrypted_segments.len())
                .map_err(|_| ContractError::FieldTooLong("encrypted_segments"))?
                .to_be_bytes(),
        );
        for segment in encrypted_segments {
            total_bytes = total_bytes
                .checked_add(segment.len() as u64)
                .ok_or(ContractError::InvalidField("total_encrypted_bytes"))?;
            hasher.update((segment.len() as u64).to_be_bytes());
            hasher.update(segment);
        }
        EncryptedContentIdentityV1::new(Digest32::new(hasher.finalize().into()), total_bytes)
    }

    fn sum_encrypted_bytes(
        encrypted_segments: &[CencFmp4SegmentIdentityV1],
    ) -> Result<u64, ContractError> {
        encrypted_segments.iter().try_fold(0u64, |acc, segment| {
            acc.checked_add(segment.ciphertext_bytes())
                .ok_or(ContractError::InvalidField("total_encrypted_bytes"))
        })
    }

    fn compute_media_manifest_root(
        encrypted_content: EncryptedContentIdentityV1,
        init_segment_sha256: Digest32,
        init_segment_bytes: u64,
        mime_type: &str,
        codecs: &str,
        encrypted_segments: &[CencFmp4SegmentIdentityV1],
    ) -> Result<Digest32, ContractError> {
        let mut hasher = Sha256::new();
        hasher.update(MEDIA_MANIFEST_ROOT_DOMAIN_V1);
        hasher.update(CENC_FMP4_MEDIA_SUITE_ID_V1.as_bytes());
        hasher.update(
            &encrypted_content
                .canonical_bytes()
                .map_err(|_| ContractError::InvalidField("encrypted_content"))?,
        );
        hasher.update(init_segment_sha256.as_bytes());
        hasher.update(init_segment_bytes.to_be_bytes());
        hasher.update(mime_type.as_bytes());
        hasher.update([0]);
        hasher.update(codecs.as_bytes());
        hasher.update([0]);
        for segment in encrypted_segments {
            hasher.update(
                &segment
                    .canonical_bytes()
                    .map_err(|_| ContractError::InvalidField("encrypted_segments"))?,
            );
        }
        Ok(Digest32::new(hasher.finalize().into()))
    }
}

impl CencFmp4MediaIdentityV1 {
    fn validate(&self) -> Result<(), ContractError> {
        self.encrypted_content
            .canonical_bytes()
            .map_err(|_| ContractError::InvalidField("encrypted_content"))?;
        self.validate_mime_and_codecs()?;
        if self.init_segment_bytes == 0 || self.init_segment_bytes > MAX_ENCRYPTED_CONTENT_BYTES {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
        if self.encrypted_segments.is_empty()
            || self.encrypted_segments.len() > MAX_MEDIA_SEGMENTS_V1
        {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        for segment in &self.encrypted_segments {
            segment.validate()?;
        }
        if self.encrypted_content.ciphertext_bytes()
            != Self::sum_encrypted_bytes(self.encrypted_segments.as_slice())?
        {
            return Err(ContractError::InvalidField("total_encrypted_bytes"));
        }
        if self.media_manifest_root
            != Self::compute_media_manifest_root(
                self.encrypted_content.clone(),
                self.init_segment_sha256,
                self.init_segment_bytes,
                &self.mime_type,
                &self.codecs,
                self.encrypted_segments.as_slice(),
            )?
        {
            return Err(ContractError::InvalidField("media_manifest_root"));
        }
        Ok(())
    }
}

impl CanonicalContract for CencFmp4MediaIdentityV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let mut bytes = canonical_prefix(CENC_FMP4_MEDIA_IDENTITY_DOMAIN_V1);
        push_string(&mut bytes, CENC_FMP4_MEDIA_SUITE_ID_V1)?;
        push_bytes(
            &mut bytes,
            "encrypted_content",
            &self.encrypted_content.canonical_bytes()?,
        )?;
        bytes.extend_from_slice(self.media_manifest_root.as_bytes());
        bytes.extend_from_slice(self.init_segment_sha256.as_bytes());
        bytes.extend_from_slice(&self.init_segment_bytes.to_be_bytes());
        push_string(&mut bytes, &self.mime_type)?;
        push_string(&mut bytes, &self.codecs)?;
        push_u16(
            &mut bytes,
            u16::try_from(self.encrypted_segments.len())
                .map_err(|_| ContractError::FieldTooLong("encrypted_segments"))?,
        );
        for segment in &self.encrypted_segments {
            push_bytes(&mut bytes, "encrypted_segment", &segment.canonical_bytes()?)?;
        }
        Ok(bytes)
    }

    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let mut cursor = canonical_cursor(bytes, CENC_FMP4_MEDIA_IDENTITY_DOMAIN_V1)?;
        let suite = read_string(
            bytes,
            &mut cursor,
            "suite_id",
            MAX_MEDIA_DECLARATION_BYTES_V1,
        )?;
        if suite != CENC_FMP4_MEDIA_SUITE_ID_V1 {
            return Err(ContractError::InvalidField("suite_id"));
        }
        let encrypted_content = EncryptedContentIdentityV1::from_canonical_bytes(&read_bytes(
            bytes,
            &mut cursor,
            "encrypted_content",
            u16::MAX as usize,
        )?)?;
        let media_manifest_root = Digest32::new(read_fixed_32(bytes, &mut cursor)?);
        let init_segment_sha256 = Digest32::new(read_fixed_32(bytes, &mut cursor)?);
        let init_segment_bytes = read_u64(bytes, &mut cursor)?;
        let mime_type = read_string(
            bytes,
            &mut cursor,
            "mime_type",
            MAX_MEDIA_DECLARATION_BYTES_V1,
        )?;
        let codecs = read_string(bytes, &mut cursor, "codecs", MAX_MEDIA_DECLARATION_BYTES_V1)?;
        let segment_count = usize::from(read_u16(bytes, &mut cursor)?);
        if segment_count == 0 || segment_count > MAX_MEDIA_SEGMENTS_V1 {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        let mut encrypted_segments = Vec::with_capacity(segment_count);
        for _ in 0..segment_count {
            encrypted_segments.push(CencFmp4SegmentIdentityV1::from_canonical_bytes(
                &read_bytes(bytes, &mut cursor, "encrypted_segment", u16::MAX as usize)?,
            )?);
        }
        let value = Self::new_recorded(
            encrypted_content,
            media_manifest_root,
            init_segment_sha256,
            init_segment_bytes,
            mime_type,
            codecs,
            encrypted_segments,
        )?;
        finish_canonical(bytes, cursor)?;
        if value.canonical_bytes()?.as_slice() != bytes {
            return Err(ContractError::InvalidField("non-canonical encoding"));
        }
        Ok(value)
    }
}

pub(crate) fn validate_visible_ascii(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), ContractError> {
    if value.is_empty() || value.len() > maximum {
        return Err(ContractError::InvalidField(field));
    }
    if !value
        .as_bytes()
        .iter()
        .all(|byte| matches!(*byte, 0x21..=0x7e))
    {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

fn canonical_prefix(domain: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(domain.len() + 1);
    bytes.extend_from_slice(domain.as_bytes());
    bytes.push(0);
    bytes
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(bytes: &mut Vec<u8>, field: &'static str, value: &[u8]) -> Result<(), ContractError> {
    let length = u16::try_from(value.len()).map_err(|_| ContractError::FieldTooLong(field))?;
    push_u16(bytes, length);
    bytes.extend_from_slice(value);
    Ok(())
}

fn push_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), ContractError> {
    push_bytes(bytes, "canonical string", value.as_bytes())
}

fn canonical_cursor(bytes: &[u8], domain: &str) -> Result<usize, ContractError> {
    let prefix = domain.as_bytes();
    if bytes.len() <= prefix.len() || &bytes[..prefix.len()] != prefix || bytes[prefix.len()] != 0 {
        return Err(ContractError::WrongDomain);
    }
    Ok(prefix.len() + 1)
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], ContractError> {
    let end = cursor
        .checked_add(length)
        .ok_or(ContractError::UnexpectedEnd)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(ContractError::UnexpectedEnd)?;
    *cursor = end;
    Ok(value)
}

fn read_fixed_32(bytes: &[u8], cursor: &mut usize) -> Result<[u8; 32], ContractError> {
    take_bytes(bytes, cursor, 32)?
        .try_into()
        .map_err(|_| ContractError::UnexpectedEnd)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, ContractError> {
    Ok(u16::from_be_bytes(
        take_bytes(bytes, cursor, 2)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, ContractError> {
    Ok(u64::from_be_bytes(
        take_bytes(bytes, cursor, 8)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_bytes(
    bytes: &[u8],
    cursor: &mut usize,
    field: &'static str,
    maximum: usize,
) -> Result<Vec<u8>, ContractError> {
    let length = usize::from(read_u16(bytes, cursor)?);
    if length > maximum {
        return Err(ContractError::FieldTooLong(field));
    }
    Ok(take_bytes(bytes, cursor, length)?.to_vec())
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    field: &'static str,
    maximum: usize,
) -> Result<String, ContractError> {
    String::from_utf8(read_bytes(bytes, cursor, field, maximum)?)
        .map_err(|_| ContractError::InvalidUtf8(field))
}

fn finish_canonical(bytes: &[u8], cursor: usize) -> Result<(), ContractError> {
    if cursor == bytes.len() {
        Ok(())
    } else {
        Err(ContractError::TrailingBytes)
    }
}

fn validate_cenc_fmp4_media_structure_v1(
    init_segment: &[u8],
    encrypted_segments: &[Vec<u8>],
) -> Result<ValidatedCencFmp4MediaLayoutV1, ContractError> {
    let init = validate_staged_init_segment_v1(
        init_segment,
        Digest32::new(Sha256::digest(init_segment).into()),
        init_segment.len() as u64,
    )?;
    let expected_segments = encrypted_segments
        .iter()
        .map(|segment| CencFmp4SegmentIdentityV1::new_from_encrypted_bytes(segment))
        .collect::<Result<Vec<_>, _>>()?;
    let mut segments = Vec::with_capacity(encrypted_segments.len());
    for (expected_segment, segment) in expected_segments.iter().zip(encrypted_segments) {
        segments.push(validate_staged_segment_v1(
            expected_segment,
            segment,
            init.protected_track_ids.as_slice(),
        )?);
    }
    Ok(ValidatedCencFmp4MediaLayoutV1 {
        init_segment_sha256: Digest32::new(Sha256::digest(init_segment).into()),
        init_segment_bytes: init_segment.len() as u64,
        protected_track_ids: init.protected_track_ids,
        track_rewrites: init.track_rewrites,
        segments,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedCencFmp4InitRewriteV1 {
    protected_track_ids: Vec<u32>,
    track_rewrites: Vec<ValidatedCencFmp4TrackRewriteV1>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedClearFmp4InitLayoutV1 {
    track_ids: Vec<u32>,
    tracks: Vec<ValidatedClearFmp4TrackLayoutV1>,
}

fn validate_staged_init_segment_v1(
    protected_init_segment: &[u8],
    expected_sha256: Digest32,
    expected_len: u64,
) -> Result<ValidatedCencFmp4InitRewriteV1, ContractError> {
    verify_exact_source_bytes(
        protected_init_segment,
        expected_sha256,
        expected_len,
        "init_segment_bytes",
    )?;
    validate_init_segment_v1(protected_init_segment)
}

fn validate_staged_segment_v1(
    expected_segment: &CencFmp4SegmentIdentityV1,
    encrypted_segment: &[u8],
    protected_track_ids: &[u32],
) -> Result<ValidatedCencFmp4SegmentLayoutV1, ContractError> {
    verify_exact_source_bytes(
        encrypted_segment,
        expected_segment.ciphertext_sha256(),
        expected_segment.ciphertext_bytes(),
        "encrypted_segments",
    )?;
    validate_media_segment_v1(encrypted_segment, protected_track_ids)
}

fn validate_clear_init_segment_v1(
    clear_init_segment: &[u8],
) -> Result<ValidatedClearFmp4InitLayoutV1, ContractError> {
    if clear_init_segment.is_empty()
        || (clear_init_segment.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES
    {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let top = scan_boxes(clear_init_segment, 0, clear_init_segment.len())?;
    if top.len() != 2 || top[0].1.box_type != *b"ftyp" || top[1].1.box_type != *b"moov" {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let (moov_off, moov_h) = top[1];
    let mut track_ids = Vec::new();
    let mut tracks = Vec::new();
    let mut mvex = None;
    for (off, h) in scan_boxes(
        clear_init_segment,
        moov_off + moov_h.header_size,
        moov_off + moov_h.size,
    )? {
        match &h.box_type {
            b"trak" => {
                let mut track = validate_clear_trak_v1(clear_init_segment, off, h)?;
                if track.track_id == 0 || track_ids.contains(&track.track_id) {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                track_ids.push(track.track_id);
                track
                    .resize_targets
                    .push(BoxResizeTargetV1 { box_off: moov_off });
                tracks.push(track);
            }
            b"mvex" => {
                if mvex.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"mvhd" | b"udta" | b"iods" | b"meta" => {}
            _ => return Err(ContractError::InvalidField("init_segment_bytes")),
        }
    }
    let (mvex_off, mvex_h) = mvex.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if track_ids.is_empty() || track_ids.len() > MAX_MEDIA_TRACKS_V1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    validate_mvex_v1(
        clear_init_segment,
        mvex_off + mvex_h.header_size,
        mvex_off + mvex_h.size,
        track_ids.as_slice(),
    )?;
    track_ids.sort_unstable();
    tracks.sort_by_key(|track| track.track_id);
    Ok(ValidatedClearFmp4InitLayoutV1 { track_ids, tracks })
}

fn rewrite_clear_init_v1(
    protected_init_segment: &[u8],
    expected_sha256: Digest32,
    expected_len: u64,
    track_rewrites: &[ValidatedCencFmp4TrackRewriteV1],
) -> Result<Vec<u8>, ContractError> {
    verify_exact_source_bytes(
        protected_init_segment,
        expected_sha256,
        expected_len,
        "init_segment_bytes",
    )?;
    rewrite_clear_init_unchecked_v1(protected_init_segment, track_rewrites)
}

fn rewrite_protected_init_v1(
    clear_init_segment: &[u8],
    clear_tracks: &[ValidatedClearFmp4TrackLayoutV1],
    key_id: [u8; 16],
) -> Result<Vec<u8>, ContractError> {
    if clear_init_segment.is_empty() {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let mut rewritten = clear_init_segment.to_vec();
    let mut ordered_tracks = clear_tracks.to_vec();
    ordered_tracks.sort_by_key(|track| std::cmp::Reverse(track.sample_entry_off));
    for track in ordered_tracks {
        let protected_fourcc = protected_sample_entry_fourcc_v1(track.original_fourcc)?;
        let sinf = make_protected_sinf_v1(track.original_fourcc, key_id)?;
        let sample_entry_size = read_box_header(&rewritten, track.sample_entry_off)?.size;
        grow_box_size_in_place(
            &mut rewritten,
            track.sample_entry_off,
            sinf.len(),
            "init_segment_bytes",
        )?;
        for target in &track.resize_targets {
            grow_box_size_in_place(
                &mut rewritten,
                target.box_off,
                sinf.len(),
                "init_segment_bytes",
            )?;
        }
        let sample_entry_type = track
            .sample_entry_off
            .checked_add(4)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        let sample_entry_type_end = sample_entry_type
            .checked_add(4)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        rewritten
            .get_mut(sample_entry_type..sample_entry_type_end)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?
            .copy_from_slice(&protected_fourcc);
        let insert_at = track
            .sample_entry_off
            .checked_add(sample_entry_size)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        rewritten.splice(insert_at..insert_at, sinf);
    }
    Ok(rewritten)
}

fn rewrite_clear_init_unchecked_v1(
    protected_init_segment: &[u8],
    track_rewrites: &[ValidatedCencFmp4TrackRewriteV1],
) -> Result<Vec<u8>, ContractError> {
    let mut rewritten = protected_init_segment.to_vec();
    let mut ordered_track_rewrites = track_rewrites.to_vec();
    ordered_track_rewrites.sort_by_key(|rewrite| std::cmp::Reverse(rewrite.sinf_off));
    for rewrite in ordered_track_rewrites {
        let sample_entry_type = rewrite
            .sample_entry_off
            .checked_add(4)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        let sample_entry_type_end = sample_entry_type
            .checked_add(4)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        rewritten
            .get_mut(sample_entry_type..sample_entry_type_end)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?
            .copy_from_slice(&rewrite.original_fourcc);
        shrink_box_size_in_place(
            &mut rewritten,
            rewrite.sample_entry_off,
            rewrite.sinf_size,
            "init_segment_bytes",
        )?;
        for target in &rewrite.resize_targets {
            shrink_box_size_in_place(
                &mut rewritten,
                target.box_off,
                rewrite.sinf_size,
                "init_segment_bytes",
            )?;
        }
        let sinf_end = rewrite
            .sinf_off
            .checked_add(rewrite.sinf_size)
            .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
        if sinf_end > rewritten.len() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
        rewritten.drain(rewrite.sinf_off..sinf_end);
    }
    Ok(rewritten)
}

fn validate_init_segment_v1(bytes: &[u8]) -> Result<ValidatedCencFmp4InitRewriteV1, ContractError> {
    if bytes.is_empty() || (bytes.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let top = scan_boxes(bytes, 0, bytes.len())?;
    if top.len() != 2 || top[0].1.box_type != *b"ftyp" || top[1].1.box_type != *b"moov" {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let (moov_off, moov_h) = top[1];
    let mut protected_track_ids = Vec::new();
    let mut track_rewrites = Vec::new();
    let mut mvex = None;
    for (off, h) in scan_boxes(bytes, moov_off + moov_h.header_size, moov_off + moov_h.size)? {
        match &h.box_type {
            b"trak" => {
                let mut rewrite = validate_trak_v1(bytes, off, h)?;
                if rewrite.track_id == 0 || protected_track_ids.contains(&rewrite.track_id) {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                protected_track_ids.push(rewrite.track_id);
                rewrite
                    .resize_targets
                    .push(BoxResizeTargetV1 { box_off: moov_off });
                track_rewrites.push(rewrite);
            }
            b"mvex" => {
                if mvex.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"mvhd" | b"udta" | b"iods" | b"meta" => {}
            _ => return Err(ContractError::InvalidField("init_segment_bytes")),
        }
    }
    let (mvex_off, mvex_h) = mvex.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if protected_track_ids.is_empty() || protected_track_ids.len() > MAX_MEDIA_TRACKS_V1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    validate_mvex_v1(
        bytes,
        mvex_off + mvex_h.header_size,
        mvex_off + mvex_h.size,
        protected_track_ids.as_slice(),
    )?;
    protected_track_ids.sort_unstable();
    track_rewrites.sort_by_key(|rewrite| rewrite.track_id);
    Ok(ValidatedCencFmp4InitRewriteV1 {
        protected_track_ids,
        track_rewrites,
    })
}

fn validate_mvex_v1(
    data: &[u8],
    start: usize,
    end: usize,
    protected_track_ids: &[u32],
) -> Result<(), ContractError> {
    let mut trex_track_ids = Vec::new();
    for (off, h) in scan_boxes(data, start, end)? {
        match &h.box_type {
            b"trex" => {
                let content = off + h.header_size;
                let (version, flags) = read_fullbox_header(data, content)?;
                if version != 0 || flags != 0 || h.size != h.header_size + 24 {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                let track_id = read_u32_at(data, content + 4)?;
                if track_id == 0 || trex_track_ids.contains(&track_id) {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                trex_track_ids.push(track_id);
            }
            b"mehd" => {}
            _ => return Err(ContractError::InvalidField("init_segment_bytes")),
        }
    }
    trex_track_ids.sort_unstable();
    if trex_track_ids != protected_track_ids {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    Ok(())
}

fn validate_trak_v1(
    data: &[u8],
    trak_off: usize,
    trak_h: BoxHeader,
) -> Result<ValidatedCencFmp4TrackRewriteV1, ContractError> {
    let mut tkhd = None;
    let mut mdia = None;
    for (off, h) in scan_boxes(data, trak_off + trak_h.header_size, trak_off + trak_h.size)? {
        match &h.box_type {
            b"tkhd" => {
                if tkhd.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"mdia" => {
                if mdia.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            _ => {}
        }
    }
    let (tkhd_off, tkhd_h) = tkhd.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (mdia_off, mdia_h) = mdia.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let track_id = parse_tkhd_track_id(data, tkhd_off, tkhd_h)?;
    let mut rewrite = validate_mdia_v1(data, mdia_off, mdia_h)?;
    rewrite.track_id = track_id;
    rewrite
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: trak_off });
    Ok(rewrite)
}

fn validate_clear_trak_v1(
    data: &[u8],
    trak_off: usize,
    trak_h: BoxHeader,
) -> Result<ValidatedClearFmp4TrackLayoutV1, ContractError> {
    let mut tkhd = None;
    let mut mdia = None;
    for (off, h) in scan_boxes(data, trak_off + trak_h.header_size, trak_off + trak_h.size)? {
        match &h.box_type {
            b"tkhd" => {
                if tkhd.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"mdia" => {
                if mdia.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            _ => {}
        }
    }
    let (tkhd_off, tkhd_h) = tkhd.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (mdia_off, mdia_h) = mdia.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let track_id = parse_tkhd_track_id(data, tkhd_off, tkhd_h)?;
    let mut track = validate_clear_mdia_v1(data, mdia_off, mdia_h)?;
    track.track_id = track_id;
    track
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: trak_off });
    Ok(track)
}

fn validate_mdia_v1(
    data: &[u8],
    mdia_off: usize,
    mdia_h: BoxHeader,
) -> Result<ValidatedCencFmp4TrackRewriteV1, ContractError> {
    let mut handler_type = None;
    let mut minf = None;
    for (off, h) in scan_boxes(data, mdia_off + mdia_h.header_size, mdia_off + mdia_h.size)? {
        match &h.box_type {
            b"hdlr" => {
                if handler_type.is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                handler_type = Some(parse_hdlr_type(data, off, h)?);
            }
            b"minf" => {
                if minf.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            _ => {}
        }
    }
    let handler_type = handler_type.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (minf_off, minf_h) = minf.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut rewrite = validate_minf_v1(data, minf_off, minf_h, handler_type)?;
    rewrite
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: mdia_off });
    Ok(rewrite)
}

fn validate_clear_mdia_v1(
    data: &[u8],
    mdia_off: usize,
    mdia_h: BoxHeader,
) -> Result<ValidatedClearFmp4TrackLayoutV1, ContractError> {
    let mut handler_type = None;
    let mut minf = None;
    for (off, h) in scan_boxes(data, mdia_off + mdia_h.header_size, mdia_off + mdia_h.size)? {
        match &h.box_type {
            b"hdlr" => {
                if handler_type.is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
                handler_type = Some(parse_hdlr_type(data, off, h)?);
            }
            b"minf" => {
                if minf.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            _ => {}
        }
    }
    let handler_type = handler_type.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (minf_off, minf_h) = minf.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut track = validate_clear_minf_v1(data, minf_off, minf_h, handler_type)?;
    track
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: mdia_off });
    Ok(track)
}

fn validate_minf_v1(
    data: &[u8],
    minf_off: usize,
    minf_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedCencFmp4TrackRewriteV1, ContractError> {
    let mut stbl = None;
    for (off, h) in scan_boxes(data, minf_off + minf_h.header_size, minf_off + minf_h.size)? {
        if &h.box_type == b"stbl" && stbl.replace((off, h)).is_some() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
    }
    let (stbl_off, stbl_h) = stbl.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut rewrite = validate_stbl_v1(data, stbl_off, stbl_h, handler_type)?;
    rewrite
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: minf_off });
    Ok(rewrite)
}

fn validate_clear_minf_v1(
    data: &[u8],
    minf_off: usize,
    minf_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedClearFmp4TrackLayoutV1, ContractError> {
    let mut stbl = None;
    for (off, h) in scan_boxes(data, minf_off + minf_h.header_size, minf_off + minf_h.size)? {
        if &h.box_type == b"stbl" && stbl.replace((off, h)).is_some() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
    }
    let (stbl_off, stbl_h) = stbl.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut track = validate_clear_stbl_v1(data, stbl_off, stbl_h, handler_type)?;
    track
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: minf_off });
    Ok(track)
}

fn validate_stbl_v1(
    data: &[u8],
    stbl_off: usize,
    stbl_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedCencFmp4TrackRewriteV1, ContractError> {
    let mut stsd = None;
    for (off, h) in scan_boxes(data, stbl_off + stbl_h.header_size, stbl_off + stbl_h.size)? {
        if &h.box_type == b"stsd" && stsd.replace((off, h)).is_some() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
    }
    let (stsd_off, stsd_h) = stsd.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut rewrite = validate_stsd_v1(data, stsd_off, stsd_h, handler_type)?;
    rewrite
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: stbl_off });
    Ok(rewrite)
}

fn validate_clear_stbl_v1(
    data: &[u8],
    stbl_off: usize,
    stbl_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedClearFmp4TrackLayoutV1, ContractError> {
    let mut stsd = None;
    for (off, h) in scan_boxes(data, stbl_off + stbl_h.header_size, stbl_off + stbl_h.size)? {
        if &h.box_type == b"stsd" && stsd.replace((off, h)).is_some() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
    }
    let (stsd_off, stsd_h) = stsd.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let mut track = validate_clear_stsd_v1(data, stsd_off, stsd_h, handler_type)?;
    track
        .resize_targets
        .push(BoxResizeTargetV1 { box_off: stbl_off });
    Ok(track)
}

fn validate_stsd_v1(
    data: &[u8],
    stsd_off: usize,
    stsd_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedCencFmp4TrackRewriteV1, ContractError> {
    let content = stsd_off + stsd_h.header_size;
    if stsd_h.size < stsd_h.header_size + 16 || read_u32_at(data, content + 4)? != 1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let entry_off = content + 8;
    let entry_h = read_box_header(data, entry_off)?;
    if entry_off + entry_h.size != stsd_off + stsd_h.size {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let (fixed, expected_original_fourcc) = match (&handler_type, &entry_h.box_type) {
        (b"vide", b"encv") => (VISUAL_SAMPLE_ENTRY_FIXED_BYTES, *b"avc1"),
        (b"soun", b"enca") => (AUDIO_SAMPLE_ENTRY_FIXED_BYTES, *b"mp4a"),
        _ => return Err(ContractError::InvalidField("init_segment_bytes")),
    };
    let entry_content_start = entry_off + entry_h.header_size;
    let child_start = entry_content_start
        .checked_add(fixed)
        .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if child_start > entry_off + entry_h.size {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let mut sinf = None;
    for (off, h) in scan_boxes(data, child_start, entry_off + entry_h.size)? {
        if &h.box_type == b"sinf" && sinf.replace((off, h)).is_some() {
            return Err(ContractError::InvalidField("init_segment_bytes"));
        }
    }
    let (sinf_off, sinf_h) = sinf.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    validate_sinf_v1(data, sinf_off, sinf_h, expected_original_fourcc)?;
    Ok(ValidatedCencFmp4TrackRewriteV1 {
        track_id: 0,
        sample_entry_off: entry_off,
        original_fourcc: expected_original_fourcc,
        sinf_off,
        sinf_size: sinf_h.size,
        resize_targets: vec![BoxResizeTargetV1 { box_off: stsd_off }],
    })
}

fn validate_sinf_v1(
    data: &[u8],
    sinf_off: usize,
    sinf_h: BoxHeader,
    expected_original_fourcc: [u8; 4],
) -> Result<(), ContractError> {
    let mut frma = None;
    let mut schm = None;
    let mut schi = None;
    for (off, h) in scan_boxes(data, sinf_off + sinf_h.header_size, sinf_off + sinf_h.size)? {
        match &h.box_type {
            b"frma" => {
                if frma.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"schm" => {
                if schm.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            b"schi" => {
                if schi.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("init_segment_bytes"));
                }
            }
            _ => return Err(ContractError::InvalidField("init_segment_bytes")),
        }
    }
    let (frma_off, frma_h) = frma.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (schm_off, schm_h) = schm.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    let (schi_off, schi_h) = schi.ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if frma_h.size != 12
        || slice_at(data, frma_off + frma_h.header_size, 4)? != expected_original_fourcc.as_slice()
    {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let schm_content = schm_off + schm_h.header_size;
    let (schm_version, schm_flags) = read_fullbox_header(data, schm_content)?;
    if schm_h.size != schm_h.header_size + 12
        || schm_version != 0
        || schm_flags != 0
        || &data[schm_content + 4..schm_content + 8] != b"cenc"
    {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let schi_boxes = scan_boxes(data, schi_off + schi_h.header_size, schi_off + schi_h.size)?;
    if schi_boxes.len() != 1 || schi_boxes[0].1.box_type != *b"tenc" {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let (tenc_off, tenc_h) = schi_boxes[0];
    let content = tenc_off + tenc_h.header_size;
    let (tenc_version, tenc_flags) = read_fullbox_header(data, content)?;
    if tenc_h.size != tenc_h.header_size + 24 || tenc_version != 0 || tenc_flags != 0 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    if data[content + 6] != 1 || data[content + 7] != SUPPORTED_COMMON_IV_SIZE_V1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    Ok(())
}

fn validate_clear_stsd_v1(
    data: &[u8],
    stsd_off: usize,
    stsd_h: BoxHeader,
    handler_type: [u8; 4],
) -> Result<ValidatedClearFmp4TrackLayoutV1, ContractError> {
    let content = stsd_off + stsd_h.header_size;
    if stsd_h.size < stsd_h.header_size + 16 || read_u32_at(data, content + 4)? != 1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let entry_off = content + 8;
    let entry_h = read_box_header(data, entry_off)?;
    if entry_off + entry_h.size != stsd_off + stsd_h.size {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let (fixed, original_fourcc) = match (&handler_type, &entry_h.box_type) {
        (b"vide", b"avc1") => (VISUAL_SAMPLE_ENTRY_FIXED_BYTES, *b"avc1"),
        (b"soun", b"mp4a") => (AUDIO_SAMPLE_ENTRY_FIXED_BYTES, *b"mp4a"),
        _ => return Err(ContractError::InvalidField("init_segment_bytes")),
    };
    let entry_content_start = entry_off + entry_h.header_size;
    let child_start = entry_content_start
        .checked_add(fixed)
        .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if child_start > entry_off + entry_h.size {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    if !scan_boxes(data, child_start, entry_off + entry_h.size)?.is_empty() {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    Ok(ValidatedClearFmp4TrackLayoutV1 {
        track_id: 0,
        sample_entry_off: entry_off,
        original_fourcc,
        resize_targets: vec![BoxResizeTargetV1 { box_off: stsd_off }],
    })
}

fn validate_media_segment_v1(
    segment: &[u8],
    protected_track_ids: &[u32],
) -> Result<ValidatedCencFmp4SegmentLayoutV1, ContractError> {
    if segment.is_empty() || (segment.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let top = scan_boxes(segment, 0, segment.len())?;
    if top.len() != 2 || top[0].1.box_type != *b"moof" || top[1].1.box_type != *b"mdat" {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let (moof_off, moof_h) = top[0];
    let (mdat_off, mdat_h) = top[1];
    if moof_off != 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if moof_off + moof_h.size != mdat_off || mdat_off + mdat_h.size != segment.len() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let moof_children = scan_boxes(
        segment,
        moof_off + moof_h.header_size,
        moof_off + moof_h.size,
    )?;
    let mfhds: Vec<_> = moof_children
        .iter()
        .copied()
        .filter(|(_, h)| h.box_type == *b"mfhd")
        .collect();
    let trafs: Vec<_> = moof_children
        .iter()
        .copied()
        .filter(|(_, h)| h.box_type == *b"traf")
        .collect();
    if mfhds.len() != 1 || trafs.len() != 1 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if moof_children
        .iter()
        .any(|(_, h)| h.box_type != *b"mfhd" && h.box_type != *b"traf")
    {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    validate_mfhd_v1(segment, mfhds[0].0, mfhds[0].1)?;
    let mdat_content_start = mdat_off + mdat_h.header_size;
    let mdat_content_end = mdat_off + mdat_h.size;
    let mut layout = validate_traf_v1(
        segment,
        trafs[0].0,
        trafs[0].1,
        moof_off,
        mdat_content_start,
        mdat_content_end,
    )?;
    if protected_track_ids.binary_search(&layout.track_id).is_err() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let total = layout.samples.iter().try_fold(0usize, |acc, sample| {
        let start = usize::try_from(sample.ciphertext_offset)
            .map_err(|_| ContractError::InvalidField("encrypted_segments"))?;
        let end = start
            .checked_add(sample.sample_size as usize)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        if start != acc {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        Ok(end)
    })?;
    if total != mdat_content_end - mdat_content_start {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    layout.source_sha256 = Digest32::new(Sha256::digest(segment).into());
    layout.source_bytes = segment.len() as u64;
    Ok(layout)
}

fn validate_clear_media_segment_v1(
    segment: &[u8],
    clear_track_ids: &[u32],
) -> Result<ValidatedClearFmp4SegmentLayoutV1, ContractError> {
    if segment.is_empty() || (segment.len() as u64) > MAX_ENCRYPTED_CONTENT_BYTES {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let top = scan_boxes(segment, 0, segment.len())?;
    if top.len() != 2 || top[0].1.box_type != *b"moof" || top[1].1.box_type != *b"mdat" {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let (moof_off, moof_h) = top[0];
    let (mdat_off, mdat_h) = top[1];
    if moof_off != 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if moof_off + moof_h.size != mdat_off || mdat_off + mdat_h.size != segment.len() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let moof_children = scan_boxes(
        segment,
        moof_off + moof_h.header_size,
        moof_off + moof_h.size,
    )?;
    let mfhds: Vec<_> = moof_children
        .iter()
        .copied()
        .filter(|(_, h)| h.box_type == *b"mfhd")
        .collect();
    let trafs: Vec<_> = moof_children
        .iter()
        .copied()
        .filter(|(_, h)| h.box_type == *b"traf")
        .collect();
    if mfhds.len() != 1 || trafs.len() != 1 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if moof_children
        .iter()
        .any(|(_, h)| h.box_type != *b"mfhd" && h.box_type != *b"traf")
    {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    validate_mfhd_v1(segment, mfhds[0].0, mfhds[0].1)?;
    let mut layout = validate_clear_traf_v1(
        segment,
        trafs[0].0,
        trafs[0].1,
        mdat_off + mdat_h.header_size,
        mdat_off + mdat_h.size,
    )?;
    if clear_track_ids.binary_search(&layout.track_id).is_err() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    layout.source_sha256 = Digest32::new(Sha256::digest(segment).into());
    layout.source_bytes = segment.len() as u64;
    Ok(layout)
}

fn validate_clear_traf_v1(
    data: &[u8],
    traf_off: usize,
    traf_h: BoxHeader,
    mdat_content_start: usize,
    mdat_content_end: usize,
) -> Result<ValidatedClearFmp4SegmentLayoutV1, ContractError> {
    let mut tfhd = None;
    let mut trun = None;
    let mut saw_tfdt = false;
    for (off, h) in scan_boxes(data, traf_off + traf_h.header_size, traf_off + traf_h.size)? {
        match &h.box_type {
            b"tfhd" => {
                if tfhd.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
            }
            b"trun" => {
                if trun.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
            }
            b"tfdt" => {
                if saw_tfdt {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
                saw_tfdt = true;
                validate_tfdt_v1(data, off, h)?;
            }
            _ => return Err(ContractError::InvalidField("encrypted_segments")),
        }
    }
    let (tfhd_off, tfhd_h) = tfhd.ok_or(ContractError::InvalidField("encrypted_segments"))?;
    let (trun_off, trun_h) = trun.ok_or(ContractError::InvalidField("encrypted_segments"))?;
    let track_id = parse_tfhd_track_id(data, tfhd_off, tfhd_h)?;
    let trun = parse_trun_layout(data, trun_off, trun_h)?;
    let sample_start = usize::try_from(trun.data_offset)
        .map_err(|_| ContractError::InvalidField("encrypted_segments"))?;
    if sample_start < mdat_content_start || sample_start > mdat_content_end {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let mut cursor = sample_start;
    let mut samples = Vec::with_capacity(trun.sample_sizes.len());
    for size in trun.sample_sizes {
        let end = cursor
            .checked_add(size as usize)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        if end > mdat_content_end {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        samples.push(ValidatedClearFmp4SampleLayoutV1 {
            mdat_offset: (cursor - mdat_content_start) as u64,
            sample_size: size,
        });
        cursor = end;
    }
    if cursor != mdat_content_end {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(ValidatedClearFmp4SegmentLayoutV1 {
        source_sha256: Digest32::new([0u8; 32]),
        source_bytes: 0,
        track_id,
        samples,
        rewrite: ValidatedClearFmp4SegmentRewriteV1 {
            moof_off: 0,
            traf_off,
            trun_off,
            trun_data_offset: trun.data_offset,
            mdat_content_start,
            mdat_content_end,
            senc_insert_off: traf_off + traf_h.size,
        },
    })
}

fn validate_traf_v1(
    data: &[u8],
    traf_off: usize,
    traf_h: BoxHeader,
    moof_off: usize,
    mdat_content_start: usize,
    mdat_content_end: usize,
) -> Result<ValidatedCencFmp4SegmentLayoutV1, ContractError> {
    let mut tfhd = None;
    let mut trun = None;
    let mut senc = None;
    let mut saw_tfdt = false;
    for (off, h) in scan_boxes(data, traf_off + traf_h.header_size, traf_off + traf_h.size)? {
        match &h.box_type {
            b"tfhd" => {
                if tfhd.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
            }
            b"trun" => {
                if trun.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
            }
            b"senc" => {
                if senc.replace((off, h)).is_some() {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
            }
            b"tfdt" => {
                if saw_tfdt {
                    return Err(ContractError::InvalidField("encrypted_segments"));
                }
                saw_tfdt = true;
                validate_tfdt_v1(data, off, h)?;
            }
            _ => return Err(ContractError::InvalidField("encrypted_segments")),
        }
    }
    let (tfhd_off, tfhd_h) = tfhd.ok_or(ContractError::InvalidField("encrypted_segments"))?;
    let (trun_off, trun_h) = trun.ok_or(ContractError::InvalidField("encrypted_segments"))?;
    let (senc_off, senc_h) = senc.ok_or(ContractError::InvalidField("encrypted_segments"))?;
    let track_id = parse_tfhd_track_id(data, tfhd_off, tfhd_h)?;
    let trun = parse_trun_layout(data, trun_off, trun_h)?;
    let senc_samples = parse_senc_layout(data, senc_off, senc_h, trun.sample_sizes.as_slice())?;
    let sample_start = usize::try_from(trun.data_offset)
        .map_err(|_| ContractError::InvalidField("encrypted_segments"))?;
    if sample_start < mdat_content_start || sample_start > mdat_content_end {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let mut cursor = sample_start;
    let mut samples = Vec::with_capacity(trun.sample_sizes.len());
    for (size, senc_sample) in trun.sample_sizes.into_iter().zip(senc_samples) {
        let end = cursor
            .checked_add(size as usize)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        if end > mdat_content_end {
            return Err(ContractError::InvalidField("encrypted_segments"));
        }
        samples.push(ValidatedCencFmp4SampleLayoutV1 {
            ciphertext_offset: (cursor - mdat_content_start) as u64,
            sample_size: size,
            iv: senc_sample.iv,
            subsamples: senc_sample.subsamples,
        });
        cursor = end;
    }
    Ok(ValidatedCencFmp4SegmentLayoutV1 {
        source_sha256: Digest32::new([0u8; 32]),
        source_bytes: 0,
        track_id,
        samples,
        rewrite: ValidatedCencFmp4SegmentRewriteV1 {
            moof_off,
            traf_off,
            trun_off,
            senc_off,
            senc_size: senc_h.size,
            trun_data_offset: trun.data_offset,
            mdat_content_start,
            mdat_content_end,
        },
    })
}

#[derive(Debug)]
struct TrunLayoutV1 {
    data_offset: i32,
    sample_sizes: Vec<u32>,
}

fn parse_trun_layout(data: &[u8], off: usize, h: BoxHeader) -> Result<TrunLayoutV1, ContractError> {
    let content = off + h.header_size;
    if h.size < h.header_size + 12 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let (version, flags) = read_fullbox_header(data, content)?;
    let allowed_flags = TRUN_FLAG_DATA_OFFSET
        | TRUN_FLAG_FIRST_SAMPLE_FLAGS
        | TRUN_FLAG_SAMPLE_DURATION
        | TRUN_FLAG_SAMPLE_SIZE
        | TRUN_FLAG_SAMPLE_FLAGS
        | TRUN_FLAG_SAMPLE_COMPOSITION_TIME_OFFSET;
    if version != 0 || flags & !allowed_flags != 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if flags & TRUN_FLAG_DATA_OFFSET == 0 || flags & TRUN_FLAG_SAMPLE_SIZE == 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let sample_count = read_u32_at(data, content + 4)? as usize;
    if sample_count == 0 || sample_count > MAX_TRACK_SAMPLES_V1 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let data_offset = read_i32_at(data, content + 8)?;
    let mut cursor = content + 12;
    if flags & TRUN_FLAG_FIRST_SAMPLE_FLAGS != 0 {
        cursor = cursor
            .checked_add(4)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    }
    let mut sample_sizes = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        if flags & TRUN_FLAG_SAMPLE_DURATION != 0 {
            cursor = cursor
                .checked_add(4)
                .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        }
        sample_sizes.push(read_u32_at(data, cursor)?);
        cursor = cursor
            .checked_add(4)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        if flags & TRUN_FLAG_SAMPLE_FLAGS != 0 {
            cursor = cursor
                .checked_add(4)
                .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        }
        if flags & TRUN_FLAG_SAMPLE_COMPOSITION_TIME_OFFSET != 0 {
            cursor = cursor
                .checked_add(4)
                .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        }
    }
    if cursor != off + h.size || sample_sizes.contains(&0) {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(TrunLayoutV1 {
        data_offset,
        sample_sizes,
    })
}

struct ParsedSencSampleV1 {
    iv: [u8; 8],
    subsamples: Vec<ValidatedCencFmp4SubsampleV1>,
}

fn parse_senc_layout(
    data: &[u8],
    off: usize,
    h: BoxHeader,
    sample_sizes: &[u32],
) -> Result<Vec<ParsedSencSampleV1>, ContractError> {
    let content = off + h.header_size;
    if h.size < h.header_size + 8 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let (version, flags) = read_fullbox_header(data, content)?;
    if version != 0 || !(flags == 0 || flags == SENC_FLAG_SUBSAMPLES) {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let sample_count = read_u32_at(data, content + 4)? as usize;
    if sample_count != sample_sizes.len() {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let mut cursor = content + 8;
    let mut samples = Vec::with_capacity(sample_sizes.len());
    for size in sample_sizes {
        let iv: [u8; 8] = slice_at(data, cursor, SUPPORTED_COMMON_IV_SIZE_V1 as usize)?
            .try_into()
            .map_err(|_| ContractError::InvalidField("encrypted_segments"))?;
        cursor = cursor
            .checked_add(SUPPORTED_COMMON_IV_SIZE_V1 as usize)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
        let mut subsamples = Vec::new();
        if flags & SENC_FLAG_SUBSAMPLES != 0 {
            let subsample_count = usize::from(read_u16_at(data, cursor)?);
            if subsample_count == 0 {
                return Err(ContractError::InvalidField("encrypted_segments"));
            }
            cursor = cursor
                .checked_add(2)
                .ok_or(ContractError::InvalidField("encrypted_segments"))?;
            let mut total = 0u64;
            for _ in 0..subsample_count {
                let clear = u64::from(read_u16_at(data, cursor)?);
                let encrypted = u64::from(read_u32_at(data, cursor + 2)?);
                cursor = cursor
                    .checked_add(6)
                    .ok_or(ContractError::InvalidField("encrypted_segments"))?;
                subsamples.push(ValidatedCencFmp4SubsampleV1 {
                    clear_bytes: clear as u16,
                    encrypted_bytes: encrypted as u32,
                });
                total = total
                    .checked_add(clear)
                    .and_then(|value| value.checked_add(encrypted))
                    .ok_or(ContractError::InvalidField("encrypted_segments"))?;
            }
            if total != u64::from(*size) {
                return Err(ContractError::InvalidField("encrypted_segments"));
            }
        }
        samples.push(ParsedSencSampleV1 { iv, subsamples });
    }
    if cursor != off + h.size {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(samples)
}

fn parse_tfhd_track_id(data: &[u8], off: usize, h: BoxHeader) -> Result<u32, ContractError> {
    let content = off + h.header_size;
    if h.size < h.header_size + 8 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let (version, flags) = read_fullbox_header(data, content)?;
    let allowed_flags = TFHD_FLAG_DEFAULT_BASE_IS_MOOF
        | TFHD_FLAG_DEFAULT_SAMPLE_DURATION
        | TFHD_FLAG_DEFAULT_SAMPLE_SIZE
        | TFHD_FLAG_DEFAULT_SAMPLE_FLAGS;
    if version != 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    if flags & TFHD_FLAG_BASE_DATA_OFFSET != 0
        || flags & TFHD_FLAG_DEFAULT_BASE_IS_MOOF == 0
        || flags & !allowed_flags != 0
    {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let track_id = read_u32_at(data, content + 4)?;
    if track_id == 0 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    let mut cursor = content + 8;
    if flags & TFHD_FLAG_DEFAULT_SAMPLE_DURATION != 0 {
        cursor = cursor
            .checked_add(4)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    }
    if flags & TFHD_FLAG_DEFAULT_SAMPLE_SIZE != 0 {
        cursor = cursor
            .checked_add(4)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    }
    if flags & TFHD_FLAG_DEFAULT_SAMPLE_FLAGS != 0 {
        cursor = cursor
            .checked_add(4)
            .ok_or(ContractError::InvalidField("encrypted_segments"))?;
    }
    if cursor != off + h.size {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(track_id)
}

fn validate_mfhd_v1(data: &[u8], off: usize, h: BoxHeader) -> Result<(), ContractError> {
    let content = off + h.header_size;
    let (version, flags) = read_fullbox_header(data, content)?;
    if version != 0 || flags != 0 || h.size != h.header_size + 8 {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(())
}

fn validate_tfdt_v1(data: &[u8], off: usize, h: BoxHeader) -> Result<(), ContractError> {
    let content = off + h.header_size;
    let (version, flags) = read_fullbox_header(data, content)?;
    let exact_size = match version {
        0 => h.header_size + 8,
        1 => h.header_size + 12,
        _ => return Err(ContractError::InvalidField("encrypted_segments")),
    };
    if flags != 0 || h.size != exact_size {
        return Err(ContractError::InvalidField("encrypted_segments"));
    }
    Ok(())
}

fn verify_exact_source_bytes(
    bytes: &[u8],
    expected_sha256: Digest32,
    expected_len: u64,
    field: &'static str,
) -> Result<(), ContractError> {
    if bytes.len() as u64 != expected_len {
        return Err(ContractError::InvalidField(field));
    }
    if Digest32::new(Sha256::digest(bytes).into()) != expected_sha256 {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

fn shrink_box_size_in_place(
    bytes: &mut [u8],
    box_off: usize,
    delta: usize,
    field: &'static str,
) -> Result<(), ContractError> {
    let header = read_box_header(bytes, box_off)?;
    let new_size = header
        .size
        .checked_sub(delta)
        .ok_or(ContractError::InvalidField(field))?;
    if new_size < header.header_size {
        return Err(ContractError::InvalidField(field));
    }
    match header.header_size {
        8 => {
            let size32 = u32::try_from(new_size).map_err(|_| ContractError::InvalidField(field))?;
            bytes[box_off..box_off + 4].copy_from_slice(&size32.to_be_bytes());
        }
        16 => {
            bytes[box_off..box_off + 4].copy_from_slice(&1u32.to_be_bytes());
            bytes[box_off + 8..box_off + 16].copy_from_slice(&(new_size as u64).to_be_bytes());
        }
        _ => return Err(ContractError::InvalidField(field)),
    }
    Ok(())
}

fn grow_box_size_in_place(
    bytes: &mut [u8],
    box_off: usize,
    delta: usize,
    field: &'static str,
) -> Result<(), ContractError> {
    let header = read_box_header(bytes, box_off)?;
    let new_size = header
        .size
        .checked_add(delta)
        .ok_or(ContractError::InvalidField(field))?;
    match header.header_size {
        8 => {
            let size32 = u32::try_from(new_size).map_err(|_| ContractError::InvalidField(field))?;
            bytes[box_off..box_off + 4].copy_from_slice(&size32.to_be_bytes());
        }
        16 => {
            bytes[box_off..box_off + 4].copy_from_slice(&1u32.to_be_bytes());
            bytes[box_off + 8..box_off + 16].copy_from_slice(&(new_size as u64).to_be_bytes());
        }
        _ => return Err(ContractError::InvalidField(field)),
    }
    Ok(())
}

fn make_box_v1(box_type: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>, ContractError> {
    let size = 8usize
        .checked_add(payload.len())
        .ok_or(ContractError::InvalidField("box_size"))?;
    let size32 = u32::try_from(size).map_err(|_| ContractError::InvalidField("box_size"))?;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&size32.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(payload);
    Ok(out)
}

fn make_fullbox_v1(
    box_type: &[u8; 4],
    version: u8,
    flags: u32,
    payload: &[u8],
) -> Result<Vec<u8>, ContractError> {
    let mut content = Vec::with_capacity(4 + payload.len());
    content.push(version);
    content.extend_from_slice(&flags.to_be_bytes()[1..]);
    content.extend_from_slice(payload);
    make_box_v1(box_type, &content)
}

fn protected_sample_entry_fourcc_v1(original_fourcc: [u8; 4]) -> Result<[u8; 4], ContractError> {
    match &original_fourcc {
        b"avc1" => Ok(*b"encv"),
        b"mp4a" => Ok(*b"enca"),
        _ => Err(ContractError::InvalidField("init_segment_bytes")),
    }
}

fn make_protected_sinf_v1(
    original_fourcc: [u8; 4],
    key_id: [u8; 16],
) -> Result<Vec<u8>, ContractError> {
    let frma = make_box_v1(b"frma", &original_fourcc)?;
    let mut schm_payload = Vec::new();
    schm_payload.extend_from_slice(b"cenc");
    schm_payload.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let schm = make_fullbox_v1(b"schm", 0, 0, &schm_payload)?;
    let mut tenc_payload = vec![0, 0, 1, SUPPORTED_COMMON_IV_SIZE_V1];
    tenc_payload.extend_from_slice(&key_id);
    let tenc = make_fullbox_v1(b"tenc", 0, 0, &tenc_payload)?;
    let schi = make_box_v1(b"schi", &tenc)?;
    let mut sinf_payload = Vec::new();
    sinf_payload.extend_from_slice(&frma);
    sinf_payload.extend_from_slice(&schm);
    sinf_payload.extend_from_slice(&schi);
    make_box_v1(b"sinf", &sinf_payload)
}

fn make_fullsample_senc_box_v1(sample_ivs: &[[u8; 8]]) -> Result<Vec<u8>, ContractError> {
    let mut payload = Vec::new();
    payload.extend_from_slice(
        &u32::try_from(sample_ivs.len())
            .map_err(|_| ContractError::InvalidField("clear_segment_bytes"))?
            .to_be_bytes(),
    );
    for iv in sample_ivs {
        payload.extend_from_slice(iv);
    }
    make_fullbox_v1(b"senc", 0, 0, &payload)
}

fn parse_tkhd_track_id(data: &[u8], off: usize, h: BoxHeader) -> Result<u32, ContractError> {
    let content = off + h.header_size;
    let version = *data
        .get(content)
        .ok_or(ContractError::InvalidField("init_segment_bytes"))?;
    if version != 0 && version != 1 {
        return Err(ContractError::InvalidField("init_segment_bytes"));
    }
    let track_id_offset = if version == 1 { 20 } else { 12 };
    read_u32_at(data, content + track_id_offset)
}

fn parse_hdlr_type(data: &[u8], off: usize, h: BoxHeader) -> Result<[u8; 4], ContractError> {
    let content = off + h.header_size;
    slice_at(data, content + 8, 4)?
        .try_into()
        .map_err(|_| ContractError::InvalidField("init_segment_bytes"))
}

fn read_box_header(data: &[u8], offset: usize) -> Result<BoxHeader, ContractError> {
    let size32 = read_u32_at(data, offset)?;
    let box_type: [u8; 4] = slice_at(data, offset + 4, 4)?
        .try_into()
        .map_err(|_| ContractError::UnexpectedEnd)?;
    if size32 == 0 {
        return Err(ContractError::InvalidField("box_size"));
    }
    if size32 == 1 {
        let size64 = read_u64_at(data, offset + 8)?;
        let size = usize::try_from(size64).map_err(|_| ContractError::InvalidField("box_size"))?;
        let end = offset
            .checked_add(size)
            .ok_or(ContractError::InvalidField("box_size"))?;
        if size < 16 || end > data.len() {
            return Err(ContractError::InvalidField("box_size"));
        }
        return Ok(BoxHeader {
            box_type,
            size,
            header_size: 16,
        });
    }
    let size = size32 as usize;
    let end = offset
        .checked_add(size)
        .ok_or(ContractError::InvalidField("box_size"))?;
    if size < 8 || end > data.len() {
        return Err(ContractError::InvalidField("box_size"));
    }
    Ok(BoxHeader {
        box_type,
        size,
        header_size: 8,
    })
}

fn scan_boxes(
    data: &[u8],
    start: usize,
    end: usize,
) -> Result<Vec<(usize, BoxHeader)>, ContractError> {
    if start > end || end > data.len() {
        return Err(ContractError::InvalidField("box_size"));
    }
    let mut out = Vec::new();
    let mut offset = start;
    while offset < end {
        let h = read_box_header(data, offset)?;
        let next = offset
            .checked_add(h.size)
            .ok_or(ContractError::InvalidField("box_size"))?;
        if next > end {
            return Err(ContractError::InvalidField("box_size"));
        }
        if out.len() == MAX_CHILD_BOXES_V1 {
            return Err(ContractError::InvalidField("box_count"));
        }
        out.push((offset, h));
        offset = next;
    }
    if offset != end {
        return Err(ContractError::TrailingBytes);
    }
    Ok(out)
}

fn slice_at(data: &[u8], start: usize, length: usize) -> Result<&[u8], ContractError> {
    let end = start
        .checked_add(length)
        .ok_or(ContractError::UnexpectedEnd)?;
    data.get(start..end).ok_or(ContractError::UnexpectedEnd)
}

fn read_fullbox_header(data: &[u8], content: usize) -> Result<(u8, u32), ContractError> {
    let version = *data.get(content).ok_or(ContractError::UnexpectedEnd)?;
    let flags = u32::from_be_bytes([
        0,
        *data.get(content + 1).ok_or(ContractError::UnexpectedEnd)?,
        *data.get(content + 2).ok_or(ContractError::UnexpectedEnd)?,
        *data.get(content + 3).ok_or(ContractError::UnexpectedEnd)?,
    ]);
    Ok((version, flags))
}

fn read_u16_at(data: &[u8], start: usize) -> Result<u16, ContractError> {
    Ok(u16::from_be_bytes(
        slice_at(data, start, 2)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_u32_at(data: &[u8], start: usize) -> Result<u32, ContractError> {
    Ok(u32::from_be_bytes(
        slice_at(data, start, 4)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_i32_at(data: &[u8], start: usize) -> Result<i32, ContractError> {
    Ok(i32::from_be_bytes(
        slice_at(data, start, 4)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

fn read_u64_at(data: &[u8], start: usize) -> Result<u64, ContractError> {
    Ok(u64::from_be_bytes(
        slice_at(data, start, 8)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)?,
    ))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use sha2::{Digest as _, Sha256};

    use super::*;

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
        let mut tenc_payload = vec![0, 0, 1, SUPPORTED_COMMON_IV_SIZE_V1];
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

    fn make_clear_sample_entry(handler_type: &[u8; 4]) -> Vec<u8> {
        let (fourcc, fixed) = match handler_type {
            b"vide" => (b"avc1", VISUAL_SAMPLE_ENTRY_FIXED_BYTES),
            b"soun" => (b"mp4a", AUDIO_SAMPLE_ENTRY_FIXED_BYTES),
            _ => panic!("unsupported handler"),
        };
        let content = vec![0u8; fixed];
        make_box(fourcc, &content)
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

    fn build_init_segment(
        tracks: &[(u32, &[u8; 4])],
        trex_ids: &[u32],
        extra_moov_children: &[Vec<u8>],
    ) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
        let traks: Vec<Vec<u8>> = tracks
            .iter()
            .map(|(track_id, handler)| make_trak(*track_id, handler))
            .collect();
        let mut mvex_content = Vec::new();
        for track_id in trex_ids {
            mvex_content.extend_from_slice(&make_trex(*track_id));
        }
        let mvex = make_box(b"mvex", &mvex_content);
        let mvhd = make_box(b"mvhd", &[0u8; 4]);

        let mut moov_content = Vec::new();
        moov_content.extend_from_slice(&mvhd);
        for trak in &traks {
            moov_content.extend_from_slice(trak);
        }
        moov_content.extend_from_slice(&mvex);
        for child in extra_moov_children {
            moov_content.extend_from_slice(child);
        }
        let moov = make_box(b"moov", &moov_content);

        let mut init = Vec::new();
        init.extend_from_slice(&ftyp);
        init.extend_from_slice(&moov);
        init
    }

    fn valid_init_segment() -> Vec<u8> {
        build_init_segment(&[(1, b"vide"), (2, b"soun")], &[1, 2], &[])
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

    fn make_segment_with(
        track_id: u32,
        payload: &[u8],
        tfhd_flags: u32,
        include_tfdt: bool,
        subsamples: Option<&[(u16, u32)]>,
    ) -> Vec<u8> {
        let mut tfhd_payload = Vec::new();
        tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_DURATION != 0 {
            tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
        }
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_SIZE != 0 {
            tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        }
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_FLAGS != 0 {
            tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
        }
        let tfhd = make_fullbox(b"tfhd", 0, tfhd_flags, &tfhd_payload);

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

        let traf = {
            let mut traf_content = Vec::new();
            traf_content.extend_from_slice(&tfhd);
            if include_tfdt {
                traf_content.extend_from_slice(&make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()));
            }
            traf_content.extend_from_slice(&trun);
            traf_content.extend_from_slice(&senc);
            make_box(b"traf", &traf_content)
        };
        let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let mut moof_content = Vec::new();
        moof_content.extend_from_slice(&mfhd);
        moof_content.extend_from_slice(&traf);
        let mut moof = make_box(b"moof", &moof_content);

        let moof_children = scan_boxes(&moof, 8, moof.len()).unwrap();
        let traf_box = moof_children
            .iter()
            .copied()
            .find(|(_, h)| h.box_type == *b"traf")
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
            .find(|(_, h)| h.box_type == *b"trun")
            .unwrap();
        let data_offset_at = trun_box.0 + trun_box.1.header_size + 8;
        let sample_offset = (moof.len() + 8) as i32;
        moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());

        let mdat = make_box(b"mdat", payload);
        let mut out = moof;
        out.extend_from_slice(&mdat);
        out
    }

    fn top_boxes(bytes: &[u8]) -> Vec<(usize, BoxHeader)> {
        scan_boxes(bytes, 0, bytes.len()).unwrap()
    }

    fn child_box(bytes: &[u8], start: usize, end: usize, box_type: &[u8; 4]) -> (usize, BoxHeader) {
        scan_boxes(bytes, start, end)
            .unwrap()
            .into_iter()
            .find(|(_, h)| h.box_type == *box_type)
            .unwrap()
    }

    fn set_fullbox_version_and_flags(bytes: &mut [u8], box_off: usize, version: u8, flags: u32) {
        let header = read_box_header(bytes, box_off).unwrap();
        let content = box_off + header.header_size;
        bytes[content] = version;
        bytes[content + 1..content + 4].copy_from_slice(&flags.to_be_bytes()[1..]);
    }

    fn append_moof_child(segment: &mut Vec<u8>, child: Vec<u8>) {
        let top = top_boxes(segment);
        let (_, moof_h) = top[0];
        let mdat_off = top[1].0;
        let child_len = child.len();
        segment.splice(mdat_off..mdat_off, child);
        segment[..4].copy_from_slice(&((moof_h.size + child_len) as u32).to_be_bytes());
        let (traf_off, traf_h) = child_box(segment, 8, moof_h.size + child_len, b"traf");
        let (trun_off, trun_h) = child_box(segment, traf_off + 8, traf_off + traf_h.size, b"trun");
        let data_offset_at = trun_off + trun_h.header_size + 8;
        let old = i32::from_be_bytes(
            segment[data_offset_at..data_offset_at + 4]
                .try_into()
                .unwrap(),
        );
        segment[data_offset_at..data_offset_at + 4]
            .copy_from_slice(&(old + child_len as i32).to_be_bytes());
    }

    fn append_traf_child(segment: &mut Vec<u8>, child: Vec<u8>) {
        let top = top_boxes(segment);
        let (moof_off, moof_h) = top[0];
        let (traf_off, traf_h) = child_box(segment, moof_off + 8, moof_off + moof_h.size, b"traf");
        let insert_at = traf_off + traf_h.size;
        let child_len = child.len();
        segment.splice(insert_at..insert_at, child);
        segment[traf_off..traf_off + 4]
            .copy_from_slice(&((traf_h.size + child_len) as u32).to_be_bytes());
        segment[moof_off..moof_off + 4]
            .copy_from_slice(&((moof_h.size + child_len) as u32).to_be_bytes());
        let (trun_off, trun_h) = child_box(
            segment,
            traf_off + 8,
            traf_off + traf_h.size + child_len,
            b"trun",
        );
        let data_offset_at = trun_off + trun_h.header_size + 8;
        let old = i32::from_be_bytes(
            segment[data_offset_at..data_offset_at + 4]
                .try_into()
                .unwrap(),
        );
        segment[data_offset_at..data_offset_at + 4]
            .copy_from_slice(&(old + child_len as i32).to_be_bytes());
    }

    fn remove_moof_child(segment: &mut Vec<u8>, box_type: &[u8; 4]) {
        let top = top_boxes(segment);
        let (_, moof_h) = top[0];
        let (child_off, child_h) = child_box(segment, 8, moof_h.size, box_type);
        segment.drain(child_off..child_off + child_h.size);
        segment[..4].copy_from_slice(&((moof_h.size - child_h.size) as u32).to_be_bytes());
        let (traf_off, traf_h) = child_box(segment, 8, moof_h.size - child_h.size, b"traf");
        let (trun_off, trun_h) = child_box(segment, traf_off + 8, traf_off + traf_h.size, b"trun");
        let data_offset_at = trun_off + trun_h.header_size + 8;
        let old = i32::from_be_bytes(
            segment[data_offset_at..data_offset_at + 4]
                .try_into()
                .unwrap(),
        );
        segment[data_offset_at..data_offset_at + 4]
            .copy_from_slice(&(old - child_h.size as i32).to_be_bytes());
    }

    fn valid_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
        make_segment_with(track_id, payload, 0x020038, true, None)
    }

    fn make_clear_segment_with(
        track_id: u32,
        payload: &[u8],
        tfhd_flags: u32,
        include_tfdt: bool,
    ) -> Vec<u8> {
        let mut tfhd_payload = Vec::new();
        tfhd_payload.extend_from_slice(&track_id.to_be_bytes());
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_DURATION != 0 {
            tfhd_payload.extend_from_slice(&1u32.to_be_bytes());
        }
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_SIZE != 0 {
            tfhd_payload.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        }
        if tfhd_flags & TFHD_FLAG_DEFAULT_SAMPLE_FLAGS != 0 {
            tfhd_payload.extend_from_slice(&0u32.to_be_bytes());
        }
        let tfhd = make_fullbox(b"tfhd", 0, tfhd_flags, &tfhd_payload);

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
            if include_tfdt {
                traf_content.extend_from_slice(&make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()));
            }
            traf_content.extend_from_slice(&trun);
            make_box(b"traf", &traf_content)
        };
        let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
        let mut moof_content = Vec::new();
        moof_content.extend_from_slice(&mfhd);
        moof_content.extend_from_slice(&traf);
        let mut moof = make_box(b"moof", &moof_content);

        let moof_children = scan_boxes(&moof, 8, moof.len()).unwrap();
        let traf_box = moof_children
            .iter()
            .copied()
            .find(|(_, h)| h.box_type == *b"traf")
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
            .find(|(_, h)| h.box_type == *b"trun")
            .unwrap();
        let data_offset_at = trun_box.0 + trun_box.1.header_size + 8;
        let sample_offset = (moof.len() + 8) as i32;
        moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());

        let mdat = make_box(b"mdat", payload);
        let mut out = moof;
        out.extend_from_slice(&mdat);
        out
    }

    fn valid_clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
        make_clear_segment_with(track_id, payload, 0x020038, true)
    }

    fn media_components() -> (Vec<u8>, Vec<Vec<u8>>, &'static str, &'static str) {
        (
            valid_init_segment(),
            vec![
                valid_segment(1, b"video-segment-ciphertext"),
                valid_segment(2, b"audio-segment-ciphertext"),
            ],
            "video/mp4",
            "avc1.64001f,mp4a.40.2",
        )
    }

    fn media() -> CencFmp4MediaIdentityV1 {
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap()
    }

    #[test]
    fn media_identity_is_ordered_and_deterministic() {
        let identity = media();
        let same = media();
        assert_eq!(identity, same);
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let reversed = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[1].clone(), encrypted_segments[0].clone()],
            mime_type,
            codecs,
        )
        .unwrap();
        assert_ne!(identity.encrypted_content(), reversed.encrypted_content());
        assert_ne!(
            identity.media_manifest_root(),
            reversed.media_manifest_root()
        );
    }

    #[test]
    fn media_identity_rejects_empty_segments_and_distinguishes_init_media_and_ciphertext() {
        let base = media();
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            b"",
            &[encrypted_segments[0].clone()],
            mime_type,
            "avc1"
        )
        .is_err());
        assert!(
            CencFmp4MediaIdentityV1::new_from_bytes(&init_segment, &[], mime_type, "avc1").is_err()
        );
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone(), Vec::new()],
            mime_type,
            codecs
        )
        .is_err());

        let mut changed_init_bytes = init_segment.clone();
        let changed_init_last = changed_init_bytes.len() - 1;
        changed_init_bytes[changed_init_last] ^= 1;
        let changed_init = CencFmp4MediaIdentityV1::new_from_bytes(
            &changed_init_bytes,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        assert_eq!(base.encrypted_content(), changed_init.encrypted_content());
        assert_ne!(
            base.media_manifest_root(),
            changed_init.media_manifest_root()
        );

        let mut changed_second_segment = encrypted_segments[1].clone();
        let last = changed_second_segment.len() - 1;
        changed_second_segment[last] ^= 1;
        let changed_segment = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone(), changed_second_segment],
            mime_type,
            codecs,
        )
        .unwrap();
        assert_ne!(
            base.encrypted_content(),
            changed_segment.encrypted_content()
        );
        assert_ne!(
            base.media_manifest_root(),
            changed_segment.media_manifest_root()
        );

        let changed_mime = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            "audio/mp4",
            codecs,
        )
        .unwrap();
        assert_eq!(base.encrypted_content(), changed_mime.encrypted_content());
        assert_ne!(
            base.media_manifest_root(),
            changed_mime.media_manifest_root()
        );

        let changed_codecs = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            "hev1.1.6.L93.B0,mp4a.40.2",
        )
        .unwrap();
        assert_eq!(base.encrypted_content(), changed_codecs.encrypted_content());
        assert_ne!(
            base.media_manifest_root(),
            changed_codecs.media_manifest_root()
        );
    }

    #[test]
    fn media_identity_rejects_invalid_declarations_and_segment_count_overflow() {
        let (init_segment, encrypted_segments, mime_type, _) = media_components();
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone()],
            "",
            "avc1"
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone()],
            mime_type,
            ""
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone()],
            "v".repeat(256),
            "avc1"
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[encrypted_segments[0].clone()],
            mime_type,
            "a".repeat(256)
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &vec![encrypted_segments[0].clone(); 513],
            mime_type,
            "avc1"
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &[Vec::new()],
            mime_type,
            "avc1"
        )
        .is_err());
    }

    #[test]
    fn media_structure_reports_exact_sample_layout() {
        let init = valid_init_segment();
        let segment_bytes = make_segment_with(1, b"abcdef", 0x020038, true, Some(&[(2, 4)]));
        let layout = CencFmp4MediaIdentityV1::validate_structure(
            &init,
            std::slice::from_ref(&segment_bytes),
        )
        .unwrap();
        assert_eq!(
            layout.init_segment_sha256(),
            Digest32::new(Sha256::digest(&init).into())
        );
        assert_eq!(layout.init_segment_bytes(), init.len() as u64);
        assert_eq!(layout.protected_track_ids(), &[1, 2]);
        assert_eq!(layout.segments().len(), 1);
        let segment = &layout.segments()[0];
        assert_eq!(
            segment.source_sha256(),
            Digest32::new(Sha256::digest(&segment_bytes).into())
        );
        assert_eq!(segment.source_bytes(), segment_bytes.len() as u64);
        assert_eq!(segment.track_id(), 1);
        assert_eq!(segment.samples().len(), 1);
        let sample = &segment.samples()[0];
        assert_eq!(sample.ciphertext_offset(), 0);
        assert_eq!(sample.sample_size(), 6);
        assert_eq!(sample.iv(), (0x10u64 + 1).to_be_bytes());
        assert_eq!(sample.subsamples().len(), 1);
        assert_eq!(sample.subsamples()[0].clear_bytes(), 2);
        assert_eq!(sample.subsamples()[0].encrypted_bytes(), 4);
    }

    #[test]
    fn clear_media_session_layout_accepts_direct_clear_fixture_and_reports_sample_ranges() {
        let init = valid_clear_init_segment();
        let segment = valid_clear_segment(1, b"clear-video");
        let session = ValidatedClearFmp4MediaSessionLayoutV1::new(&init).unwrap();
        let segment_layout = session.validate_segment(&segment).unwrap();

        assert_eq!(session.track_ids(), &[1, 2]);
        assert_eq!(session.tracks.len(), 2);
        assert_eq!(session.tracks[0].original_fourcc, *b"avc1");
        assert_eq!(session.tracks[1].original_fourcc, *b"mp4a");
        assert!(session.tracks[0].sample_entry_off > 0);
        assert_eq!(segment_layout.track_id(), 1);
        assert_eq!(segment_layout.samples().len(), 1);
        assert_eq!(segment_layout.samples()[0].mdat_offset(), 0);
        assert_eq!(segment_layout.samples()[0].sample_size(), 11);
    }

    #[test]
    fn clear_media_session_layout_rejects_unsupported_sample_entry_and_unknown_track() {
        let mut bad_init = valid_clear_init_segment();
        let (moov_off, moov_h) = child_box(&bad_init, 0, bad_init.len(), b"moov");
        let (trak_off, trak_h) =
            child_box(&bad_init, moov_off + 8, moov_off + moov_h.size, b"trak");
        let (mdia_off, mdia_h) =
            child_box(&bad_init, trak_off + 8, trak_off + trak_h.size, b"mdia");
        let (minf_off, minf_h) =
            child_box(&bad_init, mdia_off + 8, mdia_off + mdia_h.size, b"minf");
        let (stbl_off, stbl_h) =
            child_box(&bad_init, minf_off + 8, minf_off + minf_h.size, b"stbl");
        let (stsd_off, _) = child_box(&bad_init, stbl_off + 8, stbl_off + stbl_h.size, b"stsd");
        let entry_off = stsd_off + 16;
        bad_init[entry_off + 4..entry_off + 8].copy_from_slice(b"encv");
        assert!(ValidatedClearFmp4MediaSessionLayoutV1::new(&bad_init).is_err());

        let session =
            ValidatedClearFmp4MediaSessionLayoutV1::new(&valid_clear_init_segment()).unwrap();
        let wrong_track = valid_clear_segment(9, b"clear-video");
        assert!(session.validate_segment(&wrong_track).is_err());
    }

    #[test]
    fn clear_media_session_layout_rejects_bad_offsets_truncation_and_sample_count_or_size_overflow()
    {
        let session =
            ValidatedClearFmp4MediaSessionLayoutV1::new(&valid_clear_init_segment()).unwrap();

        let mut bad_data_offset = valid_clear_segment(1, b"clear-video");
        let top = top_boxes(&bad_data_offset);
        let (traf_off, traf_h) = child_box(&bad_data_offset, 8, top[0].1.size, b"traf");
        let (trun_off, trun_h) = child_box(
            &bad_data_offset,
            traf_off + 8,
            traf_off + traf_h.size,
            b"trun",
        );
        let data_offset_at = trun_off + trun_h.header_size + 8;
        bad_data_offset[data_offset_at..data_offset_at + 4].copy_from_slice(&0i32.to_be_bytes());
        assert!(session.validate_segment(&bad_data_offset).is_err());

        let mut huge_sample = valid_clear_segment(1, b"clear-video");
        huge_sample[data_offset_at + 4..data_offset_at + 8]
            .copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(session.validate_segment(&huge_sample).is_err());

        let mut huge_count = valid_clear_segment(1, b"clear-video");
        let sample_count_at = trun_off + trun_h.header_size + 4;
        huge_count[sample_count_at..sample_count_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(session.validate_segment(&huge_count).is_err());

        let truncated = valid_clear_segment(1, b"clear-video")[..30].to_vec();
        assert!(session.validate_segment(&truncated).is_err());

        let mut malformed_bounds = valid_clear_segment(1, b"clear-video");
        malformed_bounds[..4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        assert!(session.validate_segment(&malformed_bounds).is_err());
    }

    #[test]
    fn clear_media_rewrite_produces_valid_protected_bytes() {
        let clear_init = valid_clear_init_segment();
        let clear_segment = valid_clear_segment(1, b"clear-video");
        let session = ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init).unwrap();
        let segment_layout = session.validate_segment(&clear_segment).unwrap();

        let protected_init = session
            .rewrite_protected_init(&clear_init, [0x55; 16])
            .unwrap();
        let protected_segment = segment_layout
            .rewrite_protected_segment(
                &clear_segment,
                &[b"encrypted!!".to_vec()],
                &[[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]],
            )
            .unwrap();

        let protected_layout = CencFmp4MediaIdentityV1::validate_structure(
            &protected_init,
            &[protected_segment.clone()],
        )
        .unwrap();
        assert_eq!(protected_layout.protected_track_ids(), &[1, 2]);
        assert_eq!(protected_layout.segments()[0].track_id(), 1);
        assert_eq!(
            protected_layout.segments()[0].samples()[0].iv(),
            [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]
        );
        assert_eq!(
            protected_layout.segments()[0]
                .exact_source_mdat_payload(&protected_segment)
                .unwrap(),
            b"encrypted!!"
        );
    }

    #[test]
    fn clear_media_rewrite_rejects_mutated_source_bad_counts_duplicate_iv_and_bad_sample_length() {
        let clear_init = valid_clear_init_segment();
        let clear_segment = valid_clear_segment(1, b"clear-video");
        let session = ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init).unwrap();
        let segment_layout = session.validate_segment(&clear_segment).unwrap();

        let mut tampered_init = clear_init.clone();
        tampered_init[0] ^= 1;
        assert!(session
            .rewrite_protected_init(&tampered_init, [0x55; 16])
            .is_err());

        let mut tampered_segment = clear_segment.clone();
        tampered_segment[0] ^= 1;
        assert!(segment_layout
            .rewrite_protected_segment(
                &tampered_segment,
                &[b"encrypted!!".to_vec()],
                &[[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]],
            )
            .is_err());

        assert!(segment_layout
            .rewrite_protected_segment(&clear_segment, &[], &[])
            .is_err());
        assert!(segment_layout
            .rewrite_protected_segment(
                &clear_segment,
                &[b"encrypted!!".to_vec()],
                &[
                    [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11],
                    [0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x22],
                ],
            )
            .is_err());
        assert!(segment_layout
            .rewrite_protected_segment(
                &clear_segment,
                &[b"short".to_vec()],
                &[[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]],
            )
            .is_err());
        assert!(segment_layout
            .rewrite_protected_segment(
                &clear_segment,
                &[b"encrypted!!".to_vec()],
                &[[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]],
            )
            .is_ok());
        assert!(segment_layout
            .rewrite_protected_segment(
                &clear_segment,
                &[b"encrypted!!".to_vec()],
                &[[0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11]],
            )
            .is_ok());
    }

    fn sample_entry_info(bytes: &[u8], track_index: usize) -> ([u8; 4], usize, usize, usize) {
        let (_, moov_h) = child_box(bytes, 0, bytes.len(), b"moov");
        let moov_off = child_box(bytes, 0, bytes.len(), b"moov").0;
        let tracks: Vec<_> =
            scan_boxes(bytes, moov_off + moov_h.header_size, moov_off + moov_h.size)
                .unwrap()
                .into_iter()
                .filter(|(_, h)| h.box_type == *b"trak")
                .collect();
        let (trak_off, trak_h) = tracks[track_index];
        let (mdia_off, mdia_h) = child_box(bytes, trak_off + 8, trak_off + trak_h.size, b"mdia");
        let (hdlr_off, hdlr_h) = child_box(bytes, mdia_off + 8, mdia_off + mdia_h.size, b"hdlr");
        let handler_type = parse_hdlr_type(bytes, hdlr_off, hdlr_h).unwrap();
        let (minf_off, minf_h) = child_box(bytes, mdia_off + 8, mdia_off + mdia_h.size, b"minf");
        let (stbl_off, stbl_h) = child_box(bytes, minf_off + 8, minf_off + minf_h.size, b"stbl");
        let (stsd_off, _) = child_box(bytes, stbl_off + 8, stbl_off + stbl_h.size, b"stsd");
        let entry_off = stsd_off + 16;
        let entry_h = read_box_header(bytes, entry_off).unwrap();
        let fixed = if handler_type == *b"vide" {
            VISUAL_SAMPLE_ENTRY_FIXED_BYTES
        } else if handler_type == *b"soun" {
            AUDIO_SAMPLE_ENTRY_FIXED_BYTES
        } else {
            unreachable!()
        };
        (entry_h.box_type, entry_off, entry_h.size, fixed)
    }

    #[test]
    fn media_layout_rewrites_every_validated_track_in_init() {
        let init = valid_init_segment();
        let segment = valid_segment(1, b"video-segment-ciphertext");
        let layout = CencFmp4MediaIdentityV1::validate_structure(&init, &[segment]).unwrap();

        let rewritten = layout.rewrite_clear_init(&init).unwrap();
        let (video_type, video_entry_off, video_entry_size, video_fixed) =
            sample_entry_info(&rewritten, 0);
        let (audio_type, audio_entry_off, audio_entry_size, audio_fixed) =
            sample_entry_info(&rewritten, 1);

        assert_eq!(video_type, *b"avc1");
        assert_eq!(audio_type, *b"mp4a");
        assert!(scan_boxes(
            &rewritten,
            video_entry_off + 8 + video_fixed,
            video_entry_off + video_entry_size,
        )
        .unwrap()
        .is_empty());
        assert!(scan_boxes(
            &rewritten,
            audio_entry_off + 8 + audio_fixed,
            audio_entry_off + audio_entry_size,
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn media_layout_rewrites_segment_with_exact_source_and_offsets() {
        let init = valid_init_segment();
        let segment = make_segment_with(1, b"abcdef", 0x020038, true, Some(&[(2, 4)]));
        let layout =
            CencFmp4MediaIdentityV1::validate_structure(&init, std::slice::from_ref(&segment))
                .unwrap();

        let rewritten = layout
            .rewrite_clear_segment(0, &segment, b"ABCDEF")
            .unwrap();
        let top = top_boxes(&rewritten);
        let (moof_off, moof_h) = top[0];
        let (mdat_off, mdat_h) = top[1];
        let (traf_off, traf_h) =
            child_box(&rewritten, moof_off + 8, moof_off + moof_h.size, b"traf");
        let traf_children = scan_boxes(&rewritten, traf_off + 8, traf_off + traf_h.size).unwrap();
        assert!(traf_children.iter().all(|(_, h)| h.box_type != *b"senc"));
        let (trun_off, trun_h) =
            child_box(&rewritten, traf_off + 8, traf_off + traf_h.size, b"trun");
        let data_offset = read_i32_at(&rewritten, trun_off + trun_h.header_size + 8).unwrap();
        assert_eq!(
            usize::try_from(data_offset).unwrap(),
            mdat_off + mdat_h.header_size
        );
        assert_eq!(
            &rewritten[mdat_off + mdat_h.header_size..mdat_off + mdat_h.size],
            b"ABCDEF"
        );
    }

    #[test]
    fn media_layout_rewrite_fails_closed_for_changed_bytes_and_wrong_segment_index() {
        let init = valid_init_segment();
        let segment = valid_segment(1, b"video-segment-ciphertext");
        let layout =
            CencFmp4MediaIdentityV1::validate_structure(&init, std::slice::from_ref(&segment))
                .unwrap();

        let mut tampered_init = init.clone();
        tampered_init[0] ^= 1;
        assert!(layout.rewrite_clear_init(&tampered_init).is_err());

        let mut tampered_segment = segment.clone();
        tampered_segment[0] ^= 1;
        assert!(layout
            .rewrite_clear_segment(0, &tampered_segment, b"video-segment-ciphertext")
            .is_err());
        assert!(layout
            .rewrite_clear_segment(1, &segment, b"video-segment-ciphertext")
            .is_err());
    }

    #[test]
    fn media_layout_rewrite_helpers_reject_extended_size_malformed_and_underflow() {
        let mut extended = Vec::new();
        extended.extend_from_slice(&1u32.to_be_bytes());
        extended.extend_from_slice(b"test");
        extended.extend_from_slice(&24u64.to_be_bytes());
        extended.extend_from_slice(&[0u8; 8]);
        shrink_box_size_in_place(&mut extended, 0, 8, "test").unwrap();
        assert_eq!(read_u64_at(&extended, 8).unwrap(), 16);

        let mut malformed = Vec::new();
        malformed.extend_from_slice(&1u32.to_be_bytes());
        malformed.extend_from_slice(b"test");
        malformed.extend_from_slice(&8u64.to_be_bytes());
        assert!(shrink_box_size_in_place(&mut malformed, 0, 1, "test").is_err());

        let mut standard = make_box(b"test", &[]);
        assert!(shrink_box_size_in_place(&mut standard, 0, 1, "test").is_err());
    }

    #[test]
    fn media_structure_rejects_invalid_init_and_segment_grammar() {
        let valid_init = valid_init_segment();
        let valid_segment = valid_segment(1, b"video-segment-ciphertext");

        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(
                &[(1, b"vide"), (2, b"soun")],
                &[1, 2],
                &[make_box(b"pssh", b"x")]
            ),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(&[(1, b"vide"), (1, b"soun")], &[1, 1], &[]),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(&[(0, b"vide"), (2, b"soun")], &[0, 2], &[]),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(&[(1, b"vide"), (2, b"soun")], &[1], &[]),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(&[(1, b"vide"), (2, b"soun")], &[1, 2, 3], &[]),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &build_init_segment(&[(1, b"vide"), (2, b"soun")], &[1, 1], &[]),
            std::slice::from_ref(&valid_segment),
        )
        .is_err());
        let many_children = (0..65).fold(Vec::new(), |mut bytes, _| {
            bytes.extend_from_slice(&make_box(b"free", b"x"));
            bytes
        });
        assert!(scan_boxes(&many_children, 0, many_children.len()).is_err());

        let mut unknown_top_level = valid_segment.clone();
        unknown_top_level.extend_from_slice(&make_box(b"free", b"x"));
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[unknown_top_level],)
                .is_err()
        );
        let mut unknown_moof_child = valid_segment.clone();
        append_moof_child(&mut unknown_moof_child, make_box(b"free", b"x"));
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[unknown_moof_child],)
                .is_err()
        );
        let mut unknown_traf_child = valid_segment.clone();
        append_traf_child(&mut unknown_traf_child, make_box(b"free", b"x"));
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[unknown_traf_child],)
                .is_err()
        );
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &valid_init,
            &[make_segment_with(
                9,
                b"video-segment-ciphertext",
                0x020038,
                true,
                None
            )],
        )
        .is_err());

        let zero_subsample_segment =
            make_segment_with(1, b"video-segment-ciphertext", 0x020038, true, Some(&[]));
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &valid_init,
            &[zero_subsample_segment],
        )
        .is_err());
    }

    #[test]
    fn media_structure_rejects_strict_box_mutations() {
        let valid_init = valid_init_segment();
        let valid_segment = valid_segment(1, b"video-segment-ciphertext");

        let mut frma_wrong = valid_init.clone();
        let (moov_off, moov_h) = child_box(&frma_wrong, 0, frma_wrong.len(), b"moov");
        let (trak_off, trak_h) =
            child_box(&frma_wrong, moov_off + 8, moov_off + moov_h.size, b"trak");
        let (mdia_off, mdia_h) =
            child_box(&frma_wrong, trak_off + 8, trak_off + trak_h.size, b"mdia");
        let (minf_off, minf_h) =
            child_box(&frma_wrong, mdia_off + 8, mdia_off + mdia_h.size, b"minf");
        let (stbl_off, stbl_h) =
            child_box(&frma_wrong, minf_off + 8, minf_off + minf_h.size, b"stbl");
        let (stsd_off, _) = child_box(&frma_wrong, stbl_off + 8, stbl_off + stbl_h.size, b"stsd");
        let entry_off = stsd_off + 16;
        let entry_h = read_box_header(&frma_wrong, entry_off).unwrap();
        let sinf_off = child_box(
            &frma_wrong,
            entry_off + entry_h.header_size + VISUAL_SAMPLE_ENTRY_FIXED_BYTES,
            entry_off + entry_h.size,
            b"sinf",
        )
        .0;
        let frma_off = child_box(
            &frma_wrong,
            sinf_off + 8,
            sinf_off + read_box_header(&frma_wrong, sinf_off).unwrap().size,
            b"frma",
        )
        .0;
        frma_wrong[frma_off + 8..frma_off + 12].copy_from_slice(b"mp4a");
        assert!(CencFmp4MediaIdentityV1::validate_structure(
            &frma_wrong,
            std::slice::from_ref(&valid_segment)
        )
        .is_err());

        let mut missing_mfhd = valid_segment.clone();
        remove_moof_child(&mut missing_mfhd, b"mfhd");
        assert!(CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[missing_mfhd]).is_err());

        let mut duplicate_mfhd = valid_segment.clone();
        append_moof_child(
            &mut duplicate_mfhd,
            make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes()),
        );
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[duplicate_mfhd]).is_err()
        );

        let mut duplicate_tfhd = valid_segment.clone();
        append_traf_child(
            &mut duplicate_tfhd,
            make_fullbox(
                b"tfhd",
                0,
                0x020038,
                &[0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 24, 0, 0, 0, 0],
            ),
        );
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[duplicate_tfhd]).is_err()
        );

        let mut duplicate_trun = valid_segment.clone();
        append_traf_child(
            &mut duplicate_trun,
            make_fullbox(
                b"trun",
                0,
                0x000205,
                &[0, 0, 0, 1, 0, 0, 0, 40, 0, 0, 0, 24],
            ),
        );
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[duplicate_trun]).is_err()
        );

        let mut duplicate_senc = valid_segment.clone();
        let mut senc_payload = Vec::new();
        senc_payload.extend_from_slice(&1u32.to_be_bytes());
        senc_payload.extend_from_slice(&(0x11u64).to_be_bytes());
        append_traf_child(
            &mut duplicate_senc,
            make_fullbox(b"senc", 0, 0, &senc_payload),
        );
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[duplicate_senc]).is_err()
        );

        let mut duplicate_tfdt = valid_segment.clone();
        append_traf_child(
            &mut duplicate_tfdt,
            make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes()),
        );
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[duplicate_tfdt]).is_err()
        );

        let mut wrong_mfhd_version = valid_segment.clone();
        let mfhd_off = child_box(
            &wrong_mfhd_version,
            8,
            top_boxes(&wrong_mfhd_version)[0].1.size,
            b"mfhd",
        )
        .0;
        set_fullbox_version_and_flags(&mut wrong_mfhd_version, mfhd_off, 1, 0);
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[wrong_mfhd_version])
                .is_err()
        );

        let mut wrong_tfdt_version = valid_segment.clone();
        let top = top_boxes(&wrong_tfdt_version);
        let (traf_off, traf_h) = child_box(&wrong_tfdt_version, 8, top[0].1.size, b"traf");
        let tfdt_off = child_box(
            &wrong_tfdt_version,
            traf_off + 8,
            traf_off + traf_h.size,
            b"tfdt",
        )
        .0;
        set_fullbox_version_and_flags(&mut wrong_tfdt_version, tfdt_off, 2, 0);
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[wrong_tfdt_version])
                .is_err()
        );

        let mut wrong_tfhd_flags = valid_segment.clone();
        let top = top_boxes(&wrong_tfhd_flags);
        let (traf_off, traf_h) = child_box(&wrong_tfhd_flags, 8, top[0].1.size, b"traf");
        let tfhd_off = child_box(
            &wrong_tfhd_flags,
            traf_off + 8,
            traf_off + traf_h.size,
            b"tfhd",
        )
        .0;
        set_fullbox_version_and_flags(&mut wrong_tfhd_flags, tfhd_off, 0, 0x020039);
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[wrong_tfhd_flags]).is_err()
        );

        let mut wrong_trun_flags = valid_segment.clone();
        let top = top_boxes(&wrong_trun_flags);
        let (traf_off, traf_h) = child_box(&wrong_trun_flags, 8, top[0].1.size, b"traf");
        let trun_off = child_box(
            &wrong_trun_flags,
            traf_off + 8,
            traf_off + traf_h.size,
            b"trun",
        )
        .0;
        set_fullbox_version_and_flags(&mut wrong_trun_flags, trun_off, 0, 0);
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[wrong_trun_flags]).is_err()
        );

        let mut wrong_senc_flags = valid_segment.clone();
        let top = top_boxes(&wrong_senc_flags);
        let (traf_off, traf_h) = child_box(&wrong_senc_flags, 8, top[0].1.size, b"traf");
        let senc_off = child_box(
            &wrong_senc_flags,
            traf_off + 8,
            traf_off + traf_h.size,
            b"senc",
        )
        .0;
        set_fullbox_version_and_flags(&mut wrong_senc_flags, senc_off, 0, 0x000003);
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[wrong_senc_flags]).is_err()
        );

        let mut malformed_bounds = valid_segment.clone();
        malformed_bounds[..4].copy_from_slice(&0xFFFF_FFF0u32.to_be_bytes());
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[malformed_bounds]).is_err()
        );

        let truncated_segment = valid_segment[..valid_segment.len() - 1].to_vec();
        assert!(
            CencFmp4MediaIdentityV1::validate_structure(&valid_init, &[truncated_segment]).is_err()
        );
    }

    #[test]
    fn media_identity_round_trips_and_rejects_canonical_mutation_and_trailing_bytes() {
        let media = media();
        let bytes = media.canonical_bytes().unwrap();
        let decoded = CencFmp4MediaIdentityV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, media);

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            CencFmp4MediaIdentityV1::from_canonical_bytes(&trailing),
            Err(ContractError::TrailingBytes)
        );

        let mut mutated = bytes.clone();
        let last = mutated.len() - 1;
        mutated[last] ^= 1;
        assert!(CencFmp4MediaIdentityV1::from_canonical_bytes(&mutated).is_err());

        let mut segment_length_mutated = bytes.clone();
        let last_segment = media
            .encrypted_segments()
            .last()
            .unwrap()
            .canonical_bytes()
            .unwrap();
        let last_segment_offset = bytes
            .windows(last_segment.len())
            .rposition(|window| window == last_segment.as_slice())
            .unwrap();
        let segment_length_offset = last_segment_offset - 2;
        segment_length_mutated[segment_length_offset + 1] ^= 1;
        assert!(CencFmp4MediaIdentityV1::from_canonical_bytes(&segment_length_mutated).is_err());

        let mut segment_digest_mutated = bytes.clone();
        segment_digest_mutated
            [last_segment_offset + CENC_FMP4_SEGMENT_IDENTITY_DOMAIN_V1.len() + 1] ^= 1;
        assert!(CencFmp4MediaIdentityV1::from_canonical_bytes(&segment_digest_mutated).is_err());
    }

    #[test]
    fn media_identity_has_stable_golden_vector() {
        let bytes = media().canonical_bytes().unwrap();
        assert_eq!(bytes.len(), 453);
        let digest = Sha256::digest(&bytes);
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut hex, "{byte:02x}").unwrap();
        }
        assert_eq!(
            hex,
            "3f544bcc0257f5ed6d5f4c67726a03abeacd0511bcd1efd9911bfa3defb6b6da"
        );
    }

    #[test]
    fn media_session_layout_accepts_fixture_and_matches_full_layout() {
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let media = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        let session = ValidatedCencFmp4MediaSessionLayoutV1::new(&media, &init_segment).unwrap();
        let full_layout =
            CencFmp4MediaIdentityV1::validate_structure(&init_segment, &encrypted_segments)
                .unwrap();

        assert_eq!(session.media_identity(), &media);
        assert_eq!(
            session.protected_track_ids(),
            full_layout.protected_track_ids()
        );
        assert_eq!(
            session.rewrite_clear_init(&init_segment).unwrap(),
            full_layout.rewrite_clear_init(&init_segment).unwrap()
        );
        for (index, expected_segment) in full_layout.segments().iter().enumerate() {
            assert_eq!(
                session
                    .validate_indexed_segment(index, &encrypted_segments[index])
                    .unwrap(),
                *expected_segment
            );
        }
        let debug = format!("{session:?}");
        assert!(!debug.contains("path"));
        assert!(!debug.contains("route"));
        assert!(!debug.contains("share"));
        assert!(!debug.contains("credential"));
        assert!(!debug.contains("handle"));
    }

    #[test]
    fn media_session_layout_rejects_wrong_init_and_changed_identity() {
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let media = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        let mut tampered_init = init_segment.clone();
        tampered_init[0] ^= 1;
        assert!(ValidatedCencFmp4MediaSessionLayoutV1::new(&media, &tampered_init).is_err());

        let other_init = build_init_segment(&[(1, b"vide")], &[1], &[]);
        let other_segments = vec![valid_segment(1, b"video-segment-ciphertext")];
        let other_media = CencFmp4MediaIdentityV1::new_from_bytes(
            &other_init,
            &other_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        assert!(ValidatedCencFmp4MediaSessionLayoutV1::new(&other_media, &init_segment).is_err());
    }

    #[test]
    fn media_session_layout_rejects_wrong_index_segment_substitution_changed_bytes_and_unknown_track(
    ) {
        let (init_segment, encrypted_segments, mime_type, codecs) = media_components();
        let media = CencFmp4MediaIdentityV1::new_from_bytes(
            &init_segment,
            &encrypted_segments,
            mime_type,
            codecs,
        )
        .unwrap();
        let session = ValidatedCencFmp4MediaSessionLayoutV1::new(&media, &init_segment).unwrap();

        assert!(session
            .validate_indexed_segment(encrypted_segments.len(), &encrypted_segments[0])
            .is_err());
        assert!(session
            .validate_indexed_segment(0, &encrypted_segments[1])
            .is_err());
        assert!(session
            .validate_indexed_segment(1, &encrypted_segments[0])
            .is_err());

        let mut tampered_segment = encrypted_segments[0].clone();
        tampered_segment[0] ^= 1;
        assert!(session
            .validate_indexed_segment(0, &tampered_segment)
            .is_err());

        let unknown_track_segment =
            make_segment_with(9, b"video-segment-ciphertext", 0x020038, true, None);
        assert!(session
            .validate_indexed_segment(0, &unknown_track_segment)
            .is_err());
    }
}
