//! Home Studio / creative jobs (dogfood).
//!
//! Gateway-owned upstream via `CREATIVE_*` (never client-supplied). Mirrors
//! Agent Live's SSRF-closed pattern. Chat stays on `OLLAMA_*`; Studio uses
//! `CREATIVE_*` only.
//!
//! - `mode=generate` → MiniMax-H3 FL2VA `POST …/v1/videos/sync`
//!   (`CREATIVE_URL` for `CREATIVE_SCALE`, default 2; optional `CREATIVE_URL_1X` / `_4X`)
//! - `mode=character` → Comfy Ref2VA (`CREATIVE_COMFY_URL`) at **1×** with face stills
//!   and optional voice clips (`ref_audios` → `LoadAudio` → `ref_audios.ref_audio_*`)
//! - Job body may include `scale` `1` \| `2` \| `4` (N-GPU picker); Character forces 1.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use rand::RngCore;
use reqwest::multipart;
use serde::Deserialize;
use serde_json::{json, Value};

use super::*;

fn new_job_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

const DEFAULT_CREATIVE_URL: &str = "http://127.0.0.1:18000/v1/videos/sync";
const DEFAULT_CREATIVE_COMFY_URL: &str = "http://127.0.0.1:18188";
const DEFAULT_CREATIVE_PROFILE: &str = "h3-serve";
const DEFAULT_CREATIVE_SCALE: u8 = 2;
const CREATIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const CREATIVE_TOTAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const COMFY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const GENERATE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PROMPT_CHARS: usize = 12_000;
/// Comfy Ref2VA allows up to 9; Home dogfood caps at 6 (POST body budget).
const MAX_REF_IMAGES: usize = 6;
const MAX_REF_IMAGE_BYTES: usize = 4_000_000;
const MAX_REF_AUDIOS: usize = 1;
const MAX_REF_AUDIO_BYTES: usize = 8_000_000;
const REF2VA_UNET: &str = "minimax_h3_ref2va_pruned_int8_convrot.safetensors";
const REF2VA_CLIP: &str = "qwen3vl_32b_minimax_h3_nvfp4_awq.safetensors";
const REF2VA_VIDEO_VAE: &str = "minimax_h3_video_vae_fp16.safetensors";
const REF2VA_AUDIO_VAE: &str = "minimax_h3_audio_vae_fp32.safetensors";

#[derive(Debug, Clone, PartialEq, Eq)]
enum JobStatus {
    Queued,
    Running,
    Done,
    Error,
}

impl JobStatus {
    fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Done => "done",
            JobStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
struct CreativeJob {
    id: String,
    status: JobStatus,
    mode: String,
    scale: u8,
    prompt: String,
    duration: f64,
    percent: f64,
    phase: String,
    message: String,
    error: Option<String>,
    path: Option<PathBuf>,
    t0: Instant,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreativeJobCreateBody {
    prompt: String,
    #[serde(default)]
    duration: Option<f64>,
    /// `"generate"` (default) or `"character"` / `"ref2va"`.
    #[serde(default)]
    mode: Option<String>,
    /// GPU scale `1` \| `2` \| `4`. Character always uses `1`.
    #[serde(default)]
    scale: Option<u8>,
    /// Face / identity stills as `data:image/…;base64,…` (or raw base64). Max 3.
    #[serde(default)]
    ref_images: Option<Vec<String>>,
    /// Optional voice refs as `data:audio/…;base64,…` (or raw base64). Max 1.
    #[serde(default)]
    ref_audios: Option<Vec<String>>,
    /// `"match"` (default) or `"max"` (stronger identity, slower).
    #[serde(default)]
    ref_image_size: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreativeStitchBody {
    /// Completed creative job ids (hex), in cut order. Max 8.
    job_ids: Vec<String>,
}

fn creative_url() -> String {
    std::env::var("CREATIVE_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CREATIVE_URL.to_string())
}

fn creative_comfy_url() -> String {
    std::env::var("CREATIVE_COMFY_URL")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CREATIVE_COMFY_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn creative_profile() -> String {
    std::env::var("CREATIVE_PROFILE")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_CREATIVE_PROFILE.to_string())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Which GPU count `CREATIVE_URL` represents (operator sets when mode-switching).
fn creative_scale_default() -> u8 {
    match env_nonempty("CREATIVE_SCALE").as_deref() {
        Some("1") => 1,
        Some("4") => 4,
        Some("2") | None => DEFAULT_CREATIVE_SCALE,
        _ => DEFAULT_CREATIVE_SCALE,
    }
}

fn parse_scale(raw: Option<u8>) -> Result<u8, &'static str> {
    match raw {
        None => Ok(creative_scale_default()),
        Some(1) | Some(2) | Some(4) => Ok(raw.unwrap()),
        _ => Err("scale must be 1, 2, or 4"),
    }
}

/// Gateway-owned generate upstream for N GPUs. Client never supplies the URL.
fn generate_url_for_scale(n: u8) -> Option<String> {
    let explicit = match n {
        1 => env_nonempty("CREATIVE_URL_1X"),
        2 => env_nonempty("CREATIVE_URL_2X"),
        4 => env_nonempty("CREATIVE_URL_4X"),
        _ => None,
    };
    if explicit.is_some() {
        return explicit;
    }
    if n == creative_scale_default() {
        Some(creative_url())
    } else {
        None
    }
}

fn models_probe_url(sync_url: &str) -> Option<String> {
    sync_url
        .find("/v1/")
        .map(|idx| format!("{}{}", &sync_url[..idx], "/v1/models"))
}

async fn generate_reachable(client: &reqwest::Client, sync_url: &str) -> bool {
    let Some(probe) = models_probe_url(sync_url) else {
        return false;
    };
    client
        .get(&probe)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn scale_chat_note(n: u8) -> &'static str {
    match n {
        1 | 2 => "pair A chat stays up",
        4 => "chat off (all Sparks on Studio)",
        _ => "",
    }
}

fn scale_product_note(n: u8) -> &'static str {
    match n {
        1 => "Learn / Character Ref2VA",
        2 => "Everyday Generate",
        4 => "Max speed — bake when chat can stop",
        _ => "",
    }
}

#[derive(Debug, Clone)]
struct PrepareState {
    status: String,
    target: String,
    phase: String,
    message: String,
    percent: f64,
    error: Option<String>,
    t0: Option<Instant>,
}

impl Default for PrepareState {
    fn default() -> Self {
        Self {
            status: "idle".into(),
            target: String::new(),
            phase: String::new(),
            message: String::new(),
            percent: 0.0,
            error: None,
            t0: None,
        }
    }
}

fn prepare_store() -> &'static Mutex<PrepareState> {
    static STORE: OnceLock<Mutex<PrepareState>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PrepareState::default()))
}

