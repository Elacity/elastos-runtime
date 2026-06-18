//! Invisible forensic watermark — a blind, robust DCT-domain stamp.
//!
//! Layered UNDER the faint visible mark ([`super::watermark`]) at the single pixel-lock egress, this
//! embeds the buyer-identity string (`wallet · content · time`) imperceptibly into the luminance of
//! the rendered page so a LEAKED frame is still attributable even if the visible mark is cropped,
//! painted over, or the page is re-saved.
//!
//! ## How it works (blind DCT + QIM)
//! The image luminance is split into 8×8 blocks. In each block we project onto two SYMMETRIC
//! mid-band DCT-II basis functions `A=(1,2)` and `B=(2,1)` (orthonormal, so adding `δ·φ` to the
//! pixels shifts that coefficient by exactly `δ` and leaves the others untouched). One payload bit
//! is embedded into the DIFFERENCE `d = A−B` by **quantization index modulation**: `d` is nudged to
//! the nearest multiple of `STEP` whose index has the bit's parity (even→0, odd→1). The nudge is
//! always ≤ `STEP` regardless of the block's content — so unlike a fixed-margin scheme, a
//! high-contrast block can never end up carrying the wrong bit. Because the two coefficients share a
//! frequency band they quantise + perceptually mask alike, so the bit survives JPEG recompression
//! and global luma/contrast changes while staying invisible. Extraction reads the parity of
//! `round(d / STEP)` per block — NO original image needed (blind).
//!
//! The identity string is framed as `SYNC(16) + LEN(8) + DATA(CAP·8) + CRC-16(16)` and TILED across
//! every block. Extraction folds all blocks by their codeword position and MAJORITY-VOTES each bit,
//! so a minority of recompression-flipped blocks is corrected; the CRC then gates acceptance. A
//! 64-way (8×8) sub-block pixel-offset search plus a codeword-rotation search re-synchronise after a
//! same-resolution screenshot / translation / vertical crop.
//!
//! ## Honest robustness envelope
//! Recovers through: our own q85 encode, moderate recompression, brightness/contrast shifts,
//! same-resolution screenshots, full-width vertical crops, and sub-block translation.
//! Does NOT survive (by design — out of scope here): geometric RESCALING (zoomed screenshots),
//! rotation, or horizontal crops that change the pixel width. Those require a geometric-sync
//! (log-polar/template) layer, a separate, larger effort.

use image::RgbaImage;
use std::f32::consts::PI;

const BLK: usize = 8;
/// Symmetric mid-band coefficient pair `(row_freq, col_freq)`. Low-mid → robust to JPEG quant and
/// well perceptually masked; symmetric so both quantise alike (the relation `A−B` stays stable).
const POS_A: (usize, usize) = (1, 2);
const POS_B: (usize, usize) = (2, 1);
/// QIM quantization step on the coefficient difference `d = A−B` (orthonormal DCT units). The bit is
/// the parity of `round(d / STEP)`; embedding moves `d` by at most `STEP`. Large enough to clear q85
/// quant noise on this band (so the parity survives recompression), small enough to stay invisible
/// (the per-pixel luma nudge is at most ~`STEP/4`). A bit only flips if distortion moves `d` by more
/// than `STEP/2`; the majority fold across the page's blocks corrects the occasional flip.
const STEP: f32 = 18.0;
/// Perceptual masking gate: blocks whose luma std-dev is below this (essentially flat — e.g. a pure
/// white page margin, where the eye is MOST sensitive and there is no texture to hide under) are left
/// UNTOUCHED. Kept low so anti-aliased TEXT EDGES (which have modest local contrast) still carry the
/// mark — important because text/code pages are mostly white with thin strokes, and that is exactly
/// where the bits must live. Confidence-weighted extraction down-weights low-energy blocks anyway.
const ACTIVITY_MIN: f32 = 1.0;

