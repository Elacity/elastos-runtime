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

use ddrm_media::{prepare, prepare_blob, PreparedSession, SessionParams};

mod grant;
mod quorum;
use quorum::{run_quorum, QuorumArgs};

/// Guess a content type from a file extension for the NON-MEDIA object path.
fn mime_for_path(path: &str) -> &'static str {
    let lower = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match lower.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        _ => "application/octet-stream",
    }
}

/// A small self-contained sample asset for the out-of-the-box non-media demo (no
/// scripts, no external refs) — renders as an image in the viewer to make the
/// click-to-play obvious without a real owned file on disk.
fn sample_object_svg() -> String {
    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 450">
  <rect width="800" height="450" fill="#0b0d12"/>
  <rect x="24" y="24" width="752" height="402" rx="18" fill="#14171f" stroke="#6ea8fe" stroke-width="2"/>
  <text x="400" y="180" fill="#e8eaf0" font-family="system-ui, sans-serif" font-size="40" font-weight="700" text-anchor="middle">Owned protected asset</text>
  <text x="400" y="240" fill="#8b90a0" font-family="system-ui, sans-serif" font-size="22" text-anchor="middle">decrypted in the dDRM boundary · rendered via ddrm-viewer</text>
  <text x="400" y="300" fill="#6ea8fe" font-family="ui-monospace, monospace" font-size="18" text-anchor="middle">elastos.viewer/document@1</text>
</svg>
"##
    .to_string()
}

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

/// Decode the optional `--rights-binding` hex into raw bytes (the rights-decision
/// receipt hash). `None`/empty => no binding (byte-identical to the pre-gate seal).
fn decode_rights_binding(hex_str: Option<&str>) -> Result<Option<Vec<u8>>, String> {
    match hex_str.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(hex_str) => {
            let bytes = hex_decode(hex_str).ok_or("--rights-binding must be valid hex")?;
            Ok(Some(bytes))
        }
    }
}

/// Minimal hex decoder (no extra dep): even-length ASCII hex -> bytes.
fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks(2) {
        out.push((val(pair[0])? << 4) | val(pair[1])?);
    }
    Some(out)
}

