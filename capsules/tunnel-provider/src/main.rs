//! ElastOS Tunnel Provider Capsule
//!
//! Manages public quick tunnels via cloudflared.
//! Wire protocol: line-delimited JSON over stdin/stdout.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    Start {
        target: String,
    },
    Stop,
    Status,
    Ping,
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: serde_json::Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn ok_empty() -> Self {
        Response::Ok { data: None }
    }

    fn error(code: &str, message: &str) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

struct TunnelProvider {
    child: Option<Child>,
    cloudflared_path: Option<PathBuf>,
    url_slot: Arc<Mutex<Option<String>>>,
    last_log: Arc<Mutex<Option<String>>>,
}

impl TunnelProvider {
    fn new() -> Self {
        Self {
            child: None,
            cloudflared_path: None,
            url_slot: Arc::new(Mutex::new(None)),
            last_log: Arc::new(Mutex::new(None)),
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::Start { target } => self.start(&target),
            Request::Stop => self.stop(),
            Request::Status => self.status(),
            Request::Ping => Response::ok(serde_json::json!({ "pong": true })),
            Request::Shutdown => {
                let _ = self.stop();
                Response::ok(serde_json::json!({ "message": "Tunnel provider shutting down" }))
            }
        }
    }

    fn init(&mut self, config: serde_json::Value) -> Response {
        if let Some(path) = config.get("cloudflared_path").and_then(|v| v.as_str()) {
            let p = PathBuf::from(path);
            if p.is_file() {
                self.cloudflared_path = Some(p);
            }
        }

        let resolver = self
            .resolve_cloudflared_path()
            .ok()
            .map(|p| p.to_string_lossy().to_string());

        Response::ok(serde_json::json!({
            "protocol_version": "1.0",
            "provider": "tunnel",
            "cloudflared": resolver,
        }))
    }

    fn start(&mut self, target: &str) -> Response {
        if !target.starts_with("http://") && !target.starts_with("https://") {
            return Response::error(
                "invalid_target",
                "target must start with http:// or https://",
            );
        }

        self.cleanup_exited();
        if self.child.is_some() {
            let url = self.url_slot.lock().ok().and_then(|s| s.clone());
            return Response::ok(serde_json::json!({
                "running": true,
                "url": url,
                "reused": true,
            }));
        }

        let cloudflared = match self.resolve_cloudflared_path() {
            Ok(p) => p,
            Err(e) => return Response::error("cloudflared_not_found", &e),
        };

        if let Ok(mut slot) = self.url_slot.lock() {
            *slot = None;
        }
        if let Ok(mut log) = self.last_log.lock() {
            *log = None;
        }

        let mut child = match Command::new(&cloudflared)
            .args(["tunnel", "--url", target, "--no-autoupdate"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return Response::error(
                    "spawn_failed",
                    &format!(
                        "Failed to start cloudflared at {}: {}",
                        cloudflared.display(),
                        e
                    ),
                )
            }
        };

        let stderr = child.stderr.take();
        if let Some(stderr) = stderr {
            let url_slot = Arc::clone(&self.url_slot);
            let last_log = Arc::clone(&self.last_log);
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(mut log) = last_log.lock() {
                        *log = Some(line.clone());
                    }
                    if let Some(url) = extract_trycloudflare_url(&line) {
                        if let Ok(mut slot) = url_slot.lock() {
                            if slot.is_none() {
                                *slot = Some(url.clone());
                                eprintln!("tunnel-provider: public URL {}", url);
                            }
                        }
                    }
                }
            });
        }

        self.child = Some(child);
        Response::ok(serde_json::json!({
            "running": true,
            "target": target,
            "url": self.url_slot.lock().ok().and_then(|s| s.clone()),
            "last_log": self.last_log.lock().ok().and_then(|l| l.clone()),
        }))
    }

    fn stop(&mut self) -> Response {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(mut slot) = self.url_slot.lock() {
            *slot = None;
        }

        Response::ok_empty()
    }

    fn status(&mut self) -> Response {
        self.cleanup_exited();
        let running = self.child.is_some();
        let url = self.url_slot.lock().ok().and_then(|s| s.clone());
        let last_log = self.last_log.lock().ok().and_then(|l| l.clone());

        Response::ok(serde_json::json!({
            "running": running,
            "url": url,
            "last_log": last_log,
        }))
    }

    fn cleanup_exited(&mut self) {
        let exited = if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(_) => true,
            }
        } else {
            false
        };

        if exited {
            self.child = None;
        }
    }

    fn resolve_cloudflared_path(&self) -> Result<PathBuf, String> {
        if let Some(p) = &self.cloudflared_path {
            if p.is_file() {
                return Ok(p.clone());
            }
        }

        if let Ok(path) = std::env::var("ELASTOS_CLOUDFLARED_BIN") {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Ok(p);
            }
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                // Release package layout: tunnel-provider and cloudflared side by side
                let bundled = dir.join("cloudflared");
                if bundled.is_file() {
                    return Ok(bundled);
                }

                // Dev layout:
                //   capsules/tunnel-provider/target/release/tunnel-provider
                //   capsules/tunnel-provider/bin/<platform>/cloudflared
                if let Some(capsule_root) = dir.parent().and_then(|p| p.parent()) {
                    let dev_bundled = capsule_root
                        .join("bin")
                        .join(platform_dir_name())
                        .join("cloudflared");
                    if dev_bundled.is_file() {
                        return Ok(dev_bundled);
                    }
                }
            }
        }

        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home)
                .join(".local/share/elastos/bin")
                .join("cloudflared");
            if p.is_file() {
                return Ok(p);
            }
        }

        if let Ok(data_dir) = std::env::var("ELASTOS_DATA_DIR") {
            let p = PathBuf::from(data_dir).join("bin/cloudflared");
            if p.is_file() {
                return Ok(p);
            }
        }

        if let Some(found) = find_in_path("cloudflared") {
            return Ok(found);
        }

        Err(
            "cloudflared not found. Run: elastos setup --with cloudflared"
                .to_string(),
        )
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn platform_dir_name() -> &'static str {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => "x86_64-linux",
        ("aarch64", "linux") => "aarch64-linux",
        ("x86_64", "macos") => "x86_64-darwin",
        ("aarch64", "macos") => "aarch64-darwin",
        _ => "unknown",
    }
}

