//! Home Studio / creative library (contract era).
//!
//! Generation runs live in the model-provider capsule (`elastos://model/*`,
//! `runs_create`/`runs_events`); this module is only what remains app-side:
//! the clip library on disk, playback, delete, ffmpeg stitch (an app
//! workflow, not a model op), and the status/prepare surface the Studio
//! chrome polls. Artifacts are content-addressed (32-hex sha256 prefix)
//! written by the provider's h3_video adapter into `creative/jobs/` — this
//! module reads that format and never talks job control to upstreams.
//!
//! `CREATIVE_*` env is operator-owned upstream config (same source
//! `server_infra` uses to configure the provider) — SSRF-closed: clients
//! never supply URLs. Retires when discovery/grants land (P6).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::*;

const DEFAULT_CREATIVE_URL: &str = "http://127.0.0.1:18000/v1/videos/sync";
const DEFAULT_CREATIVE_COMFY_URL: &str = "http://127.0.0.1:18188";
const DEFAULT_CREATIVE_PROFILE: &str = "h3-serve";
const DEFAULT_CREATIVE_SCALE: u8 = 2;
const COMFY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const GENERATE_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn new_job_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn creative_url() -> String {
    env_nonempty("CREATIVE_URL").unwrap_or_else(|| DEFAULT_CREATIVE_URL.to_string())
}

fn creative_comfy_url() -> String {
    env_nonempty("CREATIVE_COMFY_URL")
        .unwrap_or_else(|| DEFAULT_CREATIVE_COMFY_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Readiness signal for the operator note: Comfy may be up, but until
/// Character is a model offer the UI must say "not wired".
async fn comfy_up_for_note(client: Option<&reqwest::Client>) -> bool {
    match client {
        Some(c) => comfy_reachable(c, &creative_comfy_url()).await,
        None => false,
    }
}

fn creative_profile() -> String {
    env_nonempty("CREATIVE_PROFILE").unwrap_or_else(|| DEFAULT_CREATIVE_PROFILE.to_string())
}

/// Which GPU count `CREATIVE_URL` represents (operator sets when mode-switching).
fn creative_scale_default() -> u8 {
    match env_nonempty("CREATIVE_SCALE").as_deref() {
        Some("1") => 1,
        Some("4") => 4,
        _ => DEFAULT_CREATIVE_SCALE,
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

// ---------------------------------------------------------------------------
// Prepare (allocator) — operator script warms Generate/Character on the
// cluster; the UI polls `prepare_snapshot` via /creative/status.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct PrepareState {
    status: String,
    target: String,
    phase: String,
    message: String,
    percent: f64,
    error: Option<String>,
    t0: Option<Instant>,
}

fn prepare_store() -> &'static Mutex<PrepareState> {
    static STORE: OnceLock<Mutex<PrepareState>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(PrepareState::default()))
}

fn prepare_cmd_path() -> Option<PathBuf> {
    // Operator-owned only — never bake a machine-local path into the binary.
    env_nonempty("CREATIVE_PREPARE_CMD").map(PathBuf::from)
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

    match child.wait().await {
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

// ---------------------------------------------------------------------------
// Artifact store on disk (creative/jobs/) — the h3_video adapter's format.
// ---------------------------------------------------------------------------

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

fn write_disk_sidecar(data_dir: &Path, job_id: &str, meta: Value) {
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

fn forbidden(err: String) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "status": "error",
            "code": "missing-home-launch-token",
            "message": err,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/apps/home/creative/status — Studio chrome: upstreams + allocator.
pub(super) async fn home_creative_status(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
    }

    let default_scale = creative_scale_default();
    let probe_client = reqwest::Client::builder()
        .connect_timeout(COMFY_PROBE_TIMEOUT)
        .timeout(GENERATE_PROBE_TIMEOUT)
        .build()
        .ok();
    let comfy_up = comfy_up_for_note(probe_client.as_ref()).await;

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
                "wired": false,
                "reachable": false,
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
        // Character/Ref2VA becomes a model offer in a later phase; until then
        // the honest answer is "not wired" even if a Comfy happens to probe up.
        "character": {
            "wired": false,
            "upstream_reachable": false,
            "scale": 1,
            "modes": [],
            "note": if comfy_up {
                "Comfy is up, but Character is not a model offer yet — coming in a later phase."
            } else {
                "Character runs as a model offer in a later phase."
            },
        },
        "allocator": prepare_snapshot(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct CreativePrepareBody {
    /// `"generate"` or `"character"`.
    target: String,
}

/// POST /api/apps/home/creative/prepare — allocator: warm Generate or Character.
pub(super) async fn home_creative_prepare(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CreativePrepareBody>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
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
                "message": "Set CREATIVE_PREPARE_CMD to the operator prepare script",
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

/// GET /api/apps/home/creative/jobs — clip library (disk; in-flight tracking
/// lives in the model-provider run registry, surfaced via runs_events).
pub(super) async fn home_creative_jobs_list(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
    }

    let mut jobs: Vec<Value> = Vec::new();
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
                    for key in ["mode", "scale", "prompt", "duration"] {
                        if let Some(v) = sidecar.get(key) {
                            obj.insert(key.into(), v.clone());
                        }
                    }
                }
            }
            jobs.push(item);
        }
    }
    jobs.sort_by(|a, b| {
        let am = a.get("mtime_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        let bm = b.get("mtime_ms").and_then(|v| v.as_u64()).unwrap_or(0);
        bm.cmp(&am)
    });
    jobs.truncate(48);

    Json(json!({ "jobs": jobs })).into_response()
}

/// DELETE /api/apps/home/creative/jobs/:id — remove clip + sidecar.
/// In-flight runs are cancelled via the contract (runs_cancel), not here.
pub(super) async fn home_creative_jobs_delete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
    }

    if !is_creative_job_id(&job_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "code": "invalid_job_id" })),
        )
            .into_response();
    }

    let mp4 = creative_job_mp4_path(&state.data_dir, &job_id);
    let meta = creative_job_meta_path(&state.data_dir, &job_id);
    let partial = mp4.with_extension("mp4.partial");
    let had_mp4 = mp4.is_file();
    let _ = std::fs::remove_file(&mp4);
    let _ = std::fs::remove_file(&meta);
    let _ = std::fs::remove_file(&partial);

    if !had_mp4 {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "error", "code": "unknown_job" })),
        )
            .into_response();
    }

    Json(json!({ "id": job_id, "status": "deleted" })).into_response()
}

