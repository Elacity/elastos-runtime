//! ElastOS exit-provider Capsule
//!
//! Internal contract behind `net-provider`. This first implementation is
//! deliberately fail-closed: it validates exit requests and refuses egress until
//! an operator configures a real local, Carrier-routed, privacy, paid, or
//! enterprise exit backend.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Read, Write};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const DEFAULT_REMOTE_CARRIER_EXIT_SERVICE: &str = "elastos://exit/open_stream";
const MAX_CARRIER_CONNECT_TICKET_BYTES: usize = 8192;
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status {
        #[serde(default)]
        principal_id: Option<String>,
    },
    DiscoverRemoteCarrierExits {
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        target: Option<String>,
    },
    Quote {
        target: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        remote_exit_id: Option<String>,
    },
    OpenStream {
        target: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        stream_nonce: Option<String>,
        #[serde(default)]
        remote_exit_id: Option<String>,
    },
    CloseStream {
        stream_id: String,
        #[serde(default)]
        principal_id: Option<String>,
    },
    HttpFetch {
        url: String,
        #[serde(default = "default_method")]
        method: String,
        #[serde(default)]
        principal_id: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

struct ExitProvider {
    backends: Vec<ExitBackendConfig>,
    remote_carrier_exits: Vec<RemoteCarrierExitConfig>,
    remote_active_streams: BTreeMap<String, RemoteActiveStream>,
    remote_reserved_streams: BTreeMap<String, u64>,
    public_agent: ureq::Agent,
    private_agent: ureq::Agent,
}

impl ExitProvider {
    fn new() -> Self {
        Self {
            backends: Vec::new(),
            remote_carrier_exits: Vec::new(),
            remote_active_streams: BTreeMap::new(),
            remote_reserved_streams: BTreeMap::new(),
            public_agent: http_agent(default_timeout_secs(), false),
            private_agent: http_agent(default_timeout_secs(), true),
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status { principal_id } => self.status(principal_id),
            Request::DiscoverRemoteCarrierExits {
                principal_id,
                target,
            } => self.discover_remote_carrier_exits(principal_id, target),
            Request::Quote {
                target,
                principal_id,
                reason,
                remote_exit_id,
            } => {
                if remote_exit_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    self.quote_with_selection(&target, principal_id, reason, remote_exit_id)
                } else {
                    self.quote(&target, principal_id, reason)
                }
            }
            Request::OpenStream {
                target,
                principal_id,
                reason,
                stream_nonce,
                remote_exit_id,
            } => {
                if remote_exit_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    self.open_stream_with_selection(
                        &target,
                        principal_id,
                        reason,
                        stream_nonce,
                        remote_exit_id,
                    )
                } else if stream_nonce
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    self.open_stream_with_nonce(&target, principal_id, reason, stream_nonce)
                } else {
                    self.open_stream(&target, principal_id, reason)
                }
            }
            Request::CloseStream {
                stream_id,
                principal_id,
            } => self.close_stream(&stream_id, principal_id),
            Request::HttpFetch {
                url,
                method,
                principal_id,
                reason,
            } => self.http_fetch(&url, &method, principal_id, reason),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, config: Value) -> Response {
        let config = match parse_config(config) {
            Ok(config) => config,
            Err(err) => return Response::error("invalid_config", err),
        };
        self.public_agent = http_agent(config.timeout_secs, false);
        self.private_agent = http_agent(config.timeout_secs, true);
        self.backends = config.backends;
        self.remote_carrier_exits = config.remote_carrier_exits;
        self.remote_active_streams.clear();
        self.remote_reserved_streams.clear();
        Response::ok(json!({
            "provider": "exit-provider",
            "protocol_version": "1.0",
            "backend_count": self.backends.len(),
            "remote_carrier_exit_count": self.remote_carrier_exits.len(),
            "direct_network": false,
        }))
    }

    fn status(&self, principal_id: Option<String>) -> Response {
        let principal = principal_id.as_deref();
        let now = current_unix_seconds();
        let remote_carrier_exits = self
            .remote_carrier_exits
            .iter()
            .filter(|exit| match principal {
                Some(principal) => exit.allows_principal(Some(principal)),
                None => true,
            })
            .map(|exit| {
                json!({
                    "id": exit.id,
                    "grant_id": exit.grant_id,
                    "peer_did": exit.peer_did,
                    "carrier_service": exit.carrier_service,
                    "transport": "carrier_stream",
                    "state": exit.state(now),
                    "allowed_for_principal": exit.allows_principal(principal),
                    "policy": {
                        "grant_id": exit.grant_id,
                        "allowed_hosts": exit.allowed_hosts,
                        "allowed_schemes": exit.allowed_schemes,
                        "allowed_ports": exit.allowed_ports,
                        "expires_at": exit.expires_at,
                    },
                    "accounting": self.remote_accounting(exit, principal),
                })
            })
            .collect::<Vec<_>>();
        let remote_carrier_exit_count = remote_carrier_exits.len();
        Response::ok(json!({
            "provider": "exit-provider",
            "protocol_version": "1.0",
            "status": if self.backends.is_empty() && self.remote_carrier_exits.is_empty() { "fail_closed" } else { "backend_configured" },
            "principal_id": principal_id,
            "backend_count": self.backends.len(),
            "remote_carrier_exit_count": remote_carrier_exit_count,
            "remote_carrier_exits": remote_carrier_exits,
            "direct_network": false,
            "operations": ["discover_remote_carrier_exits", "quote", "open_stream", "close_stream", "http_fetch"],
        }))
    }

    fn discover_remote_carrier_exits(
        &self,
        principal_id: Option<String>,
        target: Option<String>,
    ) -> Response {
        let Some(principal_id) = principal_id else {
            return Response::error(
                "exit_permission_denied",
                "Remote Carrier Exit discovery requires a principal_id",
            );
        };
        let parsed_target = match target.as_deref() {
            Some(target) => {
                let parsed = match validate_target(target) {
                    Ok(parsed) => parsed,
                    Err(err) => return Response::error("invalid_request", err),
                };
                if let Some(host) = parsed.host_str() {
                    if let Err(err) = validate_public_host(host) {
                        return Response::error("private_network_blocked", err);
                    }
                }
                Some(parsed)
            }
            None => None,
        };
        let now = current_unix_seconds();
        let exits = self
            .remote_carrier_exits
            .iter()
            .filter(|exit| !exit.is_expired(now))
            .filter(|exit| exit.allows_principal(Some(&principal_id)))
            .filter(|exit| {
                parsed_target
                    .as_ref()
                    .map(|target| exit.allows_target(target))
                    .unwrap_or(true)
            })
            .map(|exit| {
                json!({
                    "id": exit.id,
                    "grant_id": exit.grant_id,
                    "byte_transport": "carrier_stream",
                    "carrier": remote_carrier_public_descriptor(exit),
                    "policy": {
                        "grant_id": exit.grant_id,
                        "allowed_hosts": exit.allowed_hosts,
                        "allowed_schemes": exit.allowed_schemes,
                        "allowed_ports": exit.allowed_ports,
                        "expires_at": exit.expires_at,
                    },
                    "accounting": self.remote_accounting(exit, Some(&principal_id)),
                })
            })
            .collect::<Vec<_>>();
        Response::ok(json!({
            "schema": "elastos.exit.remote-carrier.discovery/v1",
            "principal_id": principal_id,
            "target": parsed_target.map(|target| target.to_string()),
            "remote_carrier_exits": exits,
            "direct_network": false,
        }))
    }

    fn quote(
        &self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        self.quote_with_selection(target, principal_id, reason, None)
    }

    fn quote_with_selection(
        &self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
        remote_exit_id: Option<String>,
    ) -> Response {
        let parsed = match validate_target(target) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        let remote_exit_id = match validate_remote_exit_selection(remote_exit_id.as_deref()) {
            Ok(value) => value,
            Err(err) => return Response::error("invalid_request", err),
        };
        if let Some(host) = parsed.host_str() {
            if let Err(err) = validate_public_host(host) {
                return Response::error("private_network_blocked", err);
            }
        }
        match self.remote_exit_for_target(&parsed, principal_id.as_deref(), remote_exit_id) {
            Ok(index) => {
                let exit = &self.remote_carrier_exits[index];
                return Response::ok(json!({
                    "schema": "elastos.exit.remote-carrier.quote/v1",
                    "backend": exit.id,
                    "grant_id": exit.grant_id,
                    "grant_expires_at": exit.expires_at,
                    "target": parsed.as_str(),
                    "scheme": parsed.scheme(),
                    "host": parsed.host_str(),
                    "principal_id": principal_id,
                    "reason": reason,
                    "byte_transport": "carrier_stream",
                    "carrier": remote_carrier_public_descriptor(exit),
                    "accounting": self.remote_accounting(exit, principal_id.as_deref()),
                }));
            }
            Err(RemoteExitReject::NoPolicyMatch) => {}
            Err(RemoteExitReject::PermissionDenied) => {
                return Response::error(
                    "exit_permission_denied",
                    "Remote Carrier Exit is not permitted for this principal",
                );
            }
            Err(RemoteExitReject::QuotaExceeded) => {
                return Response::error(
                    "exit_quota_exceeded",
                    "Remote Carrier Exit active stream quota is exhausted",
                );
            }
            Err(RemoteExitReject::Expired) => {
                return Response::error(
                    "exit_permission_denied",
                    "Remote Carrier Exit grant is expired",
                );
            }
        }
        if !self.backends.is_empty() || !self.remote_carrier_exits.is_empty() {
            return Response::error(
                "exit_policy_blocked",
                "No Browser Exit backend allows this target; exit-provider refuses direct host networking",
            );
        }
        self.exit_unavailable(
            "quote",
            json!({
                "target": target,
                "principal_id": principal_id,
                "reason": reason,
            }),
        )
    }

    fn open_stream(
        &mut self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        self.open_stream_with_nonce(target, principal_id, reason, None)
    }

    fn open_stream_with_nonce(
        &mut self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
        stream_nonce: Option<String>,
    ) -> Response {
        self.open_stream_with_selection(target, principal_id, reason, stream_nonce, None)
    }

    fn open_stream_with_selection(
        &mut self,
        target: &str,
        principal_id: Option<String>,
        reason: Option<String>,
        stream_nonce: Option<String>,
        remote_exit_id: Option<String>,
    ) -> Response {
        let parsed = match validate_target(target) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        let remote_exit_id = match validate_remote_exit_selection(remote_exit_id.as_deref()) {
            Ok(value) => value,
            Err(err) => return Response::error("invalid_request", err),
        };
        let Some(host) = parsed.host_str() else {
            return Response::error("invalid_request", "stream target requires a host");
        };
        if remote_exit_id.is_some() {
            if let Err(err) = validate_public_host(host) {
                return Response::error("private_network_blocked", err);
            }
            match self.remote_exit_for_target(&parsed, principal_id.as_deref(), remote_exit_id) {
                Ok(index) => {
                    let exit = self.remote_carrier_exits[index].clone();
                    return self.reserve_remote_carrier_stream(
                        &exit,
                        &parsed,
                        principal_id,
                        reason,
                        stream_nonce,
                    );
                }
                Err(RemoteExitReject::NoPolicyMatch) => {
                    return Response::error(
                        "exit_policy_blocked",
                        format!("Selected Browser Exit node does not allow host {host}; exit-provider refuses direct host networking"),
                    );
                }
                Err(RemoteExitReject::PermissionDenied) => {
                    return Response::error(
                        "exit_permission_denied",
                        "Remote Carrier Exit is not permitted for this principal",
                    );
                }
                Err(RemoteExitReject::QuotaExceeded) => {
                    return Response::error(
                        "exit_quota_exceeded",
                        "Remote Carrier Exit active stream quota is exhausted",
                    );
                }
                Err(RemoteExitReject::Expired) => {
                    return Response::error(
                        "exit_permission_denied",
                        "Remote Carrier Exit grant is expired",
                    );
                }
            }
        }
        let backend = self.backend_for_stream(&parsed);
        if let Err(err) = validate_public_host(host) {
            if !backend.is_some_and(|backend| {
                backend.allow_private_targets || backend.allows_private_target(&parsed)
            }) {
                return Response::error("private_network_blocked", err);
            }
        }
        let Some(backend) = backend else {
            match self.remote_exit_for_target(&parsed, principal_id.as_deref(), remote_exit_id) {
                Ok(index) => {
                    let exit = self.remote_carrier_exits[index].clone();
                    return self.reserve_remote_carrier_stream(
                        &exit,
                        &parsed,
                        principal_id,
                        reason,
                        stream_nonce,
                    );
                }
                Err(RemoteExitReject::NoPolicyMatch) => {}
                Err(RemoteExitReject::PermissionDenied) => {
                    return Response::error(
                        "exit_permission_denied",
                        "Remote Carrier Exit is not permitted for this principal",
                    );
                }
                Err(RemoteExitReject::QuotaExceeded) => {
                    return Response::error(
                        "exit_quota_exceeded",
                        "Remote Carrier Exit active stream quota is exhausted",
                    );
                }
                Err(RemoteExitReject::Expired) => {
                    return Response::error(
                        "exit_permission_denied",
                        "Remote Carrier Exit grant is expired",
                    );
                }
            }
            if self.backends.is_empty() && self.remote_carrier_exits.is_empty() {
                return self.exit_unavailable(
                    "open_stream",
                    json!({
                        "target": target,
                        "principal_id": principal_id,
                        "reason": reason,
                    }),
                );
            }
            return Response::error(
                "exit_policy_blocked",
                format!("No Browser Exit backend allows host {host}; exit-provider refuses direct host networking"),
            );
        };
        let stream_nonce = match validate_stream_nonce(stream_nonce.as_deref()) {
            Ok(value) => value.map(str::to_string),
            Err(err) => return Response::error("invalid_request", err),
        };
        let stream_id = format!(
            "stream:{}:{}",
            backend.id,
            stable_stream_suffix(
                parsed.as_str(),
                principal_id.as_deref(),
                stream_nonce.as_deref(),
            )
        );
        let adapter_ipc = backend.adapter_ipc.as_ref().map(|ipc| {
            json!({
                "schema": "elastos.adapter-ipc/v1",
                "kind": ipc.kind,
                "path": ipc.path,
                "stream_id": stream_id,
            })
        });
        let relay_ipc = backend.relay_ipc.as_ref().map(|ipc| {
            json!({
                "schema": "elastos.exit.relay-ipc/v1",
                "kind": ipc.kind,
                "path": ipc.path,
                "stream_id": stream_id,
            })
        });
        Response::ok(json!({
            "schema": "elastos.exit.stream-session/v1",
            "backend": backend.id,
            "stream_id": stream_id,
            "target": parsed.as_str(),
            "scheme": parsed.scheme(),
            "host": host,
            "principal_id": principal_id,
            "reason": reason,
            "stream_nonce": stream_nonce,
            "engine_owns_tls": matches!(parsed.scheme(), "tls" | "https"),
            "state": "reserved",
            "byte_transport": if adapter_ipc.is_some() { "adapter_ipc" } else { "not_attached" },
            "adapter_ipc": adapter_ipc,
            "relay_ipc": relay_ipc
        }))
    }

    fn close_stream(&mut self, stream_id: &str, principal_id: Option<String>) -> Response {
        if !is_safe_id(stream_id) {
            return Response::error("invalid_request", "close_stream requires a safe stream_id");
        }
        if let Some(active) = self.remote_active_streams.get(stream_id) {
            if principal_id.as_deref() != Some(active.principal_id.as_str()) {
                return Response::error(
                    "exit_permission_denied",
                    "close_stream principal does not own this Remote Carrier Exit stream",
                );
            }
        }
        if let Some(active) = self.remote_active_streams.remove(stream_id) {
            let backend = active.exit_id;
            let grant_id = self
                .remote_carrier_exits
                .iter()
                .find(|exit| exit.id == backend)
                .map(|exit| exit.grant_id.clone());
            return Response::ok(json!({
                "closed": true,
                "stream_id": stream_id,
                "backend": backend,
                "grant_id": grant_id,
                "principal_id": principal_id,
                "byte_transport": "carrier_stream"
            }));
        }
        Response::ok(json!({
            "closed": false,
            "stream_id": stream_id,
            "principal_id": principal_id,
            "reason": "no exit backend is configured"
        }))
    }

    fn http_fetch(
        &self,
        raw_url: &str,
        method: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        if !matches!(method, "GET" | "HEAD") {
            return Response::error("invalid_request", "http_fetch method must be GET or HEAD");
        }
        let parsed = match validate_http_fetch_url(raw_url) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_request", err),
        };
        let Some(host) = parsed.host_str() else {
            return Response::error("invalid_request", "http_fetch URL requires a host");
        };
        let backend = self.backend_for_http_fetch(&parsed);
        if let Err(err) = validate_public_host(host) {
            if !backend.is_some_and(|backend| {
                backend.allow_private_targets || backend.allows_private_target(&parsed)
            }) {
                return Response::error("private_network_blocked", err);
            }
        }
        let Some(backend) = backend else {
            return self.exit_unavailable(
                "http_fetch",
                json!({
                    "url": raw_url,
                    "method": method,
                    "principal_id": principal_id,
                    "reason": reason,
                }),
            );
        };
        self.http_fetch_with_backend(backend, parsed, method, principal_id, reason)
    }

    fn exit_unavailable(&self, operation: &str, request: Value) -> Response {
        let _ = request;
        Response::error(
            "exit_unavailable",
            format!(
                "No Browser Exit backend is configured for {operation}; exit-provider refuses direct host networking"
            ),
        )
    }

    fn backend_for_http_fetch(&self, target: &Url) -> Option<&ExitBackendConfig> {
        self.backends.iter().find(|backend| {
            backend.kind == ExitBackendKind::HttpFetch && backend.allows_target(target)
        })
    }

    fn backend_for_stream(&self, target: &Url) -> Option<&ExitBackendConfig> {
        self.backends.iter().find(|backend| {
            backend.kind == ExitBackendKind::StreamRelay && backend.allows_target(target)
        })
    }

    fn remote_exit_for_target(
        &self,
        target: &Url,
        principal_id: Option<&str>,
        remote_exit_id: Option<&str>,
    ) -> Result<usize, RemoteExitReject> {
        let mut permission_denied = false;
        let mut quota_exceeded = false;
        let mut expired = false;
        let now = current_unix_seconds();
        for (index, exit) in self.remote_carrier_exits.iter().enumerate() {
            if remote_exit_id.is_some_and(|selected| exit.id != selected) {
                continue;
            }
            if !exit.allows_target(target) {
                continue;
            }
            if !exit.allows_principal(principal_id) {
                permission_denied = true;
                continue;
            }
            if exit.is_expired(now) {
                expired = true;
                continue;
            }
            if self.remote_active_count(&exit.id) >= exit.max_active_streams {
                quota_exceeded = true;
                continue;
            }
            if let Some(principal_id) = principal_id {
                if self.remote_principal_active_count(&exit.id, principal_id)
                    >= exit.max_active_streams_per_principal()
                {
                    quota_exceeded = true;
                    continue;
                }
            }
            return Ok(index);
        }
        if permission_denied {
            Err(RemoteExitReject::PermissionDenied)
        } else if quota_exceeded {
            Err(RemoteExitReject::QuotaExceeded)
        } else if expired {
            Err(RemoteExitReject::Expired)
        } else {
            Err(RemoteExitReject::NoPolicyMatch)
        }
    }

    fn reserve_remote_carrier_stream(
        &mut self,
        exit: &RemoteCarrierExitConfig,
        target: &Url,
        principal_id: Option<String>,
        reason: Option<String>,
        stream_nonce: Option<String>,
    ) -> Response {
        let Some(host) = target.host_str() else {
            return Response::error("invalid_request", "stream target requires a host");
        };
        let Some(active_principal_id) = principal_id.clone() else {
            return Response::error(
                "exit_permission_denied",
                "Remote Carrier Exit stream reservation requires a principal_id",
            );
        };
        let stream_nonce = match validate_stream_nonce(stream_nonce.as_deref()) {
            Ok(value) => value.map(str::to_string),
            Err(err) => return Response::error("invalid_request", err),
        };
        let reserved = self
            .remote_reserved_streams
            .get(&exit.id)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.remote_reserved_streams
            .insert(exit.id.clone(), reserved);
        let stream_id = format!(
            "remote-carrier:{}:{}:{reserved}",
            exit.id,
            stable_stream_suffix(
                target.as_str(),
                principal_id.as_deref(),
                stream_nonce.as_deref(),
            )
        );
        self.remote_active_streams.insert(
            stream_id.clone(),
            RemoteActiveStream {
                exit_id: exit.id.clone(),
                principal_id: active_principal_id,
            },
        );
        let accounting = self.remote_accounting(exit, principal_id.as_deref());
        Response::ok(json!({
            "schema": "elastos.exit.remote-carrier-session/v1",
            "backend": exit.id,
            "grant_id": exit.grant_id,
            "grant_expires_at": exit.expires_at,
            "stream_id": stream_id,
            "target": target.as_str(),
            "scheme": target.scheme(),
            "host": host,
            "principal_id": principal_id,
            "reason": reason,
            "stream_nonce": stream_nonce,
            "engine_owns_tls": matches!(target.scheme(), "tls" | "https"),
            "state": "reserved",
            "byte_transport": "carrier_stream",
            "carrier": remote_carrier_private_descriptor(exit),
            "accounting": accounting,
        }))
    }

    fn remote_accounting(
        &self,
        exit: &RemoteCarrierExitConfig,
        principal_id: Option<&str>,
    ) -> Value {
        let active_streams = self.remote_active_count(&exit.id);
        let principal_active_streams = principal_id
            .map(|principal_id| self.remote_principal_active_count(&exit.id, principal_id));
        let principal_active_streams_remaining = principal_active_streams.map(|count| {
            exit.max_active_streams_per_principal()
                .saturating_sub(count)
        });
        json!({
            "grant_id": exit.grant_id,
            "grant_expires_at": exit.expires_at,
            "active_streams": active_streams,
            "reserved_streams": self.remote_reserved_streams.get(&exit.id).copied().unwrap_or(0),
            "max_active_streams": exit.max_active_streams,
            "active_streams_remaining": exit.max_active_streams.saturating_sub(active_streams),
            "max_active_streams_per_principal": exit.max_active_streams_per_principal(),
            "principal_id": principal_id,
            "principal_active_streams": principal_active_streams,
            "principal_active_streams_remaining": principal_active_streams_remaining,
        })
    }

    fn remote_active_count(&self, exit_id: &str) -> u64 {
        self.remote_active_streams
            .values()
            .filter(|active| active.exit_id == exit_id)
            .count() as u64
    }

    fn remote_principal_active_count(&self, exit_id: &str, principal_id: &str) -> u64 {
        self.remote_active_streams
            .values()
            .filter(|active| active.exit_id == exit_id && active.principal_id == principal_id)
            .count() as u64
    }

    fn http_fetch_with_backend(
        &self,
        backend: &ExitBackendConfig,
        url: Url,
        method: &str,
        principal_id: Option<String>,
        reason: Option<String>,
    ) -> Response {
        let agent = if backend.allow_private_targets {
            &self.private_agent
        } else {
            &self.public_agent
        };
        let request = match method {
            "HEAD" => agent.head(url.as_str()),
            _ => agent.get(url.as_str()),
        }
        .set("User-Agent", "ElastOS-exit-provider/0.1");

        let response = match request.call() {
            Ok(response) => response,
            Err(err) => return Response::error("backend_error", err.to_string()),
        };
        let status_code = response.status();
        let content_type = response.header("content-type").map(str::to_string);
        let mut body = Vec::new();
        let mut truncated = false;
        if method != "HEAD" {
            let limit = backend.max_body_bytes.saturating_add(1) as u64;
            if let Err(err) = response.into_reader().take(limit).read_to_end(&mut body) {
                return Response::error("backend_error", err.to_string());
            }
            if body.len() > backend.max_body_bytes {
                body.truncate(backend.max_body_bytes);
                truncated = true;
            }
        }
        Response::ok(json!({
            "schema": "elastos.exit.http-fetch.result/v1",
            "backend": backend.id,
            "url": url.as_str(),
            "method": method,
            "principal_id": principal_id,
            "reason": reason,
            "status_code": status_code,
            "content_type": content_type,
            "body_bytes": body.len(),
            "body_truncated": truncated,
            "body_text": String::from_utf8_lossy(&body),
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitConfig {
    #[serde(default)]
    backends: Vec<ExitBackendConfig>,
    #[serde(default)]
    remote_carrier_exits: Vec<RemoteCarrierExitConfig>,
    #[serde(default = "default_timeout_secs")]
    timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitBackendConfig {
    id: String,
    kind: ExitBackendKind,
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_schemes: Vec<String>,
    #[serde(default)]
    allowed_ports: Vec<u16>,
    #[serde(default)]
    allow_private_targets: bool,
    #[serde(default)]
    allowed_private_targets: Vec<PrivateTargetConfig>,
    #[serde(default = "default_max_body_bytes")]
    max_body_bytes: usize,
    #[serde(default)]
    adapter_ipc: Option<AdapterIpcConfig>,
    #[serde(default)]
    relay_ipc: Option<RelayIpcConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateTargetConfig {
    host: String,
    ports: Vec<u16>,
    #[serde(default)]
    schemes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteCarrierExitConfig {
    id: String,
    grant_id: String,
    peer_did: String,
    #[serde(default = "default_remote_carrier_service")]
    carrier_service: String,
    allowed_principals: Vec<String>,
    allowed_hosts: Vec<String>,
    #[serde(default)]
    allowed_schemes: Vec<String>,
    #[serde(default)]
    allowed_ports: Vec<u16>,
    #[serde(default = "default_max_active_remote_streams")]
    max_active_streams: u64,
    #[serde(default)]
    max_active_streams_per_principal: Option<u64>,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    connect_ticket: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteActiveStream {
    exit_id: String,
    principal_id: String,
}

impl RemoteCarrierExitConfig {
    fn allows_target(&self, target: &Url) -> bool {
        let Some(host) = target.host_str() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let host_allowed = self.allowed_hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            if allowed == "*" {
                return true;
            }
            if let Some(suffix) = allowed.strip_prefix("*.") {
                host.ends_with(&format!(".{suffix}"))
            } else {
                host == allowed
            }
        });
        host_allowed
            && self.allows_scheme(target.scheme())
            && self.allows_port(target.port_or_known_default())
    }

    fn allows_scheme(&self, scheme: &str) -> bool {
        if self.allowed_schemes.is_empty() {
            return matches!(scheme, "tcp" | "tls");
        }
        self.allowed_schemes.iter().any(|allowed| allowed == scheme)
    }

    fn allows_port(&self, port: Option<u16>) -> bool {
        let Some(port) = port else {
            return false;
        };
        self.allowed_ports.is_empty() || self.allowed_ports.contains(&port)
    }

    fn allows_principal(&self, principal_id: Option<&str>) -> bool {
        let Some(principal_id) = principal_id else {
            return false;
        };
        self.allowed_principals
            .iter()
            .any(|allowed| allowed == "*" || allowed == principal_id)
    }

    fn is_expired(&self, now: u64) -> bool {
        self.expires_at
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
    }

    fn state(&self, now: u64) -> &'static str {
        if self.is_expired(now) {
            "expired"
        } else {
            "active"
        }
    }

    fn max_active_streams_per_principal(&self) -> u64 {
        self.max_active_streams_per_principal
            .unwrap_or(self.max_active_streams)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteExitReject {
    NoPolicyMatch,
    PermissionDenied,
    QuotaExceeded,
    Expired,
}

impl ExitBackendConfig {
    fn allows_target(&self, target: &Url) -> bool {
        if self.allows_private_target(target) {
            return true;
        }
        let Some(host) = target.host_str() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        let host_allowed = self.allowed_hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            if allowed == "*" {
                return true;
            }
            if let Some(suffix) = allowed.strip_prefix("*.") {
                host.ends_with(&format!(".{suffix}"))
            } else {
                host == allowed
            }
        });
        host_allowed
            && self.allows_scheme(target.scheme())
            && self.allows_port(target.port_or_known_default())
    }

    fn allows_scheme(&self, scheme: &str) -> bool {
        if self.allowed_schemes.is_empty() {
            return match self.kind {
                ExitBackendKind::StreamRelay => matches!(scheme, "tcp" | "tls"),
                ExitBackendKind::HttpFetch => matches!(scheme, "http" | "https"),
            };
        }
        self.allowed_schemes.iter().any(|allowed| allowed == scheme)
    }

    fn allows_port(&self, port: Option<u16>) -> bool {
        let Some(port) = port else {
            return false;
        };
        self.allowed_ports.is_empty() || self.allowed_ports.contains(&port)
    }

    fn allows_private_target(&self, target: &Url) -> bool {
        let Some(host) = target.host_str() else {
            return false;
        };
        let Some(port) = target.port_or_known_default() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        self.allowed_private_targets.iter().any(|private| {
            private
                .host
                .trim_matches(['[', ']'])
                .eq_ignore_ascii_case(&host)
                && private.ports.contains(&port)
                && (private.schemes.is_empty()
                    || private
                        .schemes
                        .iter()
                        .any(|scheme| scheme == target.scheme()))
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExitBackendKind {
    HttpFetch,
    StreamRelay,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdapterIpcConfig {
    kind: AdapterIpcKind,
    path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelayIpcConfig {
    kind: RelayIpcKind,
    path: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AdapterIpcKind {
    UnixSocket,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RelayIpcKind {
    UnixSocket,
}

fn parse_config(config: Value) -> Result<ExitConfig, String> {
    let config = match config.get("extra") {
        Some(extra) if extra.is_null() => json!({}),
        Some(extra) => extra.clone(),
        None if looks_like_bridge_provider_config(&config) => json!({}),
        None => config,
    };
    let config = serde_json::from_value::<ExitConfig>(config).map_err(|err| err.to_string())?;
    if config.timeout_secs == 0 || config.timeout_secs > 60 {
        return Err("exit-provider timeout_secs must be between 1 and 60".to_string());
    }
    for backend in &config.backends {
        validate_backend(backend)?;
    }
    for exit in &config.remote_carrier_exits {
        validate_remote_carrier_exit(exit)?;
    }
    Ok(config)
}

fn looks_like_bridge_provider_config(config: &Value) -> bool {
    config.get("base_path").is_some()
        || config.get("allowed_paths").is_some()
        || config.get("read_only").is_some()
        || config.get("encryption_key").is_some()
}

fn validate_backend(backend: &ExitBackendConfig) -> Result<(), String> {
    if !is_safe_id(&backend.id) {
        return Err("exit backend id must be a safe identifier".to_string());
    }
    if backend.allowed_hosts.is_empty() {
        return Err(format!(
            "exit backend '{}' must declare at least one allowed host",
            backend.id
        ));
    }
    if backend.max_body_bytes == 0 || backend.max_body_bytes > 1024 * 1024 {
        return Err(format!(
            "exit backend '{}' max_body_bytes must be between 1 and 1048576",
            backend.id
        ));
    }
    if let Some(adapter_ipc) = &backend.adapter_ipc {
        if backend.kind != ExitBackendKind::StreamRelay {
            return Err(format!(
                "exit backend '{}' adapter_ipc is only valid for stream_relay backends",
                backend.id
            ));
        }
        validate_adapter_ipc(adapter_ipc)?;
    }
    if let Some(relay_ipc) = &backend.relay_ipc {
        if backend.kind != ExitBackendKind::StreamRelay {
            return Err(format!(
                "exit backend '{}' relay_ipc is only valid for stream_relay backends",
                backend.id
            ));
        }
        if backend.adapter_ipc.is_none() {
            return Err(format!(
                "exit backend '{}' relay_ipc requires adapter_ipc",
                backend.id
            ));
        }
        validate_relay_ipc(relay_ipc)?;
    }
    if let (Some(adapter_ipc), Some(relay_ipc)) = (&backend.adapter_ipc, &backend.relay_ipc) {
        if adapter_ipc.path == relay_ipc.path {
            return Err(format!(
                "exit backend '{}' adapter_ipc and relay_ipc paths must differ",
                backend.id
            ));
        }
    }
    for host in &backend.allowed_hosts {
        validate_allowed_host(host)?;
    }
    for scheme in &backend.allowed_schemes {
        validate_allowed_scheme(backend.kind, scheme)?;
    }
    for port in &backend.allowed_ports {
        if *port == 0 {
            return Err(format!(
                "exit backend '{}' allowed_ports must contain TCP ports between 1 and 65535",
                backend.id
            ));
        }
    }
    for target in &backend.allowed_private_targets {
        validate_private_target(&backend.id, backend.kind, target)?;
    }
    Ok(())
}

fn validate_private_target(
    backend_id: &str,
    backend_kind: ExitBackendKind,
    target: &PrivateTargetConfig,
) -> Result<(), String> {
    let host = target.host.trim();
    if host.is_empty() || host == "*" || host.starts_with("*.") {
        return Err(format!(
            "exit backend '{backend_id}' allowed_private_targets host must be an exact host"
        ));
    }
    validate_host_shape(host)?;
    if target.ports.is_empty() {
        return Err(format!(
            "exit backend '{backend_id}' allowed_private_targets ports must not be empty"
        ));
    }
    for port in &target.ports {
        if *port == 0 {
            return Err(format!(
                "exit backend '{backend_id}' allowed_private_targets ports must contain TCP ports between 1 and 65535"
            ));
        }
    }
    for scheme in &target.schemes {
        validate_allowed_scheme(backend_kind, scheme)?;
    }
    Ok(())
}

fn validate_remote_carrier_exit(exit: &RemoteCarrierExitConfig) -> Result<(), String> {
    if !is_safe_id(&exit.id) {
        return Err("remote Carrier Exit id must be a safe identifier".to_string());
    }
    if !is_safe_id(&exit.grant_id) {
        return Err("remote Carrier Exit grant_id must be a safe identifier".to_string());
    }
    validate_plain_token("remote Carrier Exit peer_did", &exit.peer_did)?;
    validate_carrier_service(&exit.carrier_service)?;
    if exit.allowed_principals.is_empty() {
        return Err(format!(
            "remote Carrier Exit '{}' must declare at least one allowed principal",
            exit.id
        ));
    }
    for principal in &exit.allowed_principals {
        if principal != "*" {
            validate_plain_token("remote Carrier Exit allowed_principals", principal)?;
        }
    }
    if exit.allowed_hosts.is_empty() {
        return Err(format!(
            "remote Carrier Exit '{}' must declare at least one allowed host",
            exit.id
        ));
    }
    for host in &exit.allowed_hosts {
        validate_allowed_host(host)?;
    }
    for scheme in &exit.allowed_schemes {
        validate_allowed_scheme(ExitBackendKind::StreamRelay, scheme)?;
    }
    for port in &exit.allowed_ports {
        if *port == 0 {
            return Err(format!(
                "remote Carrier Exit '{}' allowed_ports must contain TCP ports between 1 and 65535",
                exit.id
            ));
        }
    }
    if exit.max_active_streams == 0 || exit.max_active_streams > 1024 {
        return Err(format!(
            "remote Carrier Exit '{}' max_active_streams must be between 1 and 1024",
            exit.id
        ));
    }
    let max_active_streams_per_principal = exit.max_active_streams_per_principal();
    if max_active_streams_per_principal == 0
        || max_active_streams_per_principal > exit.max_active_streams
    {
        return Err(format!(
            "remote Carrier Exit '{}' max_active_streams_per_principal must be between 1 and max_active_streams",
            exit.id
        ));
    }
    if exit.expires_at == Some(0) {
        return Err(format!(
            "remote Carrier Exit '{}' expires_at must be a positive Unix timestamp",
            exit.id
        ));
    }
    let Some(connect_ticket) = &exit.connect_ticket else {
        return Err(format!(
            "remote Carrier Exit '{}' must declare connect_ticket for Browser Carrier stream dial",
            exit.id
        ));
    };
    validate_carrier_connect_ticket(connect_ticket)?;
    Ok(())
}

fn validate_allowed_scheme(kind: ExitBackendKind, scheme: &str) -> Result<(), String> {
    let valid = match kind {
        ExitBackendKind::StreamRelay => matches!(scheme, "tcp" | "tls"),
        ExitBackendKind::HttpFetch => matches!(scheme, "http" | "https"),
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{kind:?} backend allowed_schemes may not contain '{scheme}'"
        ))
    }
}

fn validate_plain_token(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(format!("{label} must not be empty or padded"));
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(format!("{label} must not contain whitespace or NUL"));
    }
    Ok(())
}

fn validate_carrier_service(service: &str) -> Result<(), String> {
    validate_plain_token("remote Carrier Exit carrier_service", service)?;
    if !service.starts_with("elastos://") {
        return Err("remote Carrier Exit carrier_service must use elastos://".to_string());
    }
    Ok(())
}

fn validate_carrier_connect_ticket(ticket: &str) -> Result<(), String> {
    validate_plain_token("remote Carrier Exit connect_ticket", ticket)?;
    if ticket.len() > MAX_CARRIER_CONNECT_TICKET_BYTES {
        return Err(format!(
            "remote Carrier Exit connect_ticket must be at most {MAX_CARRIER_CONNECT_TICKET_BYTES} bytes"
        ));
    }
    Ok(())
}

fn validate_remote_exit_selection(value: Option<&str>) -> Result<Option<&str>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 128 || !is_safe_id(value) {
        return Err("remote_exit_id must be a safe identifier up to 128 bytes".to_string());
    }
    Ok(Some(value))
}

fn validate_adapter_ipc(adapter_ipc: &AdapterIpcConfig) -> Result<(), String> {
    validate_ipc_path("adapter_ipc", &adapter_ipc.path)
}

fn validate_relay_ipc(relay_ipc: &RelayIpcConfig) -> Result<(), String> {
    validate_ipc_path("relay_ipc", &relay_ipc.path)
}

fn validate_ipc_path(label: &str, path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{label} path must not be empty"));
    }
    if !path.starts_with('/') {
        return Err(format!("{label} path must be absolute"));
    }
    if path
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(format!("{label} path must not contain whitespace or NUL"));
    }
    Ok(())
}

fn validate_allowed_host(host: &str) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() {
        return Err("allowed host must not be empty".to_string());
    }
    if host == "*" {
        return Ok(());
    }
    let host = host.strip_prefix("*.").unwrap_or(host);
    validate_public_host_shape(host)
}

fn validate_target(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|err| err.to_string())?;
    if !matches!(parsed.scheme(), "tcp" | "tls" | "http" | "https") {
        return Err("exit target must use tcp, tls, http, or https".to_string());
    }
    let Some(host) = parsed.host_str() else {
        return Err("exit target requires a host".to_string());
    };
    validate_host_shape(host)?;
    Ok(parsed)
}

fn validate_http_fetch_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|err| err.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("http_fetch URL must use http or https".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("http_fetch URL must not contain credentials".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("http_fetch URL requires a host".to_string());
    }
    Ok(parsed)
}

fn validate_public_host(host: &str) -> Result<(), String> {
    let host = host.trim().trim_matches(['[', ']']);
    validate_public_host_shape(host)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return validate_public_ip(ip).map_err(|_| format!("private IP blocked: {host}"));
    }
    Ok(())
}

fn validate_public_host_shape(host: &str) -> Result<(), String> {
    validate_host_shape(host)?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        return Err(format!("private host blocked: {host}"));
    }
    Ok(())
}

fn validate_host_shape(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("host must not be empty".to_string());
    }
    if host
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'\\' | b'\0'))
    {
        return Err(format!("invalid host: {host}"));
    }
    Ok(())
}