fn extract_trycloudflare_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let tail = &line[start..];
    let end = tail.find(|c: char| c.is_whitespace()).unwrap_or(tail.len());
    let mut url = tail[..end]
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';')
        .to_string();

    // Trim common trailing punctuation that appears in logs
    while url.ends_with('.') || url.ends_with(')') {
        url.pop();
    }

    // Accept only root host URLs for quick tunnels.
    // Reject control-plane/API URLs such as https://api.trycloudflare.com/tunnel.
    let without_scheme = url.strip_prefix("https://")?;
    let host = without_scheme.split('/').next().unwrap_or("");
    let path = &without_scheme[host.len()..];
    if host.ends_with(".trycloudflare.com")
        && host != "api.trycloudflare.com"
        && (path.is_empty() || path == "/")
    {
        Some(format!("https://{}", host))
    } else {
        None
    }
}

fn main() {
    eprintln!("tunnel-provider: starting v{}", PROVIDER_VERSION);
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = TunnelProvider::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::error("bad_request", &format!("Invalid JSON request: {}", e));
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"internal\",\"message\":\"serialize failed\"}".to_string()));
                let _ = stdout.flush();
                continue;
            }
        };

        let is_shutdown = matches!(&req, Request::Shutdown);
        let resp = provider.handle(req);

        match serde_json::to_string(&resp) {
            Ok(json) => {
                let _ = writeln!(stdout, "{}", json);
                let _ = stdout.flush();
            }
            Err(_) => {
                let _ = writeln!(
                    stdout,
                    "{{\"status\":\"error\",\"code\":\"internal\",\"message\":\"serialize failed\"}}"
                );
                let _ = stdout.flush();
            }
        }

        if is_shutdown {
            break;
        }
    }

    let _ = provider.stop();
}

#[cfg(test)]
mod tests {
    use super::extract_trycloudflare_url;

    #[test]
    fn test_extract_trycloudflare_url() {
        let line = "INF +--------------------------------------------------------------------------------------------+";
        assert!(extract_trycloudflare_url(line).is_none());

        let line = "INF |  https://fuzzy-forest-123.trycloudflare.com                                             |";
        let url = extract_trycloudflare_url(line).expect("url");
        assert_eq!(url, "https://fuzzy-forest-123.trycloudflare.com");

        let line = "visit https://abc.trycloudflare.com now";
        let url = extract_trycloudflare_url(line).expect("url");
        assert_eq!(url, "https://abc.trycloudflare.com");

        // API endpoint should be ignored
        let line = "POST https://api.trycloudflare.com/tunnel";
        assert!(extract_trycloudflare_url(line).is_none());
    }
}
