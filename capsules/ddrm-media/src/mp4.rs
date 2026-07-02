//! Minimal fMP4 (ISO BMFF) box surgery for the local CENC demo.
//!
//! This is the encrypt-side counterpart of `decrypt-provider`'s `cenc` engine —
//! the "fMP4 box surgery (mp4box)" the providers vendor only the *cipher core* of.
//! It does exactly enough to turn a clean fragmented MP4 (ffmpeg `+frag_keyframe
//! +empty_moov+default_base_moof`) into a multi-segment CENC asset the decrypt
//! boundary opens, and back:
//!   - split a fragmented MP4 into an init segment (ftyp..moov) + media fragments
//!     (each `moof`+`mdat`);
//!   - CENC-encrypt a fragment's `mdat` samples (AES-128-CTR, per-sample 8-byte
//!     IVs) and inject a `senc` box, fixing `trun.data_offset` + `traf`/`moof`
//!     sizes so the fragment stays internally consistent;
//!   - strip `senc` back out of a decrypted fragment (the inverse) so the browser
//!     receives byte-identical clean fragments;
//!   - read the avc1 codec string from the init for the MSE `mime`.
//!
//! Both sides agree on full-sample encryption (no subsamples), 8-byte IVs, and an
//! unencrypted-looking init (no `encv`/`tenc`/`pssh`) — so the decrypted output is
//! exactly the original clean fragment the browser can decode, with no EME/CDM.

use aes::cipher::{KeyIvInit, StreamCipher};

use crate::mpd::{SegmentInfo, TrackInfo, TrackKind};

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    box_type: [u8; 4],
    size: usize,
    header_size: usize,
}

fn read_box_header(data: &[u8], offset: usize) -> Option<BoxHeader> {
    if offset + 8 > data.len() {
        return None;
    }
    let size32 = u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]);
    let box_type = [
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    let (size, header_size) = if size32 == 1 {
        if offset + 16 > data.len() {
            return None;
        }
        let size64 = u64::from_be_bytes([
            data[offset + 8],
            data[offset + 9],
            data[offset + 10],
            data[offset + 11],
            data[offset + 12],
            data[offset + 13],
            data[offset + 14],
            data[offset + 15],
        ]);
        (size64 as usize, 16usize)
    } else if size32 == 0 {
        (data.len() - offset, 8usize)
    } else {
        (size32 as usize, 8usize)
    };
    Some(BoxHeader {
        box_type,
        size,
        header_size,
    })
}

/// Walk the top-level boxes of `data`, returning `(offset, header)` for each.
fn top_level_boxes(data: &[u8]) -> Result<Vec<(usize, BoxHeader)>, String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let h = read_box_header(data, offset).ok_or_else(|| format!("truncated box at {offset}"))?;
        if h.size < 8 || offset + h.size > data.len() {
            return Err(format!(
                "box {} size {} overruns data at {offset}",
                String::from_utf8_lossy(&h.box_type),
                h.size
            ));
        }
        out.push((offset, h));
        offset += h.size;
    }
    Ok(out)
}

/// A split fragmented MP4: the init segment and the ordered media fragments.
pub struct SplitAsset {
    pub init: Vec<u8>,
    pub fragments: Vec<Vec<u8>>,
}

/// Split a fragmented MP4 into init (everything before the first `moof`) + media
/// fragments (each `moof` paired with its following `mdat`). Trailing index boxes
/// (`mfra`, `sidx`, `styp`) are not part of a media fragment and are dropped.
pub fn split_fragmented(data: &[u8]) -> Result<SplitAsset, String> {
    let boxes = top_level_boxes(data)?;
    let first_moof = boxes
        .iter()
        .position(|(_, h)| &h.box_type == b"moof")
        .ok_or("no moof box — input is not a fragmented MP4 (use ffmpeg -movflags +frag_keyframe+empty_moov+default_base_moof)")?;

    let init_end = boxes[first_moof].0;
    if init_end == 0 {
        return Err("fragmented MP4 has no init (no ftyp/moov before the first moof)".to_string());
    }
    let init = data[..init_end].to_vec();

    let mut fragments = Vec::new();
    let mut i = first_moof;
    while i < boxes.len() {
        let (moof_off, moof_h) = boxes[i];
        if &moof_h.box_type != b"moof" {
            i += 1;
            continue;
        }
        // The mdat must immediately follow the moof.
        let Some((mdat_off, mdat_h)) = boxes.get(i + 1).copied() else {
            return Err(format!("moof at {moof_off} has no following box"));
        };
        if &mdat_h.box_type != b"mdat" {
            return Err(format!(
                "expected mdat after moof at {moof_off}, found {}",
                String::from_utf8_lossy(&mdat_h.box_type)
            ));
        }
        let end = mdat_off + mdat_h.size;
        fragments.push(data[moof_off..end].to_vec());
        i += 2;
    }
    if fragments.is_empty() {
        return Err("fragmented MP4 had no moof+mdat media fragments".to_string());
    }
    Ok(SplitAsset { init, fragments })
}

/// Find a child box of `box_type` within `data[start..end]`, returning its start
/// offset (absolute in `data`) and header.
fn find_box(data: &[u8], start: usize, end: usize, box_type: &[u8; 4]) -> Option<(usize, BoxHeader)> {
    let mut offset = start;
    while offset + 8 <= end {
        let h = read_box_header(data, offset)?;
        if h.size < 8 || offset + h.size > end {
            return None;
        }
        if &h.box_type == box_type {
            return Some((offset, h));
        }
        offset += h.size;
    }
    None
}

struct TrunInfo {
    /// `true` if the data_offset field is present (flag 0x1).
    has_data_offset: bool,
    /// Per-sample sizes (requires flag 0x200).
    sample_sizes: Vec<u32>,
}

fn parse_trun(frag: &[u8], trun_start: usize, trun_h: BoxHeader) -> Result<TrunInfo, String> {
    let c = trun_start + trun_h.header_size;
    if c + 8 > frag.len() {
        return Err("trun too short".into());
    }
    let flags = u32::from_be_bytes([0, frag[c + 1], frag[c + 2], frag[c + 3]]);
    let sample_count = u32::from_be_bytes([frag[c + 4], frag[c + 5], frag[c + 6], frag[c + 7]]) as usize;
    let has_data_offset = flags & 0x000001 != 0;
    let has_first_flags = flags & 0x000004 != 0;
    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;
    let has_cto = flags & 0x000800 != 0;
    if !has_size {
        return Err("trun has no per-sample sizes (flag 0x200) — cannot locate CENC sample boundaries".into());
    }
    let mut p = c + 8;
    if has_data_offset {
        p += 4;
    }
    if has_first_flags {
        p += 4;
    }
    let per_sample_words = [has_duration, has_size, has_flags, has_cto]
        .iter()
        .filter(|b| **b)
        .count();
    let size_word_index = [has_duration].iter().filter(|b| **b).count(); // size is after duration
    let mut sizes = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let size_pos = p + size_word_index * 4;
        if size_pos + 4 > frag.len() {
            return Err("trun sample table truncated".into());
        }
        let sz = u32::from_be_bytes([
            frag[size_pos],
            frag[size_pos + 1],
            frag[size_pos + 2],
            frag[size_pos + 3],
        ]);
        sizes.push(sz);
        p += per_sample_words * 4;
    }
    let _ = trun_start;
    Ok(TrunInfo {
        has_data_offset,
        sample_sizes: sizes,
    })
}

fn write_u32(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let size = (8 + content.len()) as u32;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(&size.to_be_bytes());
    b.extend_from_slice(box_type);
    b.extend_from_slice(content);
    b
}