fn prepare_cmd_path() -> Option<PathBuf> {
    if let Some(p) = env_nonempty("CREATIVE_PREPARE_CMD") {
        return Some(PathBuf::from(p));
    }
    let default = PathBuf::from("/Users/sash/Sparks/configs/h3/prepare-studio.sh");
    if default.is_file() {
        Some(default)
    } else {
        None
    }
}

fn prepare_snapshot() -> Value {
    let Ok(guard) = prepare_store().lock() else {
        return json!({ "status": "idle", "configured": prepare_cmd_path().is_some() });
    };
    let elapsed = guard.t0.map(|t| t.elapsed().as_secs()).unwrap_or(0);
    json!({
        "status": guard.status,
        "target": guard.target,
        "phase": guard.phase,
        "message": guard.message,
        "percent": guard.percent,
        "error": guard.error,
        "elapsed_s": elapsed,
        "configured": prepare_cmd_path().is_some(),
    })
}

fn update_prepare(f: impl FnOnce(&mut PrepareState)) {
    if let Ok(mut guard) = prepare_store().lock() {
        f(&mut guard);
    }
}

fn parse_prepare_phase_line(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut phase = None;
    let mut target = None;
    let mut already = false;
    for part in line.split_whitespace() {
        if let Some(v) = part.strip_prefix("phase=") {
            phase = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("target=") {
            target = Some(v.to_string());
        } else if part.starts_with("already=") {
            already = true;
        } else if let Some(err) = part.strip_prefix("error=") {
            update_prepare(|p| {
                p.status = "error".into();
                p.error = Some(err.to_string());
                p.phase = "Failed".into();
                p.message = err.to_string();
            });
            return;
        }
    }
    let Some(phase) = phase else {
        update_prepare(|p| {
            p.message = line.chars().take(200).collect();
        });
        return;
    };
    let percent = match phase.as_str() {
        "ready" => 100.0,
        "prepare" => 5.0,
        "park_comfy" | "park_2x" => 15.0,
        "start_2x" | "start_comfy" => 35.0,
        "wait_2x" | "wait_comfy" => 55.0,
        _ => 40.0,
    };
    update_prepare(|p| {
        p.phase = phase.clone();
        if let Some(t) = target {
            p.target = t;
        }
        p.percent = percent;
        p.message = if already {
            format!("{phase} (already warm)")
        } else {
            phase
        };
        if p.phase == "ready" {
            p.status = "ready".into();
            p.error = None;
        }
    });
}

async fn run_prepare_script(target: String, cmd: PathBuf) {
    update_prepare(|p| {
        p.status = "preparing".into();
        p.target = target.clone();
        p.phase = "Starting".into();
        p.message = format!("Preparing {target}…");
        p.percent = 2.0;
        p.error = None;
        p.t0 = Some(Instant::now());
    });

    let mut child = match tokio::process::Command::new(&cmd)
        .arg(&target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(err) => {
            update_prepare(|p| {
                p.status = "error".into();
                p.phase = "Failed".into();
                p.error = Some(format!("spawn prepare: {err}"));
                p.message = format!("Could not start prepare: {err}");
            });
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            parse_prepare_phase_line(&line);
        }
    }

    let status = child.wait().await;
    match status {
        Ok(s) if s.success() => {
            update_prepare(|p| {
                if p.status != "error" {
                    p.status = "ready".into();
                    p.phase = "ready".into();
                    p.percent = 100.0;
                    p.message = format!("{target} ready");
                    p.error = None;
                }
            });
        }
        Ok(s) => {
            update_prepare(|p| {
                p.status = "error".into();
                p.phase = "Failed".into();
                p.error = Some(format!("prepare exited {s}"));
                p.message = format!("Prepare failed ({s})");
            });
        }
        Err(err) => {
            update_prepare(|p| {
                p.status = "error".into();
                p.phase = "Failed".into();
                p.error = Some(err.to_string());
                p.message = err.to_string();
            });
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct CreativePrepareBody {
    /// `"generate"` or `"character"`.
    target: String,
}

fn jobs_store() -> &'static Mutex<HashMap<String, CreativeJob>> {
    static STORE: OnceLock<Mutex<HashMap<String, CreativeJob>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn creative_artifacts_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("creative").join("jobs")
}

fn is_creative_job_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn creative_job_mp4_path(data_dir: &Path, job_id: &str) -> PathBuf {
    creative_artifacts_dir(data_dir).join(format!("{job_id}.mp4"))
}

fn creative_job_meta_path(data_dir: &Path, job_id: &str) -> PathBuf {
    creative_artifacts_dir(data_dir).join(format!("{job_id}.json"))
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_job_meta(data_dir: &Path, job_id: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(creative_job_meta_path(data_dir, job_id)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_job_meta_sidecar(data_dir: &Path, job_id: &str) {
    let Ok(guard) = jobs_store().lock() else {
        return;
    };
    let Some(job) = guard.get(job_id) else {
        return;
    };
    let meta = json!({
        "id": job.id,
        "status": "done",
        "mode": job.mode,
        "scale": job.scale,
        "prompt": job.prompt,
        "duration": job.duration,
    });
    let path = creative_job_meta_path(data_dir, job_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, meta.to_string());
}

fn require_home_gui(state: &GatewayState, headers: &HeaderMap) -> Result<(), String> {
    require_home_launch_token_for_any_context(&state.data_dir, headers, &[HOME_GUI_SHELL_ID])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn clamp_duration(raw: Option<f64>) -> Result<f64, &'static str> {
    let d = raw.unwrap_or(2.0);
    if ![2.0, 5.0, 10.0, 15.0].contains(&d) {
        return Err("duration must be 2, 5, 10, or 15");
    }
    Ok(d)
}

fn frames_for_duration(duration_s: f64) -> u32 {
    let m = ((duration_s * 24.0).round() as i64).max(5) as u32;
    m + (5 - (m % 17)) % 17
}

fn update_job(id: &str, f: impl FnOnce(&mut CreativeJob)) {
    if let Ok(mut guard) = jobs_store().lock() {
        if let Some(job) = guard.get_mut(id) {
            f(job);
        }
    }
}

fn decode_ref_image(raw: &str) -> Result<(Vec<u8>, &'static str), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty ref image".into());
    }
    let (mime_hint, b64) = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| "invalid data URL".to_string())?;
        if !meta.contains(";base64") {
            return Err("ref image data URL must be base64".into());
        }
        let mime = meta.split(';').next().unwrap_or("image/png");
        let ext = if mime.contains("jpeg") || mime.contains("jpg") {
            "jpg"
        } else if mime.contains("webp") {
            "webp"
        } else {
            "png"
        };
        (ext, data)
    } else {
        ("png", trimmed)
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| format!("ref image base64: {e}"))?;
    if bytes.is_empty() || bytes.len() > MAX_REF_IMAGE_BYTES {
        return Err(format!(
            "ref image must be 1..{MAX_REF_IMAGE_BYTES} bytes after decode"
        ));
    }
    Ok((bytes, mime_hint))
}

fn decode_ref_audio(raw: &str) -> Result<(Vec<u8>, &'static str, &'static str), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty ref audio".into());
    }
    let (ext, mime, b64) = if let Some(rest) = trimmed.strip_prefix("data:") {
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| "invalid data URL".to_string())?;
        if !meta.contains(";base64") {
            return Err("ref audio data URL must be base64".into());
        }
        let mime = meta.split(';').next().unwrap_or("audio/wav");
        let (ext, mime) = if mime.contains("mpeg") || mime.contains("mp3") {
            ("mp3", "audio/mpeg")
        } else if mime.contains("mp4") || mime.contains("m4a") || mime.contains("x-m4a") {
            ("m4a", "audio/mp4")
        } else if mime.contains("ogg") {
            ("ogg", "audio/ogg")
        } else if mime.contains("flac") {
            ("flac", "audio/flac")
        } else {
            ("wav", "audio/wav")
        };
        (ext, mime, data)
    } else {
        ("wav", "audio/wav", trimmed)
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| format!("ref audio base64: {e}"))?;
    if bytes.is_empty() || bytes.len() > MAX_REF_AUDIO_BYTES {
        return Err(format!(
            "ref audio must be 1..{MAX_REF_AUDIO_BYTES} bytes after decode"
        ));
    }
    Ok((bytes, ext, mime))
}

