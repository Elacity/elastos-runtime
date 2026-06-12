//! DASH MPD generator — a byte-faithful Rust port of PC2's
//! `pc2-node/src/services/media/mpdGenerator.ts` (`Elacity/pc2.net`).
//!
//! It emits the exact same `SegmentTemplate` + `SegmentTimeline` MPD that PC2's
//! `mpdParser.ts`/playback expects, so an asset packaged here plays through the
//! same code path. Parity is by-construction: the line layout, indentation,
//! attribute order, and duration formatting mirror the TypeScript source.
//!
//! Deliberately, like PC2, the MPD carries NO `ContentProtection`/EME element:
//! decryption is out-of-band (Lit in PC2; the in-VM `decrypt-provider` here,
//! which returns CLEAR fragments). The content key id (KID) therefore lives in
//! the dDRM metadata envelope, never in the manifest.
//!
//! Pure: no I/O, no external tools, no key material.

use serde::{Deserialize, Serialize};

/// One media track's descriptor, mirroring PC2 `mp4split.ts` `TrackInfo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub track_id: u32,
    pub kind: TrackKind,
    pub codec: String,
    pub timescale: u32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bandwidth: u64,
    pub audio_sample_rate: Option<u32>,
    pub audio_channels: Option<u32>,
}

/// `'video' | 'audio'` in PC2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Video,
    Audio,
}

impl TrackKind {
    fn as_str(self) -> &'static str {
        match self {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        }
    }
}

/// One segment's timing, mirroring PC2 `mp4split.ts` `SegmentInfo`. `duration` is
/// already an integer in the track's timescale units (the sum of `trun` sample
/// durations), so no rounding is ever applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub track_id: u32,
    pub duration: u64,
}

/// MPD segment timing (PC2 `MPDSegment`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpdSegment {
    pub duration: u64,
}

/// A track ready for MPD emission (PC2 `MPDTrack`): its info, ordered segments,
/// and the `initialization`/`media` template patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpdTrack {
    pub info: TrackInfo,
    pub segments: Vec<MpdSegment>,
    pub init_filename: String,
    pub media_pattern: String,
}

/// `formatDuration(seconds)` → ISO-8601 `PT..H..M..S` with 3-decimal seconds.
/// Mirrors the TS `Math.floor` + float `% 60` + `toFixed(3)` semantics exactly.
fn format_duration(seconds: f64) -> String {
    let hours = (seconds / 3600.0).floor() as i64;
    let mins = ((seconds % 3600.0) / 60.0).floor() as i64;
    let secs = seconds % 60.0;

    let mut result = String::from("PT");
    if hours > 0 {
        result.push_str(&format!("{hours}H"));
    }
    if mins > 0 {
        result.push_str(&format!("{mins}M"));
    }
    result.push_str(&format!("{secs:.3}S"));
    result
}

/// `escapeXml(str)` — identical replacement order to the TS source.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// `buildSegmentTimeline(segments)` — run-length-encoded `<S>` entries. The first
/// entry carries `t="0"`; consecutive equal durations collapse via `r="..."`.
fn build_segment_timeline(segments: &[MpdSegment]) -> String {
    if segments.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push("            <SegmentTimeline>".to_string());

    let mut i = 0usize;
    while i < segments.len() {
        let d = segments[i].duration;
        let mut r = 0usize;
        while i + r + 1 < segments.len() && segments[i + r + 1].duration == d {
            r += 1;
        }

        if i == 0 {
            if r > 0 {
                lines.push(format!("              <S t=\"0\" d=\"{d}\" r=\"{r}\"/>"));
            } else {
                lines.push(format!("              <S t=\"0\" d=\"{d}\"/>"));
            }
        } else if r > 0 {
            lines.push(format!("              <S d=\"{d}\" r=\"{r}\"/>"));
        } else {
            lines.push(format!("              <S d=\"{d}\"/>"));
        }

        i += r + 1;
    }

    lines.push("            </SegmentTimeline>".to_string());
    lines.join("\n")
}