/// CENC-encrypt a single media fragment in place, returning the encrypted fragment
/// (with an injected `senc` box and fixed offsets/sizes). `iv_counter` is advanced
/// by one per sample so every sample across the whole asset gets a unique IV.
pub fn encrypt_fragment(frag: &[u8], cek: &[u8; 16], iv_counter: &mut u64) -> Result<Vec<u8>, String> {
    let boxes = top_level_boxes(frag)?;
    let (moof_off, moof_h) = boxes
        .iter()
        .copied()
        .find(|(_, h)| &h.box_type == b"moof")
        .ok_or("fragment has no moof")?;
    let (mdat_off, mdat_h) = boxes
        .iter()
        .copied()
        .find(|(_, h)| &h.box_type == b"mdat")
        .ok_or("fragment has no mdat")?;

    let moof_end = moof_off + moof_h.size;
    let (traf_off, traf_h) = find_box(frag, moof_off + moof_h.header_size, moof_end, b"traf")
        .ok_or("moof has no traf")?;
    let traf_end = traf_off + traf_h.size;
    let (trun_off, trun_h) = find_box(frag, traf_off + traf_h.header_size, traf_end, b"trun")
        .ok_or("traf has no trun")?;
    let trun = parse_trun(frag, trun_off, trun_h)?;
    if !trun.has_data_offset {
        return Err("trun has no data_offset (flag 0x1) — this demo expects ffmpeg default_base_moof fragments".into());
    }

    // Encrypt the mdat samples and collect per-sample IVs.
    let mdat_content_start = mdat_off + mdat_h.header_size;
    let mdat_content_end = mdat_off + mdat_h.size;
    let mut mdat = frag[mdat_content_start..mdat_content_end].to_vec();
    let mut ivs: Vec<[u8; 8]> = Vec::with_capacity(trun.sample_sizes.len());
    let mut pos = 0usize;
    for &sz in &trun.sample_sizes {
        let sz = sz as usize;
        if pos + sz > mdat.len() {
            return Err(format!(
                "sample overruns mdat: pos={pos} size={sz} mdat={}",
                mdat.len()
            ));
        }
        let iv8 = iv_counter.to_be_bytes();
        *iv_counter += 1;
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(&iv8);
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut mdat[pos..pos + sz]);
        ivs.push(iv8);
        pos += sz;
    }

    // Build the senc box: version(1)=0, flags(3)=0 (no subsamples), sample_count, IVs.
    let mut senc_content = vec![0u8, 0, 0, 0];
    senc_content.extend_from_slice(&(ivs.len() as u32).to_be_bytes());
    for iv in &ivs {
        senc_content.extend_from_slice(iv);
    }
    let senc = make_box(b"senc", &senc_content);
    let senc_len = senc.len();

    // Rebuild the traf content with senc appended, patching trun.data_offset (+senc_len).
    let traf_content_start = traf_off + traf_h.header_size;
    let mut new_traf_content = frag[traf_content_start..traf_end].to_vec();
    // data_offset is at trun_start + header_size + 8 (after version+flags+sample_count),
    // expressed relative to traf_content_start in the new buffer.
    let do_pos_in_traf = (trun_off + trun_h.header_size + 8) - traf_content_start;
    let old_off = i32::from_be_bytes([
        new_traf_content[do_pos_in_traf],
        new_traf_content[do_pos_in_traf + 1],
        new_traf_content[do_pos_in_traf + 2],
        new_traf_content[do_pos_in_traf + 3],
    ]);
    let new_off = old_off + senc_len as i32;
    new_traf_content[do_pos_in_traf..do_pos_in_traf + 4].copy_from_slice(&new_off.to_be_bytes());
    new_traf_content.extend_from_slice(&senc);
    let new_traf = make_box(b"traf", &new_traf_content);

    // Rebuild moof content: everything in moof content, with the old traf replaced.
    let moof_content_start = moof_off + moof_h.header_size;
    let mut new_moof_content = Vec::new();
    new_moof_content.extend_from_slice(&frag[moof_content_start..traf_off]);
    new_moof_content.extend_from_slice(&new_traf);
    new_moof_content.extend_from_slice(&frag[traf_end..moof_end]);
    let new_moof = make_box(b"moof", &new_moof_content);

    // Reassemble: new moof + (other boxes between moof and mdat, if any) + encrypted mdat.
    let mut out = Vec::with_capacity(frag.len() + senc_len);
    out.extend_from_slice(&new_moof);
    out.extend_from_slice(&frag[moof_end..mdat_off]); // usually empty
    // mdat header unchanged (same length), content now encrypted.
    out.extend_from_slice(&frag[mdat_off..mdat_content_start]);
    out.extend_from_slice(&mdat);
    out.extend_from_slice(&frag[mdat_content_end..]); // usually empty
    Ok(out)
}

/// Strip the `senc` box from a (decrypted) fragment, fixing `moof`/`traf` sizes and
/// `trun.data_offset` — the inverse of [`encrypt_fragment`]'s injection, so the
/// browser receives a byte-identical clean fragment. No-op if there is no senc.
pub fn strip_senc(frag: &[u8]) -> Result<Vec<u8>, String> {
    let boxes = top_level_boxes(frag)?;
    let (moof_off, moof_h) = match boxes.iter().copied().find(|(_, h)| &h.box_type == b"moof") {
        Some(v) => v,
        None => return Ok(frag.to_vec()),
    };
    let moof_end = moof_off + moof_h.size;
    let (traf_off, traf_h) = match find_box(frag, moof_off + moof_h.header_size, moof_end, b"traf") {
        Some(v) => v,
        None => return Ok(frag.to_vec()),
    };
    let traf_end = traf_off + traf_h.size;
    let (senc_off, senc_h) = match find_box(frag, traf_off + traf_h.header_size, traf_end, b"senc") {
        Some(v) => v,
        None => return Ok(frag.to_vec()),
    };
    let removed = senc_h.size;

    // Build output with the senc bytes removed.
    let mut out = Vec::with_capacity(frag.len() - removed);
    out.extend_from_slice(&frag[..senc_off]);
    out.extend_from_slice(&frag[senc_off + removed..]);

    // Fix moof size, traf size (32-bit assumed for fragment boxes).
    write_u32(&mut out, moof_off, (moof_h.size - removed) as u32);
    write_u32(&mut out, traf_off, (traf_h.size - removed) as u32);

    // Fix trun.data_offset (-removed). trun precedes the senc we removed, so its
    // position is unchanged in `out`.
    if let Some((trun_off, trun_h)) = find_box(&out, traf_off + traf_h.header_size, traf_off + (traf_h.size - removed), b"trun") {
        let c = trun_off + trun_h.header_size;
        let flags = u32::from_be_bytes([0, out[c + 1], out[c + 2], out[c + 3]]);
        if flags & 0x000001 != 0 {
            let do_pos = c + 8;
            let old = i32::from_be_bytes([out[do_pos], out[do_pos + 1], out[do_pos + 2], out[do_pos + 3]]);
            let new = old - removed as i32;
            out[do_pos..do_pos + 4].copy_from_slice(&new.to_be_bytes());
        }
    }
    Ok(out)
}

/// Wrap arbitrary bytes as a single-sample fragmented-MP4 fragment so a NON-MEDIA
/// Audio sample-entry 4CCs — everything else is treated as video. Used to pick the
/// protected entry type (`enca` vs `encv`) per ISO/IEC 23001-7 §4.
fn is_audio_fourcc(fourcc: &[u8; 4]) -> bool {
    matches!(
        fourcc,
        b"mp4a" | b"Opus" | b"opus" | b"fLaC" | b"flac" | b"ac-3" | b"ec-3" | b"enca"
    )
}

/// Rebuild `container` (located at `container_off`) with the single child at
/// `child_off` (of `child_old_size` bytes) replaced by `new_child`, fixing the
/// container's size. Assumes a 32-bit box header (true for moov/trak/.../stsd).
fn splice_child(
    data: &[u8],
    container_off: usize,
    container_h: BoxHeader,
    child_off: usize,
    child_old_size: usize,
    new_child: &[u8],
) -> Vec<u8> {
    let content_start = container_off + container_h.header_size;
    let content_end = container_off + container_h.size;
    let mut content = Vec::with_capacity(content_end - content_start + new_child.len());
    content.extend_from_slice(&data[content_start..child_off]);
    content.extend_from_slice(new_child);
    content.extend_from_slice(&data[child_off + child_old_size..content_end]);
    make_box(&container_h.box_type, &content)
}

/// Walk `data[moov_off..]` down `moov > trak > mdia > minf > stbl > stsd` and return
/// `(offset, header)` for each box on the path plus the first sample entry in `stsd`.
/// The producer emits one `trak` per standalone init (PC2 `demux_tracks`), so the
/// first `trak` is the track.
struct StsdPath {
    moov: (usize, BoxHeader),
    trak: (usize, BoxHeader),
    mdia: (usize, BoxHeader),
    minf: (usize, BoxHeader),
    stbl: (usize, BoxHeader),
    stsd: (usize, BoxHeader),
    entry: (usize, BoxHeader),
}

