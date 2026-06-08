//! Minimal fMP4 (ISO BMFF) box parser for CENC decryption.
//!
//! Parses only the boxes needed for CENC sample decryption:
//! - moof (Movie Fragment) -> traf (Track Fragment)
//!   - trun: sample count, sizes, offsets
//!   - senc: per-sample IVs and optional subsample ranges
//!   - sbgp/sgpd: sample group description (for constant IV patterns)
//! - mdat: encrypted sample data
//! - moov -> trak -> stbl -> stsd -> sinf -> schi -> tenc: default encryption params

use std::io::{Cursor, Read};

#[derive(Debug, Clone)]
pub struct BoxHeader {
    pub box_type: [u8; 4],
    pub size: u64,
    pub header_size: u64,
}

#[derive(Debug, Clone)]
pub struct TrunEntry {
    pub sample_duration: Option<u32>,
    pub sample_size: Option<u32>,
    pub sample_flags: Option<u32>,
    pub composition_time_offset: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct TrunBox {
    pub version: u8,
    pub flags: u32,
    pub data_offset: Option<i32>,
    pub first_sample_flags: Option<u32>,
    pub entries: Vec<TrunEntry>,
}

#[derive(Debug, Clone)]
pub struct SubsampleEntry {
    pub clear_bytes: u16,
    pub encrypted_bytes: u32,
}

#[derive(Debug, Clone)]
pub struct SencSample {
    pub iv: Vec<u8>,
    pub subsamples: Vec<SubsampleEntry>,
}

#[derive(Debug, Clone)]
pub struct SencBox {
    pub version: u8,
    pub flags: u32,
    pub samples: Vec<SencSample>,
}

#[derive(Debug, Clone)]
pub struct TencBox {
    pub default_is_protected: u8,
    pub default_per_sample_iv_size: u8,
    pub default_kid: [u8; 16],
    pub default_constant_iv: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct TrackFragment {
    pub trun: Option<TrunBox>,
    pub senc: Option<SencBox>,
}

#[derive(Debug)]
pub struct ParsedSegment {
    pub traf: Option<TrackFragment>,
    pub mdat_offset: usize,
    pub mdat_size: usize,
}

pub fn read_box_header(data: &[u8], offset: usize) -> Option<BoxHeader> {
    if offset + 8 > data.len() {
        return None;
    }
    let size32 = u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
    let box_type = [data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]];

    let (size, header_size) = if size32 == 1 {
        if offset + 16 > data.len() {
            return None;
        }
        let size64 = u64::from_be_bytes([
            data[offset + 8], data[offset + 9], data[offset + 10], data[offset + 11],
            data[offset + 12], data[offset + 13], data[offset + 14], data[offset + 15],
        ]);
        (size64, 16u64)
    } else if size32 == 0 {
        ((data.len() - offset) as u64, 8u64)
    } else {
        (size32 as u64, 8u64)
    };