/// `computeEffectiveDuration` — longest track's duration from its SegmentTimeline
/// (so `mediaPresentationDuration` exactly equals what the player can fetch).
fn compute_effective_duration(tracks: &[MpdTrack], fallback_seconds: f64) -> f64 {
    let mut best_seconds = 0.0f64;
    for track in tracks {
        if track.segments.is_empty() || track.info.timescale == 0 {
            continue;
        }
        let sum_units: u64 = track.segments.iter().map(|s| s.duration).sum();
        let seconds = sum_units as f64 / track.info.timescale as f64;
        if seconds > best_seconds {
            best_seconds = seconds;
        }
    }
    if best_seconds > 0.0 {
        best_seconds
    } else {
        fallback_seconds
    }
}

/// `generateMPD(tracks, totalDuration)` — emit the full MPD XML. Byte-faithful to
/// PC2: same root attributes, indentation, attribute order, and the audio
/// `audioSamplingRate` quirk on both AdaptationSet and Representation.
pub fn generate_mpd(tracks: &[MpdTrack], total_duration: f64) -> String {
    let effective_duration = compute_effective_duration(tracks, total_duration);

    let mut lines: Vec<String> = Vec::new();
    lines.push("<?xml version=\"1.0\" encoding=\"utf-8\"?>".to_string());
    lines.push(format!(
        "<MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns=\"urn:mpeg:dash:schema:mpd:2011\" xsi:schemaLocation=\"urn:mpeg:dash:schema:mpd:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\" type=\"static\" mediaPresentationDuration=\"{}\" minBufferTime=\"PT2S\" profiles=\"urn:mpeg:dash:profile:isoff-on-demand:2011\">",
        format_duration(effective_duration)
    ));
    lines.push("  <Period>".to_string());

    for track in tracks {
        let info = &track.info;
        let mime_type = match info.kind {
            TrackKind::Video => "video/mp4",
            TrackKind::Audio => "audio/mp4",
        };
        let content_type = info.kind.as_str();

        let mut as_attrs = format!(
            "mimeType=\"{}\" contentType=\"{}\"",
            escape_xml(mime_type),
            content_type
        );
        if info.kind == TrackKind::Video {
            if let (Some(w), Some(h)) = (info.width, info.height) {
                as_attrs.push_str(&format!(" maxWidth=\"{w}\" maxHeight=\"{h}\""));
            }
        }
        if let Some(rate) = info.audio_sample_rate {
            as_attrs.push_str(&format!(" audioSamplingRate=\"{rate}\""));
        }

        lines.push(format!("    <AdaptationSet {as_attrs}>"));

        let rep_id = format!(
            "{}{}",
            if info.kind == TrackKind::Video { "v" } else { "a" },
            info.track_id
        );
        let mut rep_attrs = format!(
            "id=\"{}\" codecs=\"{}\" bandwidth=\"{}\"",
            rep_id,
            escape_xml(&info.codec),
            info.bandwidth
        );
        if info.kind == TrackKind::Video {
            if let (Some(w), Some(h)) = (info.width, info.height) {
                rep_attrs.push_str(&format!(" width=\"{w}\" height=\"{h}\""));
            }
        }
        if info.audio_channels.is_some() {
            if let Some(rate) = info.audio_sample_rate {
                rep_attrs.push_str(&format!(" audioSamplingRate=\"{rate}\""));
            }
        }

        lines.push(format!("      <Representation {rep_attrs}>"));

        let seg_timeline = build_segment_timeline(&track.segments);
        lines.push(format!(
            "        <SegmentTemplate timescale=\"{}\" initialization=\"{}\" media=\"{}\" startNumber=\"1\">",
            info.timescale,
            escape_xml(&track.init_filename),
            escape_xml(&track.media_pattern)
        ));
        lines.push(seg_timeline);
        lines.push("        </SegmentTemplate>".to_string());

        lines.push("      </Representation>".to_string());
        lines.push("    </AdaptationSet>".to_string());
    }

    lines.push("  </Period>".to_string());
    lines.push("</MPD>".to_string());

    lines.join("\n")
}