fn locate_stsd_path(init: &[u8]) -> Result<StsdPath, String> {
    let (moov_off, moov_h) = top_level_boxes(init)?
        .into_iter()
        .find(|(_, h)| &h.box_type == b"moov")
        .ok_or("init has no moov")?;
    let moov_end = moov_off + moov_h.size;
    let (trak_off, trak_h) = find_box(init, moov_off + moov_h.header_size, moov_end, b"trak")
        .ok_or("moov has no trak")?;
    let trak_end = trak_off + trak_h.size;
    let (mdia_off, mdia_h) = find_box(init, trak_off + trak_h.header_size, trak_end, b"mdia")
        .ok_or("trak has no mdia")?;
    let mdia_end = mdia_off + mdia_h.size;
    let (minf_off, minf_h) = find_box(init, mdia_off + mdia_h.header_size, mdia_end, b"minf")
        .ok_or("mdia has no minf")?;
    let minf_end = minf_off + minf_h.size;
    let (stbl_off, stbl_h) = find_box(init, minf_off + minf_h.header_size, minf_end, b"stbl")
        .ok_or("minf has no stbl")?;
    let stbl_end = stbl_off + stbl_h.size;
    let (stsd_off, stsd_h) = find_box(init, stbl_off + stbl_h.header_size, stbl_end, b"stsd")
        .ok_or("stbl has no stsd")?;
    let stsd_end = stsd_off + stsd_h.size;
    // stsd content: FullBox header (4) + entry_count (4) + entries.
    let entry_off = stsd_off + stsd_h.header_size + 8;
    let entry_h = read_box_header(init, entry_off).ok_or("stsd has no sample entry")?;
    if entry_h.size < 8 || entry_off + entry_h.size > stsd_end {
        return Err("sample entry overruns stsd".into());
    }
    Ok(StsdPath {
        moov: (moov_off, moov_h),
        trak: (trak_off, trak_h),
        mdia: (mdia_off, mdia_h),
        minf: (minf_off, minf_h),
        stbl: (stbl_off, stbl_h),
        stsd: (stsd_off, stsd_h),
        entry: (entry_off, entry_h),
    })
}

/// Build a CENC `sinf` (Protection Scheme Information) for `orig_fourcc`, declaring the
/// `cenc` scheme and a `tenc` (TrackEncryptionBox v0) with `default_kid` + `iv_size`-byte
/// per-sample IVs (full-sample CTR encryption, matching [`encrypt_fragment`]).
fn build_sinf(orig_fourcc: &[u8; 4], default_kid: &[u8; 16], iv_size: u8) -> Vec<u8> {
    let frma = make_box(b"frma", orig_fourcc);
    // schm (FullBox v0, flags 0): scheme_type 'cenc', scheme_version 0x00010000.
    let mut schm = vec![0u8, 0, 0, 0];
    schm.extend_from_slice(b"cenc");
    schm.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let schm = make_box(b"schm", &schm);
    // tenc (FullBox v0): reserved, reserved(v0), default_isProtected=1, IV size, default_KID.
    let mut tenc = vec![0u8, 0, 0, 0];
    tenc.push(0); // reserved
    tenc.push(0); // reserved (v0)
    tenc.push(1); // default_isProtected
    tenc.push(iv_size); // default_Per_Sample_IV_Size
    tenc.extend_from_slice(default_kid);
    let schi = make_box(b"schi", &make_box(b"tenc", &tenc));
    let mut sinf = Vec::new();
    sinf.extend_from_slice(&frma);
    sinf.extend_from_slice(&schm);
    sinf.extend_from_slice(&schi);
    make_box(b"sinf", &sinf)
}

/// CENC-signal a (single-track) fMP4 **init** segment for MPEG-DASH/CENC (ISO/IEC
/// 23001-7) compliance: wrap the sample entry as `encv`/`enca` carrying a `sinf`
/// (`frma` + `schm` 'cenc' + `tenc` with `default_kid`), and inject `pssh_box` as a
/// child of `moov`. `iv_size` MUST match the per-sample IV size the fragments use
/// (8 here, see [`encrypt_fragment`]). The inverse is [`strip_cenc_signal`].
///
/// This is the producer half of the "one compliant asset" model: the published init
/// is standards-compliant (a stock CENC player / FFmpeg keys decryption off `tenc`),
/// and the server-side decrypt rail calls [`strip_cenc_signal`] to hand its own
/// player an unencrypted-looking init.
pub fn cenc_signal_init(
    init: &[u8],
    default_kid: &[u8; 16],
    iv_size: u8,
    pssh_box: &[u8],
) -> Result<Vec<u8>, String> {
    let p = locate_stsd_path(init)?;
    let (entry_off, entry_h) = p.entry;
    let orig_fourcc = entry_h.box_type;
    if &orig_fourcc == b"encv" || &orig_fourcc == b"enca" {
        return Err("init is already CENC-signaled (encv/enca present)".into());
    }
    let prot_fourcc: &[u8; 4] = if is_audio_fourcc(&orig_fourcc) {
        b"enca"
    } else {
        b"encv"
    };

    // New sample entry: same fixed fields + child boxes, type swapped, sinf appended.
    let sinf = build_sinf(&orig_fourcc, default_kid, iv_size);
    let entry_content_start = entry_off + entry_h.header_size;
    let entry_content_end = entry_off + entry_h.size;
    let mut new_entry_content = init[entry_content_start..entry_content_end].to_vec();
    new_entry_content.extend_from_slice(&sinf);
    let new_entry = make_box(prot_fourcc, &new_entry_content);

    // Rebuild the path bottom-up (stsd entry -> stsd -> stbl -> minf -> mdia -> trak).
    let (stsd_off, stsd_h) = p.stsd;
    let new_stsd = splice_child(init, stsd_off, stsd_h, entry_off, entry_h.size, &new_entry);
    let (stbl_off, stbl_h) = p.stbl;
    let new_stbl = splice_child(init, stbl_off, stbl_h, stsd_off, stsd_h.size, &new_stsd);
    let (minf_off, minf_h) = p.minf;
    let new_minf = splice_child(init, minf_off, minf_h, stbl_off, stbl_h.size, &new_stbl);
    let (mdia_off, mdia_h) = p.mdia;
    let new_mdia = splice_child(init, mdia_off, mdia_h, minf_off, minf_h.size, &new_minf);
    let (trak_off, trak_h) = p.trak;
    let new_trak = splice_child(init, trak_off, trak_h, mdia_off, mdia_h.size, &new_mdia);

    // Rebuild moov: replace the trak and append the pssh box as a moov child.
    let (moov_off, moov_h) = p.moov;
    let moov_end = moov_off + moov_h.size;
    let moov_content_start = moov_off + moov_h.header_size;
    let mut new_moov_content = Vec::new();
    new_moov_content.extend_from_slice(&init[moov_content_start..trak_off]);
    new_moov_content.extend_from_slice(&new_trak);
    new_moov_content.extend_from_slice(&init[trak_off + trak_h.size..moov_end]);
    new_moov_content.extend_from_slice(pssh_box);
    let new_moov = make_box(b"moov", &new_moov_content);

    let mut out = Vec::with_capacity(init.len() + sinf.len() + pssh_box.len() + 16);
    out.extend_from_slice(&init[..moov_off]);
    out.extend_from_slice(&new_moov);
    out.extend_from_slice(&init[moov_end..]);
    Ok(out)
}

