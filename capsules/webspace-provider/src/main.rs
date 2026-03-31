use std::io::{self, BufRead, Write};

use elastos_common::localhost::{parse_localhost_path, parse_localhost_uri};
use serde::{Deserialize, Serialize};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const SUPPORTED_OPS: &[&str] = &[
    "init",
    "ping",
    "shutdown",
    "resolve",
    "read",
    "list",
    "stat",
    "exists",
];

const UNSUPPORTED_OPS: &[&str] = &["write", "delete", "mkdir"];

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    Resolve {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        moniker: Option<String>,
    },
    Read {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(rename = "offset")]
        _offset: Option<u64>,
        #[serde(rename = "length")]
        _length: Option<u64>,
    },
    List {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Stat {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Exists {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Write {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(rename = "content")]
        _content: Vec<u8>,
        #[serde(rename = "append")]
        _append: bool,
    },
    Delete {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(rename = "recursive")]
        _recursive: bool,
    },
    Mkdir {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(rename = "parents")]
        _parents: bool,
    },
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

#[derive(Debug, Serialize)]
struct DirEntry {
    name: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
}

#[derive(Debug, Serialize)]
struct FileStat {
    path: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    readonly: bool,
    modified: Option<u64>,
    created: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct WebSpaceHandle {
    moniker: String,
    handle_uri: String,
    namespace_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_uri: Option<String>,
    resolver_state: String,
    kind: String,
    traversable: bool,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_step: Option<String>,
}

#[derive(Debug, Clone)]
enum ResolvedPath {
    Root,
    Handle { handle: WebSpaceHandle },
    Meta { handle: WebSpaceHandle },
}

fn meta_path(handle: &WebSpaceHandle) -> String {
    format!("{}/_meta.json", handle.handle_uri.trim_end_matches('/'))
}

fn known_mounts() -> Vec<WebSpaceHandle> {
    vec![mount_handle(
        "Elastos",
        Some("elastos://".to_string()),
        "Local interpreted handle into the broader elastos:// namespace.",
        Some(
            "List this handle to discover typed child spaces such as content, peer, did, and ai."
                .to_string(),
        ),
    )]
}

fn normalize_moniker(moniker: &str) -> String {
    moniker
        .trim()
        .trim_matches('/')
        .trim_end_matches("://")
        .to_string()
}

fn resolve_handle(moniker: &str) -> Result<WebSpaceHandle, String> {
    let normalized = normalize_moniker(moniker);
    if normalized.is_empty() {
        return Err("missing WebSpace moniker".to_string());
    }
    resolve_handle_segments(std::slice::from_ref(&normalized.as_str()))
}

fn mount_handle(
    moniker: &str,
    namespace_uri: Option<String>,
    description: &str,
    next_step: Option<String>,
) -> WebSpaceHandle {
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri: format!("localhost://WebSpaces/{}", moniker),
        namespace_uri,
        target_uri: None,
        resolver_state: "mounted".to_string(),
        kind: "dynamic-webspace".to_string(),
        traversable: true,
        description: description.to_string(),
        next_step,
    }
}

fn folder_handle(
    moniker: &str,
    handle_uri: String,
    target_uri: Option<String>,
    description: &str,
    next_step: Option<String>,
) -> WebSpaceHandle {
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri,
        namespace_uri: Some("elastos://".to_string()),
        target_uri,
        resolver_state: "resolved".to_string(),
        kind: "folder-handle".to_string(),
        traversable: true,
        description: description.to_string(),
        next_step,
    }
}

fn file_handle(
    moniker: &str,
    handle_uri: String,
    target_uri: String,
    description: &str,
) -> WebSpaceHandle {
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri,
        namespace_uri: Some("elastos://".to_string()),
        target_uri: Some(target_uri),
        resolver_state: "resolved".to_string(),
        kind: "file-endpoint".to_string(),
        traversable: false,
        description: description.to_string(),
        next_step: Some(
            "Read this handle for the current descriptor view, or inspect _meta.json for structured metadata."
                .to_string(),
        ),
    }
}

