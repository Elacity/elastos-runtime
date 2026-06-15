//! ElastOS Media Provider Capsule
//!
//! The runtime-native analogue of PC2's host-side media packaging pipeline
//! (`pc2-node/src/services/media/{encoder,mp4split,mpdGenerator}.ts`): take a
//! plaintext source asset and produce the **fragmented, multi-bitrate DASH
//! segments + track/segment metadata** a CENC packager consumes.
//!
//! Crucially, this provider holds **NO key material** (PRINCIPLE #15 — trust and
//! access travel with signed content; the CEK never crosses a process boundary).
//! It emits only PLAINTEXT segments + structural metadata. The minting of the CEK,
//! the CENC encryption of these segments, and the dKMS escrow all happen *inside*
//! `encrypt-provider`. The split mirrors PC2 (transcode/fragment is the host's job;
//! the cipher is the wasm boundary's) while keeping the runtime's containment.
//!
//! No ambient authority (PRINCIPLE #3): the only external tool is ffmpeg/ffprobe —
//! the same dependency PC2 has — and its path + a scratch directory are supplied by
//! a narrow operator config (`ELASTOS_MEDIA_PROVIDER_CONFIG`). No network. Confined
//! to the scratch dir. Unconfigured ⇒ explicit `not_configured` error, never a
//! silent skip (PRINCIPLE #11 — fail closed).
//!
//! Parity anchors (from PC2 source):
//!   transcode: `libx264 -crf {ladder} -preset slow -profile:v <p> -pix_fmt yuv420p`
//!              + `aac -b:a 128k`;
//!   fragment:  `-movflags +frag_keyframe+empty_moov+default_base_moof+separate_moof`
//!              (`+separate_moof` ⇒ one `traf` per `moof`, which `ddrm-media::mp4`
//!              and the CENC sample encryptor both assume);
//!   split + metadata: `ddrm-media::mp4::{split_fragmented, parse_fragment_metadata}`
//!              (faithful ports of `mp4split.ts`).

use ddrm_media::{mp4, mpd};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const PACKAGE_RESPONSE_SCHEMA: &str = "elastos.media.package/v1";
const CONFIG_ENV: &str = "ELASTOS_MEDIA_PROVIDER_CONFIG";

// ---------------------------------------------------------------------------
// Operator config (PRINCIPLE #3: a narrow capability handed in, not discovered).
// ---------------------------------------------------------------------------

/// One rung of the bitrate ladder. Mirrors the knobs PC2's `encoder.ts` exposes
/// per rendition. `height` drives `scale=-2:height` (width auto, even); `crf` and
/// `profile` follow PC2's quality tiers.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Rendition {
    id: String,
    height: u32,
    crf: u32,
    #[serde(default = "default_preset")]
    preset: String,
    #[serde(default = "default_profile")]
    profile: String,
    #[serde(default = "default_audio_bitrate")]
    audio_bitrate: String,
}

fn default_preset() -> String {
    "slow".to_string()
}
fn default_profile() -> String {
    "high".to_string()
}
fn default_audio_bitrate() -> String {
    "128k".to_string()
}