/// Inverse of [`cenc_signal_init`]: restore the original `avc1`/`mp4a`/... sample entry
/// (from the `sinf`'s `frma`), drop the `sinf`, and remove every `pssh` child from
/// `moov` — yielding the "unencrypted-looking" init the server-side decrypt rail serves
/// its own player. No-op-ish if the init is not CENC-signaled (returns it unchanged).
pub fn strip_cenc_signal(init: &[u8]) -> Result<Vec<u8>, String> {
    let p = locate_stsd_path(init)?;
    let (entry_off, entry_h) = p.entry;
    if &entry_h.box_type != b"encv" && &entry_h.box_type != b"enca" {
        return Ok(init.to_vec()); // not CENC-signaled
    }
    let entry_content_start = entry_off + entry_h.header_size;
    let entry_content_end = entry_off + entry_h.size;
    // Child boxes start after the fixed sample-entry fields (both include the 8-byte
    // SampleEntry preamble); the protected entry keeps the original entry's field layout.
    let fixed = if &entry_h.box_type == b"encv" {
        VISUAL_SAMPLE_ENTRY_FIXED_BYTES
    } else {
        AUDIO_SAMPLE_ENTRY_FIXED_BYTES
    };
    let children_start = entry_content_start + fixed;
    if children_start > entry_content_end {
        return Err("protected sample entry shorter than its fixed fields".into());
    }
    // Walk the entry's child boxes; pull the original 4CC out of sinf>frma and drop sinf.
    let mut orig_fourcc: Option<[u8; 4]> = None;
    let mut kept_children = Vec::new();
    let mut off = children_start;
    while off + 8 <= entry_content_end {
        let h = read_box_header(init, off).ok_or("malformed child box in sample entry")?;
        if h.size < 8 || off + h.size > entry_content_end {
            return Err("child box overruns sample entry".into());
        }
        if &h.box_type == b"sinf" {
            let sinf_end = off + h.size;
            let (frma_off, frma_h) = find_box(init, off + h.header_size, sinf_end, b"frma")
                .ok_or("sinf has no frma")?;
            let fc = init
                .get(frma_off + frma_h.header_size..frma_off + frma_h.header_size + 4)
                .ok_or("frma too short")?;
            orig_fourcc = Some([fc[0], fc[1], fc[2], fc[3]]);
        } else {
            kept_children.extend_from_slice(&init[off..off + h.size]);
        }
        off += h.size;
    }
    let orig_fourcc = orig_fourcc.ok_or("CENC-signaled entry has no sinf/frma")?;

    let mut new_entry_content = init[entry_content_start..children_start].to_vec();
    new_entry_content.extend_from_slice(&kept_children);
    let new_entry = make_box(&orig_fourcc, &new_entry_content);

    let (stsd_off, stsd_h) = p.stsd;
    let new_stsd = splice_child(init, stsd_off, stsd_h, entry_off, entry_h.size, &new_entry);
    let (stbl_off, stbl_h) = p.stbl;
    let new_stbl = splice_child(init, stbl_off, stbl_h, stsd_off, stsd_h.size, &new_stsd);
    let (minf_off, minf_h) = p.minf;
    let new_minf = splice_child(init, minf_off, minf_h, stbl_off, stbl_h.size, &new_stbl);
    let (mdia_off, mdia_h) = p.mdia;
    let new_mdia = splice_child(init, mdia_off, mdia_h, minf_off, minf_h.size, &new_minf);
    let (trak_off, trak_h) = p.trak;
    let new_trak = splice_child(init, trak_off, trak_h, mdia_off, mdia_h.size, &new_mdia);

    // Rebuild moov: replace the trak, dropping any pssh children.
    let (moov_off, moov_h) = p.moov;
    let moov_end = moov_off + moov_h.size;
    let moov_content_start = moov_off + moov_h.header_size;
    let mut new_moov_content = Vec::new();
    let mut moff = moov_content_start;
    while moff + 8 <= moov_end {
        let h = read_box_header(init, moff).ok_or("malformed moov child")?;
        if h.size < 8 || moff + h.size > moov_end {
            return Err("moov child overruns moov".into());
        }
        if moff == trak_off {
            new_moov_content.extend_from_slice(&new_trak);
        } else if &h.box_type != b"pssh" {
            new_moov_content.extend_from_slice(&init[moff..moff + h.size]);
        }
        moff += h.size;
    }
    let new_moov = make_box(b"moov", &new_moov_content);

    let mut out = Vec::with_capacity(init.len());
    out.extend_from_slice(&init[..moov_off]);
    out.extend_from_slice(&new_moov);
    out.extend_from_slice(&init[moov_end..]);
    Ok(out)
}

/// object can ride the exact same CENC rail as media. The blob becomes one sample
/// inside `mdat`; the synthesized `moof/traf/trun` carry the one flag set
/// [`encrypt_fragment`] requires: `data_offset` (0x1) + per-sample `size` (0x200),
/// `sample_count = 1`, `size = blob.len()`. [`encrypt_fragment`] then CTR-encrypts
/// the whole blob under one IV and injects `senc`; the unchanged decrypt boundary
/// reverses it, and [`extract_mdat`] recovers the original bytes.
pub fn wrap_blob_as_fragment(blob: &[u8]) -> Vec<u8> {
    // trun content: version(0)+flags(0x000201), sample_count(1), data_offset(0),
    // sample[0].size(blob.len()).
    let mut trun_content = Vec::with_capacity(16);
    trun_content.extend_from_slice(&[0x00, 0x00, 0x02, 0x01]); // version 0, flags 0x000201
    trun_content.extend_from_slice(&1u32.to_be_bytes()); // sample_count
    trun_content.extend_from_slice(&0i32.to_be_bytes()); // data_offset (patched on encrypt)
    trun_content.extend_from_slice(&(blob.len() as u32).to_be_bytes()); // sample[0] size
    let trun = make_box(b"trun", &trun_content);
    let traf = make_box(b"traf", &trun);
    let moof = make_box(b"moof", &traf);
    let mdat = make_box(b"mdat", blob);

    let mut out = Vec::with_capacity(moof.len() + mdat.len());
    out.extend_from_slice(&moof);
    out.extend_from_slice(&mdat);
    out
}

/// Extract the `mdat` payload from a fragment — the inverse of the container that
/// [`wrap_blob_as_fragment`] built. Used after the decrypt boundary returns the
/// (senc-stripped) cleartext fragment, to recover the original NON-MEDIA bytes.
pub fn extract_mdat(frag: &[u8]) -> Result<Vec<u8>, String> {
    let boxes = top_level_boxes(frag)?;
    let (mdat_off, mdat_h) = boxes
        .iter()
        .copied()
        .find(|(_, h)| &h.box_type == b"mdat")
        .ok_or("decrypted object fragment has no mdat")?;
    let start = mdat_off + mdat_h.header_size;
    let end = mdat_off + mdat_h.size;
    Ok(frag[start..end].to_vec())
}

/// The ISO-BMFF `free`-box marker used by [`embed_placeholder_variant`] — a domain tag the reader
/// matches so an unrelated `free` box is never mistaken for a variant marker.
const AV_PLACEHOLDER_MARK: &[u8] = b"elastos-av-variant/v1";

/// Append an ISO-BMFF `free` box carrying a single variant `symbol` to a (plaintext) media
/// fragment. `free` boxes are defined as ignorable free space, so the result is still a valid,
/// playable `moof`+`mdat` fragment the browser decodes unchanged — but its bytes differ per symbol,
/// which is all the chunk-3/4/5 *routing + weld* needs: distinct, attributable served bytes welded
/// into the transcript AAD. Because the box is appended AFTER the `mdat`, [`encrypt_fragment`] (CENC)
/// and [`strip_senc`] (decrypt) both carry it through verbatim, so the selected variant stays
/// byte-distinct end-to-end and the marker survives back to the clean fragment.
///
/// **THIS IS A BOUNDED PLACEHOLDER, NOT A WATERMARK.** It carries no perceptual signal and does NOT
/// survive transcode / re-encode / screen-capture. Its only job is to make the mint→serve→select→weld
/// pipeline real and testable. The certified per-variant DSP embed (which modifies media *samples*)
/// swaps in behind this same `(fragment, symbol) -> fragment` interface ONLY after media-survival
/// certification — see `docs/AV_WATERMARKING.md`.
pub fn embed_placeholder_variant(fragment: &[u8], symbol: u8) -> Vec<u8> {
    let payload_len = AV_PLACEHOLDER_MARK.len() + 1;
    let box_size = 8 + payload_len;
    let mut out = Vec::with_capacity(fragment.len() + box_size);
    out.extend_from_slice(fragment);
    out.extend_from_slice(&(box_size as u32).to_be_bytes());
    out.extend_from_slice(b"free");
    out.extend_from_slice(AV_PLACEHOLDER_MARK);
    out.push(symbol);
    out
}

/// Recover the variant `symbol` written by [`embed_placeholder_variant`], if present — the trailing
/// `free` box whose payload starts with [`AV_PLACEHOLDER_MARK`]. Returns `None` for an unmarked
/// (single-encode) fragment. This is the placeholder's stand-in for the offline forensic extractor:
/// it confirms WHICH variant was served end-to-end. The real extractor (`tools/av-forensics`)
/// recovers the symbol from the media samples instead, once the certified DSP embed replaces this.
pub fn read_placeholder_variant(fragment: &[u8]) -> Option<u8> {
    let boxes = top_level_boxes(fragment).ok()?;
    let (off, h) = boxes.iter().copied().rev().find(|(_, h)| &h.box_type == b"free")?;
    let content_start = off + h.header_size;
    let content_end = off + h.size;
    let content = fragment.get(content_start..content_end)?;
    let want = AV_PLACEHOLDER_MARK.len() + 1;
    if content.len() != want || !content.starts_with(AV_PLACEHOLDER_MARK) {
        return None;
    }
    content.last().copied()
}