fn validate_public_ip(ip: IpAddr) -> Result<(), ()> {
    match ip {
        IpAddr::V4(ip) => {
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
            {
                Err(())
            } else {
                Ok(())
            }
        }
        IpAddr::V6(ip) => {
            // Normalize IPv4-in-IPv6 forms FIRST: a dual-stack kernel routes
            // `::ffff:a.b.c.d` (IPv4-mapped) to the bare IPv4, so the v4
            // private/loopback/link-local guards MUST apply to the embedded
            // address. Without this, `::ffff:169.254.169.254` slips past every
            // v6 predicate and reaches cloud metadata / loopback (audit T3 —
            // confirmed end-to-end SSRF bypass).
            if let Some(v4) = ip.to_ipv4_mapped() {
                return validate_public_ip(IpAddr::V4(v4));
            }
            if ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
            {
                Err(())
            } else if let Some(v4) = ip.to_ipv4() {
                // Deprecated IPv4-compatible (`::a.b.c.d`) and any other ::x form
                // that resolves to a v4 address. `::1`/`::` are already caught
                // above, so this only reaches real embedded v4 addresses — apply
                // the full v4 guard to them too, defensively.
                validate_public_ip(IpAddr::V4(v4))
            } else {
                Ok(())
            }
        }
    }
}