    Some(BoxHeader { box_type, size, header_size })
}

fn box_type_str(t: &[u8; 4]) -> &str {
    std::str::from_utf8(t).unwrap_or("????")
}

pub fn parse_segment(data: &[u8], iv_size: u8) -> Result<ParsedSegment, String> {
    let mut result = ParsedSegment {
        traf: None,
        mdat_offset: 0,
        mdat_size: 0,
    };

    let mut offset = 0usize;
    while offset < data.len() {
        let header = read_box_header(data, offset)
            .ok_or_else(|| format!("truncated box at offset {offset}"))?;
        let box_end = offset + header.size as usize;
        if box_end > data.len() {
            return Err(format!("{} box size {} exceeds data at offset {offset}", box_type_str(&header.box_type), header.size));
        }
        let content_start = offset + header.header_size as usize;

        match &header.box_type {
            b"moof" => {
                result.traf = parse_moof(&data[content_start..box_end], iv_size)?;
            }
            b"mdat" => {
                result.mdat_offset = content_start;
                result.mdat_size = box_end - content_start;
            }
            _ => {}
        }

        offset = box_end;
    }

    Ok(result)
}

fn parse_moof(data: &[u8], iv_size: u8) -> Result<Option<TrackFragment>, String> {
    let mut offset = 0usize;
    while offset < data.len() {
        let header = read_box_header(data, offset)
            .ok_or_else(|| "truncated box in moof".to_string())?;
        let box_end = offset + header.size as usize;
        let content_start = offset + header.header_size as usize;

        if &header.box_type == b"traf" {
            return Ok(Some(parse_traf(&data[content_start..box_end], iv_size)?));
        }

        offset = box_end;
    }
    Ok(None)
}

fn parse_traf(data: &[u8], iv_size: u8) -> Result<TrackFragment, String> {
    let mut frag = TrackFragment { trun: None, senc: None };
    let mut offset = 0usize;

    while offset < data.len() {
        let header = read_box_header(data, offset)
            .ok_or_else(|| "truncated box in traf".to_string())?;
        let box_end = offset + header.size as usize;
        let content_start = offset + header.header_size as usize;
        let content = &data[content_start..box_end];

        match &header.box_type {
            b"trun" => frag.trun = Some(parse_trun(content)?),
            b"senc" => frag.senc = Some(parse_senc(content, iv_size)?),
            _ => {}
        }

        offset = box_end;
    }

    Ok(frag)
}

fn parse_trun(data: &[u8]) -> Result<TrunBox, String> {
    if data.len() < 8 {
        return Err("trun too short".into());
    }
    let version = data[0];
    let flags = u32::from_be_bytes([0, data[1], data[2], data[3]]);
    let sample_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;

    let mut cursor = Cursor::new(&data[8..]);
    let mut buf4 = [0u8; 4];

    let data_offset = if flags & 0x000001 != 0 {
        cursor.read_exact(&mut buf4).map_err(|e| format!("trun data_offset: {e}"))?;
        Some(i32::from_be_bytes(buf4))
    } else {
        None
    };

    let first_sample_flags = if flags & 0x000004 != 0 {
        cursor.read_exact(&mut buf4).map_err(|e| format!("trun first_sample_flags: {e}"))?;
        Some(u32::from_be_bytes(buf4))
    } else {
        None
    };

    let has_duration = flags & 0x000100 != 0;
    let has_size = flags & 0x000200 != 0;
    let has_flags = flags & 0x000400 != 0;
    let has_cto = flags & 0x000800 != 0;

    let mut entries = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let sample_duration = if has_duration {
            cursor.read_exact(&mut buf4).map_err(|e| format!("trun duration: {e}"))?;
            Some(u32::from_be_bytes(buf4))
        } else {
            None
        };
        let sample_size = if has_size {
            cursor.read_exact(&mut buf4).map_err(|e| format!("trun size: {e}"))?;
            Some(u32::from_be_bytes(buf4))
        } else {
            None
        };
        let sample_flags = if has_flags {
            cursor.read_exact(&mut buf4).map_err(|e| format!("trun flags: {e}"))?;
            Some(u32::from_be_bytes(buf4))
        } else {
            None
        };
        let composition_time_offset = if has_cto {
            cursor.read_exact(&mut buf4).map_err(|e| format!("trun cto: {e}"))?;
            if version == 0 {
                Some(u32::from_be_bytes(buf4) as i32)
            } else {
                Some(i32::from_be_bytes(buf4))
            }
        } else {
            None
        };

        entries.push(TrunEntry { sample_duration, sample_size, sample_flags, composition_time_offset });
    }

    Ok(TrunBox { version, flags, data_offset, first_sample_flags, entries })
}

pub fn parse_senc(data: &[u8], default_iv_size: u8) -> Result<SencBox, String> {
    if data.len() < 8 {
        return Err("senc too short".into());
    }
    let version = data[0];
    let flags = u32::from_be_bytes([0, data[1], data[2], data[3]]);
    let sample_count = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let has_subsamples = flags & 0x000002 != 0;
    let iv_size = default_iv_size as usize;

    let mut cursor = Cursor::new(&data[8..]);
    let mut samples = Vec::with_capacity(sample_count);

    for _ in 0..sample_count {
        let mut iv = vec![0u8; iv_size];
        cursor.read_exact(&mut iv).map_err(|e| format!("senc iv: {e}"))?;

        let subsamples = if has_subsamples {
            let mut buf2 = [0u8; 2];
            cursor.read_exact(&mut buf2).map_err(|e| format!("senc subsample count: {e}"))?;
            let count = u16::from_be_bytes(buf2) as usize;
            let mut subs = Vec::with_capacity(count);
            for _ in 0..count {
                let mut clear = [0u8; 2];
                let mut enc = [0u8; 4];
                cursor.read_exact(&mut clear).map_err(|e| format!("senc clear_bytes: {e}"))?;
                cursor.read_exact(&mut enc).map_err(|e| format!("senc encrypted_bytes: {e}"))?;
                subs.push(SubsampleEntry {
                    clear_bytes: u16::from_be_bytes(clear),
                    encrypted_bytes: u32::from_be_bytes(enc),
                });
            }
            subs
        } else {
            Vec::new()
        };

        samples.push(SencSample { iv, subsamples });
    }

    Ok(SencBox { version, flags, samples })
}