fn resolve_elastos_handle(parts: &[&str]) -> Result<WebSpaceHandle, String> {
    match parts {
        [] => Ok(mount_handle(
            "Elastos",
            Some("elastos://".to_string()),
            "Local interpreted handle into the broader elastos:// namespace.",
            Some(
                "List this handle to discover typed child spaces such as content, peer, did, and ai."
                    .to_string(),
            ),
        )),
        ["content"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/content".to_string(),
            Some("elastos://<cid>".to_string()),
            "Content-addressed objects in the Elastos WebSpace. Append a content id to resolve a file endpoint.",
            Some("Append a content id, for example localhost://WebSpaces/Elastos/content/<cid>.".to_string()),
        )),
        ["content", cid] if !cid.is_empty() => Ok(file_handle(
            "Elastos",
            format!("localhost://WebSpaces/Elastos/content/{}", cid),
            format!("elastos://{}", cid),
            "Typed file endpoint resolved from the Elastos WebSpace content-addressed namespace.",
        )),
        ["content", ..] => Err(
            "content endpoints do not support traversal beyond localhost://WebSpaces/Elastos/content/<cid>"
                .to_string(),
        ),
        ["peer"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/peer".to_string(),
            Some("elastos://peer/".to_string()),
            "Peer-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a peer identifier or ticket path segment.".to_string()),
        )),
        ["peer", peer_id] if !peer_id.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/peer/{}", peer_id),
                Some(format!("elastos://peer/{}", peer_id)),
                "Typed peer handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper peer traversal is not implemented yet.".to_string()),
            ))
        }
        ["peer", ..] => Err(
            "peer handles do not support traversal beyond localhost://WebSpaces/Elastos/peer/<peer-id> yet"
                .to_string(),
        ),
        ["did"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/did".to_string(),
            Some("elastos://did/".to_string()),
            "DID-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a DID or DID-method path segment.".to_string()),
        )),
        ["did", did] if !did.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/did/{}", did),
                Some(format!("elastos://did/{}", did)),
                "Typed DID handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper DID traversal is not implemented yet.".to_string()),
            ))
        }
        ["did", ..] => Err(
            "did handles do not support traversal beyond localhost://WebSpaces/Elastos/did/<did> yet"
                .to_string(),
        ),
        ["ai"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/ai".to_string(),
            Some("elastos://ai/".to_string()),
            "AI-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a backend or model path segment.".to_string()),
        )),
        ["ai", backend] if !backend.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/ai/{}", backend),
                Some(format!("elastos://ai/{}", backend)),
                "Typed AI handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper AI traversal is not implemented yet.".to_string()),
            ))
        }
        ["ai", ..] => Err(
            "ai handles do not support traversal beyond localhost://WebSpaces/Elastos/ai/<backend> yet"
                .to_string(),
        ),
        [child, ..] => Err(format!(
            "unknown Elastos WebSpace child: {} (known typed children: content, peer, did, ai)",
            child
        )),
    }
}

fn resolve_handle_segments(parts: &[&str]) -> Result<WebSpaceHandle, String> {
    let Some((moniker, rest)) = parts.split_first() else {
        return Err("missing WebSpace moniker".to_string());
    };
    let normalized = normalize_moniker(moniker);
    if normalized.is_empty() {
        return Err("missing WebSpace moniker".to_string());
    }
    match normalized.as_str() {
        "Elastos" => resolve_elastos_handle(rest),
        _ => Err(format!("unknown WebSpace moniker: {}", normalized)),
    }
}

fn resolve_path(path: &str) -> Result<ResolvedPath, String> {
    let trimmed = path.trim();
    let (_, rest) = parse_localhost_uri(trimmed)
        .or_else(|| parse_localhost_path(trimmed))
        .ok_or_else(|| format!("invalid rooted localhost path: {}", path))?;
    let rest = rest.trim_matches('/');
    if rest.is_empty() {
        return Ok(ResolvedPath::Root);
    }

    let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Ok(ResolvedPath::Root);
    }

    let wants_meta = parts.last().copied() == Some("_meta.json");
    if wants_meta {
        parts.pop();
    }

    let handle = resolve_handle_segments(&parts)?;
    if wants_meta {
        Ok(ResolvedPath::Meta { handle })
    } else {
        Ok(ResolvedPath::Handle { handle })
    }
}