/// Read the avc1 `codecs` string (e.g. `avc1.42E01E`) from the init segment by
/// finding the `avcC` box and reading profile/constraints/level. Falls back to a
/// safe baseline string if not found.
pub fn avc_codec_string(init: &[u8]) -> String {
    // avcC content: configurationVersion(1) AVCProfileIndication(1)
    //               profile_compatibility(1) AVCLevelIndication(1) ...
    if let Some(pos) = find_bytes(init, b"avcC") {
        let c = pos + 4; // box content starts right after the 4-byte type (size precedes type)
        if c + 4 <= init.len() {
            let profile = init[c + 1];
            let compat = init[c + 2];
            let level = init[c + 3];
            return format!("avc1.{:02X}{:02X}{:02X}", profile, compat, level);
        }
    }
    "avc1.42E01E".to_string()
}

fn find_bytes(buf: &[u8], needle: &[u8; 4]) -> Option<usize> {
    buf.windows(4).position(|w| w == needle)
}

// ---------------------------------------------------------------------------
// fMP4 metadata extraction — a faithful port of PC2
// `pc2-node/src/services/media/mp4split.ts` (`extractTrackInfo` / `parseTrak` /
// `parseCodecString` / `parseMoofTrackId` / `parseMoofDuration` + the
// `splitFragmentedMP4` driver). Produces the exact [`TrackInfo`]/[`SegmentInfo`]
// the [`crate::mpd`] generator consumes, so the emitted MPD matches PC2's.
//
// Visual/audio SampleEntry fixed-byte prefixes (ISO/IEC 14496-12 §8.5.2), copied
// from PC2's constants so codec-string offsets line up byte-for-byte.
// ---------------------------------------------------------------------------
const VISUAL_SAMPLE_ENTRY_FIXED_BYTES: usize = 78;
const AUDIO_SAMPLE_ENTRY_FIXED_BYTES: usize = 28;

/// The result of parsing a fragmented MP4's structure: tracks (with bandwidth
/// computed), the ordered media segments, and the presentation duration (seconds).
#[derive(Debug, Clone)]
pub struct FragmentMetadata {
    pub tracks: Vec<TrackInfo>,
    pub segments: Vec<SegmentInfo>,
    pub total_duration: f64,
}

fn read_u32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u16(d: &[u8], at: usize) -> Option<u16> {
    d.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}

/// `splitFragmentedMP4` (metadata half): walk the boxes, extract tracks from
/// `moov`, accumulate per-segment timing from each `moof`, then compute each
/// track's bandwidth and the overall presentation duration exactly like PC2.
pub fn parse_fragment_metadata(data: &[u8]) -> Result<FragmentMetadata, String> {
    let mut tracks: Vec<TrackInfo> = Vec::new();
    let mut segments: Vec<SegmentInfo> = Vec::new();
    // Parallel to `segments`: each segment's byte length, for bandwidth math.
    let mut seg_bytes: Vec<(u32, u64)> = Vec::new();

    let mut pos = 0usize;
    while pos < data.len() {
        let Some(h) = read_box_header(data, pos) else { break };
        if h.size < 8 || pos + h.size > data.len() {
            break;
        }
        match &h.box_type {
            b"ftyp" | b"moov" | b"free" | b"skip" => {
                if &h.box_type == b"moov" {
                    tracks = extract_track_info(data, pos, h.size);
                }
                pos += h.size;
            }
            b"moof" => {
                let moof_end = pos + h.size;
                let next = read_box_header(data, moof_end);
                let seg_end = match next {
                    Some(n) if &n.box_type == b"mdat" => moof_end + n.size,
                    _ => moof_end,
                };
                let content_start = pos + h.header_size;
                let track_id = parse_moof_track_id(data, content_start, moof_end);
                let (duration, _sample_count) = parse_moof_duration(data, content_start, moof_end);
                segments.push(SegmentInfo { track_id, duration });
                seg_bytes.push((track_id, (seg_end - pos) as u64));
                pos = seg_end;
            }
            _ => pos += h.size,
        }
    }

    // Per-track byte + duration totals → bandwidth = round(bytes*8*timescale/dur).
    for track in tracks.iter_mut() {
        let total_bytes: u64 = seg_bytes
            .iter()
            .filter(|(tid, _)| *tid == track.track_id)
            .map(|(_, b)| *b)
            .sum();
        let total_dur: u64 = segments
            .iter()
            .filter(|s| s.track_id == track.track_id)
            .map(|s| s.duration)
            .sum();
        if total_dur > 0 {
            let bw = (total_bytes as f64 * 8.0 * track.timescale as f64) / total_dur as f64;
            track.bandwidth = bw.round() as u64;
        }
    }

    // Presentation duration follows the primary (video, else first) track.
    let mut total_duration = 0.0f64;
    let primary = tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .or_else(|| tracks.first());
    if let Some(p) = primary {
        let dur: u64 = segments
            .iter()
            .filter(|s| s.track_id == p.track_id)
            .map(|s| s.duration)
            .sum();
        if p.timescale > 0 {
            total_duration = dur as f64 / p.timescale as f64;
        }
    }

    Ok(FragmentMetadata {
        tracks,
        segments,
        total_duration,
    })
}

/// `extractTrackInfo` — parse every `trak` under a `moov`.
fn extract_track_info(data: &[u8], moov_off: usize, moov_size: usize) -> Vec<TrackInfo> {
    let mut tracks = Vec::new();
    let moov_end = moov_off + moov_size;
    let mut pos = moov_off + 8;
    while pos < moov_end {
        let Some(h) = read_box_header(data, pos) else { break };
        if h.size < 8 || pos + h.size > moov_end {
            break;
        }
        if &h.box_type == b"trak" {
            if let Some(track) = parse_trak(data, pos + h.header_size, pos + h.size) {
                tracks.push(track);
            }
        }
        pos += h.size;
    }
    tracks
}

/// `parseTrak` — `tkhd` (id/dims), `mdhd` (timescale), `hdlr` (kind), `stsd`
/// (codec + audio params). Version-dependent offsets mirror PC2 exactly.
fn parse_trak(data: &[u8], start: usize, end: usize) -> Option<TrackInfo> {
    let (tkhd_off, tkhd_h) = find_box(data, start, end, b"tkhd")?;
    let tkhd_content = tkhd_off + tkhd_h.header_size;
    let version = *data.get(tkhd_content)?;
    let (track_id, width, height) = if version == 1 {
        (
            read_u32(data, tkhd_content + 20)?,
            read_u32(data, tkhd_content + 84)? >> 16,
            read_u32(data, tkhd_content + 88)? >> 16,
        )
    } else {
        (
            read_u32(data, tkhd_content + 12)?,
            read_u32(data, tkhd_content + 76)? >> 16,
            read_u32(data, tkhd_content + 80)? >> 16,
        )
    };

    let (mdia_off, mdia_h) = find_box(data, start, end, b"mdia")?;
    let mdia_content = mdia_off + mdia_h.header_size;
    let mdia_end = mdia_off + mdia_h.size;

    let mut timescale = 90000u32;
    if let Some((mdhd_off, mdhd_h)) = find_box(data, mdia_content, mdia_end, b"mdhd") {
        let mdhd_content = mdhd_off + mdhd_h.header_size;
        let mdhd_version = *data.get(mdhd_content)?;
        timescale = if mdhd_version == 1 {
            read_u32(data, mdhd_content + 20)?
        } else {
            read_u32(data, mdhd_content + 12)?
        };
    }

    let mut handler_type = *b"vide";
    if let Some((hdlr_off, hdlr_h)) = find_box(data, mdia_content, mdia_end, b"hdlr") {
        let h_at = hdlr_off + hdlr_h.header_size + 8;
        if let Some(slice) = data.get(h_at..h_at + 4) {
            handler_type.copy_from_slice(slice);
        }
    }
    let kind = match &handler_type {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        _ => return None,
    };

    let (minf_off, minf_h) = find_box(data, mdia_content, mdia_end, b"minf")?;
    let (stbl_off, stbl_h) = find_box(
        data,
        minf_off + minf_h.header_size,
        minf_off + minf_h.size,
        b"stbl",
    )?;
    let stsd = find_box(
        data,
        stbl_off + stbl_h.header_size,
        stbl_off + stbl_h.size,
        b"stsd",
    );

    let mut codec = "unknown".to_string();
    let mut audio_sample_rate: Option<u32> = None;
    let mut audio_channels: Option<u32> = None;
    if let Some((stsd_off, stsd_h)) = stsd {
        codec = parse_codec_string(data, stsd_off, stsd_h.size);
        if kind == TrackKind::Audio {
            let entry_start = stsd_off + 16;
            if let Some(entry_box) = read_box_header(data, entry_start) {
                let base = entry_start + entry_box.header_size;
                audio_channels = read_u16(data, base + 16).map(u32::from);
                audio_sample_rate = read_u32(data, base + 24).map(|v| v >> 16);
            }
        }
    }

    let (width, height) = if kind == TrackKind::Video && width > 0 {
        (Some(width), Some(height))
    } else {
        (None, None)
    };
    // PC2 only records audio params when a sample rate was found.
    let (audio_sample_rate, audio_channels) = match audio_sample_rate {
        Some(rate) => (Some(rate), audio_channels),
        None => (None, None),
    };

    Some(TrackInfo {
        track_id,
        kind,
        codec,
        timescale,
        width,
        height,
        bandwidth: 0,
        audio_sample_rate,
        audio_channels,
    })
}