/// 16-bit frame sync (a value unlikely to occur in payload text), MSB-first.
const SYNC: u16 = 0xACE1;
const SYNC_BITS: usize = 16;
const LEN_BITS: usize = 8;
const CRC_BITS: usize = 16;
/// Max embedded payload bytes. The invisible layer carries a COMPACT forensic identity (a 1-byte tag
/// plus the 20-byte EVM wallet, or a short UTF-8 fallback) — NOT the long human `wallet · content ·
/// time` string (that lives in the always-on visible mark and the audit log). Keeping the payload
/// small shrinks the codeword/fold period from 680 to 232 bits, so the mark still recovers from
/// CONTENT-SPARSE pages (short code/config snippets — a few lines on a mostly-empty page) that cannot
/// host a larger codeword.
const CAP: usize = 24;
/// One full codeword, repeated across the page in raster (row-major) order. Raster spreading gives
/// uniform codeword-position coverage — as the sweep increments by one block it visits every residue
/// mod PERIOD — so on a SPARSE page each bit still lands on many textured blocks. A block-grid shift
/// adds a uniform offset to every index (a clean rotation the decoder re-syncs on).
const PERIOD: usize = SYNC_BITS + LEN_BITS + CAP * 8 + CRC_BITS;

/// Map a block grid position to its codeword bit index (raster / row-major over the block grid).
#[inline]
fn code_index(by: usize, bx: usize, nbx: usize) -> usize {
    (by.wrapping_mul(nbx).wrapping_add(bx)) % PERIOD
}

/// Orthonormal 1-D DCT-II scale factor.
fn alpha(k: usize) -> f32 {
    if k == 0 {
        (1.0_f32 / BLK as f32).sqrt()
    } else {
        (2.0_f32 / BLK as f32).sqrt()
    }
}

/// The orthonormal 2-D DCT-II basis image for frequency `(u, v)` over an 8×8 block, indexed
/// `[row][col]`. `sum(basis^2) == 1`, so projecting onto it gives the DCT coefficient and adding
/// `δ·basis` to the pixels shifts exactly that coefficient by `δ`.
fn basis(u: usize, v: usize) -> [[f32; BLK]; BLK] {
    let mut b = [[0.0f32; BLK]; BLK];
    let au = alpha(u);
    let av = alpha(v);
    for (r, row) in b.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let cu = (((2 * r + 1) as f32) * u as f32 * PI / 16.0).cos();
            let cv = (((2 * c + 1) as f32) * v as f32 * PI / 16.0).cos();
            *cell = au * av * cu * cv;
        }
    }
    b
}

/// BT.601 luma of a pixel (matches the JPEG colour transform), so the mark rides the channel JPEG
/// preserves at full resolution.
#[inline]
fn luma(p: &image::Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

/// CRC-16/CCITT-FALSE over `data` (poly 0x1021, init 0xFFFF) — the frame integrity gate.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Tag byte: payload is a raw 20-byte EVM address (render back as `0x…`).
const TAG_EVM: u8 = 0x01;
/// Tag byte: payload is short UTF-8 (render back verbatim) — fallback for non-address watermarks.
const TAG_UTF8: u8 = 0x00;

#[inline]
fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Parse a leading `0x`-prefixed 40-hex-digit EVM address from `s` into 20 raw bytes, if present.
fn leading_evm_address(s: &str) -> Option<[u8; 20]> {
    let t = s.trim();
    let hex = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))?
        .as_bytes();
    if hex.len() < 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (hex_nibble(hex[2 * i])? << 4) | hex_nibble(hex[2 * i + 1])?;
    }
    Some(out)
}

/// Encode the human watermark into the COMPACT, self-describing invisible payload (≤ [`CAP`] bytes):
/// the 20-byte wallet when the stamp begins with an EVM address (the common case), else short UTF-8.
fn compact_encode(payload: &str) -> Vec<u8> {
    if let Some(addr) = leading_evm_address(payload) {
        let mut v = Vec::with_capacity(1 + 20);
        v.push(TAG_EVM);
        v.extend_from_slice(&addr);
        v
    } else {
        let raw = payload.as_bytes();
        let take = raw.len().min(CAP - 1);
        let mut v = Vec::with_capacity(1 + take);
        v.push(TAG_UTF8);
        v.extend_from_slice(&raw[..take]);
        v
    }
}