fn handle_from_resolved_path(resolved: ResolvedPath) -> Result<WebSpaceHandle, String> {
    match resolved {
        ResolvedPath::Handle { handle } => Ok(handle),
        ResolvedPath::Meta { handle } => Err(format!(
            "resolve targets WebSpace handles, not metadata files: {}",
            meta_path(&handle)
        )),
        ResolvedPath::Root => Err("resolve requires a specific WebSpace moniker".to_string()),
    }
}

fn resolve_handle_request(path: Option<String>, moniker: Option<String>) -> Result<WebSpaceHandle, String> {
    match (path, moniker) {
        (Some(path), _) => handle_from_resolved_path(resolve_path(&path)?),
        (None, Some(moniker)) => {
            if moniker.starts_with("localhost://") || moniker.contains('/') {
                let rooted = if moniker.starts_with("localhost://") {
                    moniker
                } else {
                    format!("localhost://WebSpaces/{}", moniker)
                };
                handle_from_resolved_path(resolve_path(&rooted)?)
            } else {
                resolve_handle(&moniker)
            }
        }
        (None, None) => Err("resolve requires path or moniker".to_string()),
    }
}

fn render_meta(handle: &WebSpaceHandle) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "moniker": handle.moniker,
        "handle_uri": handle.handle_uri,
        "namespace_uri": handle.namespace_uri,
        "target_uri": handle.target_uri,
        "resolver_state": handle.resolver_state,
        "kind": handle.kind,
        "traversable": handle.traversable,
        "description": handle.description,
        "next_step": handle.next_step,
        "note": "The WebSpace daemon owns the moniker first and returns a typed handle before any further traversal.",
    }))
    .unwrap_or_else(|_| b"{}".to_vec())
}

fn render_endpoint(handle: &WebSpaceHandle) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "handle_uri": handle.handle_uri,
        "target_uri": handle.target_uri,
        "kind": handle.kind,
        "description": handle.description,
        "resolver_state": handle.resolver_state,
    }))
    .unwrap_or_else(|_| b"{}".to_vec())
}

fn stat_for(resolved: &ResolvedPath, original_path: &str) -> FileStat {
    match resolved {
        ResolvedPath::Root => FileStat {
            path: original_path.to_string(),
            is_file: false,
            is_dir: true,
            size: 0,
            readonly: true,
            modified: None,
            created: None,
        },
        ResolvedPath::Handle { handle } => FileStat {
            path: original_path.to_string(),
            is_file: !handle.traversable,
            is_dir: handle.traversable,
            size: if handle.traversable {
                0
            } else {
                render_endpoint(handle).len() as u64
            },
            readonly: true,
            modified: None,
            created: None,
        },
        ResolvedPath::Meta { handle } => FileStat {
            path: original_path.to_string(),
            is_file: true,
            is_dir: false,
            size: render_meta(handle).len() as u64,
            readonly: true,
            modified: None,
            created: None,
        },
    }
}

fn list_for(resolved: &ResolvedPath) -> Result<Vec<DirEntry>, String> {
    match resolved {
        ResolvedPath::Root => Ok(known_mounts()
            .into_iter()
            .map(|entry| DirEntry {
                name: entry.moniker,
                is_file: false,
                is_dir: true,
                size: 0,
            })
            .collect()),
        ResolvedPath::Handle { handle } if !handle.traversable => {
            Err(format!("not a directory: {}", handle.handle_uri))
        }
        ResolvedPath::Handle { handle } => {
            let mut entries = vec![DirEntry {
                name: "_meta.json".to_string(),
                is_file: true,
                is_dir: false,
                size: render_meta(handle).len() as u64,
            }];

            match handle.handle_uri.as_str() {
                "localhost://WebSpaces/Elastos" => {
                    entries.extend([
                        DirEntry {
                            name: "content".to_string(),
                            is_file: false,
                            is_dir: true,
                            size: 0,
                        },
                        DirEntry {
                            name: "peer".to_string(),
                            is_file: false,
                            is_dir: true,
                            size: 0,
                        },
                        DirEntry {
                            name: "did".to_string(),
                            is_file: false,
                            is_dir: true,
                            size: 0,
                        },
                        DirEntry {
                            name: "ai".to_string(),
                            is_file: false,
                            is_dir: true,
                            size: 0,
                        },
                    ]);
                }
                _ => {}
            }

            Ok(entries)
        }
        ResolvedPath::Meta { handle } => Err(format!("not a directory: {}", meta_path(handle))),
    }
}

