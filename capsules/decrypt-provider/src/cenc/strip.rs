//! fMP4 box stripping — remove CENC encryption signaling from init segments
//! and encryption metadata boxes from media segments.
//!
//! Rust port of `stripEncryptionSignaling` and `stripSegmentEncryptionBoxes`
//! from media.ts. Handles both 32-bit and 64-bit extended box sizes.

use super::mp4box::read_box_header;

/// Strip encryption signaling from an init segment:
/// - Replace `encv`/`enca` sample entry type with original format from `frma`
/// - Remove `sinf` box from the sample entry
/// - Remove top-level `pssh` boxes
/// - Adjust all ancestor box sizes accordingly
pub fn strip_encryption_signaling(init: &[u8]) -> Vec<u8> {
    let buf = init;

    // Walk moov → trak → mdia → minf → stbl → stsd to find the sample entry
    let stsd = match find_box_path(buf, 0, buf.len(), &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"]) {
        Some(info) => info,
        None => return buf.to_vec(),
    };

    // stsd content: version+flags(4) + entry_count(4) + entries
    let entry_start = stsd.content_start + 8;
    if entry_start + 8 > buf.len() {
        return buf.to_vec();
    }

    let entry_header = match read_box_header(buf, entry_start) {
        Some(h) => h,
        None => return buf.to_vec(),
    };

    let entry_type = &entry_header.box_type;
    let is_encv = entry_type == b"encv";
    let is_enca = entry_type == b"enca";

    if !is_encv && !is_enca {
        return buf.to_vec();
    }

    let entry_content_end = entry_start + entry_header.size as usize;

    // Skip format-specific header to find child boxes
    // VisualSampleEntry (encv): header + 8 (SampleEntry) + 70 = header + 78
    // AudioSampleEntry (enca):  header + 8 (SampleEntry) + 20 = header + 28
    let format_header_size: usize = if is_encv { 78 } else { 28 };
    let mut scan_pos = entry_start + entry_header.header_size as usize + format_header_size;

    let mut sinf_offset: Option<usize> = None;
    let mut sinf_size: usize = 0;
    let mut original_format = [0u8; 4];

    while scan_pos + 8 <= entry_content_end {
        let child = match read_box_header(buf, scan_pos) {
            Some(h) => h,
            None => break,
        };
        if child.size < 8 || scan_pos + child.size as usize > entry_content_end {
            break;
        }

        if &child.box_type == b"sinf" {
            sinf_offset = Some(scan_pos);
            sinf_size = child.size as usize;

            // Find frma within sinf
            let sinf_content_start = scan_pos + child.header_size as usize;
            let sinf_end = scan_pos + child.size as usize;
            let mut inner = sinf_content_start;
            while inner + 8 <= sinf_end {
                let inner_h = match read_box_header(buf, inner) {
                    Some(h) => h,
                    None => break,
                };
                if inner_h.size < 8 { break; }
                if &inner_h.box_type == b"frma" && inner + inner_h.header_size as usize + 4 <= sinf_end {
                    let frma_data_start = inner + inner_h.header_size as usize;
                    original_format.copy_from_slice(&buf[frma_data_start..frma_data_start + 4]);
                }
                inner += inner_h.size as usize;
            }
            break;
        }
        scan_pos += child.size as usize;
    }

    let sinf_off = match sinf_offset {
        Some(off) if original_format != [0u8; 4] => off,
        _ => return buf.to_vec(),
    };

    // Collect removal ranges: sinf + top-level pssh boxes + pssh inside moov
    let mut removals: Vec<Removal> = vec![Removal { start: sinf_off, size: sinf_size }];

    // Scan top-level boxes for pssh
    let mut top_pos = 0;
    while top_pos + 8 <= buf.len() {
        let h = match read_box_header(buf, top_pos) {
            Some(h) => h,
            None => break,
        };
        if h.size < 8 || top_pos + h.size as usize > buf.len() { break; }
        if &h.box_type == b"pssh" {
            removals.push(Removal { start: top_pos, size: h.size as usize });
        }
        top_pos += h.size as usize;
    }

    // Also scan inside moov for pssh (our DASH packager injects pssh into moov)
    if let Some(moov_info) = find_box_path(buf, 0, buf.len(), &[b"moov"]) {
        let moov_end = moov_info.box_end;
        let mut pos = moov_info.content_start;
        while pos + 8 <= moov_end {
            let h = match read_box_header(buf, pos) {
                Some(h) => h,
                None => break,
            };
            if h.size < 8 || pos + h.size as usize > moov_end { break; }
            if &h.box_type == b"pssh" {
                removals.push(Removal { start: pos, size: h.size as usize });
            }
            pos += h.size as usize;
        }
    }

    removals.sort_by_key(|r| r.start);

    // Build output without removed ranges
    let mut output = build_without_removals(buf, &removals);

    // Replace entry type with original format
    let enc_type = if is_encv { b"encv" } else { b"enca" };
    if let Some(pos) = find_bytes(&output, enc_type) {
        output[pos..pos + 4].copy_from_slice(&original_format);
    }

    // Adjust ancestor box sizes
    let ancestors = collect_ancestor_positions(buf, &[b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"]);
    let mut ancestor_list: Vec<(usize, u64)> = ancestors;

    // Include sample entry box
    if ancestor_list.len() == 6 {
        let stsd_pos = ancestor_list[5].0;
        if let Some(stsd_h) = read_box_header(buf, stsd_pos) {
            let se_pos = stsd_pos + stsd_h.header_size as usize + 8;
            if let Some(se_h) = read_box_header(buf, se_pos) {
                ancestor_list.push((se_pos, se_h.size));
            }
        }
    }

    for &(orig_pos, orig_size) in &ancestor_list {
        let anc_end = orig_pos + orig_size as usize;
        let removed_within: usize = removals.iter()
            .filter(|r| r.start >= orig_pos && r.start + r.size <= anc_end)
            .map(|r| r.size)
            .sum();

        if removed_within > 0 {
            let out_pos = map_to_output(orig_pos, &removals);
            let new_size = orig_size - removed_within as u64;
            write_box_size(&mut output, out_pos, new_size);
        }
    }

    output
}

/// Strip encryption-related boxes (senc, saiz, saio, sbgp, sgpd) from a
/// decrypted fMP4 media segment. Also fixes moof/traf sizes and trun data_offset.
pub fn strip_segment_encryption_boxes(segment: &[u8]) -> Vec<u8> {
    let buf = segment;
    let enc_box_types: &[&[u8; 4]] = &[b"senc", b"saiz", b"saio", b"sbgp", b"sgpd"];

    // Find moof
    let moof_start = match find_box_start(buf, 0, buf.len(), b"moof") {
        Some(pos) => pos,
        None => return buf.to_vec(),
    };
    let moof_header = match read_box_header(buf, moof_start) {
        Some(h) => h,
        None => return buf.to_vec(),
    };
    let moof_end = moof_start + moof_header.size as usize;

    // Find traf inside moof
    let moof_content = moof_start + moof_header.header_size as usize;
    let traf_start = match find_box_start(buf, moof_content, moof_end, b"traf") {
        Some(pos) => pos,
        None => return buf.to_vec(),
    };
    let traf_header = match read_box_header(buf, traf_start) {
        Some(h) => h,
        None => return buf.to_vec(),
    };
    let traf_end = traf_start + traf_header.size as usize;

    // Collect encryption boxes within traf
    let mut removals: Vec<Removal> = Vec::new();
    let mut scan_pos = traf_start + traf_header.header_size as usize;
    while scan_pos + 8 <= traf_end {
        let h = match read_box_header(buf, scan_pos) {
            Some(h) => h,
            None => break,
        };
        if h.size < 8 || scan_pos + h.size as usize > traf_end { break; }
        if enc_box_types.iter().any(|t| **t == h.box_type) {
            removals.push(Removal { start: scan_pos, size: h.size as usize });
        }
        scan_pos += h.size as usize;
    }

    if removals.is_empty() {
        return buf.to_vec();
    }

    let total_removed: usize = removals.iter().map(|r| r.size).sum();
    removals.sort_by_key(|r| r.start);

    let mut output = build_without_removals(buf, &removals);

    // Adjust moof size
    let new_moof_size = moof_header.size - total_removed as u64;
    write_box_size(&mut output, moof_start, new_moof_size);

    // Adjust traf size
    let new_traf_size = traf_header.size - total_removed as u64;
    write_box_size(&mut output, traf_start, new_traf_size);

    // Fix trun.data_offset — offset from moof start to mdat data
    let traf_out_content = traf_start + traf_header.header_size as usize;
    let traf_out_end = traf_start + new_traf_size as usize;
    if let Some(trun_start) = find_box_start(&output, traf_out_content, traf_out_end, b"trun") {
        if trun_start + 12 <= output.len() {
            let trun_flags = ((output[trun_start + 9] as u32) << 16)
                | ((output[trun_start + 10] as u32) << 8)
                | (output[trun_start + 11] as u32);

            // data_offset_present flag
            if trun_flags & 0x1 != 0 {
                let do_pos = trun_start + 16;
                if do_pos + 4 <= output.len() {
                    let old_offset = i32::from_be_bytes([
                        output[do_pos], output[do_pos + 1],
                        output[do_pos + 2], output[do_pos + 3],
                    ]);
                    let new_offset = old_offset - total_removed as i32;
                    output[do_pos..do_pos + 4].copy_from_slice(&new_offset.to_be_bytes());
                }
            }
        }
    }

    output
}

// ── Helpers ──────────────────────────────────────────────────────────

struct Removal {
    start: usize,
    size: usize,
}

fn build_without_removals(buf: &[u8], removals: &[Removal]) -> Vec<u8> {
    let total_removed: usize = removals.iter().map(|r| r.size).sum();
    let mut output = Vec::with_capacity(buf.len() - total_removed);
    let mut prev_end = 0;
    for rem in removals {
        if rem.start > prev_end {
            output.extend_from_slice(&buf[prev_end..rem.start]);
        }
        prev_end = rem.start + rem.size;
    }
    if prev_end < buf.len() {
        output.extend_from_slice(&buf[prev_end..]);
    }
    output
}

fn map_to_output(orig_pos: usize, removals: &[Removal]) -> usize {
    let shift: usize = removals.iter()
        .filter(|r| r.start < orig_pos)
        .map(|r| r.size)
        .sum();
    orig_pos - shift
}

fn find_box_start(buf: &[u8], start: usize, end: usize, box_type: &[u8; 4]) -> Option<usize> {
    let mut pos = start;
    while pos + 8 <= end {
        let h = read_box_header(buf, pos)?;
        if h.size < 8 || pos + h.size as usize > end { return None; }
        if &h.box_type == box_type { return Some(pos); }
        pos += h.size as usize;
    }
    None
}

struct BoxInfo {
    box_start: usize,
    content_start: usize,
    box_end: usize,
}

fn find_box_path(buf: &[u8], start: usize, end: usize, path: &[&[u8; 4]]) -> Option<BoxInfo> {
    if path.is_empty() { return None; }

    let mut pos = start;
    while pos + 8 <= end {
        let h = read_box_header(buf, pos)?;
        if h.size < 8 || pos + h.size as usize > end { return None; }
        let content_start = pos + h.header_size as usize;
        let box_end = pos + h.size as usize;

        if h.box_type == *path[0] {
            if path.len() == 1 {
                return Some(BoxInfo { box_start: pos, content_start, box_end });
            }
            return find_box_path(buf, content_start, box_end, &path[1..]);
        }
        pos += h.size as usize;
    }
    None
}

fn collect_ancestor_positions(buf: &[u8], path: &[&[u8; 4]]) -> Vec<(usize, u64)> {
    let mut ancestors = Vec::new();
    let mut start = 0;
    let mut end = buf.len();

    for target in path {
        let mut pos = start;
        let mut found = false;
        while pos + 8 <= end {
            let h = match read_box_header(buf, pos) {
                Some(h) => h,
                None => break,
            };
            if h.size < 8 || pos + h.size as usize > end { break; }
            if h.box_type == **target {
                ancestors.push((pos, h.size));
                start = pos + h.header_size as usize;
                end = pos + h.size as usize;
                found = true;
                break;
            }
            pos += h.size as usize;
        }
        if !found { break; }
    }

    ancestors
}

fn find_bytes(buf: &[u8], needle: &[u8; 4]) -> Option<usize> {
    buf.windows(4).position(|w| w == needle)
}

/// Write a box size, handling both standard (32-bit) and extended (64-bit) sizes.
/// Standard boxes: 4 bytes at `offset`.
/// Extended boxes (where the original 32-bit size was 1): 8 bytes at `offset+8`.
fn write_box_size(buf: &mut [u8], offset: usize, new_size: u64) {
    if offset + 4 > buf.len() { return; }
    let existing_size32 = u32::from_be_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]]);

    if existing_size32 == 1 {
        // Extended size — write 64-bit value at offset+8
        if offset + 16 <= buf.len() {
            buf[offset + 8..offset + 16].copy_from_slice(&new_size.to_be_bytes());
        }
    } else {
        buf[offset..offset + 4].copy_from_slice(&(new_size as u32).to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = Vec::with_capacity(size as usize);
        b.extend_from_slice(&size.to_be_bytes());
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    #[test]
    fn strip_segment_removes_senc() {
        // Build: moof { traf { tfhd, senc } }, mdat { data }
        let tfhd = make_box(b"tfhd", &[0; 12]);
        let senc = make_box(b"senc", &[0; 20]);
        let mut traf_content = Vec::new();
        traf_content.extend_from_slice(&tfhd);
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", b"cleardata");

        let mut segment = Vec::new();
        segment.extend_from_slice(&moof);
        segment.extend_from_slice(&mdat);

        let result = strip_segment_encryption_boxes(&segment);

        // senc should be removed (28 bytes)
        assert_eq!(result.len(), segment.len() - senc.len());

        // Verify no senc in output
        assert!(find_box_start(&result, 0, result.len(), b"senc").is_none());
    }

    #[test]
    fn no_strip_needed() {
        let tfhd = make_box(b"tfhd", &[0; 12]);
        let traf = make_box(b"traf", &tfhd);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", b"cleardata");

        let mut segment = Vec::new();
        segment.extend_from_slice(&moof);
        segment.extend_from_slice(&mdat);

        let result = strip_segment_encryption_boxes(&segment);
        assert_eq!(result, segment);
    }
}
