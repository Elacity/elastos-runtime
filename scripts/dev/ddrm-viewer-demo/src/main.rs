//! ddrm-viewer-demo — play an OWNED, CENC-encrypted video end-to-end, locally, in
//! the browser, through the REAL dDRM decrypt boundary.
//!
//! What it does (all on this machine, no servers, no Lit, no external KMS):
//!   1. Take a video (or generate a test clip), transcode to a fragmented MP4, and
//!      CENC-encrypt it as a multi-segment asset under a fresh random CEK.
//!   2. Launch a REAL `decrypt-provider` (rail-stream + rail-mint). It mints a
//!      hybrid session keypair IN-VM and publishes the public key.
//!   3. Act as the local key authority: seal the CEK to that published session key,
//!      bound to the exact decrypt transcript (the same `ddrm-envelope` handshake a
//!      dKMS/key-provider uses). The raw CEK never leaves this process; the sealed
//!      material carries no key.
//!   4. Serve the REAL `elacity-player` capsule + the scoped media routes
//!      (`/api/viewers/elacity-player/media/{session}` + `/init` + `/segment/{i}`).
//!      Each segment is decrypted IN-VM by the provider and relayed to the browser's
//!      MSE SourceBuffer — the CEK/IV NEVER cross to the browser.
//!
//! Usage:
//!   cargo run --manifest-path scripts/dev/ddrm-viewer-demo/Cargo.toml -- \
//!       [--video PATH] [--port 8099] [--decrypt-bin PATH] [--self-test]
//!
//! With no --video it generates a 6s test clip with ffmpeg. --decrypt-bin defaults
//! to the rail-stream build under capsules/decrypt-provider/target/debug.

mod mp4;
mod rail;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rand::RngCore;
use serde_json::{json, Value};