/// The default ladder — PC2-shaped quality tiers (crf 23/24/26, profile by tier).
/// Rungs above the source height are dropped at package time (never upscale).
fn default_ladder() -> Vec<Rendition> {
    vec![
        Rendition {
            id: "1080p".into(),
            height: 1080,
            crf: 23,
            preset: "slow".into(),
            profile: "high".into(),
            audio_bitrate: "128k".into(),
        },
        Rendition {
            id: "720p".into(),
            height: 720,
            crf: 23,
            preset: "slow".into(),
            profile: "high".into(),
            audio_bitrate: "128k".into(),
        },
        Rendition {
            id: "480p".into(),
            height: 480,
            crf: 24,
            preset: "slow".into(),
            profile: "main".into(),
            audio_bitrate: "128k".into(),
        },
        Rendition {
            id: "360p".into(),
            height: 360,
            crf: 26,
            preset: "slow".into(),
            profile: "baseline".into(),
            audio_bitrate: "128k".into(),
        },
    ]
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaConfig {
    #[serde(default)]
    ffmpeg_path: Option<String>,
    #[serde(default)]
    ffprobe_path: Option<String>,
    #[serde(default)]
    scratch_dir: Option<String>,
    #[serde(default)]
    renditions: Vec<Rendition>,
}

impl MediaConfig {
    /// Load from the `ELASTOS_MEDIA_PROVIDER_CONFIG` env var, which is either inline
    /// JSON (`{...}`) or a path to a JSON file. Absent ⇒ empty (fail-closed) config.
    fn from_env() -> Result<Self, String> {
        let Ok(raw) = std::env::var(CONFIG_ENV) else {
            return Ok(MediaConfig::default());
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(MediaConfig::default());
        }
        let text = if trimmed.starts_with('{') {
            trimmed.to_string()
        } else {
            std::fs::read_to_string(trimmed)
                .map_err(|e| format!("failed to read {CONFIG_ENV} file {trimmed}: {e}"))?
        };
        serde_json::from_str(&text).map_err(|e| format!("invalid {CONFIG_ENV} JSON: {e}"))
    }

    /// Overlay an `init`-supplied config object (operator may also configure at init).
    fn merge_init(&mut self, init: &Value) {
        if let Some(v) = init.get("ffmpeg_path").and_then(Value::as_str) {
            self.ffmpeg_path = Some(v.to_string());
        }
        if let Some(v) = init.get("ffprobe_path").and_then(Value::as_str) {
            self.ffprobe_path = Some(v.to_string());
        }
        if let Some(v) = init.get("scratch_dir").and_then(Value::as_str) {
            self.scratch_dir = Some(v.to_string());
        }
        if let Some(arr) = init.get("renditions") {
            if let Ok(r) = serde_json::from_value::<Vec<Rendition>>(arr.clone()) {
                self.renditions = r;
            }
        }
    }

    fn ladder(&self) -> Vec<Rendition> {
        if self.renditions.is_empty() {
            default_ladder()
        } else {
            self.renditions.clone()
        }
    }

    /// The effective tooling, or a fail-closed reason why packaging can't run.
    fn resolve(&self) -> Result<ResolvedTools, String> {
        let ffmpeg = self
            .ffmpeg_path
            .clone()
            .ok_or("ffmpeg_path not configured (set it in ELASTOS_MEDIA_PROVIDER_CONFIG)")?;
        let scratch = self
            .scratch_dir
            .clone()
            .ok_or("scratch_dir not configured (set it in ELASTOS_MEDIA_PROVIDER_CONFIG)")?;
        // ffprobe defaults to a sibling of ffmpeg if unset, but never to bare PATH.
        let ffprobe = self.ffprobe_path.clone().unwrap_or_else(|| {
            let p = Path::new(&ffmpeg);
            match p.parent() {
                Some(dir) => dir.join("ffprobe").to_string_lossy().to_string(),
                None => "ffprobe".to_string(),
            }
        });
        if !Path::new(&ffmpeg).exists() {
            return Err(format!("configured ffmpeg not found at {ffmpeg}"));
        }
        Ok(ResolvedTools {
            ffmpeg,
            ffprobe,
            scratch: PathBuf::from(scratch),
        })
    }
}

struct ResolvedTools {
    ffmpeg: String,
    ffprobe: String,
    scratch: PathBuf,
}

// ---------------------------------------------------------------------------
// Wire protocol (stdio JSON line per request — matches encrypt-provider).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    /// Package a plaintext source asset into fragmented DASH renditions.
    /// Returns PLAINTEXT segments + per-track/segment metadata — no key material.
    Package {
        /// The source asset bytes (handed in by a content/storage capability; this
        /// boundary does NOT fetch). Base64.
        content_b64: String,
        /// Optional source filename hint (used only for the scratch input extension).
        #[serde(default)]
        filename: Option<String>,
    },
    /// Package a source asset into a DASH DIRECTORY (one rendition): per-track standalone
    /// init + segments named in the PC2 layout + the `manifest.mpd`. PLAINTEXT — the creator
    /// route CENC-encrypts + escrows under one asset CEK before publishing.
    PackageDash {
        content_b64: String,
        #[serde(default)]
        filename: Option<String>,
        /// Optional free-preview length in seconds. When > 0 and the source is longer, an
        /// UNENCRYPTED, lower-quality first-N-seconds clip is produced (PC2 `previewDuration`,
        /// `media.ts:1753`). Capped at 60s. The clip never carries the CEK — it's a teaser.
        #[serde(default)]
        preview_duration: Option<u64>,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }
    fn empty_ok() -> Self {
        Response::Ok { data: None }
    }
    fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Default)]