/// Parse an init segment to extract tenc (Track Encryption Box) default params.
/// Path: moov -> trak -> mdia -> minf -> stbl -> stsd -> encv/enca -> sinf -> schi -> tenc
pub fn parse_init_for_tenc(data: &[u8]) -> Option<TencBox> {
    find_box_recursive(data, &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"])
        .and_then(|stsd_data| {
            if stsd_data.len() < 8 {
                return None;
            }
            // stsd content: version+flags(4) + entry_count(4) + entries...
            let entry_count = u32::from_be_bytes([stsd_data[4], stsd_data[5], stsd_data[6], stsd_data[7]]);
            if entry_count == 0 {
                return None;
            }
            // First sample entry starts at offset 8 within stsd content
            let entry_data = &stsd_data[8..];
            let header = read_box_header(entry_data, 0)?;
            let entry_type = &header.box_type;

            // Determine format-specific header size to skip before child boxes.
            // Common SampleEntry: 6 reserved + 2 data_ref_index = 8 bytes
            // VisualSampleEntry (encv): + 70 bytes = 78 total
            // AudioSampleEntry  (enca): + 20 bytes = 28 total
            let skip = if entry_type == b"encv" { 78usize } else { 28usize };
            let child_start = header.header_size as usize + skip;
            if child_start >= header.size as usize {
                return None;
            }
            let children = &entry_data[child_start..header.size as usize];
            find_sinf_tenc(children)
        })
}

fn find_sinf_tenc(sample_entry: &[u8]) -> Option<TencBox> {
    // Skip the format-specific header by scanning for 'sinf' box
    let mut offset = 0usize;
    while offset < sample_entry.len() {
        let header = read_box_header(sample_entry, offset)?;
        let box_end = offset + header.size as usize;
        if box_end > sample_entry.len() {
            return None;
        }
        let content_start = offset + header.header_size as usize;

        if &header.box_type == b"sinf" {
            return find_tenc_in_sinf(&sample_entry[content_start..box_end]);
        }

        offset = box_end;
    }
    None
}

fn find_tenc_in_sinf(sinf: &[u8]) -> Option<TencBox> {
    let mut offset = 0usize;
    while offset < sinf.len() {
        let header = read_box_header(sinf, offset)?;
        let box_end = offset + header.size as usize;
        if box_end > sinf.len() {
            return None;
        }
        let content_start = offset + header.header_size as usize;

        if &header.box_type == b"schi" {
            return find_tenc_in_schi(&sinf[content_start..box_end]);
        }

        offset = box_end;
    }
    None
}

fn find_tenc_in_schi(schi: &[u8]) -> Option<TencBox> {
    let mut offset = 0usize;
    while offset < schi.len() {
        let header = read_box_header(schi, offset)?;
        let box_end = offset + header.size as usize;
        if box_end > schi.len() {
            return None;
        }
        let content_start = offset + header.header_size as usize;

        if &header.box_type == b"tenc" {
            return parse_tenc(&schi[content_start..box_end]);
        }

        offset = box_end;
    }
    None
}

fn parse_tenc(data: &[u8]) -> Option<TencBox> {
    // tenc: version(1) + flags(3) + reserved(1) + reserved(1) +
    //       default_isProtected(1) + default_Per_Sample_IV_Size(1) + default_KID(16)
    if data.len() < 22 {
        return None;
    }
    let version = data[0];
    let default_is_protected = data[6];
    let default_per_sample_iv_size = data[7];
    let mut default_kid = [0u8; 16];
    default_kid.copy_from_slice(&data[8..24]);

    let default_constant_iv = if version >= 1 && default_per_sample_iv_size == 0 && data.len() > 24 {
        let iv_len = data[24] as usize;
        if data.len() >= 25 + iv_len {
            Some(data[25..25 + iv_len].to_vec())
        } else {
            None
        }
    } else {
        None
    };

    Some(TencBox {
        default_is_protected,
        default_per_sample_iv_size,
        default_kid,
        default_constant_iv,
    })
}

fn find_box_recursive(data: &[u8], path: &[&[u8; 4]]) -> Option<Vec<u8>> {
    if path.is_empty() {
        return Some(data.to_vec());
    }

    let target = path[0];
    let mut offset = 0usize;
    while offset < data.len() {
        let header = read_box_header(data, offset)?;
        let box_end = offset + header.size as usize;
        if box_end > data.len() {
            return None;
        }
        let content_start = offset + header.header_size as usize;

        if header.box_type == *target {
            // For fullbox containers (moov, trak, mdia, etc.), content starts right after header
            // For stsd, there's a version+flags+entry_count prefix
            if path.len() == 1 {
                return Some(data[content_start..box_end].to_vec());
            }
            return find_box_recursive(&data[content_start..box_end], &path[1..]);
        }

        offset = box_end;
    }
    None
}