async fn comfy_reachable(client: &reqwest::Client, base: &str) -> bool {
    let url = format!("{base}/system_stats");
    match client.get(&url).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => {
            let fallback = format!("{base}/");
            client
                .get(&fallback)
                .send()
                .await
                .map(|r| r.status().is_success() || r.status().as_u16() == 404)
                .unwrap_or(false)
        }
    }
}

fn ref2va_prompt_graph(
    prompt: &str,
    width: u32,
    height: u32,
    length: u32,
    steps: u32,
    seed: u64,
    ref_image_size: &str,
    uploaded_images: &[String],
    uploaded_audios: &[String],
) -> Value {
    let mut graph = json!({
        "127": {
            "class_type": "UNETLoader",
            "inputs": {
                "unet_name": REF2VA_UNET,
                "weight_dtype": "default"
            }
        },
        "128": {
            "class_type": "CLIPLoader",
            "inputs": {
                "clip_name": REF2VA_CLIP,
                "type": "minimax",
                "device": "default"
            }
        },
        "119": {
            "class_type": "VAELoader",
            "inputs": { "vae_name": REF2VA_VIDEO_VAE }
        },
        "120": {
            "class_type": "VAELoader",
            "inputs": { "vae_name": REF2VA_AUDIO_VAE }
        },
        "129": {
            "class_type": "RandomNoise",
            "inputs": { "noise_seed": seed }
        },
        "123": {
            "class_type": "KSamplerSelect",
            "inputs": { "sampler_name": "res_multistep" }
        },
        "124": {
            "class_type": "BasicScheduler",
            "inputs": {
                "model": ["127", 0],
                "scheduler": "simple",
                "steps": steps,
                "denoise": 1.0
            }
        },
        "136": {
            "class_type": "MiniMaxH3ReferenceToVideo",
            "inputs": {
                "clip": ["128", 0],
                "vae": ["119", 0],
                "audio_vae": ["120", 0],
                "prompt": prompt,
                "width": width,
                "height": height,
                "length": length,
                "ref_image_size": ref_image_size
            }
        },
        "126": {
            "class_type": "BasicGuider",
            "inputs": {
                "model": ["127", 0],
                "conditioning": ["136", 0]
            }
        },
        "125": {
            "class_type": "SamplerCustomAdvanced",
            "inputs": {
                "noise": ["129", 0],
                "guider": ["126", 0],
                "sampler": ["123", 0],
                "sigmas": ["124", 0],
                "latent_image": ["136", 1]
            }
        },
        "122": {
            "class_type": "VAEDecode",
            "inputs": {
                "samples": ["125", 0],
                "vae": ["119", 0]
            }
        },
        "121": {
            "class_type": "VAEDecodeAudio",
            "inputs": {
                "samples": ["125", 0],
                "vae": ["120", 0]
            }
        },
        "130": {
            "class_type": "CreateVideo",
            "inputs": {
                "images": ["122", 0],
                "audio": ["121", 0],
                "fps": 24.0,
                "bit_depth": 8
            }
        },
        "92": {
            "class_type": "SaveVideo",
            "inputs": {
                "video": ["130", 0],
                "filename_prefix": "video/elastos_character",
                "format": "auto",
                "codec": "auto"
            }
        }
    });

    for (i, name) in uploaded_images.iter().enumerate() {
        let load_id = format!("{}", 200 + i);
        graph.as_object_mut().unwrap().insert(
            load_id.clone(),
            json!({
                "class_type": "LoadImage",
                "inputs": { "image": name }
            }),
        );
        graph
            .pointer_mut("/136/inputs")
            .and_then(|v| v.as_object_mut())
            .expect("ref2va inputs")
            .insert(format!("ref_images.ref_image_{i}"), json!([load_id, 0]));
    }
    for (i, name) in uploaded_audios.iter().enumerate() {
        let load_id = format!("{}", 300 + i);
        graph.as_object_mut().unwrap().insert(
            load_id.clone(),
            json!({
                "class_type": "LoadAudio",
                "inputs": { "audio": name }
            }),
        );
        graph
            .pointer_mut("/136/inputs")
            .and_then(|v| v.as_object_mut())
            .expect("ref2va inputs")
            .insert(format!("ref_audios.ref_audio_{i}"), json!([load_id, 0]));
    }
    graph
}

