//! ElastOS Object Provider Capsule
//!
//! Principal-root object authority. Library remains the app capsule; Runtime
//! injects the authenticated principal and forwards typed operations to this
//! provider process.

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const PROVIDER_ID: &str = "object-provider";

struct ObjectProviderProcess {
    data_dir: Option<PathBuf>,
}

impl ObjectProviderProcess {
    fn new() -> Self {
        Self { data_dir: None }
    }

    fn handle(&mut self, request: Value) -> Value {
        match request.get("op").and_then(Value::as_str) {
            Some("init") => self.init(request.get("config").cloned().unwrap_or(Value::Null)),
            Some("shutdown") => response_ok(json!({
                "provider": PROVIDER_ID,
            })),
            Some(_) => {
                let Some(data_dir) = &self.data_dir else {
                    return response_error("not_initialized", "object-provider is not initialized");
                };
                elastos_server::library::handle_object_provider_raw_request(data_dir, &request)
            }
            None => response_error("invalid_request", "provider request missing op"),
        }
    }

    fn init(&mut self, config: Value) -> Value {
        let base_path = config
            .get("base_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(base_path) = base_path else {
            return response_error(
                "invalid_config",
                "object-provider requires config.base_path",
            );
        };
        let data_dir = PathBuf::from(base_path);
        if !data_dir.is_absolute() {
            return response_error("invalid_config", "config.base_path must be absolute");
        }
        self.data_dir = Some(data_dir.clone());
        response_ok(json!({
            "provider": PROVIDER_ID,
            "protocol_version": "1.0",
            "version": PROVIDER_VERSION,
            "base_path": data_dir,
        }))
    }
}

fn response_ok(data: Value) -> Value {
    json!({
        "status": "ok",
        "data": data,
    })
}

fn response_error(code: &str, message: &str) -> Value {
    json!({
        "status": "error",
        "code": code,
        "message": message,
    })
}

fn main() {
    eprintln!("object-provider: starting v{PROVIDER_VERSION}");
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = ObjectProviderProcess::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("object-provider read error: {err}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<Value>(&line) {
            Ok(request) => request,
            Err(err) => response_error("invalid_request", &err.to_string()),
        };
        let is_shutdown = request.get("op").and_then(Value::as_str) == Some("shutdown");
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("object-provider: exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_requests_before_init() {
        let mut provider = ObjectProviderProcess::new();

        let response = provider.handle(json!({
            "op": "list",
            "principal_id": "alice",
            "uri": "localhost://Users/alice/Documents"
        }));

        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "not_initialized");
    }

    #[test]
    fn init_requires_absolute_base_path() {
        let mut provider = ObjectProviderProcess::new();

        let missing = provider.handle(json!({
            "op": "init",
            "config": {}
        }));
        assert_eq!(missing["status"], "error");
        assert_eq!(missing["code"], "invalid_config");

        let relative = provider.handle(json!({
            "op": "init",
            "config": {
                "base_path": "relative/path"
            }
        }));
        assert_eq!(relative["status"], "error");
        assert_eq!(relative["code"], "invalid_config");
    }

    #[test]
    fn initialized_raw_provider_refuses_runtime_coordinated_content_ops() {
        let dir = tempfile::tempdir().unwrap();
        let mut provider = ObjectProviderProcess::new();

        let init = provider.handle(json!({
            "op": "init",
            "config": {
                "base_path": dir.path().to_string_lossy().to_string()
            }
        }));
        assert_eq!(init["status"], "ok");

        let response = provider.handle(json!({
            "op": "publish",
            "principal_id": "alice",
            "uri": "localhost://Users/alice/Documents/file.txt"
        }));
        assert_eq!(response["status"], "error");
        assert_eq!(response["code"], "library_error");
        assert!(response["message"]
            .as_str()
            .unwrap()
            .contains("Runtime content coordinator"));
    }
}