fn is_safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

fn validate_stream_nonce(value: Option<&str>) -> Result<Option<&str>, &'static str> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > 128 || !is_safe_id(value) {
        return Err("stream_nonce must be a safe identifier up to 128 bytes");
    }
    Ok(Some(value))
}

fn stable_stream_suffix(
    target: &str,
    principal_id: Option<&str>,
    stream_nonce: Option<&str>,
) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in target
        .bytes()
        .chain(principal_id.unwrap_or("").bytes())
        .chain(stream_nonce.unwrap_or("").bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn default_method() -> String {
    "GET".to_string()
}

fn default_timeout_secs() -> u64 {
    10
}

fn default_max_body_bytes() -> usize {
    64 * 1024
}

fn default_max_active_remote_streams() -> u64 {
    8
}

fn default_remote_carrier_service() -> String {
    DEFAULT_REMOTE_CARRIER_EXIT_SERVICE.to_string()
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn remote_carrier_public_descriptor(exit: &RemoteCarrierExitConfig) -> Value {
    json!({
        "schema": "elastos.exit.remote-carrier/v1",
        "peer_did": exit.peer_did,
        "carrier_service": exit.carrier_service,
        "grant_id": exit.grant_id,
        "transport": "carrier_stream",
    })
}

fn remote_carrier_private_descriptor(exit: &RemoteCarrierExitConfig) -> Value {
    let mut descriptor = remote_carrier_public_descriptor(exit);
    if let Some(connect_ticket) = &exit.connect_ticket {
        if let Some(object) = descriptor.as_object_mut() {
            object.insert("connect_ticket".to_string(), json!(connect_ticket));
        }
    }
    descriptor
}

fn public_dns_resolver(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let addrs = netloc
        .to_socket_addrs()
        .map(|iter| iter.collect::<Vec<_>>())?;
    for addr in &addrs {
        if validate_public_ip(addr.ip()).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("resolved private IP blocked: {}", addr.ip()),
            ));
        }
    }
    Ok(addrs)
}