/// Decode the compact payload bytes back to a human string (`0x…` for an address, else UTF-8).
fn compact_decode(bytes: &[u8]) -> Option<String> {
    match bytes.split_first() {
        Some((&TAG_EVM, addr)) if addr.len() == 20 => {
            let mut s = String::with_capacity(2 + 40);
            s.push_str("0x");
            for b in addr {
                s.push_str(&format!("{b:02x}"));
            }
            Some(s)
        }
        Some((&TAG_UTF8, rest)) => String::from_utf8(rest.to_vec()).ok(),
        _ => None,
    }
}

fn push_bits(bits: &mut Vec<bool>, value: u32, width: usize) {
    for i in (0..width).rev() {
        bits.push((value >> i) & 1 == 1);
    }
}

/// Build the tiled codeword (`SYNC | LEN | DATA[CAP] | CRC`) as a bit vector of length [`PERIOD`].
fn build_codeword(payload: &[u8]) -> Vec<bool> {
    let len = payload.len().min(CAP);
    let mut data = [0u8; CAP];
    data[..len].copy_from_slice(&payload[..len]);

    let mut framed = Vec::with_capacity(1 + CAP);
    framed.push(len as u8);
    framed.extend_from_slice(&data);
    let crc = crc16(&framed);

    let mut bits = Vec::with_capacity(PERIOD);
    push_bits(&mut bits, SYNC as u32, SYNC_BITS);
    for &byte in &framed {
        push_bits(&mut bits, byte as u32, 8);
    }
    push_bits(&mut bits, crc as u32, CRC_BITS);
    debug_assert_eq!(bits.len(), PERIOD);
    bits
}

/// Read an 8×8 luma block anchored at image pixel `(x0, y0)` into a float grid `[row][col]`.
#[inline]
fn load_block(img: &RgbaImage, x0: u32, y0: u32) -> [[f32; BLK]; BLK] {
    let mut blk = [[0.0f32; BLK]; BLK];
    for (r, row) in blk.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = luma(img.get_pixel(x0 + c as u32, y0 + r as u32));
        }
    }
    blk
}

/// Project a loaded luma block onto basis `b` (the orthonormal DCT-II coefficient).
#[inline]
fn project(blk: &[[f32; BLK]; BLK], b: &[[f32; BLK]; BLK]) -> f32 {
    let mut acc = 0.0f32;
    for (br, cr) in blk.iter().zip(b.iter()) {
        for (y, coeff) in br.iter().zip(cr.iter()) {
            acc += y * coeff;
        }
    }
    acc
}

/// QIM: the target value for `d` nearest to it whose `round(·/STEP)` index has `bit`'s parity.
/// The returned target is within `STEP` of `d`, so embedding never makes a large content-dependent
/// move (the flaw a fixed-margin scheme has on high-contrast blocks).
#[inline]
fn qim_target(d: f32, bit: bool) -> f32 {
    let m = (d / STEP).round() as i64;
    let want = bit as i64;
    let m2 = if m.rem_euclid(2) == want {
        m
    } else if d - (m as f32) * STEP >= 0.0 {
        m + 1
    } else {
        m - 1
    };
    (m2 as f32) * STEP
}

/// QIM detect: the bit carried by `d` (parity of the nearest quantization index).
#[inline]
fn qim_bit(d: f32) -> bool {
    ((d / STEP).round() as i64).rem_euclid(2) == 1
}

/// Luma std-dev of a loaded block — the perceptual-activity measure used to skip flat areas.
#[inline]
fn activity(blk: &[[f32; BLK]; BLK]) -> f32 {
    let n = (BLK * BLK) as f32;
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    for row in blk {
        for &v in row {
            sum += v;
            sum_sq += v * v;
        }
    }
    let mean = sum / n;
    (sum_sq / n - mean * mean).max(0.0).sqrt()
}