/// `parseCodecString` — emit MSE-valid codec strings (`avc1.640028`, `mp4a.40.2`,
/// `av01.0.05M.08`, …) from the first sample entry, byte-faithful to PC2.
fn parse_codec_string(data: &[u8], stsd_off: usize, stsd_size: usize) -> String {
    let content_start = stsd_off + 16;
    if content_start >= stsd_off + stsd_size {
        return "unknown".to_string();
    }
    let Some(entry) = read_box_header(data, content_start) else {
        return "unknown".to_string();
    };
    let fourcc = entry.box_type;
    let is_audio_entry = matches!(&fourcc, b"mp4a" | b"Opus" | b"fLaC" | b"enca");
    let child_start = content_start
        + entry.header_size
        + if is_audio_entry {
            AUDIO_SAMPLE_ENTRY_FIXED_BYTES
        } else {
            VISUAL_SAMPLE_ENTRY_FIXED_BYTES
        };
    let child_end = content_start + entry.size;

    match &fourcc {
        b"avc1" | b"avc3" => {
            if let Some((avc_c_off, avc_c_h)) = find_box(data, child_start, child_end, b"avcC") {
                let p = avc_c_off + avc_c_h.header_size;
                if let (Some(&profile), Some(&compat), Some(&level)) =
                    (data.get(p + 1), data.get(p + 2), data.get(p + 3))
                {
                    return format!("avc1.{profile:02x}{compat:02x}{level:02x}");
                }
            }
            String::from_utf8_lossy(&fourcc).into_owned()
        }
        b"hev1" | b"hvc1" => String::from_utf8_lossy(&fourcc).into_owned(),
        b"av01" => {
            if let Some((av1_c_off, av1_c_h)) = find_box(data, child_start, child_end, b"av1C") {
                let p = av1_c_off + av1_c_h.header_size;
                if let (Some(&b1), Some(&b2)) = (data.get(p + 1), data.get(p + 2)) {
                    let profile = (b1 >> 5) & 0x7;
                    let level = b1 & 0x1f;
                    let tier = (b2 >> 7) & 0x1;
                    let high_bitdepth = (b2 >> 6) & 0x1;
                    let twelve_bit = (b2 >> 5) & 0x1;
                    let bit_depth = if twelve_bit == 1 {
                        12
                    } else if high_bitdepth == 1 {
                        10
                    } else {
                        8
                    };
                    let tier_ch = if tier == 1 { 'H' } else { 'M' };
                    return format!("av01.{profile}.{level:02}{tier_ch}.{bit_depth:02}");
                }
            }
            "av01.0.01M.08".to_string()
        }
        b"mp4a" => "mp4a.40.2".to_string(),
        b"Opus" => "opus".to_string(),
        b"fLaC" => "flac".to_string(),
        _ => String::from_utf8_lossy(&fourcc).into_owned(),
    }
}

/// `parseMoofTrackId` — the `tfhd.track_ID`.
fn parse_moof_track_id(data: &[u8], moof_content_start: usize, moof_end: usize) -> u32 {
    let Some((traf_off, traf_h)) = find_box(data, moof_content_start, moof_end, b"traf") else {
        return 0;
    };
    let Some((tfhd_off, tfhd_h)) =
        find_box(data, traf_off + traf_h.header_size, traf_off + traf_h.size, b"tfhd")
    else {
        return 0;
    };
    read_u32(data, tfhd_off + tfhd_h.header_size + 4).unwrap_or(0)
}

/// `parseMoofDuration` — sum the `trun` per-sample durations (falling back to the
/// `tfhd` default_sample_duration), returning `(duration, sample_count)`.
fn parse_moof_duration(data: &[u8], moof_content_start: usize, moof_end: usize) -> (u64, u32) {
    let Some((traf_off, traf_h)) = find_box(data, moof_content_start, moof_end, b"traf") else {
        return (0, 0);
    };
    let traf_inner = traf_off + traf_h.header_size;
    let traf_box_end = traf_off + traf_h.size;

    let mut default_duration = 0u32;
    if let Some((tfhd_off, tfhd_h)) = find_box(data, traf_inner, traf_box_end, b"tfhd") {
        let tfhd_content = tfhd_off + tfhd_h.header_size;
        if let Some(flags) = read_u32(data, tfhd_content) {
            let flags = flags & 0x00FF_FFFF;
            let mut o = tfhd_content + 8;
            if flags & 0x1 != 0 {
                o += 8;
            }
            if flags & 0x2 != 0 {
                o += 4;
            }
            if flags & 0x8 != 0 {
                default_duration = read_u32(data, o).unwrap_or(0);
            }
        }
    }

    let Some((trun_off, trun_h)) = find_box(data, traf_inner, traf_box_end, b"trun") else {
        return (0, 0);
    };
    let trun_content = trun_off + trun_h.header_size;
    let Some(flags) = read_u32(data, trun_content) else {
        return (0, 0);
    };
    let flags = flags & 0x00FF_FFFF;
    let sample_count = read_u32(data, trun_content + 4).unwrap_or(0);

    let has_duration = flags & 0x100 != 0;
    let has_size = flags & 0x200 != 0;
    let has_flags = flags & 0x400 != 0;
    let has_cto = flags & 0x800 != 0;

    let mut offset = trun_content + 8;
    if flags & 0x1 != 0 {
        offset += 4;
    }
    if flags & 0x4 != 0 {
        offset += 4;
    }
    let entry_size = (has_duration as usize + has_size as usize + has_flags as usize + has_cto as usize) * 4;

    let mut total_duration = 0u64;
    for _ in 0..sample_count {
        if has_duration {
            total_duration += read_u32(data, offset).unwrap_or(default_duration) as u64;
        } else {
            total_duration += default_duration as u64;
        }
        offset += entry_size;
    }

    (total_duration, sample_count)
}

/// A single demuxed track ready for the DASH directory layout: its descriptor, a
/// STANDALONE init segment (`ftyp` + a `moov` carrying ONLY this `trak` + its `trex`),
/// and the ordered PLAINTEXT media fragments (`moof`+`mdat`) belonging to this track.
#[derive(Debug, Clone)]
pub struct TrackStream {
    pub info: TrackInfo,
    pub init: Vec<u8>,
    pub segments: Vec<Vec<u8>>,
}

