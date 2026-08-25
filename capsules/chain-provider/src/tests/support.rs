use super::*;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_DATA_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn spawn_rpc_server(expected_method: &'static str, result: Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request
                .windows(expected_method.len())
                .any(|window| window == expected_method.as_bytes())
            {
                break;
            }
        }
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": result,
        })
        .to_string();
        let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{addr}")
}

pub(super) fn spawn_rpc_sequence_server(responses: Vec<(&'static str, Value)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (expected_method, result) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            let body_start = loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    panic!("connection closed before headers");
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..body_start]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .expect("request must include Content-Length");
            while request.len() < body_start + content_length {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let body = &request[body_start..body_start + content_length];
            let rpc: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(rpc["method"], expected_method);
            let body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": result,
            })
            .to_string();
            let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}")
}

#[derive(Clone)]
pub(super) enum RpcReply {
    Result(Value),
    Error(Value),
}

pub(super) fn spawn_rpc_sequence_asserting_server(
    responses: Vec<(&'static str, Value, Value)>,
) -> String {
    spawn_rpc_sequence_asserting_server_with_replies(
        responses
            .into_iter()
            .map(|(method, params, result)| (method, params, RpcReply::Result(result)))
            .collect(),
    )
}

pub(super) fn spawn_rpc_sequence_asserting_server_with_replies(
    responses: Vec<(&'static str, Value, RpcReply)>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (expected_method, expected_params, reply) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            let body_start = loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    panic!("connection closed before headers");
                }
                request.extend_from_slice(&buf[..n]);
                if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..body_start]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .or_else(|| line.strip_prefix("content-length:"))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .expect("request must include Content-Length");
            while request.len() < body_start + content_length {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let body = &request[body_start..body_start + content_length];
            let rpc: Value = serde_json::from_slice(body).unwrap();
            assert_eq!(rpc["method"], expected_method);
            assert_eq!(rpc["params"], expected_params);
            let body = match reply {
                RpcReply::Result(result) => json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": result,
                }),
                RpcReply::Error(error) => json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": error,
                }),
            }
            .to_string();
            let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}")
}

pub(super) fn spawn_eth_call_server(expected_data: String, result: Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 1024];
        let body_start = loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                panic!("connection closed before headers");
            }
            request.extend_from_slice(&buf[..n]);
            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..body_start]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length:")
                    .or_else(|| line.strip_prefix("content-length:"))
            })
            .and_then(|value| value.trim().parse::<usize>().ok())
            .expect("request must include Content-Length");
        while request.len() < body_start + content_length {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
        let body = &request[body_start..body_start + content_length];
        let rpc: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(rpc["method"], "eth_call");
        assert_eq!(
            rpc["params"][0]["to"],
            "0x0000000000000000000000000000000000000001"
        );
        assert_eq!(rpc["params"][0]["data"], expected_data);
        assert_eq!(rpc["params"][1], "latest");
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": result,
        })
        .to_string();
        let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        stream.write_all(response.as_bytes()).unwrap();
    });
    format!("http://{addr}")
}

pub(super) fn spawn_http_sequence_server(
    responses: Vec<(&'static str, String, &'static str)>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for (expected_path, body, content_type) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(
                request.starts_with(&format!("GET {expected_path} ")),
                "unexpected request: {request}"
            );
            let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    format!("http://{addr}")
}

pub(super) struct TestDataDir {
    path: PathBuf,
}

impl TestDataDir {
    pub(super) fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = TEST_DATA_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "chain-provider-test-{}-{unique}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDataDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(super) fn assert_json_strings_do_not_contain(value: &Value, needle: &str) {
    match value {
        Value::String(value) => assert!(
            !value.contains(needle),
            "JSON string unexpectedly contained {needle}: {value}"
        ),
        Value::Array(values) => {
            for value in values {
                assert_json_strings_do_not_contain(value, needle);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                assert_ne!(key, "rpc_url");
                assert_json_strings_do_not_contain(value, needle);
            }
        }
        _ => {}
    }
}