fn http_agent(timeout_secs: u64, allow_private_targets: bool) -> ureq::Agent {
    // Fail-closed egress (audit T5): NEVER auto-follow redirects. ureq's default
    // (5 redirects) would let an allowlisted host `302` the fetch to any other
    // host — the private agent has no IP-validating resolver on redirect hops,
    // and the backend host allowlist is only checked against the INITIAL URL —
    // so a redirect could reach cloud metadata / a non-allowlisted host. With
    // `redirects(0)` the mediator returns the 3xx to the caller instead of
    // following; the capsule must re-issue `http_fetch` for the new URL, which
    // re-runs the full URL + host + allowlist + resolver validation per hop.
    let builder = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(timeout_secs))
        .redirects(0);
    if allow_private_targets {
        builder.build()
    } else {
        builder.resolver(public_dns_resolver).build()
    }
}

fn main() {
    eprintln!(
        "exit-provider: starting v{} (backend required)",
        PROVIDER_VERSION
    );

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut provider = ExitProvider::new();

    for line in stdin.lock().lines() {
        let response = match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => match serde_json::from_str::<Request>(&line) {
                Ok(Request::Shutdown) => {
                    let response = Response::empty_ok();
                    let _ = write_response(&mut stdout, &response);
                    break;
                }
                Ok(request) => provider.handle(request),
                Err(err) => Response::error("invalid_request", err.to_string()),
            },
            Err(err) => Response::error("stdin_error", err.to_string()),
        };

        if write_response(&mut stdout, &response).is_err() {
            break;
        }
    }

    eprintln!("exit-provider: exiting");
}

