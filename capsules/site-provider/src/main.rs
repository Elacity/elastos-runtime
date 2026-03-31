//! ElastOS Site Provider
//!
//! Serves the staged `localhost://MyWebSite` root over local HTTP.
//! Wire protocol: line-delimited JSON over stdin/stdout.

use std::io::{self, BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_http::services::ServeDir;

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
        addr: String,
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
        Self::Ok { data: Some(data) }
    }

    fn ok_empty() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Serialize)]
struct SiteStatus {
    running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    local_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bind_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    site_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reused: Option<bool>,
}

struct RunningServer {
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
    bind_addr: SocketAddr,
}

struct SiteProvider {
    runtime: Runtime,
    site_root: Option<PathBuf>,
    server: Option<RunningServer>,
    last_error: Option<String>,
}

impl SiteProvider {
    fn new() -> Self {
        Self {
            runtime: Runtime::new().expect("tokio runtime"),
            site_root: None,
            server: None,
            last_error: None,
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::Start { addr } => self.start(&addr),
            Request::Stop => self.stop(),
            Request::Status => self.status(),
            Request::Ping => Response::ok(serde_json::json!({ "pong": true })),
            Request::Shutdown => {
                let _ = self.stop();
                Response::ok(serde_json::json!({
                    "message": "site-provider shutting down"
                }))
            }
        }
    }

    fn init(&mut self, config: serde_json::Value) -> Response {
        let site_root = config
            .get("site_root")
            .and_then(|v| v.as_str())
            .or_else(|| {
                config
                    .get("extra")
                    .and_then(|extra| extra.get("site_root"))
                    .and_then(|v| v.as_str())
            })
            .or_else(|| config.get("base_path").and_then(|v| v.as_str()))
            .map(PathBuf::from);

        let Some(site_root) = site_root else {
            return Response::error("invalid_config", "site_root missing from init config");
        };

        self.site_root = Some(site_root.clone());
        self.last_error = None;

        Response::ok(serde_json::json!({
            "protocol_version": "1.0",
            "provider": "site",
            "site_root": site_root,
            "version": PROVIDER_VERSION,
        }))
    }

    fn start(&mut self, addr: &str) -> Response {
        self.cleanup_exited();

        let Some(site_root) = self.site_root.clone() else {
            return Response::error("not_initialized", "site-provider must be initialized first");
        };

        if let Err(err) = validate_site_root(&site_root) {
            self.last_error = Some(err.clone());
            return Response::error("invalid_site_root", err);
        }

        if let Some(server) = &self.server {
            return Response::ok(status_json(
                true,
                Some(local_url(server.bind_addr)),
                Some(server.bind_addr.to_string()),
                Some(site_root),
                self.last_error.clone(),
                Some(true),
            ));
        }

        let site_root_for_server = site_root.clone();
        let bind_result = self.runtime.block_on(async move {
            let listener = TcpListener::bind(addr).await.map_err(|e| e.to_string())?;
            let bind_addr = listener.local_addr().map_err(|e| e.to_string())?;
            let app = Router::new().fallback_service(
                ServeDir::new(&site_root_for_server).append_index_html_on_directories(true),
            );
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            let task = tokio::spawn(async move {
                let _ = axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await;
            });
            Ok::<_, String>((bind_addr, shutdown_tx, task))
        });

        match bind_result {
            Ok((bind_addr, shutdown_tx, task)) => {
                self.last_error = None;
                self.server = Some(RunningServer {
                    shutdown_tx: Some(shutdown_tx),
                    task,
                    bind_addr,
                });
                Response::ok(status_json(
                    true,
                    Some(local_url(bind_addr)),
                    Some(bind_addr.to_string()),
                    Some(site_root),
                    None,
                    Some(false),
                ))
            }
            Err(err) => {
                self.last_error = Some(err.clone());
                Response::error("bind_failed", err)
            }
        }
    }

    fn stop(&mut self) -> Response {
        if let Some(mut server) = self.server.take() {
            self.runtime.block_on(async {
                if let Some(tx) = server.shutdown_tx.take() {
                    let _ = tx.send(());
                }
                let _ = server.task.await;
            });
        }
        Response::ok_empty()
    }

    fn status(&mut self) -> Response {
        self.cleanup_exited();
        let running = self.server.is_some();
        let bind_addr = self.server.as_ref().map(|s| s.bind_addr);
        Response::ok(status_json(
            running,
            bind_addr.map(local_url),
            bind_addr.map(|addr| addr.to_string()),
            self.site_root.clone(),
            self.last_error.clone(),
            None,
        ))
    }

    fn cleanup_exited(&mut self) {
        let finished = self
            .server
            .as_ref()
            .map(|server| server.task.is_finished())
            .unwrap_or(false);
        if finished {
            if let Some(server) = self.server.take() {
                self.runtime.block_on(async {
                    let _ = server.task.await;
                });
            }
        }
    }
}

fn status_json(
    running: bool,
    local_url: Option<String>,
    bind_addr: Option<String>,
    site_root: Option<PathBuf>,
    last_error: Option<String>,
    reused: Option<bool>,
) -> serde_json::Value {
    serde_json::to_value(SiteStatus {
        running,
        local_url,
        bind_addr,
        site_root: site_root.map(|p| p.display().to_string()),
        last_error,
        reused,
    })
    .unwrap_or_else(|_| serde_json::json!({ "running": running }))
}

fn validate_site_root(site_root: &PathBuf) -> Result<(), String> {
    if !site_root.exists() {
        return Err(format!("site root does not exist: {}", site_root.display()));
    }
    if !site_root.join("index.html").exists() {
        return Err(format!(
            "site root {} is missing index.html",
            site_root.display()
        ));
    }
    Ok(())
}

fn local_url(addr: SocketAddr) -> String {
    let host = match addr.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        _ => addr.ip().to_string(),
    };
    format!("http://{}:{}/", host, addr.port())
}

fn main() {
    eprintln!("site-provider: starting v{}", PROVIDER_VERSION);
    let stdin = io::stdin();
    let stdout = io::stdout();
    let reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();
    let mut provider = SiteProvider::new();

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                eprintln!("site-provider: stdin error: {}", e);
                break;
            }
        };

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => provider.handle(req),
            Err(e) => Response::error("invalid_json", format!("Failed to parse request: {}", e)),
        };

        match serde_json::to_string(&response) {
            Ok(json) => {
                if writer.write_all(json.as_bytes()).is_err()
                    || writer.write_all(b"\n").is_err()
                    || writer.flush().is_err()
                {
                    break;
                }
            }
            Err(e) => {
                let fallback = Response::error(
                    "serialization_error",
                    format!("Failed to serialize response: {}", e),
                );
                if let Ok(json) = serde_json::to_string(&fallback) {
                    let _ = writer.write_all(json.as_bytes());
                    let _ = writer.write_all(b"\n");
                    let _ = writer.flush();
                }
            }
        }
    }

    let _ = provider.stop();
    eprintln!("site-provider: exiting");
}
