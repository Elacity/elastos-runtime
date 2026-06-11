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