/// `buildMPDTracks(trackInfos, segmentInfos)` — pair each track with its ordered
/// segments and the `video|audio/<trackId>/...` template paths PC2 uses.
pub fn build_mpd_tracks(track_infos: &[TrackInfo], segment_infos: &[SegmentInfo]) -> Vec<MpdTrack> {
    track_infos
        .iter()
        .map(|info| {
            let dir_name = format!("{}/{}", info.kind.as_str(), info.track_id);
            let segments = segment_infos
                .iter()
                .filter(|s| s.track_id == info.track_id)
                .map(|s| MpdSegment { duration: s.duration })
                .collect();
            MpdTrack {
                info: info.clone(),
                segments,
                init_filename: format!("{dir_name}/init.mp4"),
                media_pattern: format!("{dir_name}/seg-$Number$.m4s"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_track() -> TrackInfo {
        TrackInfo {
            track_id: 1,
            kind: TrackKind::Video,
            codec: "avc1.640028".to_string(),
            timescale: 12800,
            width: Some(1920),
            height: Some(1080),
            bandwidth: 4_500_000,
            audio_sample_rate: None,
            audio_channels: None,
        }
    }

    #[test]
    fn format_duration_matches_tofixed3() {
        assert_eq!(format_duration(0.0), "PT0.000S");
        assert_eq!(format_duration(5.5), "PT5.500S");
        assert_eq!(format_duration(65.25), "PT1M5.250S");
        assert_eq!(format_duration(3725.0), "PT1H2M5.000S");
    }

    #[test]
    fn escape_xml_order() {
        assert_eq!(escape_xml("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
    }

    #[test]
    fn segment_timeline_run_length_encodes() {
        let segs = vec![
            MpdSegment { duration: 1000 },
            MpdSegment { duration: 1000 },
            MpdSegment { duration: 1000 },
            MpdSegment { duration: 512 },
        ];
        let timeline = build_segment_timeline(&segs);
        assert_eq!(
            timeline,
            "            <SegmentTimeline>\n              <S t=\"0\" d=\"1000\" r=\"2\"/>\n              <S d=\"512\"/>\n            </SegmentTimeline>"
        );
    }

    #[test]
    fn empty_segment_timeline_is_blank() {
        assert_eq!(build_segment_timeline(&[]), "");
    }

    #[test]
    fn build_tracks_uses_pc2_template_paths() {
        let infos = vec![video_track()];
        let segs = vec![
            SegmentInfo { track_id: 1, duration: 1000 },
            SegmentInfo { track_id: 2, duration: 999 },
        ];
        let tracks = build_mpd_tracks(&infos, &segs);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].init_filename, "video/1/init.mp4");
        assert_eq!(tracks[0].media_pattern, "video/1/seg-$Number$.m4s");
        // Only the matching trackId segment is kept.
        assert_eq!(tracks[0].segments, vec![MpdSegment { duration: 1000 }]);
    }

    #[test]
    fn generate_mpd_single_video_track_is_byte_faithful() {
        let infos = vec![video_track()];
        let segs = vec![
            SegmentInfo { track_id: 1, duration: 25600 },
            SegmentInfo { track_id: 1, duration: 25600 },
        ];
        let tracks = build_mpd_tracks(&infos, &segs);
        let mpd = generate_mpd(&tracks, 0.0);
        let expected = concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
            "<MPD xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns=\"urn:mpeg:dash:schema:mpd:2011\" xsi:schemaLocation=\"urn:mpeg:dash:schema:mpd:2011 http://standards.iso.org/ittf/PubliclyAvailableStandards/MPEG-DASH_schema_files/DASH-MPD.xsd\" type=\"static\" mediaPresentationDuration=\"PT4.000S\" minBufferTime=\"PT2S\" profiles=\"urn:mpeg:dash:profile:isoff-on-demand:2011\">\n",
            "  <Period>\n",
            "    <AdaptationSet mimeType=\"video/mp4\" contentType=\"video\" maxWidth=\"1920\" maxHeight=\"1080\">\n",
            "      <Representation id=\"v1\" codecs=\"avc1.640028\" bandwidth=\"4500000\" width=\"1920\" height=\"1080\">\n",
            "        <SegmentTemplate timescale=\"12800\" initialization=\"video/1/init.mp4\" media=\"video/1/seg-$Number$.m4s\" startNumber=\"1\">\n",
            "            <SegmentTimeline>\n",
            "              <S t=\"0\" d=\"25600\" r=\"1\"/>\n",
            "            </SegmentTimeline>\n",
            "        </SegmentTemplate>\n",
            "      </Representation>\n",
            "    </AdaptationSet>\n",
            "  </Period>\n",
            "</MPD>"
        );
        assert_eq!(mpd, expected);
    }
}