fn ok(data: serde_json::Value) -> Response {
    Response::Ok { data: Some(data) }
}

fn error(code: &str, message: impl Into<String>) -> Response {
    Response::Error {
        code: code.to_string(),
        message: message.into(),
    }
}

fn init_payload(config: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": "1.0",
        "provider": "webspace",
        "kind": "dynamic-resolver",
        "config": config,
        "supported_ops": SUPPORTED_OPS,
        "unsupported_ops": UNSUPPORTED_OPS,
        "surface_note": "Read-only resolver slice: resolve, read, list, stat, and exists return mounted handles or metadata views. Mutation ops are explicit errors.",
    })
}

fn main() {
    if std::env::var("ELASTOS_DEBUG_PROVIDER_STARTUP")
        .ok()
        .as_deref()
        == Some("1")
    {
        eprintln!("webspace-provider: starting v{}", PROVIDER_VERSION);
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&error("read_stdin_failed", err.to_string()))
                        .unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"serialize_failed\",\"message\":\"failed to serialize error\"}".to_string())
                );
                let _ = stdout.flush();
                break;
            }
        };

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Init { config }) => ok(init_payload(config)),
            Ok(Request::Resolve { path, moniker }) => {
                match resolve_handle_request(path, moniker) {
                    Ok(handle) => ok(serde_json::to_value(handle).unwrap_or(serde_json::json!({}))),
                    Err(err) => error("resolve_failed", err),
                }
            }
            Ok(Request::Read { path, .. }) => match resolve_path(&path) {
                Ok(ResolvedPath::Handle { handle }) if !handle.traversable => ok(serde_json::json!({
                    "content": render_endpoint(&handle),
                    "size": render_endpoint(&handle).len(),
                })),
                Ok(ResolvedPath::Meta { handle }) => ok(serde_json::json!({
                    "content": render_meta(&handle),
                    "size": render_meta(&handle).len(),
                })),
                Ok(_) => error(
                    "read_failed",
                    "WebSpace folder handles are traversable directories. Read localhost://WebSpaces/<moniker>/_meta.json for metadata or resolve a file endpoint such as localhost://WebSpaces/Elastos/content/<cid>.",
                ),
                Err(err) => error("read_failed", err),
            },
            Ok(Request::List { path, .. }) => match resolve_path(&path) {
                Ok(resolved) => match list_for(&resolved) {
                    Ok(entries) => ok(serde_json::to_value(entries).unwrap_or(serde_json::json!([]))),
                    Err(err) => error("list_failed", err),
                },
                Err(err) => error("list_failed", err),
            },
            Ok(Request::Stat { path, .. }) => match resolve_path(&path) {
                Ok(resolved) => ok(serde_json::to_value(stat_for(&resolved, &path)).unwrap_or(serde_json::json!({}))),
                Err(err) => error("stat_failed", err),
            },
            Ok(Request::Exists { path, .. }) => ok(serde_json::json!({
                "exists": resolve_path(&path).is_ok(),
            })),
            Ok(Request::Write { path, .. }) => error(
                "write_failed",
                format!("WebSpaces are resolver-owned handles, not ordinary writable storage: {}", path),
            ),
            Ok(Request::Delete { path, .. }) => error(
                "delete_failed",
                format!("WebSpaces are resolver-owned handles, not ordinary deletable storage: {}", path),
            ),
            Ok(Request::Mkdir { path, .. }) => error(
                "mkdir_failed",
                format!("WebSpaces are resolver-owned handles, not ordinary directories: {}", path),
            ),
            Ok(Request::Ping) => ok(serde_json::json!({ "pong": true })),
            Ok(Request::Shutdown) => {
                let response = ok(serde_json::json!({
                    "message": "WebSpace provider shutting down",
                }));
                let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap_or_default());
                let _ = stdout.flush();
                break;
            }
            Err(err) => error("invalid_request", err.to_string()),
        };

        let _ = writeln!(
            stdout,
            "{}",
            serde_json::to_string(&response).unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"serialize_failed\",\"message\":\"failed to serialize response\"}".to_string())
        );
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_elastos_mount() {
        let resolved =
            resolve_path("localhost://WebSpaces/Elastos").expect("should resolve Elastos mount");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.handle_uri, "localhost://WebSpaces/Elastos");
                assert!(handle.traversable);
                assert_eq!(handle.kind, "dynamic-webspace");
            }
            _ => panic!("expected mounted handle"),
        }
    }

    #[test]
    fn resolves_content_endpoint() {
        let resolved = resolve_path("localhost://WebSpaces/Elastos/content/QmExampleCid")
            .expect("should resolve content endpoint");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "file-endpoint");
                assert!(!handle.traversable);
                assert_eq!(handle.target_uri.as_deref(), Some("elastos://QmExampleCid"));
            }
            _ => panic!("expected file endpoint"),
        }
    }

    #[test]
    fn resolves_peer_folder() {
        let resolved = resolve_path("localhost://WebSpaces/Elastos/peer/alice")
            .expect("should resolve peer folder");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "folder-handle");
                assert!(handle.traversable);
                assert_eq!(handle.target_uri.as_deref(), Some("elastos://peer/alice"));
            }
            _ => panic!("expected folder handle"),
        }
    }

    #[test]
    fn rejects_deeper_peer_traversal() {
        let err = resolve_path("localhost://WebSpaces/Elastos/peer/alice/messages")
            .expect_err("deeper peer traversal should fail");
        assert!(err.contains("peer handles do not support traversal"));
    }

    #[test]
    fn rejects_deeper_did_traversal() {
        let err = resolve_path("localhost://WebSpaces/Elastos/did/did:key:z6Mk/example")
            .expect_err("deeper did traversal should fail");
        assert!(err.contains("did handles do not support traversal"));
    }

    #[test]
    fn rejects_deeper_ai_traversal() {
        let err = resolve_path("localhost://WebSpaces/Elastos/ai/openai/gpt-5.4")
            .expect_err("deeper ai traversal should fail");
        assert!(err.contains("ai handles do not support traversal"));
    }

    #[test]
    fn init_payload_advertises_supported_and_unsupported_ops() {
        let payload = init_payload(serde_json::json!({"seeded": true}));
        let supported = payload["supported_ops"]
            .as_array()
            .expect("supported ops should be an array");
        let unsupported = payload["unsupported_ops"]
            .as_array()
            .expect("unsupported ops should be an array");

        assert!(supported.iter().any(|value| value == "resolve"));
        assert!(supported.iter().any(|value| value == "read"));
        assert!(unsupported.iter().any(|value| value == "write"));
        assert!(unsupported.iter().any(|value| value == "delete"));
        assert!(unsupported.iter().any(|value| value == "mkdir"));
    }

    #[test]
    fn lists_elastos_children() {
        let resolved =
            resolve_path("localhost://WebSpaces/Elastos").expect("should resolve Elastos mount");
        let entries = list_for(&resolved).expect("should list Elastos children");
        let names: Vec<_> = entries.into_iter().map(|entry| entry.name).collect();
        assert!(names.contains(&"_meta.json".to_string()));
        assert!(names.contains(&"content".to_string()));
        assert!(names.contains(&"peer".to_string()));
        assert!(names.contains(&"did".to_string()));
        assert!(names.contains(&"ai".to_string()));
    }

    #[test]
    fn listing_content_endpoint_fails() {
        let resolved = resolve_path("localhost://WebSpaces/Elastos/content/QmExampleCid")
            .expect("should resolve content endpoint");
        assert!(list_for(&resolved).is_err());
    }

    #[test]
    fn listing_meta_path_fails() {
        let resolved = resolve_path("localhost://WebSpaces/Elastos/_meta.json")
            .expect("should resolve metadata path");
        let err = list_for(&resolved).expect_err("metadata path should not list");
        assert!(err.contains("not a directory"));
        assert!(err.contains("_meta.json"));
    }

    #[test]
    fn resolve_request_rejects_meta_path() {
        let err = resolve_handle_request(
            Some("localhost://WebSpaces/Elastos/_meta.json".to_string()),
            None,
        )
        .expect_err("resolve should stay handle-only");
        assert!(err.contains("not metadata files"));
        assert!(err.contains("_meta.json"));
    }
}