fn pick_comfy_media(outputs: &Value) -> Option<(String, String, String)> {
    let obj = outputs.as_object()?;
    for (_nid, out) in obj {
        for key in ["gifs", "videos", "images"] {
            if let Some(arr) = out.get(key).and_then(|v| v.as_array()) {
                for item in arr {
                    let filename = item.get("filename")?.as_str()?.to_string();
                    let subfolder = item
                        .get("subfolder")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let ty = item
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("output")
                        .to_string();
                    if filename.ends_with(".mp4")
                        || filename.ends_with(".webm")
                        || key == "gifs"
                        || key == "videos"
                    {
                        return Some((filename, subfolder, ty));
                    }
                }
            }
        }
    }
    None
}

async fn run_generate_job(
    job_id: String,
    prompt: String,
    duration: f64,
    scale: u8,
    upstream_url: String,
    out_path: PathBuf,
) {
    update_job(&job_id, |j| {
        j.status = JobStatus::Running;
        j.phase = "Starting".into();
        j.percent = 3.0;
        j.message = format!("{duration}s · {scale}×");
    });

    let client = match reqwest::Client::builder()
        .connect_timeout(CREATIVE_CONNECT_TIMEOUT)
        .timeout(CREATIVE_TOTAL_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(err.to_string());
            });
            return;
        }
    };

    let extra = json!({
        "task": "t2va",
        "duration": duration,
        "audio_flow_shift": 3.0,
    })
    .to_string();

    let form = multipart::Form::new()
        .text("prompt", prompt)
        .text("width", "768")
        .text("height", "448")
        .text("fps", "24")
        .text("num_inference_steps", "20")
        .text("flow_shift", "12")
        .text("seed", "42")
        .text("extra_params", extra);

    update_job(&job_id, |j| {
        j.phase = "Generating".into();
        j.percent = 10.0;
        j.message = format!("{scale}× on cluster");
    });

    let resp = match client.post(&upstream_url).multipart(form).send().await {
        Ok(r) => r,
        Err(err) => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(format!("upstream connect: {err}"));
            });
            return;
        }
    };

    update_job(&job_id, |j| {
        j.phase = "Saving".into();
        j.percent = 90.0;
        j.message = "writing clip on this Home".into();
    });

    if let Err(err) = stream_response_to_file(resp, &out_path).await {
        update_job(&job_id, |j| {
            j.status = JobStatus::Error;
            j.phase = "Failed".into();
            j.error = Some(err);
        });
        return;
    }

    finish_job_file(&job_id, &out_path).await;
}