use ddrm_envelope::seal::{mldsa_seal_keypair, seal_bound};
use ddrm_envelope::transcript::{release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::{segment_digests, session_public_from_bytes, SUITE_PQ_HYBRID};

use rail::DecryptProviderProc;

const SESSION_ID: &str = "demo";
const VIEWER_ID: &str = "elacity-player";
const PRINCIPAL_ID: &str = "demo-user";
const TRANSCRIPT_SESSION: &str = "demo-session";
const OBJECT_CID: &str = "demo-object-cid";

// The real elacity-player capsule, served verbatim.
const PLAYER_INDEX: &str = include_str!("../../../../capsules/elacity-player/index.html");
const PLAYER_JS: &str = include_str!("../../../../capsules/elacity-player/player.js");
const PLAYER_CSS: &str = include_str!("../../../../capsules/elacity-player/style.css");
const PLAYER_FAVICON: &str = include_str!("../../../../capsules/elacity-player/favicon.svg");

/// Everything the media routes need, built once at startup.
struct MediaSession {
    mime: String,
    init: Vec<u8>,
    /// The sealed material (CEK-free) relayed to the provider per segment.
    material: Value,
    /// The authenticated decrypt request (no key material).
    request: Value,
    segment_count: usize,
    provider: DecryptProviderProc,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("\nddrm-viewer-demo: FAILED: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut video: Option<String> = None;
    let mut port: u16 = 8099;
    let mut decrypt_bin: Option<String> = None;
    let mut self_test = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--video" => video = args.next(),
            "--port" => port = args.next().and_then(|p| p.parse().ok()).ok_or("--port needs a number")?,
            "--decrypt-bin" => decrypt_bin = args.next(),
            "--self-test" => self_test = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .map_err(|e| format!("resolve repo root: {e}"))?;
    let decrypt_bin = decrypt_bin.unwrap_or_else(|| {
        repo_root
            .join("capsules/decrypt-provider/target/debug/decrypt-provider")
            .to_string_lossy()
            .into_owned()
    });

    println!("== ddrm-viewer-demo ==");

    // 1. Produce a clean fragmented MP4 (transcode for predictable MSE support).
    let work = std::env::temp_dir().join(format!("ddrm-viewer-demo-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| format!("mkdir work: {e}"))?;
    let fragmented = work.join("asset.mp4");
    produce_fragmented_mp4(video.as_deref(), &fragmented)?;
    let raw = std::fs::read(&fragmented).map_err(|e| format!("read asset: {e}"))?;
    let split = mp4::split_fragmented(&raw)?;
    let mime = format!("video/mp4; codecs=\"{}\"", mp4::avc_codec_string(&split.init));
    println!(
        "[1] fragmented MP4: {} init bytes, {} media fragments, mime {}",
        split.init.len(),
        split.fragments.len(),
        mime
    );

    // 2. Fresh CEK + a local key-authority seal identity (ML-DSA-65).
    let mut cek = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut cek);
    let mut seal_seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seal_seed);
    let (signer, authority_vk) = mldsa_seal_keypair(seal_seed);
    let authority_vk_b64 = base64::engine::general_purpose::STANDARD.encode(&authority_vk);

    // 3. CENC-encrypt every fragment under the CEK (globally-unique per-sample IVs).
    let mut iv_counter: u64 = 1;
    let mut encrypted: Vec<Vec<u8>> = Vec::with_capacity(split.fragments.len());
    for frag in &split.fragments {
        encrypted.push(mp4::encrypt_fragment(frag, &cek, &mut iv_counter)?);
    }
    println!("[2] CENC-encrypted {} fragments under a fresh CEK (the CEK never leaves this process)", encrypted.len());

    // 4. Launch the REAL rail decrypt-provider; it mints + publishes its session key.
    let provider = DecryptProviderProc::launch(&decrypt_bin, &authority_vk_b64)?;
    let session_pub_bytes = base64::engine::general_purpose::STANDARD
        .decode(&provider.session_pub_b64)
        .map_err(|e| format!("decode session pub: {e}"))?;
    println!("[3] launched decrypt-provider; it minted + published an in-VM session key ({} bytes)", session_pub_bytes.len());

    // 5. Seal the CEK to the published session key, bound to the decrypt transcript.
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut content_hash = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut content_hash);
    let expires_at = now_unix() + 3600;

    let release_schema = "elastos.release.receipt/v1";
    let rr_request_id = "demo-release";
    let rr_provider = "key-provider";
    let rr_status = "released";
    let issued_at = now_unix();
    let rr_hash = release_receipt_hash(
        release_schema,
        rr_request_id,
        OBJECT_CID,
        PRINCIPAL_ID,
        TRANSCRIPT_SESSION,
        "stream",
        rr_provider,
        rr_status,
        issued_at,
        expires_at,
    );

    let public = session_public_from_bytes(&session_pub_bytes)
        .ok_or("could not parse the provider's published session key")?;
    let seg_refs: Vec<&[u8]> = encrypted.iter().map(|s| s.as_slice()).collect();
    let digests = segment_digests(&seg_refs);
    let aad = DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt-provider",
        principal_id: PRINCIPAL_ID,
        session_id: TRANSCRIPT_SESSION,
        object_cid: OBJECT_CID,
        content_hash: &content_hash,
        action: "stream",
        viewer_interface: "elastos.viewer/media@1",
        output_kind: "stream",
        expires_at,
        release_receipt_hash: rr_hash,
        decrypt_session_pub: &session_pub_bytes,
        nonce: &nonce,
        node_set_id: None,
    }
    .to_aad_with_segments(Some(&digests));
    let sealed = seal_bound(&public, &cek, &aad, &signer).to_bytes();
    // Scrub the CEK from this process now that it is sealed.
    cek.iter_mut().for_each(|b| *b = 0);
    println!("[4] sealed the CEK to the session key, transcript-bound ({} sealed bytes); CEK zeroized", sealed.len());

    let b64 = base64::engine::general_purpose::STANDARD;
    let material = json!({
        "suite": SUITE_PQ_HYBRID,
        "sealed_cek_b64": b64.encode(&sealed),
        "ciphertext_b64": b64.encode(&encrypted[0]),
        "init_segment_b64": Value::Null,
        "nonce_b64": b64.encode(nonce),
        "content_hash_b64": b64.encode(content_hash),
        "extra_segments_b64": encrypted[1..].iter().map(|s| b64.encode(s)).collect::<Vec<_>>(),
    });
    let request = json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "demo-decrypt",
        "principal_id": PRINCIPAL_ID,
        "session_id": TRANSCRIPT_SESSION,
        "object_cid": OBJECT_CID,
        "action": "stream",
        "viewer_interface": "elastos.viewer/media@1",
        "release_receipt": {
            "schema": release_schema,
            "request_id": rr_request_id,
            "object_cid": OBJECT_CID,
            "principal_id": PRINCIPAL_ID,
            "session_id": TRANSCRIPT_SESSION,
            "action": "stream",
            "provider": rr_provider,
            "status": rr_status,
            "issued_at": issued_at,
            "expires_at": expires_at,
        },
        "output_kind": "stream",
        "reason": "local viewer-seam demo",
        "expires_at": expires_at,
    });

    let session = Arc::new(MediaSession {
        mime,
        init: split.init,
        material,
        request,
        segment_count: encrypted.len(),
        provider,
    });

    // Optional: prove the rail round-trips before the browser (decrypt every
    // segment, strip senc, reassemble, ffprobe the result), then exit.
    if self_test {
        run_self_test(&session, &work)?;
        return Ok(());
    }

    serve(session, port)
}

/// Transcode/normalize the input into a fragmented MP4 (or generate a test clip).
fn produce_fragmented_mp4(input: Option<&str>, out: &std::path::Path) -> Result<(), String> {
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
    let status = cmd.status().map_err(|e| format!("run ffmpeg (is it installed?): {e}"))?;
    if !status.success() {
        return Err("ffmpeg failed to produce the fragmented MP4".to_string());
    }
    Ok(())
}