fn write_response(stdout: &mut io::Stdout, response: &Response) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// Audit T3: IPv4-mapped IPv6 literals must NOT bypass the private-network
    /// guard. A dual-stack kernel routes `::ffff:169.254.169.254` to the bare
    /// IPv4, so the mapped forms must be refused exactly like their v4 forms.
    #[test]
    fn validate_public_ip_blocks_ipv4_mapped_private_targets() {
        for mapped in [
            "::ffff:169.254.169.254", // cloud metadata (link-local)
            "::ffff:127.0.0.1",       // loopback
            "::ffff:192.168.1.1",     // RFC1918
            "::ffff:10.0.0.5",        // RFC1918
        ] {
            let ip: IpAddr = mapped.parse().expect("parse mapped v6");
            assert!(
                validate_public_ip(ip).is_err(),
                "{mapped} must be blocked as a private/loopback/link-local target"
            );
        }
        // A genuinely public v6 address still passes, and a public v4-mapped one too.
        assert!(validate_public_ip("2606:4700:4700::1111".parse().unwrap()).is_ok());
        assert!(validate_public_ip("::ffff:1.1.1.1".parse().unwrap()).is_ok());
    }

    fn error_code(response: Response) -> String {
        serde_json::to_value(response).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn status_is_fail_closed_without_backend() {
        let provider = ExitProvider::new();
        let response =
            serde_json::to_value(provider.status(Some("person:local:test".to_string()))).unwrap();
        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["status"], "fail_closed");
        assert_eq!(response["data"]["direct_network"], false);
    }

    #[test]
    fn provider_bridge_default_config_initializes_empty() {
        let mut provider = ExitProvider::new();
        let response = serde_json::to_value(provider.init(json!({
            "base_path": "",
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": ""
        })))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["backend_count"], 0);
    }

    #[test]
    fn public_stream_target_fails_closed_until_backend_exists() {
        let mut provider = ExitProvider::new();
        let response = provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        );
        assert_eq!(error_code(response), "exit_unavailable");
    }

    #[test]
    fn configured_stream_backend_returns_reserved_session_receipt() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["schema"], "elastos.exit.stream-session/v1");
        assert_eq!(response["data"]["backend"], "stream-proof");
        assert_eq!(response["data"]["target"], "tls://glidefinance.io:443");
        assert_eq!(response["data"]["engine_owns_tls"], true);
        assert_eq!(response["data"]["state"], "reserved");
        assert_eq!(response["data"]["byte_transport"], "not_attached");
        assert_eq!(response["data"]["adapter_ipc"], serde_json::Value::Null);
        assert_eq!(response["data"]["relay_ipc"], serde_json::Value::Null);
    }

    #[test]
    fn configured_stream_backend_uses_stream_nonce_for_page_scoped_sessions() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let first = serde_json::to_value(provider.open_stream_with_nonce(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("first browser window".to_string()),
            Some("open-first".to_string()),
        ))
        .unwrap();
        let second = serde_json::to_value(provider.open_stream_with_nonce(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("second browser window".to_string()),
            Some("open-second".to_string()),
        ))
        .unwrap();

        assert_eq!(first["status"], "ok");
        assert_eq!(second["status"], "ok");
        assert_eq!(first["data"]["stream_nonce"], "open-first");
        assert_eq!(second["data"]["stream_nonce"], "open-second");
        assert_ne!(first["data"]["stream_id"], second["data"]["stream_id"]);
    }

    #[test]
    fn configured_stream_backend_can_return_adapter_ipc_descriptor() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["byte_transport"], "adapter_ipc");
        assert_eq!(
            response["data"]["adapter_ipc"]["schema"],
            "elastos.adapter-ipc/v1"
        );
        assert_eq!(response["data"]["adapter_ipc"]["kind"], "unix_socket");
        assert_eq!(
            response["data"]["adapter_ipc"]["path"],
            "/tmp/elastos-browser-stream.sock"
        );
        assert_eq!(
            response["data"]["adapter_ipc"]["stream_id"],
            response["data"]["stream_id"]
        );
        assert_eq!(response["data"]["relay_ipc"], serde_json::Value::Null);
    }

    #[test]
    fn configured_stream_backend_can_return_exit_relay_ipc_descriptor() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "stream-proof",
                "kind": "stream_relay",
                "allowed_hosts": ["glidefinance.io"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                },
                "relay_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-exit-relay.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.open_stream(
            "tls://glidefinance.io:443",
            Some("person:local:test".to_string()),
            Some("open browser stream".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(
            response["data"]["relay_ipc"]["schema"],
            "elastos.exit.relay-ipc/v1"
        );
        assert_eq!(response["data"]["relay_ipc"]["kind"], "unix_socket");
        assert_eq!(
            response["data"]["relay_ipc"]["path"],
            "/tmp/elastos-exit-relay.sock"
        );
        assert_eq!(
            response["data"]["relay_ipc"]["stream_id"],
            response["data"]["stream_id"]
        );
    }

    #[test]
    fn http_fetch_blocks_private_targets() {
        let provider = ExitProvider::new();
        for url in [
            "http://localhost/",
            "http://127.0.0.1/",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://router.local/",
        ] {
            assert_eq!(
                error_code(provider.http_fetch(url, "GET", None, None)),
                "private_network_blocked"
            );
        }
    }

    #[test]
    fn public_dns_resolver_rejects_private_resolved_addresses() {
        let err = public_dns_resolver("127.0.0.1:80").expect_err("private literal must fail");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("resolved private IP blocked"));
    }

    #[test]
    fn public_dns_resolver_allows_public_resolved_addresses() {
        let addrs = public_dns_resolver("93.184.216.34:80").expect("public literal must resolve");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].ip().to_string(), "93.184.216.34");
    }

    #[test]
    fn request_decode_rejects_hidden_network_authority_fields() {
        let err = serde_json::from_value::<Request>(json!({
            "op": "open_stream",
            "target": "tls://glidefinance.io:443",
            "raw_socket": true
        }))
        .unwrap_err()
        .to_string();

        assert!(err.contains("unknown field"));
    }

    #[test]
    fn backend_config_rejects_invalid_adapter_ipc() {
        let mut provider = ExitProvider::new();
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-http-ipc",
                    "kind": "http_fetch",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-http-relay",
                    "kind": "http_fetch",
                    "allowed_hosts": ["glidefinance.io"],
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-exit-relay.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-relay-without-adapter",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-exit-relay.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-shared-ipc",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    },
                    "relay_ipc": {
                        "kind": "unix_socket",
                        "path": "/tmp/elastos-browser-stream.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "backends": [{
                    "id": "bad-relative-ipc",
                    "kind": "stream_relay",
                    "allowed_hosts": ["glidefinance.io"],
                    "adapter_ipc": {
                        "kind": "unix_socket",
                        "path": "relative.sock"
                    }
                }]
            }))),
            "invalid_config"
        );
    }

    #[test]
    fn configured_http_fetch_backend_can_fetch_allowlisted_target() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: text/plain\r\n\r\nok",
                )
                .unwrap();
        });

        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "local-test",
                "kind": "http_fetch",
                "allowed_hosts": ["127.0.0.1"],
                "allow_private_targets": true,
                "max_body_bytes": 16
            }],
            "timeout_secs": 2
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let response = serde_json::to_value(provider.http_fetch(
            &format!("http://{addr}/"),
            "GET",
            Some("person:local:test".to_string()),
            Some("test controlled exit".to_string()),
        ))
        .unwrap();

        assert_eq!(response["status"], "ok");
        assert_eq!(response["data"]["backend"], "local-test");
        assert_eq!(response["data"]["status_code"], 200);
        assert_eq!(response["data"]["body_text"], "ok");
    }

    #[test]
    fn configured_http_fetch_backend_rejects_unallowlisted_target() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "only-example",
                "kind": "http_fetch",
                "allowed_hosts": ["example.com"]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        assert_eq!(
            error_code(provider.http_fetch("https://glidefinance.io/", "GET", None, None)),
            "exit_unavailable"
        );
    }

    #[test]
    fn wildcard_stream_backend_allows_public_hosts_but_not_private_targets() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "public-web",
                "kind": "stream_relay",
                "allowed_hosts": ["*"],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let public = serde_json::to_value(provider.open_stream(
            "tls://whatismyip.com:443",
            Some("person:local:test".to_string()),
            Some("check exit IP".to_string()),
        ))
        .unwrap();
        assert_eq!(public["status"], "ok");
        assert_eq!(public["data"]["target"], "tls://whatismyip.com:443");

        assert_eq!(
            error_code(provider.open_stream("tcp://127.0.0.1:80", None, None)),
            "private_network_blocked"
        );
    }

    #[test]
    fn stream_backend_can_allow_exact_runtime_gateway_private_target_only() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "public-web-with-runtime-gateway",
                "kind": "stream_relay",
                "allowed_hosts": ["*"],
                "allowed_private_targets": [{
                    "host": "localhost",
                    "schemes": ["tcp"],
                    "ports": [61180]
                }],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-stream.sock"
                }
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let allowed = serde_json::to_value(provider.open_stream(
            "tcp://localhost:61180",
            Some("person:local:test".to_string()),
            Some("browser wallet bridge".to_string()),
        ))
        .unwrap();
        assert_eq!(allowed["status"], "ok");

        assert_eq!(
            error_code(provider.open_stream("tcp://localhost:80", None, None)),
            "private_network_blocked"
        );
    }

    #[test]
    fn remote_carrier_exit_requires_allowed_principal() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:alice",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-alice",
                "allowed_principals": ["person:local:alice"],
                "allowed_hosts": ["*"],
                "max_active_streams": 2
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        assert_eq!(
            error_code(provider.quote("tls://example.com:443", None, None)),
            "exit_permission_denied"
        );
        assert_eq!(
            error_code(provider.open_stream(
                "tls://example.com:443",
                Some("person:local:bob".to_string()),
                None,
            )),
            "exit_permission_denied"
        );
    }

    #[test]
    fn remote_carrier_exit_returns_quote_and_session_without_socket_descriptors() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:alice",
                "peer_did": "did:elastos:server",
                "carrier_service": "elastos://exit/open_stream",
                "connect_ticket": "carrier-ticket-server-exit-alice",
                "allowed_principals": ["person:local:alice"],
                "allowed_hosts": ["*.example.com"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443],
                "max_active_streams": 2
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let status =
            serde_json::to_value(provider.status(Some("person:local:alice".to_string()))).unwrap();
        assert_eq!(status["data"]["remote_carrier_exit_count"], 1);
        assert_eq!(
            status["data"]["remote_carrier_exits"][0]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(
            status["data"]["remote_carrier_exits"][0]["allowed_for_principal"],
            true
        );
        assert_eq!(
            status["data"]["remote_carrier_exits"][0]["allowed_principals"],
            serde_json::Value::Null
        );
        assert_eq!(
            status["data"]["remote_carrier_exits"][0].get("connect_ticket"),
            None
        );

        let quote = serde_json::to_value(provider.quote(
            "tls://www.example.com:443",
            Some("person:local:alice".to_string()),
            Some("browse through server exit".to_string()),
        ))
        .unwrap();
        assert_eq!(quote["status"], "ok");
        assert_eq!(
            quote["data"]["schema"],
            "elastos.exit.remote-carrier.quote/v1"
        );
        assert_eq!(
            quote["data"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(quote["data"]["byte_transport"], "carrier_stream");
        assert_eq!(
            quote["data"]["carrier"]["schema"],
            "elastos.exit.remote-carrier/v1"
        );
        assert_eq!(
            quote["data"]["carrier"].get("connect_ticket"),
            None,
            "quote is a preview surface and must not expose route secrets"
        );
        assert_eq!(
            quote["data"]["accounting"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(quote["data"]["accounting"]["active_streams"], 0);

        let session = serde_json::to_value(provider.open_stream(
            "tls://www.example.com:443",
            Some("person:local:alice".to_string()),
            Some("browse through server exit".to_string()),
        ))
        .unwrap();
        assert_eq!(session["status"], "ok");
        assert_eq!(
            session["data"]["schema"],
            "elastos.exit.remote-carrier-session/v1"
        );
        assert_eq!(
            session["data"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(session["data"]["byte_transport"], "carrier_stream");
        assert_eq!(session["data"]["adapter_ipc"], serde_json::Value::Null);
        assert_eq!(session["data"]["relay_ipc"], serde_json::Value::Null);
        assert_eq!(
            session["data"]["carrier"]["connect_ticket"],
            "carrier-ticket-server-exit-alice"
        );
        assert_eq!(
            session["data"]["accounting"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(session["data"]["accounting"]["active_streams"], 1);
        assert_eq!(session["data"]["accounting"]["reserved_streams"], 1);

        let stream_id = session["data"]["stream_id"].as_str().unwrap();
        let close = serde_json::to_value(
            provider.close_stream(stream_id, Some("person:local:alice".to_string())),
        )
        .unwrap();
        assert_eq!(close["status"], "ok");
        assert_eq!(
            close["data"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
    }

    #[test]
    fn remote_carrier_exit_selection_is_explicit_and_policy_checked() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "local-exit",
                "kind": "stream_relay",
                "allowed_hosts": ["*"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443],
                "adapter_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-local-exit-adapter.sock"
                },
                "relay_ipc": {
                    "kind": "unix_socket",
                    "path": "/tmp/elastos-browser-local-exit-relay.sock"
                }
            }],
            "remote_carrier_exits": [
                {
                    "id": "server-exit-a",
                    "grant_id": "operator-grant:server-exit:a",
                    "peer_did": "did:elastos:server-a",
                    "connect_ticket": "carrier-ticket-server-exit-a",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "allowed_schemes": ["tls"],
                    "allowed_ports": [443],
                    "max_active_streams": 2
                },
                {
                    "id": "server-exit-b",
                    "grant_id": "operator-grant:server-exit:b",
                    "peer_did": "did:elastos:server-b",
                    "connect_ticket": "carrier-ticket-server-exit-b",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "allowed_schemes": ["tls"],
                    "allowed_ports": [443],
                    "max_active_streams": 2
                }
            ]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let session = serde_json::to_value(provider.handle(Request::OpenStream {
            target: "tls://example.com:443".to_string(),
            principal_id: Some("person:local:alice".to_string()),
            reason: Some("choose server exit".to_string()),
            stream_nonce: None,
            remote_exit_id: Some("server-exit-b".to_string()),
        }))
        .unwrap();
        assert_eq!(session["status"], "ok");
        assert_eq!(session["data"]["backend"], "server-exit-b");
        assert_eq!(session["data"]["grant_id"], "operator-grant:server-exit:b");
        assert_eq!(
            session["data"]["carrier"]["connect_ticket"],
            "carrier-ticket-server-exit-b"
        );

        let blocked = provider.handle(Request::OpenStream {
            target: "tls://example.com:443".to_string(),
            principal_id: Some("person:local:alice".to_string()),
            reason: None,
            stream_nonce: None,
            remote_exit_id: Some("../server-exit-b".to_string()),
        });
        assert_eq!(error_code(blocked), "invalid_request");
    }

    #[test]
    fn remote_carrier_exit_discovery_is_principal_scoped_and_policy_filtered() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:alice",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-alice",
                "allowed_principals": ["person:local:alice"],
                "allowed_hosts": ["*.example.com"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443],
                "max_active_streams": 2
            }, {
                "id": "friend-exit",
                "grant_id": "operator-grant:friend-exit:bob",
                "peer_did": "did:elastos:friend",
                "connect_ticket": "carrier-ticket-friend-exit-bob",
                "allowed_principals": ["person:local:bob"],
                "allowed_hosts": ["example.net"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443],
                "max_active_streams": 1
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        assert_eq!(
            error_code(provider.handle(Request::DiscoverRemoteCarrierExits {
                principal_id: None,
                target: None,
            })),
            "exit_permission_denied"
        );

        let discovery =
            serde_json::to_value(provider.handle(Request::DiscoverRemoteCarrierExits {
                principal_id: Some("person:local:alice".to_string()),
                target: Some("tls://www.example.com:443".to_string()),
            }))
            .unwrap();
        assert_eq!(discovery["status"], "ok");
        assert_eq!(
            discovery["data"]["schema"],
            "elastos.exit.remote-carrier.discovery/v1"
        );
        let exits = discovery["data"]["remote_carrier_exits"]
            .as_array()
            .unwrap();
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0]["id"], "server-exit");
        assert_eq!(exits[0]["grant_id"], "operator-grant:server-exit:alice");
        assert_eq!(exits[0]["byte_transport"], "carrier_stream");
        assert_eq!(exits[0]["carrier"]["peer_did"], "did:elastos:server");
        assert_eq!(
            exits[0]["carrier"].get("connect_ticket"),
            None,
            "discovery must not expose private Carrier route tickets"
        );
        assert_eq!(
            exits[0]["accounting"]["grant_id"],
            "operator-grant:server-exit:alice"
        );
        assert_eq!(exits[0]["accounting"]["max_active_streams"], 2);
        assert_eq!(exits[0]["allowed_principals"], serde_json::Value::Null);
        assert_eq!(discovery["data"]["direct_network"], false);

        let status =
            serde_json::to_value(provider.status(Some("person:local:alice".to_string()))).unwrap();
        let status_exits = status["data"]["remote_carrier_exits"].as_array().unwrap();
        assert_eq!(status_exits.len(), 1);
        assert_eq!(status_exits[0]["id"], "server-exit");
        assert_eq!(status_exits[0].get("connect_ticket"), None);
        assert_eq!(
            status_exits[0]["allowed_principals"],
            serde_json::Value::Null
        );

        let filtered = serde_json::to_value(provider.handle(Request::DiscoverRemoteCarrierExits {
            principal_id: Some("person:local:alice".to_string()),
            target: Some("tls://example.net:443".to_string()),
        }))
        .unwrap();
        assert_eq!(
            filtered["data"]["remote_carrier_exits"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn remote_carrier_exit_expired_grant_is_diagnosable_but_not_usable() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:alice",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-alice",
                "allowed_principals": ["person:local:alice"],
                "allowed_hosts": ["example.com"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443],
                "max_active_streams": 2,
                "expires_at": 1
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let status =
            serde_json::to_value(provider.status(Some("person:local:alice".to_string()))).unwrap();
        let exit = &status["data"]["remote_carrier_exits"][0];
        assert_eq!(exit["state"], "expired");
        assert_eq!(exit["policy"]["expires_at"], 1);
        assert_eq!(exit["accounting"]["grant_expires_at"], 1);

        let discovery =
            serde_json::to_value(provider.handle(Request::DiscoverRemoteCarrierExits {
                principal_id: Some("person:local:alice".to_string()),
                target: Some("tls://example.com:443".to_string()),
            }))
            .unwrap();
        assert_eq!(discovery["status"], "ok");
        assert_eq!(
            discovery["data"]["remote_carrier_exits"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        assert_eq!(
            error_code(provider.quote(
                "tls://example.com:443",
                Some("person:local:alice".to_string()),
                None,
            )),
            "exit_permission_denied"
        );
        assert_eq!(
            error_code(provider.open_stream(
                "tls://example.com:443",
                Some("person:local:alice".to_string()),
                None,
            )),
            "exit_permission_denied"
        );
    }

    #[test]
    fn remote_carrier_exit_enforces_active_stream_quota() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:alice",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-alice",
                "allowed_principals": ["person:local:alice"],
                "allowed_hosts": ["example.com"],
                "max_active_streams": 1
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let first = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:alice".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(first["status"], "ok");
        assert_eq!(
            error_code(provider.open_stream(
                "tls://example.com:443",
                Some("person:local:alice".to_string()),
                None,
            )),
            "exit_quota_exceeded"
        );

        let stream_id = first["data"]["stream_id"].as_str().unwrap();
        let close = serde_json::to_value(
            provider.close_stream(stream_id, Some("person:local:alice".to_string())),
        )
        .unwrap();
        assert_eq!(close["status"], "ok");
        assert_eq!(close["data"]["closed"], true);
        assert_eq!(
            close["data"]["grant_id"],
            "operator-grant:server-exit:alice"
        );

        let second = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:alice".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(second["status"], "ok");
        assert_eq!(second["data"]["accounting"]["active_streams"], 1);
        assert_eq!(second["data"]["accounting"]["reserved_streams"], 2);
    }

    #[test]
    fn remote_carrier_exit_enforces_principal_stream_quota_on_shared_grant() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:shared",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-shared",
                "allowed_principals": ["person:local:alice", "person:local:bob"],
                "allowed_hosts": ["example.com"],
                "max_active_streams": 2,
                "max_active_streams_per_principal": 1
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let alice = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:alice".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(alice["status"], "ok");
        assert_eq!(
            alice["data"]["accounting"]["max_active_streams_per_principal"],
            1
        );
        assert_eq!(
            alice["data"]["accounting"]["principal_active_streams_remaining"],
            0
        );
        assert_eq!(
            error_code(provider.open_stream(
                "tls://example.com:443",
                Some("person:local:alice".to_string()),
                None,
            )),
            "exit_quota_exceeded"
        );
        assert_eq!(
            error_code(provider.quote(
                "tls://example.com:443",
                Some("person:local:alice".to_string()),
                None,
            )),
            "exit_quota_exceeded"
        );

        let bob = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:bob".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(bob["status"], "ok");
        assert_eq!(bob["data"]["accounting"]["active_streams"], 2);
        assert_eq!(bob["data"]["accounting"]["active_streams_remaining"], 0);
        assert_eq!(
            bob["data"]["accounting"]["principal_active_streams_remaining"],
            0
        );
    }

    #[test]
    fn remote_carrier_exit_close_requires_stream_owner() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:shared",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-shared",
                "allowed_principals": ["person:local:alice", "person:local:bob"],
                "allowed_hosts": ["example.com"],
                "max_active_streams": 1
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let session = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:alice".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(session["status"], "ok");

        let stream_id = session["data"]["stream_id"].as_str().unwrap();
        assert_eq!(
            error_code(provider.close_stream(stream_id, Some("person:local:bob".to_string()))),
            "exit_permission_denied"
        );
        assert_eq!(
            error_code(provider.open_stream(
                "tls://example.com:443",
                Some("person:local:bob".to_string()),
                None,
            )),
            "exit_quota_exceeded"
        );

        let close = serde_json::to_value(
            provider.close_stream(stream_id, Some("person:local:alice".to_string())),
        )
        .unwrap();
        assert_eq!(close["status"], "ok");
        assert_eq!(close["data"]["closed"], true);

        let reopened = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:bob".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(reopened["status"], "ok");
    }

    #[test]
    fn remote_carrier_exit_accounting_is_principal_scoped() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "remote_carrier_exits": [{
                "id": "server-exit",
                "grant_id": "operator-grant:server-exit:shared",
                "peer_did": "did:elastos:server",
                "connect_ticket": "carrier-ticket-server-exit-shared",
                "allowed_principals": ["person:local:alice", "person:local:bob"],
                "allowed_hosts": ["example.com"],
                "max_active_streams": 3
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let alice = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:alice".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(alice["status"], "ok");
        let bob = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:bob".to_string()),
            None,
        ))
        .unwrap();
        assert_eq!(bob["status"], "ok");

        let status =
            serde_json::to_value(provider.status(Some("person:local:alice".to_string()))).unwrap();
        let accounting = &status["data"]["remote_carrier_exits"][0]["accounting"];
        assert_eq!(accounting["active_streams"], 2);
        assert_eq!(accounting["principal_id"], "person:local:alice");
        assert_eq!(accounting["principal_active_streams"], 1);

        let discovery =
            serde_json::to_value(provider.handle(Request::DiscoverRemoteCarrierExits {
                principal_id: Some("person:local:alice".to_string()),
                target: Some("tls://example.com:443".to_string()),
            }))
            .unwrap();
        let discovery_accounting = &discovery["data"]["remote_carrier_exits"][0]["accounting"];
        assert_eq!(discovery_accounting["active_streams"], 2);
        assert_eq!(discovery_accounting["principal_id"], "person:local:alice");
        assert_eq!(discovery_accounting["principal_active_streams"], 1);

        let alice_stream_id = alice["data"]["stream_id"].as_str().unwrap();
        let close = serde_json::to_value(
            provider.close_stream(alice_stream_id, Some("person:local:alice".to_string())),
        )
        .unwrap();
        assert_eq!(close["status"], "ok");

        let status =
            serde_json::to_value(provider.status(Some("person:local:alice".to_string()))).unwrap();
        let accounting = &status["data"]["remote_carrier_exits"][0]["accounting"];
        assert_eq!(accounting["active_streams"], 1);
        assert_eq!(accounting["principal_active_streams"], 0);
    }

    #[test]
    fn remote_carrier_exit_config_rejects_missing_permission_and_hidden_fields() {
        let mut provider = ExitProvider::new();
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"]
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_hosts": ["example.com"]
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"]
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "connect_ticket": "carrier-ticket-server-exit-alice",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "raw_socket": true
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "connect_ticket": " ticket-with-padding"
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "max_active_streams": 2,
                    "max_active_streams_per_principal": 0
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "max_active_streams": 2,
                    "max_active_streams_per_principal": 3
                }]
            }))),
            "invalid_config"
        );
        assert_eq!(
            error_code(provider.init(json!({
                "remote_carrier_exits": [{
                    "id": "server-exit",
                    "grant_id": "operator-grant:server-exit:alice",
                    "peer_did": "did:elastos:server",
                    "allowed_principals": ["person:local:alice"],
                    "allowed_hosts": ["example.com"],
                    "expires_at": 0
                }]
            }))),
            "invalid_config"
        );
    }

    #[test]
    fn stream_backend_can_limit_schemes_and_ports() {
        let mut provider = ExitProvider::new();
        let init = provider.init(json!({
            "backends": [{
                "id": "https-only",
                "kind": "stream_relay",
                "allowed_hosts": ["example.com"],
                "allowed_schemes": ["tls"],
                "allowed_ports": [443]
            }]
        }));
        assert_eq!(serde_json::to_value(init).unwrap()["status"], "ok");

        let allowed = serde_json::to_value(provider.open_stream(
            "tls://example.com:443",
            Some("person:local:test".to_string()),
            Some("test constrained exit".to_string()),
        ))
        .unwrap();
        assert_eq!(allowed["status"], "ok");

        assert_eq!(
            error_code(provider.open_stream("tcp://example.com:443", None, None)),
            "exit_policy_blocked"
        );
        assert_eq!(
            error_code(provider.open_stream("tls://example.com:8443", None, None)),
            "exit_policy_blocked"
        );
    }

    #[test]
    fn backend_config_rejects_hidden_fields() {
        let mut provider = ExitProvider::new();
        let response = provider.init(json!({
            "backends": [{
                "id": "bad",
                "kind": "http_fetch",
                "allowed_hosts": ["example.com"],
                "raw_socket": true
            }]
        }));

        assert_eq!(error_code(response), "invalid_config");
    }
}