async fn run_character_job(
    job_id: String,
    prompt: String,
    duration: f64,
    ref_image_size: String,
    ref_images: Vec<String>,
    ref_audios: Vec<String>,
    out_path: PathBuf,
) {
    update_job(&job_id, |j| {
        j.status = JobStatus::Running;
        j.phase = "Starting".into();
        j.percent = 3.0;
        j.message = format!("Ref2VA · {duration}s");
    });

    let base = creative_comfy_url();
    let client = match reqwest::Client::builder()
        .connect_timeout(CREATIVE_CONNECT_TIMEOUT)
        .timeout(CREATIVE_TOTAL_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(err.to_string());
            });
            return;
        }
    };

    if !comfy_reachable(&client, &base).await {
        update_job(&job_id, |j| {
            j.status = JobStatus::Error;
            j.phase = "Failed".into();
            j.error = Some(
                "Comfy Ref2VA unreachable. Park 2× (`make down`), start Comfy on spark3, ensure tunnel :18188."
                    .into(),
            );
        });
        return;
    }

    update_job(&job_id, |j| {
        j.phase = "Uploading refs".into();
        j.percent = 8.0;
    });

    let mut uploaded_images = Vec::new();
    for (i, raw) in ref_images.iter().enumerate() {
        let (bytes, ext) = match decode_ref_image(raw) {
            Ok(v) => v,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(err);
                });
                return;
            }
        };
        let filename = format!("elastos_char_{job_id}_{i}.{ext}");
        let part = match multipart::Part::bytes(bytes)
            .file_name(filename.clone())
            .mime_str(match ext {
                "jpg" => "image/jpeg",
                "webp" => "image/webp",
                _ => "image/png",
            }) {
            Ok(p) => p,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("ref part: {err}"));
                });
                return;
            }
        };
        let form = multipart::Form::new()
            .part("image", part)
            .text("overwrite", "true");
        let upload_url = format!("{base}/upload/image");
        let resp = match client.post(&upload_url).multipart(form).send().await {
            Ok(r) => r,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("comfy upload: {err}"));
                });
                return;
            }
        };
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(format!("comfy upload failed: {msg}").chars().take(800).collect());
            });
            return;
        }
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("comfy upload json: {err}"));
                });
                return;
            }
        };
        let name = body
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&filename)
            .to_string();
        uploaded_images.push(name);
    }

    let mut uploaded_audios = Vec::new();
    for (i, raw) in ref_audios.iter().enumerate() {
        let (bytes, ext, mime) = match decode_ref_audio(raw) {
            Ok(v) => v,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(err);
                });
                return;
            }
        };
        let filename = format!("elastos_voice_{job_id}_{i}.{ext}");
        /* Comfy has no /upload/audio; files land in input/ via /upload/image. */
        let part = match multipart::Part::bytes(bytes)
            .file_name(filename.clone())
            .mime_str(mime)
        {
            Ok(p) => p,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("voice part: {err}"));
                });
                return;
            }
        };
        let form = multipart::Form::new()
            .part("image", part)
            .text("overwrite", "true");
        let upload_url = format!("{base}/upload/image");
        let resp = match client.post(&upload_url).multipart(form).send().await {
            Ok(r) => r,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("comfy voice upload: {err}"));
                });
                return;
            }
        };
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(
                    format!("comfy voice upload failed: {msg}")
                        .chars()
                        .take(800)
                        .collect(),
                );
            });
            return;
        }
        let body: Value = match resp.json().await {
            Ok(v) => v,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("comfy voice upload json: {err}"));
                });
                return;
            }
        };
        let name = body
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&filename)
            .to_string();
        uploaded_audios.push(name);
    }

    let length = frames_for_duration(duration);
    // Short identity smoke defaults (TEST-REF2VA): ~0.2 MP 16:9.
    let (width, height) = (608u32, 352u32);
    let graph = ref2va_prompt_graph(
        &prompt,
        width,
        height,
        length,
        20,
        42,
        &ref_image_size,
        &uploaded_images,
        &uploaded_audios,
    );

    update_job(&job_id, |j| {
        j.phase = "Queued on Comfy".into();
        j.percent = 15.0;
        j.message = format!("{length} frames");
    });

    let prompt_url = format!("{base}/prompt");
    let payload = json!({
        "prompt": graph,
        "client_id": job_id,
    });
    let resp = match client.post(&prompt_url).json(&payload).send().await {
        Ok(r) => r,
        Err(err) => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(format!("comfy /prompt: {err}"));
            });
            return;
        }
    };
    if !resp.status().is_success() {
        let msg = resp.text().await.unwrap_or_default();
        update_job(&job_id, |j| {
            j.status = JobStatus::Error;
            j.phase = "Failed".into();
            j.error = Some(format!("comfy /prompt failed: {msg}").chars().take(800).collect());
        });
        return;
    }
    let submitted: Value = match resp.json().await {
        Ok(v) => v,
        Err(err) => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(format!("comfy /prompt json: {err}"));
            });
            return;
        }
    };
    let prompt_id = match submitted.get("prompt_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some("comfy /prompt missing prompt_id".into());
            });
            return;
        }
    };

    update_job(&job_id, |j| {
        j.phase = "Generating".into();
        j.percent = 20.0;
        j.message = format!("comfy {prompt_id}");
    });

    let history_url = format!("{base}/history/{prompt_id}");
    let deadline = Instant::now() + CREATIVE_TOTAL_TIMEOUT;
    let mut ticks = 0u32;
    loop {
        if Instant::now() > deadline {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some("comfy timeout".into());
            });
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        ticks += 1;
        let pct = (20.0 + (ticks as f64) * 2.5).min(90.0);
        update_job(&job_id, |j| {
            j.percent = pct;
            j.message = format!("comfy · {}s", j.t0.elapsed().as_secs());
        });

        let hist_resp = match client.get(&history_url).send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !hist_resp.status().is_success() {
            continue;
        }
        let hist: Value = match hist_resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(entry) = hist.get(&prompt_id) else {
            continue;
        };
        let status_str = entry
            .pointer("/status/status_str")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if status_str == "error" {
            let err = entry
                .pointer("/status/messages")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "comfy error".into());
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(err.chars().take(800).collect());
            });
            return;
        }
        if status_str != "success" {
            continue;
        }
        let outputs = entry.get("outputs").cloned().unwrap_or(json!({}));
        let Some((filename, subfolder, ty)) = pick_comfy_media(&outputs) else {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some("comfy success but no video in outputs".into());
            });
            return;
        };

        update_job(&job_id, |j| {
            j.phase = "Downloading".into();
            j.percent = 95.0;
        });

        let view_url = format!(
            "{base}/view?filename={}&subfolder={}&type={}",
            urlencoding_encode(&filename),
            urlencoding_encode(&subfolder),
            urlencoding_encode(&ty)
        );
        let video_resp = match client.get(&view_url).send().await {
            Ok(r) => r,
            Err(err) => {
                update_job(&job_id, |j| {
                    j.status = JobStatus::Error;
                    j.phase = "Failed".into();
                    j.error = Some(format!("comfy /view: {err}"));
                });
                return;
            }
        };
        if !video_resp.status().is_success() {
            let msg = video_resp.text().await.unwrap_or_default();
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(format!("comfy /view failed: {msg}").chars().take(800).collect());
            });
            return;
        }
        update_job(&job_id, |j| {
            j.phase = "Saving".into();
            j.percent = 90.0;
            j.message = "writing clip on this Home".into();
        });
        if let Err(err) = stream_response_to_file(video_resp, &out_path).await {
            update_job(&job_id, |j| {
                j.status = JobStatus::Error;
                j.phase = "Failed".into();
                j.error = Some(err);
            });
            return;
        }
        finish_job_file(&job_id, &out_path).await;
        return;
    }
}

fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Stream upstream body to `*.mp4.partial`, then atomic rename to `out_path`.
/// Survives browser disconnect while the gateway task stays alive; reduces loss
/// if the process dies mid-buffer of a fully received body.
async fn stream_response_to_file(resp: reqwest::Response, out_path: &Path) -> Result<(), String> {
    let status = resp.status();
    let partial = out_path.with_extension("mp4.partial");
    if let Some(parent) = partial.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir: {e}"))?;
    }
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(|e| format!("create partial: {e}"))?;
    let mut err_preview = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(next) = stream.next().await {
        let chunk = next.map_err(|e| format!("upstream body: {e}"))?;
        if !status.is_success() {
            if err_preview.len() < 800 {
                let take = (800 - err_preview.len()).min(chunk.len());
                err_preview.extend_from_slice(&chunk[..take]);
            }
            continue;
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write partial: {e}"))?;
    }
    if !status.is_success() {
        let _ = tokio::fs::remove_file(&partial).await;
        let msg = String::from_utf8_lossy(&err_preview)
            .chars()
            .take(800)
            .collect::<String>();
        return Err(format!("upstream HTTP {status}: {msg}"));
    }
    file.flush()
        .await
        .map_err(|e| format!("flush partial: {e}"))?;
    drop(file);
    tokio::fs::rename(&partial, out_path)
        .await
        .map_err(|e| format!("rename artifact: {e}"))?;
    Ok(())
}