/// Decrypt every segment through the real provider, strip senc, reassemble, and
/// ffprobe the result to confirm the round-trip yields a valid playable video.
fn run_self_test(session: &MediaSession, work: &std::path::Path) -> Result<(), String> {
    println!("[self-test] decrypting all {} segments through the real provider…", session.segment_count);
    let mut whole = session.init.clone();
    for i in 0..session.segment_count {
        let decrypted = session
            .provider
            .stream_segment(&session.request, &session.material, i, now_unix())?;
        let clean = mp4::strip_senc(&decrypted)?;
        whole.extend_from_slice(&clean);
    }
    let out = work.join("roundtrip.mp4");
    std::fs::write(&out, &whole).map_err(|e| format!("write roundtrip: {e}"))?;
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration,format_name", "-of", "default=nw=1", out.to_str().unwrap()])
        .output()
        .map_err(|e| format!("run ffprobe: {e}"))?;
    if !probe.status.success() {
        return Err(format!(
            "ffprobe rejected the reassembled video — the round-trip is not playable:\n{}",
            String::from_utf8_lossy(&probe.stderr)
        ));
    }
    println!(
        "[self-test] PASS — reassembled {} bytes; ffprobe: {}",
        whole.len(),
        String::from_utf8_lossy(&probe.stdout).trim().replace('\n', ", ")
    );
    Ok(())
}

// ── Minimal HTTP/1.1 server (localhost demo) ───────────────────────────────────

fn serve(session: Arc<MediaSession>, port: u16) -> Result<(), String> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let url = format!("http://localhost:{port}/");
    println!("\n[5] serving the elacity-player at {url}");
    println!("    open it in your browser — the owned, encrypted video will play.");
    println!("    (the CEK/IV never reach the browser; each segment is decrypted in-VM)\n");
    // Best-effort auto-open on macOS.
    let _ = Command::new("open").arg(&url).status();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let session = session.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_conn(stream, &session) {
                        eprintln!("[http] connection error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("[http] accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_conn(mut stream: TcpStream, session: &MediaSession) -> Result<(), String> {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).map_err(|e| format!("read request: {e}"))?;
    if n == 0 {
        return Ok(());
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut parts = head.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let path = path.split('?').next().unwrap_or("/");

    let media_base = format!("/api/viewers/{VIEWER_ID}/media/{SESSION_ID}");
    if path == "/" {
        let location = format!("/player/?session={SESSION_ID}&home_token=demo");
        return write_redirect(&mut stream, &location);
    } else if path == "/player/" || path == "/player/index.html" {
        return write_text(&mut stream, "200 OK", "text/html; charset=utf-8", PLAYER_INDEX.as_bytes());
    } else if path == "/player/player.js" {
        return write_text(&mut stream, "200 OK", "text/javascript; charset=utf-8", PLAYER_JS.as_bytes());
    } else if path == "/player/style.css" {
        return write_text(&mut stream, "200 OK", "text/css; charset=utf-8", PLAYER_CSS.as_bytes());
    } else if path == "/player/favicon.svg" {
        return write_text(&mut stream, "200 OK", "image/svg+xml", PLAYER_FAVICON.as_bytes());
    } else if path == media_base {
        let manifest = json!({
            "schema": "elastos.viewer.media/v1",
            "mime": session.mime,
            "segment_count": session.segment_count,
            "has_init": true,
            "is_protected": true,
            "expires_at": session.request["expires_at"].clone(),
        });
        return write_text(&mut stream, "200 OK", "application/json", manifest.to_string().as_bytes());
    } else if path == format!("{media_base}/init") {
        return write_bytes(&mut stream, "200 OK", "application/octet-stream", &session.init);
    } else if let Some(idx) = path.strip_prefix(&format!("{media_base}/segment/")) {
        let index: usize = match idx.parse() {
            Ok(i) => i,
            Err(_) => return write_text(&mut stream, "400 Bad Request", "text/plain", b"bad segment index"),
        };
        match session
            .provider
            .stream_segment(&session.request, &session.material, index, now_unix())
        {
            Ok(decrypted) => {
                let clean = mp4::strip_senc(&decrypted)?;
                return write_bytes(&mut stream, "200 OK", "application/octet-stream", &clean);
            }
            Err(e) => {
                eprintln!("[media] segment {index} fail-closed: {e}");
                return write_text(&mut stream, "404 Not Found", "text/plain", b"segment unavailable");
            }
        }
    }
    write_text(&mut stream, "404 Not Found", "text/plain", b"not found")
}

fn write_redirect(stream: &mut TcpStream, location: &str) -> Result<(), String> {
    let resp = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(resp.as_bytes()).map_err(|e| format!("write redirect: {e}"))
}

fn write_text(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> Result<(), String> {
    write_bytes(stream, status, content_type, body)
}

fn write_bytes(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> Result<(), String> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).map_err(|e| format!("write header: {e}"))?;
    stream.write_all(body).map_err(|e| format!("write body: {e}"))?;
    stream.flush().map_err(|e| format!("flush: {e}"))
}