/// GET /api/apps/home/creative/jobs/:id/video — playback from disk.
pub(super) async fn home_creative_jobs_video(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    AxumPath(job_id): AxumPath<String>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
    }

    if !is_creative_job_id(&job_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let path = creative_job_mp4_path(&state.data_dir, &job_id);
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

#[derive(Debug, Deserialize)]
pub(super) struct CreativeStitchBody {
    /// Completed clip ids (32-hex), in cut order. 2–8.
    job_ids: Vec<String>,
}

fn ffmpeg_bin() -> String {
    env_nonempty("HOME_FFMPEG").unwrap_or_else(|| "ffmpeg".to_string())
}

fn concat_demuxer_line(path: &Path) -> String {
    let escaped = path.to_string_lossy().replace('\'', "'\\''");
    format!("file '{escaped}'")
}

/// POST /api/apps/home/creative/stitch — ffmpeg-concat N clips into a new
/// library artifact. App workflow (SP-STUDIO), not a model op.
pub(super) async fn home_creative_jobs_stitch(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CreativeStitchBody>,
) -> Response {
    if let Err(err) = require_home_gui(&state, &headers) {
        return forbidden(err);
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

    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut list_body = String::new();
    for id in &ids {
        let path = creative_job_mp4_path(&state.data_dir, id);
        list_body.push_str(&concat_demuxer_line(&path));
        list_body.push('\n');
    }
    if let Err(err) = std::fs::write(&list_path, &list_body) {
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

    write_disk_sidecar(
        &state.data_dir,
        &stitch_id,
        json!({
            "id": stitch_id,
            "status": "done",
            "mode": "storyboard",
            "scale": null,
            "prompt": prompt,
            "duration": null,
            "sources": ids.clone(),
        }),
    );

    Json(json!({
        "id": stitch_id,
        "status": "done",
        "mode": "storyboard",
        "sources": ids,
    }))
    .into_response()
}