/// Demux a fragmented MP4 into PER-TRACK streams — the runtime analogue of PC2
/// `mp4split`'s track separation. Each stream gets its OWN init (a `moov` reduced to a
/// single `trak` + matching `trex`) and the fragments whose `moof.tfhd.track_ID` matches.
///
/// This is what separate DASH video/audio `AdaptationSet`s require: a player attaches
/// each `Representation`'s init to its own MSE `SourceBuffer`, so a combined (multi-`trak`)
/// init would mis-initialize the buffer. The fragments are returned UNENCRYPTED — CENC +
/// dKMS escrow remain the encrypt-provider's job (PRINCIPLE #15).
pub fn demux_tracks(data: &[u8]) -> Result<Vec<TrackStream>, String> {
    let boxes = top_level_boxes(data)?;
    let (moov_off, moov_h) = boxes
        .iter()
        .copied()
        .find(|(_, h)| &h.box_type == b"moov")
        .ok_or("no moov box — input is not an initialized fragmented MP4")?;
    let ftyp_bytes: Vec<u8> = match boxes.iter().copied().find(|(_, h)| &h.box_type == b"ftyp") {
        Some((off, h)) => data[off..off + h.size].to_vec(),
        None => Vec::new(),
    };

    let tracks = extract_track_info(data, moov_off, moov_h.size);
    if tracks.is_empty() {
        return Err("moov has no video/audio tracks".into());
    }

    // Collect the flat media fragments once, tagged by their moof's track_id.
    let mut frags: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut i = 0usize;
    while i < boxes.len() {
        let (off, h) = boxes[i];
        if &h.box_type == b"moof" {
            let moof_end = off + h.size;
            let Some((mdat_off, mdat_h)) = boxes.get(i + 1).copied() else {
                return Err(format!("moof at {off} has no following box"));
            };
            if &mdat_h.box_type != b"mdat" {
                return Err(format!(
                    "expected mdat after moof at {off}, found {}",
                    String::from_utf8_lossy(&mdat_h.box_type)
                ));
            }
            let end = mdat_off + mdat_h.size;
            let tid = parse_moof_track_id(data, off + h.header_size, moof_end);
            frags.push((tid, data[off..end].to_vec()));
            i += 2;
        } else {
            i += 1;
        }
    }

    let mut out: Vec<TrackStream> = Vec::with_capacity(tracks.len());
    for info in &tracks {
        let moov = build_per_track_moov(data, moov_off, moov_h, info.track_id);
        let mut init = ftyp_bytes.clone();
        init.extend_from_slice(&moov);
        let segments: Vec<Vec<u8>> = frags
            .iter()
            .filter(|(tid, _)| *tid == info.track_id)
            .map(|(_, f)| f.clone())
            .collect();

        // Per-track bandwidth = round(bytes·8·timescale / duration), over THIS track's
        // fragments (the same formula parse_fragment_metadata uses, scoped to the track).
        let mut stream = TrackStream {
            info: info.clone(),
            init,
            segments,
        };
        let timescale = stream.info.timescale.max(1) as f64;
        let total_bytes: u64 = stream.segments.iter().map(|f| f.len() as u64).sum();
        let total_dur: u64 = stream
            .segments
            .iter()
            .map(|f| match read_box_header(f, 0) {
                Some(h) => parse_moof_duration(f, h.header_size, h.size).0,
                None => 0,
            })
            .sum();
        if total_dur > 0 {
            stream.info.bandwidth =
                ((total_bytes as f64 * 8.0 * timescale) / total_dur as f64).round() as u64;
        }
        out.push(stream);
    }
    Ok(out)
}

/// Rebuild a `moov` carrying only the `trak` for `target_id` (+ its `trex`), so the
/// resulting init initializes a single-track MSE SourceBuffer. `mvhd` and any other
/// moov-level boxes are carried through unchanged; `mvex` is filtered to the matching
/// `trex`; non-matching `trak`s are dropped.
fn build_per_track_moov(data: &[u8], moov_off: usize, moov_h: BoxHeader, target_id: u32) -> Vec<u8> {
    let moov_end = moov_off + moov_h.size;
    let mut content: Vec<u8> = Vec::new();
    let mut pos = moov_off + moov_h.header_size;
    while pos < moov_end {
        let Some(h) = read_box_header(data, pos) else { break };
        if h.size < 8 || pos + h.size > moov_end {
            break;
        }
        let child = &data[pos..pos + h.size];
        match &h.box_type {
            b"trak" => {
                if tkhd_track_id(data, pos + h.header_size, pos + h.size) == Some(target_id) {
                    content.extend_from_slice(child);
                }
            }
            b"mvex" => content.extend_from_slice(&build_per_track_mvex(data, pos, h, target_id)),
            // mvhd / iods / udta / etc. — carried through unchanged.
            _ => content.extend_from_slice(child),
        }
        pos += h.size;
    }
    make_box(b"moov", &content)
}

/// Rebuild an `mvex` keeping only the `trex` for `target_id` (plus any non-`trex`
/// children such as `mehd`).
fn build_per_track_mvex(data: &[u8], mvex_off: usize, mvex_h: BoxHeader, target_id: u32) -> Vec<u8> {
    let mvex_end = mvex_off + mvex_h.size;
    let mut content: Vec<u8> = Vec::new();
    let mut pos = mvex_off + mvex_h.header_size;
    while pos < mvex_end {
        let Some(h) = read_box_header(data, pos) else { break };
        if h.size < 8 || pos + h.size > mvex_end {
            break;
        }
        let child = &data[pos..pos + h.size];
        if &h.box_type == b"trex" {
            // trex: fullbox (version+flags, 4 bytes) then track_ID (4 bytes).
            if read_u32(data, pos + h.header_size + 4) == Some(target_id) {
                content.extend_from_slice(child);
            }
        } else {
            content.extend_from_slice(child);
        }
        pos += h.size;
    }
    make_box(b"mvex", &content)
}

/// `tkhd.track_ID` for a `trak` spanning `[start, end)` (version-dependent offset).
fn tkhd_track_id(data: &[u8], trak_start: usize, trak_end: usize) -> Option<u32> {
    let (tkhd_off, tkhd_h) = find_box(data, trak_start, trak_end, b"tkhd")?;
    let content = tkhd_off + tkhd_h.header_size;
    let version = *data.get(content)?;
    if version == 1 {
        read_u32(data, content + 20)
    } else {
        read_u32(data, content + 12)
    }
}

#[cfg(test)]
mod meta_tests {
    use super::*;

    /// stsd → avc1 sample entry → avcC(profile=0x64,compat=0x00,level=0x28).
    #[test]
    fn parse_codec_string_avc1() {
        let avc_c = make_box(b"avcC", &[0x01, 0x64, 0x00, 0x28]);
        let mut avc1_content = vec![0u8; VISUAL_SAMPLE_ENTRY_FIXED_BYTES];
        avc1_content.extend_from_slice(&avc_c);
        let avc1 = make_box(b"avc1", &avc1_content);

        let mut stsd_content = vec![0, 0, 0, 0, 0, 0, 0, 1]; // version/flags + entry_count=1
        stsd_content.extend_from_slice(&avc1);
        let stsd = make_box(b"stsd", &stsd_content);

        assert_eq!(parse_codec_string(&stsd, 0, stsd.len()), "avc1.640028");
    }

    #[test]
    fn parse_codec_string_mp4a() {
        let mut entry_content = vec![0u8; AUDIO_SAMPLE_ENTRY_FIXED_BYTES];
        entry_content.extend_from_slice(&make_box(b"esds", &[0u8; 4]));
        let mp4a = make_box(b"mp4a", &entry_content);
        let mut stsd_content = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd_content.extend_from_slice(&mp4a);
        let stsd = make_box(b"stsd", &stsd_content);
        assert_eq!(parse_codec_string(&stsd, 0, stsd.len()), "mp4a.40.2");
    }

    /// moof → traf → { tfhd(track_ID=1), trun(2 samples, dur 1000 each) }.
    #[test]
    fn parse_moof_duration_and_track_id() {
        let tfhd = make_box(b"tfhd", &[0, 0, 0, 0, 0, 0, 0, 1]); // flags=0, track_ID=1

        // trun: version0, flags=0x000301 (data_offset|duration|size), count=2.
        let mut trun_content = vec![0x00, 0x00, 0x03, 0x01];
        trun_content.extend_from_slice(&2u32.to_be_bytes()); // sample_count
        trun_content.extend_from_slice(&0i32.to_be_bytes()); // data_offset
        for _ in 0..2 {
            trun_content.extend_from_slice(&1000u32.to_be_bytes()); // duration
            trun_content.extend_from_slice(&500u32.to_be_bytes()); // size
        }
        let trun = make_box(b"trun", &trun_content);

        let mut traf_content = tfhd.clone();
        traf_content.extend_from_slice(&trun);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);