struct MediaProvider {
    config: MediaConfig,
}

impl MediaProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Package {
                content_b64,
                filename,
            } => self.package(&content_b64, filename.as_deref()),
            Request::PackageDash {
                content_b64,
                filename,
                preview_duration,
            } => self.package_dash(&content_b64, filename.as_deref(), preview_duration.unwrap_or(0)),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        match MediaConfig::from_env() {
            Ok(mut cfg) => {
                if config.is_object() {
                    cfg.merge_init(&config);
                }
                self.config = cfg;
            }
            Err(e) => return Response::error("invalid_config", e),
        }
        let configured = self.config.resolve().is_ok();
        Response::ok(json!({
            "provider": "media-provider",
            "version": PROVIDER_VERSION,
            "configured": configured,
            "supported_operations": ["status", "package", "package_dash"],
        }))
    }

    fn status(&self) -> Response {
        let (configured, reason) = match self.config.resolve() {
            Ok(_) => (true, Value::Null),
            Err(e) => (false, json!(e)),
        };
        Response::ok(json!({
            "provider": "media-provider",
            "version": PROVIDER_VERSION,
            "configured": configured,
            "not_configured_reason": reason,
            "ladder": self.config.ladder().iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "supported_operations": ["status", "package", "package_dash"],
        }))
    }

    fn package(&self, content_b64: &str, filename: Option<&str>) -> Response {
        let tools = match self.config.resolve() {
            Ok(t) => t,
            Err(e) => return Response::error("not_configured", e),
        };

        let bytes = match base64_decode(content_b64) {
            Ok(b) => b,
            Err(e) => return Response::error("invalid_request", format!("content_b64: {e}")),
        };
        if bytes.is_empty() {
            return Response::error("invalid_request", "content_b64 decoded to 0 bytes");
        }

        let workdir = match self.make_workdir(&tools.scratch) {
            Ok(d) => d,
            Err(e) => return Response::error("scratch_error", e),
        };

        let result = self.package_in(&tools, &workdir, &bytes, filename);

        // Best-effort cleanup — never leak plaintext in the scratch dir.
        let _ = std::fs::remove_dir_all(&workdir);

        match result {
            Ok(data) => Response::ok(data),
            Err(e) => Response::error("package_failed", e),
        }
    }

    fn make_workdir(&self, scratch: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(scratch)
            .map_err(|e| format!("cannot create scratch dir {}: {e}", scratch.display()))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = scratch.join(format!("mp-{}-{}", std::process::id(), nonce));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create workdir: {e}"))?;
        Ok(dir)
    }

    fn package_in(
        &self,
        tools: &ResolvedTools,
        workdir: &Path,
        bytes: &[u8],
        filename: Option<&str>,
    ) -> Result<Value, String> {
        let ext = filename
            .and_then(|f| Path::new(f).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let input = workdir.join(format!("source.{ext}"));
        std::fs::write(&input, bytes).map_err(|e| format!("write source: {e}"))?;

        let probe = probe_source(&tools.ffprobe, &input)?;

        // Drop ladder rungs above the source height (never upscale); always keep at
        // least one rung. Clamp the top rung's height to the source height.
        let mut ladder: Vec<Rendition> = self
            .config
            .ladder()
            .into_iter()
            .filter(|r| probe.height == 0 || r.height <= probe.height)
            .collect();
        if ladder.is_empty() {
            let mut top = self.config.ladder().into_iter().next().unwrap();
            if probe.height > 0 {
                top.height = probe.height;
            }
            ladder.push(top);
        }

        let mut renditions_out: Vec<Value> = Vec::new();
        for r in &ladder {
            let rd = self.package_rendition(tools, workdir, &input, r, &probe)?;
            renditions_out.push(rd);
        }

        Ok(json!({
            "schema": PACKAGE_RESPONSE_SCHEMA,
            "source": {
                "width": probe.width,
                "height": probe.height,
                "duration": probe.duration,
                "has_audio": probe.has_audio,
            },
            "renditions": renditions_out,
        }))
    }

    fn package_rendition(
        &self,
        tools: &ResolvedTools,
        workdir: &Path,
        input: &Path,
        r: &Rendition,
        probe: &ProbeResult,
    ) -> Result<Value, String> {
        let frag_bytes = self.fragment_rendition(tools, workdir, input, r, probe)?;
        let split = mp4::split_fragmented(&frag_bytes)?;
        let meta = mp4::parse_fragment_metadata(&frag_bytes)?;

        let segments_b64: Vec<String> = split.fragments.iter().map(|f| base64_encode(f)).collect();

        Ok(json!({
            "rendition_id": r.id,
            "height": r.height,
            "init_b64": base64_encode(&split.init),
            "segments_b64": segments_b64,
            "tracks": meta.tracks,
            "segment_durations": meta.segments,
            "total_duration": meta.total_duration,
        }))
    }

    /// Transcode + DASH-fragment one rung, returning the fragmented MP4 bytes. The two
    /// ffmpeg steps mirror PC2 `encoder.ts` exactly (transcode then `-c copy` fragment
    /// with the movflags the CENC packager + ddrm-media::mp4 assume).
    fn fragment_rendition(
        &self,
        tools: &ResolvedTools,
        workdir: &Path,
        input: &Path,
        r: &Rendition,
        probe: &ProbeResult,
    ) -> Result<Vec<u8>, String> {
        let transcoded = workdir.join(format!("t-{}.mp4", r.id));
        let fragmented = workdir.join(format!("f-{}.mp4", r.id));

        // Step 1 — transcode. PC2 splits video and audio-only sources into two encode
        // shapes (`encoder.ts`): a video source is scaled + libx264'd (+ aac if it carries
        // sound); an audio-only source skips the video filter/codec entirely (a `-vf scale`
        // on a streamless input would abort ffmpeg) and just re-encodes to aac. Both then
        // fragment identically below.
        let has_video = probe.width > 0 || probe.height > 0;
        let mut tx = Command::new(&tools.ffmpeg);
        tx.arg("-y").arg("-i").arg(input);
        if has_video {
            tx.arg("-vf")
                .arg(format!("scale=-2:{}", r.height))
                .arg("-c:v")
                .arg("libx264")
                .arg("-crf")
                .arg(r.crf.to_string())
                .arg("-preset")
                .arg(&r.preset)
                .arg("-profile:v")
                .arg(&r.profile)
                .arg("-pix_fmt")
                .arg("yuv420p");
            if probe.has_audio {
                tx.arg("-c:a").arg("aac").arg("-b:a").arg(&r.audio_bitrate);
            } else {
                tx.arg("-an");
            }
        } else {
            // Audio-only rung: drop any (non-existent) video, encode to aac.
            tx.arg("-vn")
                .arg("-c:a")
                .arg("aac")
                .arg("-b:a")
                .arg(&r.audio_bitrate);
        }
        tx.arg(&transcoded);
        run_ffmpeg(&mut tx, &format!("transcode {}", r.id))?;

        // Step 2 — fragment: copy streams into a fragmented MP4.
        let mut fr = Command::new(&tools.ffmpeg);
        fr.arg("-y")
            .arg("-i")
            .arg(&transcoded)
            .arg("-c")
            .arg("copy")
            .arg("-movflags")
            .arg("+frag_keyframe+empty_moov+default_base_moof+separate_moof")
            .arg(&fragmented);
        run_ffmpeg(&mut fr, &format!("fragment {}", r.id))?;

        std::fs::read(&fragmented).map_err(|e| format!("read fragmented {}: {e}", r.id))
    }

    /// Package a source asset into a DASH directory for ONE rendition (the top rung the
    /// source supports): per-track standalone init + media fragments, named in the PC2
    /// layout (`<kind>/<track_id>/init.mp4`, `seg-<N>.m4s`), plus the `manifest.mpd` that
    /// references them. Segments are PLAINTEXT — the creator route CENC-encrypts + escrows
    /// them under a single asset CEK before publishing (PRINCIPLE #15). The full multi-rung
    /// adaptive ladder is P4.
    fn package_dash(
        &self,
        content_b64: &str,
        filename: Option<&str>,
        preview_duration: u64,
    ) -> Response {
        let tools = match self.config.resolve() {
            Ok(t) => t,
            Err(e) => return Response::error("not_configured", e),
        };
        let bytes = match base64_decode(content_b64) {
            Ok(b) if !b.is_empty() => b,
            Ok(_) => return Response::error("invalid_request", "content_b64 decoded to 0 bytes"),
            Err(e) => return Response::error("invalid_request", format!("content_b64: {e}")),
        };
        let workdir = match self.make_workdir(&tools.scratch) {
            Ok(d) => d,
            Err(e) => return Response::error("scratch_error", e),
        };
        let result = self.package_dash_in(&tools, &workdir, &bytes, filename, preview_duration);
        let _ = std::fs::remove_dir_all(&workdir);
        match result {
            Ok(data) => Response::ok(data),
            Err(e) => Response::error("package_failed", e),
        }
    }

    fn package_dash_in(
        &self,
        tools: &ResolvedTools,
        workdir: &Path,
        bytes: &[u8],
        filename: Option<&str>,
        preview_duration: u64,
    ) -> Result<Value, String> {
        let ext = filename
            .and_then(|f| Path::new(f).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let input = workdir.join(format!("source.{ext}"));
        std::fs::write(&input, bytes).map_err(|e| format!("write source: {e}"))?;

        let probe = probe_source(&tools.ffprobe, &input)?;

        // The top rung the source supports (never upscale); clamp its height to the source.
        let mut top = self
            .config
            .ladder()
            .into_iter()
            .filter(|r| probe.height == 0 || r.height <= probe.height)
            .next()
            .or_else(|| self.config.ladder().into_iter().next())
            .ok_or("empty ladder")?;
        if probe.height > 0 && top.height > probe.height {
            top.height = probe.height;
        }

        // Free preview (PC2 `media.ts:1753`): an UNENCRYPTED first-N-seconds teaser at reduced
        // quality. Produced only when requested AND the source is longer than the window. The
        // clip carries no CEK — it's a public sample; the full asset stays dKMS-encrypted.
        let preview = if preview_duration > 0 {
            let secs = preview_duration.min(PREVIEW_MAX_SECONDS);
            if probe.duration > secs as f64 {
                let has_video = probe.width > 0 || probe.height > 0;
                match make_preview_clip(tools, workdir, &input, secs, has_video) {
                    Ok(bytes) => Some((bytes, secs)),
                    Err(e) => {
                        // Non-fatal: a preview failure must never block the mint.
                        eprintln!("media-provider: preview clip skipped ({e})");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let frag_bytes = self.fragment_rendition(tools, workdir, &input, &top, &probe)?;
        let streams = mp4::demux_tracks(&frag_bytes)?;
        let meta = mp4::parse_fragment_metadata(&frag_bytes)?;

        // The MPD is generated from per-track infos + segment durations; build_mpd_tracks
        // emits the exact `<kind>/<track_id>/{init.mp4,seg-$Number$.m4s}` paths we name below.
        let track_infos: Vec<mpd::TrackInfo> = streams.iter().map(|s| s.info.clone()).collect();
        let mpd_tracks = mpd::build_mpd_tracks(&track_infos, &meta.segments);
        let manifest = mpd::generate_mpd(&mpd_tracks, meta.total_duration);

        let mut tracks_out: Vec<Value> = Vec::with_capacity(streams.len());
        for s in &streams {
            let kind = match s.info.kind {
                mpd::TrackKind::Video => "video",
                mpd::TrackKind::Audio => "audio",
            };
            let dir = format!("{kind}/{}", s.info.track_id);
            let segments: Vec<Value> = s
                .segments
                .iter()
                .enumerate()
                .map(|(i, seg)| {
                    json!({
                        "path": format!("{dir}/seg-{}.m4s", i + 1),
                        "b64": base64_encode(seg),
                    })
                })
                .collect();
            tracks_out.push(json!({
                "kind": kind,
                "track_id": s.info.track_id,
                "codec": s.info.codec,
                "bandwidth": s.info.bandwidth,
                "timescale": s.info.timescale,
                "width": s.info.width,
                "height": s.info.height,
                "audio_sample_rate": s.info.audio_sample_rate,
                "audio_channels": s.info.audio_channels,
                "dir": dir,
                "init_path": format!("{dir}/init.mp4"),
                "init_b64": base64_encode(&s.init),
                "segment_count": s.segments.len(),
                "segments": segments,
            }));
        }

        let mut out = json!({
            "schema": "elastos.media.dash/v1",
            "rendition_id": top.id,
            "manifest_path": "manifest.mpd",
            "mpd": manifest,
            "total_duration": meta.total_duration,
            "source": {
                "width": probe.width,
                "height": probe.height,
                "duration": probe.duration,
                "has_audio": probe.has_audio,
            },
            // Presentation hints surfaced into the public envelope (PC2 `media.{duration,
            // resolution,codec}`): the top rendition's resolution + the lead video codec.
            "resolution": if probe.width > 0 && probe.height > 0 {
                json!(format!("{}x{}", probe.width, probe.height))
            } else {
                Value::Null
            },
            "codec": streams
                .iter()
                .find(|s| matches!(s.info.kind, mpd::TrackKind::Video))
                .or_else(|| streams.first())
                .map(|s| json!(s.info.codec))
                .unwrap_or(Value::Null),
            "tracks": tracks_out,
        });
        if let Some((bytes, secs)) = preview {
            out["preview_b64"] = json!(base64_encode(&bytes));
            out["preview_duration"] = json!(secs);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// ffprobe / ffmpeg helpers.
// ---------------------------------------------------------------------------

struct ProbeResult {
    width: u32,
    height: u32,
    duration: f64,
    has_audio: bool,
}

fn probe_source(ffprobe: &str, input: &Path) -> Result<ProbeResult, String> {
    let out = Command::new(ffprobe)
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg("-show_streams")
        .arg(input)
        .output()
        .map_err(|e| format!("ffprobe spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("ffprobe JSON parse failed: {e}"))?;

    let mut width = 0u32;
    let mut height = 0u32;
    let mut has_audio = false;
    if let Some(streams) = v.get("streams").and_then(Value::as_array) {
        for s in streams {
            match s.get("codec_type").and_then(Value::as_str) {
                Some("video") if width == 0 => {
                    width = s.get("width").and_then(Value::as_u64).unwrap_or(0) as u32;
                    height = s.get("height").and_then(Value::as_u64).unwrap_or(0) as u32;
                }
                Some("audio") => has_audio = true,
                _ => {}
            }
        }
    }
    let duration = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    Ok(ProbeResult {
        width,
        height,
        duration,
        has_audio,
    })
}

/// Max free-preview length (PC2 caps `previewDuration` at 60s, `media.ts:1683`).
const PREVIEW_MAX_SECONDS: u64 = 60;

/// Produce an UNENCRYPTED, reduced-quality first-`secs`-seconds clip from `input` (PC2's
/// preview ffmpeg recipe, `media.ts:1763`): H.264 CRF 28 / `scale=min(640,iw)`, AAC 96k,
/// `+faststart`; audio-only sources drop the video track. Returns the mp4 bytes.
fn make_preview_clip(
    tools: &ResolvedTools,
    workdir: &Path,
    input: &Path,
    secs: u64,
    has_video: bool,
) -> Result<Vec<u8>, String> {
    let preview_path = workdir.join("preview.mp4");
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.arg("-i").arg(input).arg("-t").arg(secs.to_string());
    if has_video {
        cmd.arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("fast")
            .arg("-crf")
            .arg("28")
            .arg("-vf")
            .arg("scale=min(640\\,iw):-2")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("96k");
    } else {
        cmd.arg("-c:a").arg("aac").arg("-b:a").arg("96k").arg("-vn");
    }
    cmd.arg("-movflags").arg("+faststart").arg("-y").arg(&preview_path);
    run_ffmpeg(&mut cmd, "preview clip")?;
    std::fs::read(&preview_path).map_err(|e| format!("read preview clip: {e}"))
}

fn run_ffmpeg(cmd: &mut Command, label: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("ffmpeg spawn failed ({label}): {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.lines().rev().take(8).collect::<Vec<_>>().join(" | ");
        return Err(format!("ffmpeg {label} exited {}: {}", out.status, tail));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// base64 (engine::general_purpose::STANDARD) — small wrappers for readability.
// ---------------------------------------------------------------------------

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn main() {
    eprintln!(
        "media-provider: starting v{} (transcode + DASH-fragment; no key material)",
        PROVIDER_VERSION
    );

    let mut provider = MediaProvider::default();
    // Load operator config at startup so `status` is meaningful before `init`.
    match MediaConfig::from_env() {
        Ok(cfg) => provider.config = cfg,
        Err(e) => eprintln!("media-provider: config load warning: {e}"),
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("media-provider read error: {}", err);
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = Response::error("invalid_request", err.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("media-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(provider: &mut MediaProvider, value: Value) -> Response {
        let req: Request = serde_json::from_value(value).unwrap();
        provider.handle(req)
    }

    fn ok_data(r: Response) -> Value {
        match r {
            Response::Ok { data } => data.unwrap_or(Value::Null),
            Response::Error { code, message } => panic!("expected ok, got error {code}: {message}"),
        }
    }

    fn err_code(r: Response) -> String {
        match r {
            Response::Error { code, .. } => code,
            Response::Ok { .. } => panic!("expected error, got ok"),
        }
    }

    #[test]
    fn status_unconfigured_is_fail_closed() {
        let mut p = MediaProvider::default();
        let data = ok_data(handle(&mut p, json!({ "op": "status" })));
        assert_eq!(data["configured"], json!(false));
        assert!(data["not_configured_reason"].is_string());
    }

    #[test]
    fn package_unconfigured_errors_not_configured() {
        let mut p = MediaProvider::default();
        let code = err_code(handle(
            &mut p,
            json!({ "op": "package", "content_b64": "AAAA" }),
        ));
        assert_eq!(code, "not_configured");
    }

    #[test]
    fn package_bad_base64_is_invalid_request() {
        // Configure a (non-existent) ffmpeg so we pass the config gate and reach decode.
        let mut p = MediaProvider::default();
        p.config = MediaConfig {
            ffmpeg_path: Some("/usr/bin/true".into()),
            ffprobe_path: Some("/usr/bin/true".into()),
            scratch_dir: Some(std::env::temp_dir().to_string_lossy().to_string()),
            renditions: vec![],
        };
        // /usr/bin/true exists on macOS/Linux; resolve() passes, so a bad b64 surfaces.
        let resp = handle(&mut p, json!({ "op": "package", "content_b64": "!!!notb64!!!" }));
        assert_eq!(err_code(resp), "invalid_request");
    }

    #[test]
    fn default_ladder_is_descending_quality_tiers() {
        let ladder = default_ladder();
        assert_eq!(ladder.len(), 4);
        assert_eq!(ladder[0].id, "1080p");
        assert_eq!(ladder.last().unwrap().id, "360p");
        // crf rises (lower quality) as resolution drops — PC2's tiering.
        assert!(ladder[0].crf <= ladder.last().unwrap().crf);
    }
}