async fn finish_job_file(job_id: &str, out_path: &Path) {
    let len = std::fs::metadata(out_path).map(|m| m.len()).unwrap_or(0);
    if let Some(parent) = out_path.parent() {
        if let Some(creative) = parent.parent() {
            if let Some(data_dir) = creative.parent() {
                write_job_meta_sidecar(data_dir, job_id);
            }
        }
    }
    update_job(job_id, |j| {
        j.status = JobStatus::Done;
        j.phase = "Done".into();
        j.percent = 100.0;
        j.path = Some(out_path.to_path_buf());
        j.message = format!("{len} bytes");
    });
}

/// GET /api/apps/home/creative/status
pub(super) async fn home_creative_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    let comfy_base = creative_comfy_url();
    let default_scale = creative_scale_default();
    let probe_client = reqwest::Client::builder()
        .connect_timeout(COMFY_PROBE_TIMEOUT)
        .timeout(GENERATE_PROBE_TIMEOUT)
        .build()
        .ok();
    let comfy_up = if let Some(client) = probe_client.as_ref() {
        comfy_reachable(client, &comfy_base).await
    } else {
        false
    };

    let mut scale_options = Vec::new();
    let mut any_generate_up = false;
    for n in [1u8, 2, 4] {
        let gen_url = generate_url_for_scale(n);
        let gen_wired = gen_url.is_some();
        let gen_up = match (probe_client.as_ref(), gen_url.as_deref()) {
            (Some(client), Some(url)) => generate_reachable(client, url).await,
            _ => false,
        };
        if gen_up {
            any_generate_up = true;
        }
        let character_wired = n == 1;
        let character_up = character_wired && comfy_up;
        scale_options.push(json!({
            "n": n,
            "label": format!("{n}×"),
            "chat": scale_chat_note(n),
            "note": scale_product_note(n),
            "generate": {
                "wired": gen_wired,
                "reachable": gen_up,
            },
            "character": {
                "wired": character_wired,
                "reachable": character_up,
            },
        }));
    }

    Json(json!({
        "status": "ok",
        "profile": creative_profile(),
        "scale": {
            "default": default_scale,
            "options": scale_options,
        },
        "generate": {
            "wired": generate_url_for_scale(default_scale).is_some(),
            "upstream_configured": generate_url_for_scale(default_scale).is_some(),
            "upstream_reachable": any_generate_up,
            "default_scale": default_scale,
            "modes": ["generate"],
        },
        "character": {
            "wired": true,
            "upstream_reachable": comfy_up,
            "scale": 1,
            "modes": ["character"],
            "note": if comfy_up {
                "Ref2VA via Comfy 1× — face stills + optional voice; <Picture 1> / <Audio 1>."
            } else {
                "Adapter wired; Character offline — use Prepare (allocator) or start Comfy / tunnel :18188."
            },
        },
        "allocator": prepare_snapshot(),
    }))
    .into_response()
}

/// POST /api/apps/home/creative/prepare — P0 allocator: warm Generate or Character.
pub(super) async fn home_creative_prepare(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CreativePrepareBody>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    let target = body.target.trim().to_ascii_lowercase();
    let target = match target.as_str() {
        "generate" | "2x" | "serve" => "generate".to_string(),
        "character" | "comfy" | "ref2va" | "1x" => "character".to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_target",
                    "message": "target must be generate or character",
                })),
            )
                .into_response();
        }
    };

    let already_preparing = prepare_store()
        .lock()
        .map(|g| g.status == "preparing")
        .unwrap_or(false);
    if already_preparing {
        return Json(json!({
            "status": "preparing",
            "allocator": prepare_snapshot(),
            "message": "Prepare already in progress",
        }))
        .into_response();
    }

    let Some(cmd) = prepare_cmd_path() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "status": "error",
                "code": "prepare_not_configured",
                "message": "Set CREATIVE_PREPARE_CMD to configs/h3/prepare-studio.sh",
            })),
        )
            .into_response();
    };

    tokio::spawn(run_prepare_script(target.clone(), cmd));

    Json(json!({
        "status": "preparing",
        "target": target,
        "allocator": prepare_snapshot(),
    }))
    .into_response()
}

/// POST /api/apps/home/creative/jobs
pub(super) async fn home_creative_jobs_create(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CreativeJobCreateBody>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    let mode = body
        .mode
        .as_deref()
        .unwrap_or("generate")
        .trim()
        .to_ascii_lowercase();
    let mode = if mode == "ref2va" {
        "character".to_string()
    } else {
        mode
    };
    if mode != "generate" && mode != "character" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "code": "invalid_mode",
                "message": "mode must be generate or character",
            })),
        )
            .into_response();
    }

    let prompt = body.prompt.trim().to_string();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_CHARS {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "code": "invalid_prompt",
                "message": "prompt required (max 12000 chars)",
            })),
        )
            .into_response();
    }

    let duration = match clamp_duration(body.duration) {
        Ok(d) => d,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_duration",
                    "message": msg,
                })),
            )
                .into_response();
        }
    };

    let scale = if mode == "character" {
        if let Some(n) = body.scale {
            if n != 1 {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "status": "error",
                        "code": "invalid_scale",
                        "message": "character / Ref2VA is 1× Comfy only today",
                    })),
                )
                    .into_response();
            }
        }
        1u8
    } else {
        match parse_scale(body.scale) {
            Ok(n) => n,
            Err(msg) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "status": "error",
                        "code": "invalid_scale",
                        "message": msg,
                    })),
                )
                    .into_response();
            }
        }
    };

    let generate_upstream = if mode == "generate" {
        match generate_url_for_scale(scale) {
            Some(url) => Some(url),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "status": "error",
                        "code": "scale_unavailable",
                        "message": format!(
                            "{scale}× Generate is not wired on this Home (set CREATIVE_URL / CREATIVE_URL_{scale}X + CREATIVE_SCALE, or pick another scale)"
                        ),
                    })),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    let mut ref_images = body.ref_images.unwrap_or_default();
    let mut ref_audios = body.ref_audios.unwrap_or_default();
    let ref_image_size = body
        .ref_image_size
        .as_deref()
        .unwrap_or("match")
        .trim()
        .to_ascii_lowercase();
    if mode == "character" {
        if ref_images.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "need_ref_image",
                    "message": "character mode needs at least one face/identity still in ref_images",
                })),
            )
                .into_response();
        }
        if ref_images.len() > MAX_REF_IMAGES {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "too_many_refs",
                    "message": format!("max {MAX_REF_IMAGES} ref_images"),
                })),
            )
                .into_response();
        }
        if ref_audios.len() > MAX_REF_AUDIOS {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "too_many_voice_refs",
                    "message": format!("max {MAX_REF_AUDIOS} ref_audios"),
                })),
            )
                .into_response();
        }
        if ref_image_size != "match" && ref_image_size != "max" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_ref_image_size",
                    "message": "ref_image_size must be match or max",
                })),
            )
                .into_response();
        }
        for raw in &ref_images {
            if let Err(err) = decode_ref_image(raw) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "status": "error",
                        "code": "invalid_ref_image",
                        "message": err,
                    })),
                )
                    .into_response();
            }
        }
        for raw in &ref_audios {
            if let Err(err) = decode_ref_audio(raw) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "status": "error",
                        "code": "invalid_ref_audio",
                        "message": err,
                    })),
                )
                    .into_response();
            }
        }
    } else {
        ref_images.clear();
        ref_audios.clear();
    }

    let job_id = new_job_id();
    let out_path = creative_artifacts_dir(&state.data_dir).join(format!("{job_id}.mp4"));
    let job = CreativeJob {
        id: job_id.clone(),
        status: JobStatus::Queued,
        mode: mode.clone(),
        scale,
        prompt: prompt.clone(),
        duration,
        percent: 0.0,
        phase: "Queued".into(),
        message: String::new(),
        error: None,
        path: None,
        t0: Instant::now(),
    };
    if let Ok(mut guard) = jobs_store().lock() {
        guard.insert(job_id.clone(), job);
    }

    if mode == "character" {
        tokio::spawn(run_character_job(
            job_id.clone(),
            prompt,
            duration,
            ref_image_size,
            ref_images,
            ref_audios,
            out_path,
        ));
    } else {
        let upstream_url = generate_upstream.expect("generate upstream checked above");
        tokio::spawn(run_generate_job(
            job_id.clone(),
            prompt,
            duration,
            scale,
            upstream_url,
            out_path,
        ));
    }

    Json(json!({ "id": job_id, "status": "queued", "mode": mode, "scale": scale })).into_response()
}

