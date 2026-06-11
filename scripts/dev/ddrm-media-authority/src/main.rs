//! ddrm-media-authority — the local key-authority as an isolated subprocess.
//!
//! The gateway spawns one of these per media open. It is the "local test KMS"
//! adapter: it CENC-packs an owned video under a fresh CEK, launches a SEPARATE
//! rail `decrypt-provider` boundary, seals the CEK to that boundary's in-VM-minted
//! session key (PQ-hybrid, transcript-bound, via `ddrm-envelope`), zeroizes the
//! CEK, and then answers per-segment reads — returning ONLY already-decrypted,
//! `senc`-stripped segment bytes. The CEK never leaves this process unsealed and
//! never reaches the gateway or the browser.
//!
//! Protocol (line-delimited JSON on stdio):
//!   - On startup it prints exactly ONE descriptor line on stdout (no key material):
//!       {"schema":"elastos.media-authority.session/v1","mime":...,
//!        "segment_count":N,"init_b64":...,"expires_at":T}
//!   - Then it reads request lines on stdin and replies one line each on stdout:
//!       {"op":"segment","index":I}  -> {"status":"ok","segment_b64":...}
//!                                    |  {"status":"error","message":...}
//!       {"op":"shutdown"}           -> exits 0
//!
//! Usage (spawned by the gateway):
//!   ddrm-media-authority --principal <id> [--video PATH] [--decrypt-bin PATH]
//!                        [--object-cid CID] [--ttl-secs N]

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

use ddrm_media::{prepare, PreparedSession, SessionParams};

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn main() {
    if let Err(e) = run() {
        // Errors go to stderr (inherited by the gateway); stdout stays protocol-pure.
        eprintln!("ddrm-media-authority: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut principal: Option<String> = None;
    let mut video: Option<String> = None;
    let mut decrypt_bin: Option<String> = None;
    let mut object_cid = "elastos-owned-media".to_string();
    let mut ttl_secs: u64 = 3600;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--principal" => principal = args.next(),
            "--video" => video = args.next(),
            "--decrypt-bin" => decrypt_bin = args.next(),
            "--object-cid" => object_cid = args.next().ok_or("--object-cid needs a value")?,
            "--ttl-secs" => {
                ttl_secs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--ttl-secs needs a number")?
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let principal = principal.ok_or("--principal is required")?;
    let decrypt_bin = decrypt_bin.ok_or("--decrypt-bin is required")?;
    if !Path::new(&decrypt_bin).is_file() {
        return Err(format!("decrypt-provider binary not found: {decrypt_bin}"));
    }

    // CENC-pack + launch the boundary + seal the CEK (all in the shared crate).
    let work = std::env::temp_dir().join(format!("ddrm-media-authority-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| format!("mkdir work: {e}"))?;
    let fragmented = work.join("asset.mp4");
    produce_fragmented_mp4(video.as_deref(), &fragmented)?;
    let raw = std::fs::read(&fragmented).map_err(|e| format!("read asset: {e}"))?;

    let mut params = SessionParams::for_object(&principal, &object_cid);
    params.ttl_secs = ttl_secs;
    let session = prepare(&raw, &decrypt_bin, &params, now_unix())?;

    // Print the descriptor line (NO key material), then serve segment reads.
    let descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "mime": session.mime,
        "segment_count": session.segment_count,
        "init_b64": base64::engine::general_purpose::STANDARD.encode(&session.init),
        "expires_at": session.expires_at,
    });
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{descriptor}").map_err(|e| format!("write descriptor: {e}"))?;
    out.flush().map_err(|e| format!("flush descriptor: {e}"))?;

    serve(&session, &mut out)
}

fn serve(session: &PreparedSession, out: &mut impl Write) -> Result<(), String> {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("read stdin: {e}"))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(e) => {
                reply(out, &json!({"status": "error", "message": format!("bad request json: {e}")}))?;
                continue;
            }
        };
        match req.get("op").and_then(Value::as_str) {
            Some("shutdown") => return Ok(()),
            Some("segment") => {
                let index = req.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                match session.decrypt_segment_clean(index, now_unix()) {
                    Ok(bytes) => reply(
                        out,
                        &json!({
                            "status": "ok",
                            "segment_b64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                        }),
                    )?,
                    Err(e) => reply(out, &json!({"status": "error", "message": e}))?,
                }
            }
            other => reply(
                out,
                &json!({"status": "error", "message": format!("unknown op: {other:?}")}),
            )?,
        }
    }
    Ok(())
}

fn reply(out: &mut impl Write, value: &Value) -> Result<(), String> {
    writeln!(out, "{value}").map_err(|e| format!("write reply: {e}"))?;
    out.flush().map_err(|e| format!("flush reply: {e}"))
}

/// Transcode/normalize the input into a fragmented MP4 (or generate a test clip).
fn produce_fragmented_mp4(input: Option<&str>, out: &PathBuf) -> Result<(), String> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");
    match input {
        Some(path) => {
            cmd.args(["-i", path]);
        }
        None => {
            cmd.args(["-f", "lavfi", "-i", "testsrc=duration=6:size=320x240:rate=15"]);
        }
    }
    cmd.args([
        "-c:v", "libx264",
        "-profile:v", "baseline",
        "-level", "3.0",
        "-pix_fmt", "yuv420p",
        "-g", "15",
        "-an",
        "-t", "6",
        "-movflags", "+frag_keyframe+empty_moov+default_base_moof",
        "-frag_duration", "1000000",
    ]);
    cmd.arg(out);
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    let status = cmd
        .status()
        .map_err(|e| format!("run ffmpeg (is it installed?): {e}"))?;
    if !status.success() {
        return Err("ffmpeg failed to produce the fragmented MP4".to_string());
    }
    Ok(())
}