        let (dur, count) = parse_moof_duration(&moof, 8, moof.len());
        assert_eq!((dur, count), (2000, 2));
        assert_eq!(parse_moof_track_id(&moof, 8, moof.len()), 1);
    }

    // ── per-track demux (P3a) ────────────────────────────────────────────────
    // Driven by a REAL ffmpeg video+audio fragmented MP4 (+separate_moof), so the
    // track separation is exercised against the exact shape media-provider emits.
    fn av_fixture() -> Vec<u8> {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        let text = include_str!("../tests/vectors/tiny_av_fragmented.mp4.b64");
        b64.decode(text.trim()).expect("decode av fixture")
    }

    #[test]
    fn demux_separates_video_and_audio_streams() {
        let data = av_fixture();
        let flat = split_fragmented(&data).expect("flat split");
        let streams = demux_tracks(&data).expect("demux");

        // Exactly one video + one audio track.
        assert_eq!(streams.len(), 2, "expected 2 tracks");
        assert!(streams.iter().any(|s| s.info.kind == TrackKind::Video));
        assert!(streams.iter().any(|s| s.info.kind == TrackKind::Audio));

        // Every flat fragment lands in exactly one track stream (a partition).
        let demuxed: usize = streams.iter().map(|s| s.segments.len()).sum();
        assert_eq!(
            demuxed,
            flat.fragments.len(),
            "per-track fragments must partition the flat fragment list"
        );
        for s in &streams {
            assert!(!s.segments.is_empty(), "a track must have fragments");
            assert!(s.info.bandwidth > 0, "per-track bandwidth must be computed");
        }
    }

    #[test]
    fn per_track_init_is_standalone_single_track() {
        let data = av_fixture();
        let streams = demux_tracks(&data).expect("demux");
        for s in &streams {
            // Reassemble this track as its own fragmented MP4 and re-parse it: the
            // init must describe EXACTLY one track (its own), and the fragment count
            // and track_id must match — proving the init is a valid standalone moov.
            let mut whole = s.init.clone();
            for frag in &s.segments {
                whole.extend_from_slice(frag);
            }
            let meta = parse_fragment_metadata(&whole)
                .unwrap_or_else(|e| panic!("re-parse track {}: {e}", s.info.track_id));
            assert_eq!(
                meta.tracks.len(),
                1,
                "per-track init must carry exactly one trak (track {})",
                s.info.track_id
            );
            assert_eq!(meta.tracks[0].track_id, s.info.track_id);
            assert_eq!(meta.tracks[0].kind, s.info.kind);
            assert_eq!(
                meta.segments.len(),
                s.segments.len(),
                "re-parsed fragment count must match (track {})",
                s.info.track_id
            );
            assert!(
                meta.segments.iter().all(|seg| seg.track_id == s.info.track_id),
                "every re-parsed segment belongs to this track"
            );
        }
    }

    /// The bounded placeholder variant marker is byte-distinct per symbol, readable back, and —
    /// crucially — SURVIVES the CENC rail: it is appended after `mdat`, so `encrypt_fragment` and
    /// `strip_senc` carry it through, and the symbol is still recoverable from the clean fragment.
    #[test]
    fn placeholder_variant_is_distinct_and_survives_the_cenc_rail() {
        let data = av_fixture();
        let flat = split_fragmented(&data).expect("flat split");
        let frag = &flat.fragments[0];

        let a = embed_placeholder_variant(frag, 0);
        let b = embed_placeholder_variant(frag, 1);
        assert_ne!(a, b, "different symbols must yield different bytes");
        assert!(a.starts_with(frag), "the marker strictly extends the fragment");
        assert_eq!(read_placeholder_variant(&a), Some(0));
        assert_eq!(read_placeholder_variant(&b), Some(1));
        assert_eq!(
            read_placeholder_variant(frag),
            None,
            "unmarked fragment reads as no variant"
        );

        // Survive the rail: embed -> CENC encrypt -> decrypt (strip senc) -> still variant B.
        let cek = [7u8; 16];
        let mut ctr = 0u64;
        let enc_a = encrypt_fragment(&a, &cek, &mut ctr).expect("encrypt A");
        let mut ctr2 = 0u64;
        let enc_b = encrypt_fragment(&b, &cek, &mut ctr2).expect("encrypt B");
        assert_ne!(enc_a, enc_b, "encrypted variants must differ (distinct served bytes)");
        let clean_b = strip_senc(&enc_b).expect("decrypt B");
        assert_eq!(
            read_placeholder_variant(&clean_b),
            Some(1),
            "the served variant symbol must survive back to the clean fragment"
        );
    }

    // ── CENC init signaling (cenc_signal_init / strip_cenc_signal) ──────────────────

    /// Build a minimal single-track init: `ftyp` + `moov { mvhd, trak { mdia { minf {
    /// stbl { stsd { <entry> } } } } } }`. `mvhd` is a sibling before `trak` so the
    /// roundtrip also proves non-trak moov children survive.
    fn minimal_init(entry_fourcc: &[u8; 4], fixed_bytes: usize, codec_child: &[u8]) -> Vec<u8> {
        let mut entry_content = vec![0u8; fixed_bytes];
        entry_content.extend_from_slice(codec_child);
        let entry = make_box(entry_fourcc, &entry_content);
        let mut stsd_content = vec![0, 0, 0, 0, 0, 0, 0, 1]; // version/flags + entry_count=1
        stsd_content.extend_from_slice(&entry);
        let stsd = make_box(b"stsd", &stsd_content);
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &minf);
        let trak = make_box(b"trak", &mdia);
        let mvhd = make_box(b"mvhd", &[0u8; 8]);
        let mut moov_content = Vec::new();
        moov_content.extend_from_slice(&mvhd);
        moov_content.extend_from_slice(&trak);
        let moov = make_box(b"moov", &moov_content);
        let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso2");
        let mut init = Vec::new();
        init.extend_from_slice(&ftyp);
        init.extend_from_slice(&moov);
        init
    }

    #[test]
    fn cenc_signal_init_roundtrips_video() {
        let init = minimal_init(
            b"avc1",
            VISUAL_SAMPLE_ENTRY_FIXED_BYTES,
            &make_box(b"avcC", &[0x01, 0x64, 0x00, 0x28]),
        );
        let kid = [0x11u8; 16];
        let pssh = make_box(b"pssh", b"PSSH-PAYLOAD-BYTES");

        let signaled = cenc_signal_init(&init, &kid, 8, &pssh).expect("signal");
        // Structure: sample entry is now `encv`, a `tenc` carrying the KID is present,
        // and the `pssh` payload was injected.
        let p = locate_stsd_path(&signaled).expect("locate");
        assert_eq!(&p.entry.1.box_type, b"encv");
        assert!(
            signaled.windows(pssh.len()).any(|w| w == pssh.as_slice()),
            "pssh box must be injected into moov"
        );
        assert!(
            signaled.windows(16).any(|w| w == kid),
            "tenc must carry the default_KID"
        );
        // Inverse restores the original init byte-for-byte.
        let stripped = strip_cenc_signal(&signaled).expect("strip");
        assert_eq!(stripped, init, "strip_cenc_signal must invert cenc_signal_init");
    }

    #[test]
    fn cenc_signal_init_roundtrips_audio() {
        let init = minimal_init(
            b"mp4a",
            AUDIO_SAMPLE_ENTRY_FIXED_BYTES,
            &make_box(b"esds", &[0u8; 4]),
        );
        let kid = [0x22u8; 16];
        let pssh = make_box(b"pssh", b"AUDIO-PSSH");

        let signaled = cenc_signal_init(&init, &kid, 8, &pssh).expect("signal");
        let p = locate_stsd_path(&signaled).expect("locate");
        assert_eq!(&p.entry.1.box_type, b"enca", "audio entry must become enca");

        let stripped = strip_cenc_signal(&signaled).expect("strip");
        assert_eq!(stripped, init);
    }

    #[test]
    fn strip_cenc_signal_is_noop_on_unsignaled_init() {
        let init = minimal_init(
            b"avc1",
            VISUAL_SAMPLE_ENTRY_FIXED_BYTES,
            &make_box(b"avcC", &[0x01, 0x42, 0x00, 0x1e]),
        );
        assert_eq!(strip_cenc_signal(&init).expect("strip"), init);
    }

    #[test]
    fn cenc_signal_init_rejects_already_signaled() {
        let init = minimal_init(
            b"avc1",
            VISUAL_SAMPLE_ENTRY_FIXED_BYTES,
            &make_box(b"avcC", &[0x01, 0x64, 0x00, 0x28]),
        );
        let signaled = cenc_signal_init(&init, &[0u8; 16], 8, &make_box(b"pssh", b"x")).unwrap();
        assert!(
            cenc_signal_init(&signaled, &[0u8; 16], 8, &make_box(b"pssh", b"y")).is_err(),
            "double-signaling must fail closed"
        );
    }
}