fn ffmpeg_bin() -> String {
    std::env::var("CREATIVE_FFMPEG")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "ffmpeg".into())
}

fn concat_demuxer_line(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\'', r"'\''");
    format!("file '{s}'")
}

/// POST /api/apps/home/creative/stitch — ffmpeg concat of completed job mp4s.
pub(super) async fn home_creative_jobs_stitch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CreativeStitchBody>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    let ids: Vec<String> = body
        .job_ids
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.len() < 2 || ids.len() > 8 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "code": "invalid_stitch",
                "message": "stitch needs 2–8 completed job ids",
            })),
        )
            .into_response();
    }
    for id in &ids {
        if !is_creative_job_id(id) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "code": "invalid_job_id",
                    "message": format!("bad job id: {id}"),
                })),
            )
                .into_response();
        }
        let path = creative_job_mp4_path(&state.data_dir, id);
        if !path.is_file() {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "status": "error",
                    "code": "missing_clip",
                    "message": format!("no mp4 for job {id}"),
                })),
            )
                .into_response();
        }
    }

    let stitch_id = new_job_id();
    let out_path = creative_job_mp4_path(&state.data_dir, &stitch_id);
    let list_path = creative_artifacts_dir(&state.data_dir).join(format!("{stitch_id}.concat.txt"));
    let prompt = format!("storyboard stitch of {} shots", ids.len());
    let job = CreativeJob {
        id: stitch_id.clone(),
        status: JobStatus::Running,
        mode: "storyboard".into(),
        scale: 0,
        prompt: prompt.clone(),
        duration: 0.0,
        percent: 10.0,
        phase: "Stitching".into(),
        message: format!("{} clips", ids.len()),
        error: None,
        path: None,
        t0: Instant::now(),
    };
    if let Ok(mut guard) = jobs_store().lock() {
        guard.insert(stitch_id.clone(), job);
    }

    let mut list_body = String::new();
    for id in &ids {
        let path = creative_job_mp4_path(&state.data_dir, id);
        list_body.push_str(&concat_demuxer_line(&path));
        list_body.push('\n');
    }
    if let Err(err) = std::fs::write(&list_path, &list_body) {
        update_job(&stitch_id, |j| {
            j.status = JobStatus::Error;
            j.phase = "Failed".into();
            j.error = Some(format!("write concat list: {err}"));
        });
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "code": "stitch_failed", "message": err.to_string() })),
        )
            .into_response();
    }

    let ff = ffmpeg_bin();
    let copy_ok = tokio::process::Command::new(&ff)
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list_path.to_str().unwrap_or(""),
            "-c",
            "copy",
            out_path.to_str().unwrap_or(""),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    let ok = if copy_ok && out_path.is_file() {
        true
    } else {
        let _ = std::fs::remove_file(&out_path);
        tokio::process::Command::new(&ff)
            .args([
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                list_path.to_str().unwrap_or(""),
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "18",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                out_path.to_str().unwrap_or(""),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
            && out_path.is_file()
    };

    let _ = std::fs::remove_file(&list_path);

    if !ok {
        update_job(&stitch_id, |j| {
            j.status = JobStatus::Error;
            j.phase = "Failed".into();
            j.error = Some("ffmpeg stitch failed".into());
        });
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "error",
                "code": "stitch_failed",
                "message": "ffmpeg could not concat clips (is ffmpeg installed?)",
            })),
        )
            .into_response();
    }

    finish_job_file(&stitch_id, &out_path).await;
    // enrich sidecar mode/prompt
    if let Ok(mut guard) = jobs_store().lock() {
        if let Some(job) = guard.get_mut(&stitch_id) {
            job.mode = "storyboard".into();
            job.prompt = prompt.clone();
        }
    }
    write_job_meta_sidecar(&state.data_dir, &stitch_id);

    Json(json!({
        "id": stitch_id,
        "status": "done",
        "mode": "storyboard",
        "sources": ids,
    }))
    .into_response()
}