fn run() -> Result<(), String> {
    // Trustless-open sidecar modes (one-shot, stdin JSON -> stdout JSON): the gateway shells out so
    // it never links the PQ session signer. Handled before the per-open arg parsing.
    match std::env::args().nth(1).as_deref() {
        Some("--grant-prepare") => return grant::run_grant_prepare(),
        Some("--grant-assemble") => return grant::run_grant_assemble(),
        _ => {}
    }

    let mut principal: Option<String> = None;
    let mut video: Option<String> = None;
    let mut object_file: Option<String> = None;
    let mut object_mime: Option<String> = None;
    let mut object_mode = false;
    let mut decrypt_bin: Option<String> = None;
    let mut object_cid = "elastos-owned-media".to_string();
    let mut ttl_secs: u64 = 3600;
    let mut rights_binding: Option<String> = None;
    // `--quorum` (dKMS consumer-open) inputs.
    let mut quorum_mode = false;
    let mut key_bin: Option<String> = None;
    let mut capsule_path: Option<String> = None;
    let mut descriptor_path: Option<String> = None;
    let mut caller_seed_b64: Option<String> = None;
    let mut access_grant_b64: Option<String> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--principal" => principal = args.next(),
            "--video" => video = args.next(),
            "--object" => object_mode = true,
            "--object-file" => object_file = args.next(),
            "--mime" => object_mime = args.next(),
            "--decrypt-bin" => decrypt_bin = args.next(),
            "--object-cid" => object_cid = args.next().ok_or("--object-cid needs a value")?,
            // Hex SHA-256 of the rights-decision receipt; welded into the decrypt
            // transcript so the seal is bound to THIS rights decision.
            "--rights-binding" => rights_binding = args.next(),
            "--ttl-secs" => {
                ttl_secs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--ttl-secs needs a number")?
            }
            // dKMS consumer-open: recover the asset's REAL CEK from the 2-of-3 quorum.
            "--quorum" => quorum_mode = true,
            "--key-bin" => key_bin = args.next(),
            "--capsule" => capsule_path = args.next(),
            "--descriptor" => descriptor_path = args.next(),
            "--caller-seed" => caller_seed_b64 = args.next(),
            // OPTIONAL base64 of the wallet-signed AccessGrantV1 JSON (trustless authorization).
            "--access-grant" => access_grant_b64 = args.next(),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let rights_binding = decode_rights_binding(rights_binding.as_deref())?;
    let principal = principal.ok_or("--principal is required")?;
    let decrypt_bin = decrypt_bin.ok_or("--decrypt-bin is required")?;
    if !Path::new(&decrypt_bin).is_file() {
        return Err(format!("decrypt-provider binary not found: {decrypt_bin}"));
    }

    // dKMS consumer-open path: recover the minted asset's CEK from the 2-of-3 quorum and serve
    // the byte-identical cleartext (the production analogue of the local-test-KMS object path).
    if quorum_mode {
        let key_bin = key_bin.ok_or("--key-bin is required for --quorum")?;
        if !Path::new(&key_bin).is_file() {
            return Err(format!("key-provider binary not found: {key_bin}"));
        }
        return run_quorum(QuorumArgs {
            principal,
            decrypt_bin,
            key_bin,
            capsule_path: capsule_path.ok_or("--capsule is required for --quorum")?,
            descriptor_path: descriptor_path.ok_or("--descriptor is required for --quorum")?,
            caller_seed_b64: caller_seed_b64.ok_or("--caller-seed is required for --quorum")?,
            object_cid,
            mime: object_mime.unwrap_or_else(|| "application/octet-stream".to_string()),
            ttl_secs,
            access_grant_b64,
        });
    }

    // NON-MEDIA object mode: seal an arbitrary owned asset (image/pdf/text/3D) and
    // serve it as a single decrypted blob through the SAME rail as media.
    if object_mode || object_file.is_some() {
        return run_object(
            &principal,
            object_file.as_deref(),
            object_mime,
            &decrypt_bin,
            &object_cid,
            ttl_secs,
            rights_binding,
        );
    }

    // CENC-pack + launch the boundary + seal the CEK (all in the shared crate).
    let work = std::env::temp_dir().join(format!("ddrm-media-authority-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| format!("mkdir work: {e}"))?;
    let fragmented = work.join("asset.mp4");
    produce_fragmented_mp4(video.as_deref(), &fragmented)?;
    let raw = std::fs::read(&fragmented).map_err(|e| format!("read asset: {e}"))?;

    let mut params = SessionParams::for_object(&principal, &object_cid);
    params.ttl_secs = ttl_secs;
    params.rights_receipt_hash = rights_binding;
    let session = prepare(&raw, &decrypt_bin, &params, now_unix())?;

    // Print the descriptor line (NO key material), then serve segment reads.
    let descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "kind": "media",
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

/// NON-MEDIA path: seal an owned object and serve it as one decrypted blob. When no
/// `--object-file` is supplied (the out-of-the-box demo, mirroring the generated test
/// clip on the media path), a built-in sample SVG stands in.
#[allow(clippy::too_many_arguments)]
fn run_object(
    principal: &str,
    path: Option<&str>,
    mime: Option<String>,
    decrypt_bin: &str,
    object_cid: &str,
    ttl_secs: u64,
    rights_binding: Option<Vec<u8>>,
) -> Result<(), String> {
    let (bytes, mime) = match path {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("read object {path}: {e}"))?;
            if bytes.is_empty() {
                return Err(format!("object {path} is empty"));
            }
            let mime = mime.unwrap_or_else(|| mime_for_path(path).to_string());
            (bytes, mime)
        }
        None => (
            sample_object_svg().into_bytes(),
            mime.unwrap_or_else(|| "image/svg+xml".to_string()),
        ),
    };

    let mut params = SessionParams::for_object(principal, object_cid);
    params.ttl_secs = ttl_secs;
    params.reason = "owned object render".to_string();
    params.rights_receipt_hash = rights_binding;
    let session = prepare_blob(&bytes, &mime, decrypt_bin, &params, now_unix())?;

    let descriptor = json!({
        "schema": "elastos.media-authority.session/v1",
        "kind": "object",
        "mime": mime,
        "byte_length": bytes.len(),
        "expires_at": session.expires_at,
    });
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{descriptor}").map_err(|e| format!("write descriptor: {e}"))?;
    out.flush().map_err(|e| format!("flush descriptor: {e}"))?;

    serve_object(&session, &mut out)
}

fn serve_object(session: &PreparedSession, out: &mut impl Write) -> Result<(), String> {
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
            Some("object") => match session.decrypt_object(now_unix()) {
                Ok(bytes) => reply(
                    out,
                    &json!({
                        "status": "ok",
                        "object_b64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    }),
                )?,
                Err(e) => reply(out, &json!({"status": "error", "message": e}))?,
            },
            other => reply(
                out,
                &json!({"status": "error", "message": format!("unknown op: {other:?}")}),
            )?,
        }
    }
    Ok(())
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