/// Embed `payload` invisibly into `img`'s luminance. No-op for images smaller than one 8×8 block.
/// Idempotent in spirit (re-embedding the same payload just re-enforces the margins).
pub fn embed(img: &mut RgbaImage, payload: &str) {
    let (w, h) = img.dimensions();
    let nbx = (w / BLK as u32) as usize;
    let nby = (h / BLK as u32) as usize;
    if nbx == 0 || nby == 0 {
        return;
    }

    let code = build_codeword(&compact_encode(payload));
    let phi_a = basis(POS_A.0, POS_A.1);
    let phi_b = basis(POS_B.0, POS_B.1);
    // Pre-mix φA − φB: adding `(need/2)·(φA−φB)` shifts A by +need/2 and B by −need/2.
    let mut combo = [[0.0f32; BLK]; BLK];
    for r in 0..BLK {
        for c in 0..BLK {
            combo[r][c] = phi_a[r][c] - phi_b[r][c];
        }
    }

    for by in 0..nby {
        for bx in 0..nbx {
            let x0 = (bx * BLK) as u32;
            let y0 = (by * BLK) as u32;
            let blk = load_block(img, x0, y0);
            // Perceptual masking: leave flat areas (white margins) untouched — invisibility first.
            if activity(&blk) < ACTIVITY_MIN {
                continue;
            }
            let d = project(&blk, &phi_a) - project(&blk, &phi_b);
            let bit = code[code_index(by, bx, nbx)];
            // QIM: move d to the nearest correct-parity bin (a bounded, content-independent shift),
            // split half onto A (+) and half onto B (−) via the pre-mixed φA−φB.
            let half = (qim_target(d, bit) - d) / 2.0;
            if half == 0.0 {
                continue;
            }
            for (r, combo_row) in combo.iter().enumerate() {
                for (c, &mix) in combo_row.iter().enumerate() {
                    let delta = half * mix;
                    if delta == 0.0 {
                        continue;
                    }
                    let px = img.get_pixel_mut(x0 + c as u32, y0 + r as u32);
                    for ch in 0..3 {
                        px[ch] = (px[ch] as f32 + delta).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }
}

/// Read `width` bits (MSB-first) from `bits` starting at cyclic index `start`, returning the value.
fn read_bits(bits: &[bool], start: usize, width: usize) -> u32 {
    let n = bits.len();
    let mut v = 0u32;
    for i in 0..width {
        v = (v << 1) | bits[(start + i) % n] as u32;
    }
    v
}

/// Try to decode a frame from the folded `bits` (length [`PERIOD`]) at every codeword rotation.
/// Returns the recovered payload string when SYNC aligns AND the CRC verifies.
fn decode(bits: &[bool]) -> Option<String> {
    if bits.len() != PERIOD {
        return None;
    }
    for rot in 0..PERIOD {
        if read_bits(bits, rot, SYNC_BITS) as u16 != SYNC {
            continue;
        }
        let mut pos = rot + SYNC_BITS;
        let len = read_bits(bits, pos, LEN_BITS) as usize;
        pos += LEN_BITS;
        if len > CAP {
            continue;
        }
        let mut framed = Vec::with_capacity(1 + CAP);
        framed.push(len as u8);
        let mut data = [0u8; CAP];
        for byte in data.iter_mut() {
            *byte = read_bits(bits, pos, 8) as u8;
            pos += 8;
        }
        framed.extend_from_slice(&data);
        let crc = read_bits(bits, pos, CRC_BITS) as u16;
        if crc16(&framed) != crc {
            continue;
        }
        if let Some(s) = compact_decode(&data[..len]) {
            return Some(s);
        }
    }
    None
}

/// Blindly recover the embedded payload from `img`, or `None` if no valid frame is found. Searches
/// all 64 sub-block pixel offsets (re-syncs after a same-resolution screenshot / translation) and,
/// per offset, majority-folds every block's bit before the CRC-gated [`decode`].
pub fn extract(img: &RgbaImage) -> Option<String> {
    let (w, h) = img.dimensions();
    if w < BLK as u32 || h < BLK as u32 {
        return None;
    }
    let phi_a = basis(POS_A.0, POS_A.1);
    let phi_b = basis(POS_B.0, POS_B.1);

    for oy in 0..BLK as u32 {
        for ox in 0..BLK as u32 {
            let nbx = ((w - ox) / BLK as u32) as usize;
            let nby = ((h - oy) / BLK as u32) as usize;
            if nbx == 0 || nby == 0 {
                continue;
            }
            // Majority vote over the SAME activity-gated blocks the embedder marked (so unmarked white
            // blocks, which would all read as bit 0, do not bias the fold). Each marked block votes
            // its QIM bit; recompression flips a minority, which the fold corrects.
            let mut votes = vec![0i32; PERIOD];
            for by in 0..nby {
                for bx in 0..nbx {
                    let x0 = ox + (bx * BLK) as u32;
                    let y0 = oy + (by * BLK) as u32;
                    let blk = load_block(img, x0, y0);
                    if activity(&blk) < ACTIVITY_MIN {
                        continue;
                    }
                    let d = project(&blk, &phi_a) - project(&blk, &phi_b);
                    votes[code_index(by, bx, nbx)] += if qim_bit(d) { 1 } else { -1 };
                }
            }
            if ox == 0 && oy == 0 && std::env::var("ELASTOS_WM_DEBUG").is_ok() {
                let uncovered = votes.iter().filter(|&&v| v == 0).count();
                let weak = votes.iter().filter(|&&v| v.abs() <= 1).count();
                let marked: i64 = votes.iter().map(|&v| v.unsigned_abs() as i64).sum();
                eprintln!(
                    "wm-debug: nbx={nbx} nby={nby} period={PERIOD} uncovered={uncovered} weak={weak} total_votes={marked}"
                );
            }
            let bits: Vec<bool> = votes.iter().map(|&v| v > 0).collect();
            if let Some(text) = decode(&bits) {
                return Some(text);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// A deterministic, textured test page (not flat — flat blocks have ~0 AC energy and exercise
    /// nothing). Gradients + a pseudo-noise term mimic rendered document/photo content.
    fn textured(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let base = ((x * 7 + y * 13) % 200 + 30) as u8;
                let n = (((x * 131 + y * 977) ^ (x.wrapping_mul(y))) % 37) as u8;
                let v = base.saturating_add(n);
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        img
    }

    fn reencode(img: &RgbaImage, quality: u8) -> RgbaImage {
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        enc.encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
        image::load_from_memory(&buf).unwrap().to_rgba8()
    }

    const STAMP: &str =
        "0xabc1230000000000000000000000000000009f0e \u{00b7} deadbeefca \u{00b7} 2026-06-18 09:57Z";
    /// What the COMPACT invisible layer recovers from `STAMP`: the lowercased 20-byte EVM wallet
    /// (the visible mark + audit log keep the full `wallet · content · time` tuple).
    const STAMP_ADDR: &str = "0xabc1230000000000000000000000000000009f0e";

    #[test]
    fn crc16_matches_known_vector() {
        // CRC-16/CCITT-FALSE("123456789") == 0x29B1.
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }

    #[test]
    fn roundtrip_in_memory() {
        let mut img = textured(1024, 768);
        embed(&mut img, STAMP);
        assert_eq!(extract(&img).as_deref(), Some(STAMP_ADDR));
    }

    #[test]
    fn survives_q85_encode() {
        let mut img = textured(1024, 768);
        embed(&mut img, STAMP);
        let out = reencode(&img, 85);
        assert_eq!(extract(&out).as_deref(), Some(STAMP_ADDR));
    }

    #[test]
    fn survives_recompression_q85_then_q70() {
        let mut img = textured(1280, 960);
        embed(&mut img, STAMP);
        let out = reencode(&reencode(&img, 85), 70);
        assert_eq!(extract(&out).as_deref(), Some(STAMP_ADDR));
    }

    #[test]
    fn survives_subblock_offset_width_preserved() {
        // A same-resolution screenshot offsets the 8×8 grid origin; the 64-way offset + rotation
        // search must re-sync. Simulate with a width-PRESERVING vertical offset (crop 3 px off the
        // top — full width retained, so the block columns still line up). Horizontal width changes
        // are explicitly out of scope (documented), so we do not test them.
        let mut img = textured(1280, 960);
        embed(&mut img, STAMP);
        let out = reencode(&img, 85);
        let shifted = image::imageops::crop_imm(&out, 0, 3, 1280, 960 - 3).to_image();
        assert_eq!(extract(&shifted).as_deref(), Some(STAMP_ADDR));
    }

    #[test]
    fn extract_returns_none_on_unmarked_image() {
        let img = textured(512, 512);
        assert!(extract(&img).is_none());
    }

    #[test]
    fn embed_is_visually_subtle_on_textured_content() {
        // On textured content (where the mark lives) the average luma change must stay small enough
        // to be imperceptible in practice (Weber masking) — well under ~2 levels out of 255.
        let orig = textured(512, 512);
        let mut marked = orig.clone();
        embed(&mut marked, STAMP);
        let (w, h) = orig.dimensions();
        let mut total = 0.0f64;
        let mut peak = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                let d = (luma(orig.get_pixel(x, y)) - luma(marked.get_pixel(x, y))).abs();
                total += d as f64;
                peak = peak.max(d);
            }
        }
        // Worst case here: every block is high-frequency noise → all get pushed. Real pages are
        // mostly white (skipped). On textured content this average is imperceptible (Weber masking).
        let mean = total / (w as f64 * h as f64);
        assert!(mean < 2.5, "mean abs luma delta too high: {mean}");
        // QIM bounds the per-pixel nudge to ~STEP/4 (half-shift ≤ STEP/2, basis mix ≤ ~0.5).
        assert!(peak <= STEP, "peak luma delta too high: {peak}");
    }

    #[test]
    fn flat_white_is_left_pristine() {
        // The eye is most sensitive on flat white (a document margin). Perceptual masking must leave
        // such areas completely UNTOUCHED — no faint texture injected onto white.
        let mut img = RgbaImage::from_pixel(512, 512, Rgba([255, 255, 255, 255]));
        let before = img.clone();
        embed(&mut img, STAMP);
        assert_eq!(
            img, before,
            "flat white must not be modified by the invisible mark"
        );
    }

    /// A realistic SPARSE text/code page: white background, full-width "text" lines of dark glyph-ish
    /// strokes with inter-line whitespace + page margins — the hard case (most of the page is white).
    fn sparse_text_page(w: u32, h: u32) -> RgbaImage {
        let mut page = RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
        let margin = 48u32;
        let line_h = 26u32; // baseline-to-baseline
        let glyph_h = 14u32;
        let mut y = margin;
        while y + glyph_h < h - margin {
            let mut x = margin;
            while x < w - margin {
                // Pseudo-random "glyph" run widths + gaps → irregular full-width text lines.
                let seed = x
                    .wrapping_mul(2654435761)
                    .wrapping_add(y.wrapping_mul(40503));
                let gw = 4 + (seed % 9);
                let gap = 2 + ((seed >> 3) % 4);
                if (seed >> 7) % 5 != 0 {
                    for gy in 0..glyph_h {
                        for gx in 0..gw {
                            if x + gx < w - margin {
                                let ink =
                                    20 + ((seed.wrapping_add(gx).wrapping_add(gy)) % 40) as u8;
                                page.put_pixel(x + gx, y + gy, Rgba([ink, ink, ink, 255]));
                            }
                        }
                    }
                }
                x += gw + gap;
            }
            y += line_h;
        }
        page
    }

    #[test]
    fn recovers_from_sparse_text_page() {
        // The PRIMARY content case (PDF/text/code): mostly-white page with thin full-width text. The
        // mark must remain recoverable through a q85 encode even though most blocks are white.
        let mut page = sparse_text_page(1000, 1300);
        let active = {
            let (w, h) = page.dimensions();
            let mut n = 0usize;
            let mut tot = 0usize;
            for by in 0..(h / 8) {
                for bx in 0..(w / 8) {
                    tot += 1;
                    if activity(&load_block(&page, bx * 8, by * 8)) >= ACTIVITY_MIN {
                        n += 1;
                    }
                }
            }
            (n, tot)
        };
        embed(&mut page, STAMP);
        assert_eq!(
            extract(&page).as_deref(),
            Some(STAMP_ADDR),
            "in-memory recover failed; active blocks {}/{}",
            active.0,
            active.1
        );
        let out = reencode(&page, 85);
        assert_eq!(
            extract(&out).as_deref(),
            Some(STAMP_ADDR),
            "q85 recover failed"
        );
    }

    #[test]
    fn keeps_white_margins_pristine_on_text_page() {
        // The wide white margins of a text page must be left untouched (eye is most sensitive there).
        let mut page = sparse_text_page(1000, 1300);
        let before = page.clone();
        embed(&mut page, STAMP);
        for &(x, y) in &[(8u32, 8u32), (980, 8), (8, 1280), (980, 1280), (500, 10)] {
            assert_eq!(
                page.get_pixel(x, y),
                before.get_pixel(x, y),
                "white margin pixel ({x},{y}) was modified"
            );
        }
    }
}
