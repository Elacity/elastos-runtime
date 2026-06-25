//! ddrm-viewer-demo — play an OWNED, CENC-encrypted video end-to-end, locally, in
//! the browser, through the REAL dDRM decrypt boundary.
//!
//! All of the crypto/packaging lives in the shared `ddrm-media` crate (the same one
//! the gateway uses). This binary is just a thin local HTTP server over a
//! `ddrm_media::PreparedSession`:
//!   1. Transcode a video to a fragmented MP4 (ffmpeg).
//!   2. `ddrm_media::prepare` CENC-packs it under a fresh CEK, launches a REAL
//!      `decrypt-provider` (rail-stream + rail-mint), and seals the CEK to the
//!      provider's in-VM-minted session key, transcript-bound. The CEK is zeroized.
//!   3. Serve the REAL `elacity-player` capsule + the scoped media routes; each
//!      segment is decrypted IN-VM and relayed to MSE — the CEK/IV never cross.
//!
//! Usage:
//!   cargo run --manifest-path scripts/dev/ddrm-viewer-demo/Cargo.toml -- \
//!       [--video PATH] [--port 8099] [--decrypt-bin PATH] [--self-test]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use ddrm_media::{prepare, PreparedSession, SessionParams};

const SESSION_ID: &str = "demo";
const VIEWER_ID: &str = "elacity-player";
const PRINCIPAL_ID: &str = "demo-user";
const OBJECT_CID: &str = "demo-object-cid";

// The real elacity-player capsule, served verbatim.
const PLAYER_INDEX: &str = include_str!("../../../../capsules/elacity-player/index.html");
const PLAYER_JS: &str = include_str!("../../../../capsules/elacity-player/player.js");
const PLAYER_CSS: &str = include_str!("../../../../capsules/elacity-player/style.css");
const PLAYER_FAVICON: &str = include_str!("../../../../capsules/elacity-player/favicon.svg");

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

    // 2-4. CENC-pack + launch provider + seal the CEK, all in the shared crate.
    let params = SessionParams::for_object(PRINCIPAL_ID, OBJECT_CID);
    let session = Arc::new(prepare(&raw, &decrypt_bin, &params, now_unix())?);
    println!(
        "[1] prepared session: {} init bytes, {} segments, mime {}; CEK sealed + zeroized",
        session.init.len(),
        session.segment_count,
        session.mime
    );

    // Optional: prove the rail round-trips before the browser, then exit.
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
fn run_self_test(session: &PreparedSession, work: &std::path::Path) -> Result<(), String> {
    println!("[self-test] decrypting all {} segments through the real provider…", session.segment_count);
    let mut whole = session.init.clone();
    for i in 0..session.segment_count {
        whole.extend_from_slice(&session.decrypt_segment_clean(i, now_unix())?);
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

fn serve(session: Arc<PreparedSession>, port: u16) -> Result<(), String> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let url = format!("http://localhost:{port}/");
    println!("\n[2] serving the elacity-player at {url}");
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

fn handle_conn(mut stream: TcpStream, session: &PreparedSession) -> Result<(), String> {
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
            "expires_at": session.expires_at,
        });
        return write_text(&mut stream, "200 OK", "application/json", manifest.to_string().as_bytes());
    } else if path == format!("{media_base}/init") {
        return write_bytes(&mut stream, "200 OK", "application/octet-stream", &session.init);
    } else if let Some(idx) = path.strip_prefix(&format!("{media_base}/segment/")) {
        let index: usize = match idx.parse() {
            Ok(i) => i,
            Err(_) => return write_text(&mut stream, "400 Bad Request", "text/plain", b"bad segment index"),
        };
        match session.decrypt_segment_clean(index, now_unix()) {
            Ok(clean) => {
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