/// GET /api/apps/home/creative/jobs — clip library (memory + disk).
pub(super) async fn home_creative_jobs_list(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    let mut by_id: HashMap<String, Value> = HashMap::new();

    if let Ok(guard) = jobs_store().lock() {
        for job in guard.values() {
            by_id.insert(
                job.id.clone(),
                json!({
                    "id": job.id,
                    "status": job.status.as_str(),
                    "mode": job.mode,
                    "scale": job.scale,
                    "prompt": job.prompt,
                    "duration": job.duration,
                    "percent": job.percent,
                    "phase": job.phase,
                    "message": job.message,
                    "error": job.error,
                    "has_video": job.status == JobStatus::Done
                        && job.path.as_ref().map(|p| p.is_file()).unwrap_or(false),
                    "bytes": job
                        .path
                        .as_ref()
                        .and_then(|p| std::fs::metadata(p).ok())
                        .map(|m| m.len()),
                    "mtime_ms": job
                        .path
                        .as_ref()
                        .and_then(|p| std::fs::metadata(p).ok())
                        .map(|m| mtime_ms(&m))
                        .unwrap_or_else(|| {
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0)
                        }),
                }),
            );
        }
    }

    let dir = creative_artifacts_dir(&state.data_dir);
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("mp4") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !is_creative_job_id(stem) {
                continue;
            }
            if by_id.contains_key(stem) {
                continue;
            }
            let Ok(meta) = ent.metadata() else {
                continue;
            };
            let mut item = json!({
                "id": stem,
                "status": "done",
                "mode": "generate",
                "scale": null,
                "prompt": "",
                "duration": null,
                "percent": 100.0,
                "phase": "Done",
                "message": format!("{} bytes", meta.len()),
                "error": null,
                "has_video": true,
                "bytes": meta.len(),
                "mtime_ms": mtime_ms(&meta),
            });
            if let Some(sidecar) = read_job_meta(&state.data_dir, stem) {
                if let Some(obj) = item.as_object_mut() {
                    if let Some(v) = sidecar.get("mode") {
                        obj.insert("mode".into(), v.clone());
                    }
                    if let Some(v) = sidecar.get("scale") {
                        obj.insert("scale".into(), v.clone());
                    }
                    if let Some(v) = sidecar.get("prompt") {
                        obj.insert("prompt".into(), v.clone());
                    }
                    if let Some(v) = sidecar.get("duration") {
                        obj.insert("duration".into(), v.clone());
                    }
                }
            }
            by_id.insert(stem.to_string(), item);
        }
    }

    let mut jobs: Vec<Value> = by_id.into_values().collect();
    jobs.sort_by(|a, b| {
        let am = a.get("mtime_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let bm = b.get("mtime_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        bm.cmp(&am)
    });
    jobs.truncate(48);

    Json(json!({ "jobs": jobs })).into_response()
}

/// GET /api/apps/home/creative/jobs/:id
pub(super) async fn home_creative_jobs_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    if !is_creative_job_id(&job_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "code": "invalid_job_id" })),
        )
            .into_response();
    }

    if let Ok(guard) = jobs_store().lock() {
        if let Some(job) = guard.get(&job_id) {
            return Json(json!({
                "id": job.id,
                "status": job.status.as_str(),
                "mode": job.mode,
                "scale": job.scale,
                "prompt": job.prompt,
                "percent": job.percent,
                "phase": job.phase,
                "message": job.message,
                "duration": job.duration,
                "elapsed_s": job.t0.elapsed().as_secs(),
                "error": job.error,
            }))
            .into_response();
        }
    }

    let path = creative_job_mp4_path(&state.data_dir, &job_id);
    if !path.is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "error", "code": "unknown_job" })),
        )
            .into_response();
    }
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut view = json!({
        "id": job_id,
        "status": "done",
        "mode": "generate",
        "scale": null,
        "prompt": "",
        "percent": 100.0,
        "phase": "Done",
        "message": format!("{bytes} bytes"),
        "duration": null,
        "elapsed_s": 0,
        "error": null,
    });
    if let Some(sidecar) = read_job_meta(&state.data_dir, &job_id) {
        if let Some(obj) = view.as_object_mut() {
            for key in ["mode", "scale", "prompt", "duration"] {
                if let Some(v) = sidecar.get(key) {
                    obj.insert(key.to_string(), v.clone());
                }
            }
        }
    }
    Json(view).into_response()
}

/// DELETE /api/apps/home/creative/jobs/:id — remove clip + sidecar from this Home.
pub(super) async fn home_creative_jobs_delete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    if !is_creative_job_id(&job_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "code": "invalid_job_id" })),
        )
            .into_response();
    }

    if let Ok(guard) = jobs_store().lock() {
        if let Some(job) = guard.get(&job_id) {
            if matches!(job.status, JobStatus::Queued | JobStatus::Running) {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "status": "error",
                        "code": "job_busy",
                        "message": "cannot delete a running job",
                    })),
                )
                    .into_response();
            }
        }
    }

    let mp4 = creative_job_mp4_path(&state.data_dir, &job_id);
    let meta = creative_job_meta_path(&state.data_dir, &job_id);
    let partial = mp4.with_extension("mp4.partial");
    let had_mp4 = mp4.is_file();
    let _ = std::fs::remove_file(&mp4);
    let _ = std::fs::remove_file(&meta);
    let _ = std::fs::remove_file(&partial);
    if let Ok(mut guard) = jobs_store().lock() {
        guard.remove(&job_id);
    }

    if !had_mp4 {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "error", "code": "unknown_job" })),
        )
            .into_response();
    }

    Json(json!({ "id": job_id, "status": "deleted" })).into_response()
}

/// GET /api/apps/home/creative/jobs/:id/video
pub(super) async fn home_creative_jobs_video(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "error",
                "code": "missing-home-launch-token",
                "message": err,
            })),
        )
            .into_response();
    }

    if !is_creative_job_id(&job_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let path = {
        let Ok(guard) = jobs_store().lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        if let Some(job) = guard.get(&job_id) {
            if job.status != JobStatus::Done {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "status": "error",
                        "code": "not_ready",
                        "message": "job not done",
                    })),
                )
                    .into_response();
            }
            job.path.clone()
        } else {
            None
        }
    };

    let path = path.unwrap_or_else(|| creative_job_mp4_path(&state.data_dir, &job_id));
    if !path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
