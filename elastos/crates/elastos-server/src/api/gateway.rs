//! Public ElastOS edge for site roots, publisher objects, and CID content.
//!
//! Owns the browser-facing HTTP application routes and resolves them from
//! runtime-owned state (`MyWebSite`, `ElastOS/SystemServices/Publisher`, and
//! `ElastOS/SystemServices/Edge`) plus read-only CID content.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{
    header::{AUTHORIZATION, COOKIE, SET_COOKIE},
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use base64::Engine as _;
use elastos_common::localhost::{
    edge_binding_path, edge_site_head_path, publisher_artifacts_path,
    publisher_install_script_path, publisher_release_head_path, publisher_release_manifest_path,
    rooted_localhost_fs_path, MY_WEBSITE_URI,
};
use elastos_runtime::provider::ProviderRegistry;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// Maximum size for a single file fetched through the gateway (100 MB).
const MAX_GATEWAY_FILE_SIZE: usize = 100 * 1024 * 1024;
const GATEWAY_VERSION: &str = env!("ELASTOS_VERSION");
pub(crate) const ROOM_SESSION_COOKIE: &str = "room-session";
const ROOM_SYNC_CONSUMER_ID: &str = "room-sync";

#[derive(Clone)]
pub struct GatewayState {
    pub ipfs_provider_binary: Option<PathBuf>,
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub cache_dir: PathBuf,
    /// Runtime data directory backing rooted Publisher/Edge/MyWebSite state.
    pub data_dir: PathBuf,
    /// Cached ipfs-provider bridge (lazily initialized, reused across fetches).
    pub ipfs_bridge: Arc<Mutex<Option<Arc<elastos_runtime::provider::ProviderBridge>>>>,
}

pub fn gateway_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(serve_public_root))
        .route("/healthz", get(healthz))
        .route(
            "/api/browser/session/pair",
            post(super::browser_sessions::browser_session_pair),
        )
        .route(
            "/api/browser/session/status",
            post(super::browser_sessions::browser_session_status),
        )
        .route("/release.json", get(serve_release_manifest))
        .route("/release-head.json", get(serve_release_head))
        .route("/install.sh", get(serve_install_script))
        .route(
            "/.well-known/elastos/site-head.json",
            get(serve_site_head_document),
        )
        .route("/artifacts/*path", get(serve_artifact_file))
        .route("/api/apps/room-browser/summary", get(room_service_summary))
        .route(
            "/api/apps/room-browser/session/leave",
            post(room_service_session_leave),
        )
        .route("/api/apps/room-browser/poll", post(room_service_poll))
        .route(
            "/api/apps/room-browser/objects/send",
            post(room_service_objects_send),
        )
        .route(
            "/api/apps/room-browser/upload/start",
            post(room_service_upload_start),
        )
        .route(
            "/api/apps/room-browser/upload/:upload_id/chunk",
            post(room_service_upload_chunk),
        )
        .route(
            "/api/apps/room-browser/upload/:upload_id/finish",
            post(room_service_upload_finish),
        )
        .route(
            "/api/apps/room-browser/attachments/:attachment_id",
            get(room_service_attachment_get),
        )
        .route(
            "/apps/:app",
            get(super::browser_capsules::redirect_browser_capsule_root),
        )
        .route(
            "/apps/:app/",
            get(super::browser_capsules::serve_browser_capsule_index),
        )
        .route(
            "/apps/:app/*path",
            get(super::browser_capsules::serve_browser_capsule_asset),
        )
        .route("/s/:cid", get(redirect_cid_root))
        .route("/s/:cid/", get(serve_cid_root))
        .route("/s/:cid/*path", get(serve_cid_file))
        // IPFS-compatible paths so install.sh can use this gateway like ipfs.io
        .route("/ipfs/:cid", get(serve_ipfs_cid_root))
        .route("/ipfs/:cid/", get(serve_cid_root))
        .route("/ipfs/:cid/*path", get(serve_cid_file))
        .route("/*path", get(serve_public_site_path))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn landing_page() -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>ElastOS Gateway</title>
  <style>
    :root {{
      --bg: #0c1217;
      --panel: #111a21;
      --text: #e9f0f4;
      --muted: #9bb1bf;
      --accent: #29b6f6;
      --ok: #54d66d;
      --border: #203241;
    }}
    body {{
      margin: 0;
      font-family: "Segoe UI", "SF Pro Text", system-ui, sans-serif;
      background: radial-gradient(circle at 20% 0%, #122130 0%, var(--bg) 45%);
      color: var(--text);
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 1.25rem;
    }}
    main {{
      width: min(820px, 100%);
      background: linear-gradient(180deg, #111a21 0%, #0f171f 100%);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 1.25rem;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.35);
    }}
    h1 {{
      margin: 0 0 0.25rem;
      font-size: 1.5rem;
    }}
    .muted {{
      color: var(--muted);
      margin: 0.125rem 0 0.75rem;
    }}
    .version {{
      font-size: 0.875rem;
      color: var(--ok);
      margin-bottom: 1rem;
    }}
    form {{
      display: flex;
      gap: 0.5rem;
      margin: 1rem 0 0.75rem;
      flex-wrap: wrap;
    }}
    input[type="text"] {{
      flex: 1 1 420px;
      min-width: 220px;
      background: #0d141b;
      color: var(--text);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 0.7rem 0.8rem;
      font-size: 0.95rem;
    }}
    button {{
      background: var(--accent);
      color: #05131d;
      border: 0;
      border-radius: 10px;
      padding: 0.7rem 1rem;
      font-weight: 700;
      cursor: pointer;
    }}
    code {{
      background: #0c141b;
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 0.1rem 0.35rem;
    }}
    ul {{
      margin-top: 0.5rem;
      color: var(--muted);
    }}
    a {{
      color: var(--accent);
    }}
  </style>
</head>
<body>
  <main>
    <h1>ElastOS Gateway</h1>
    <p class="muted">Public ElastOS edge for MyWebSite, publisher objects, and read-only CID content.</p>
    <p class="version">Version {}</p>

    <form id="cid-form">
      <input id="cid-input" type="text" placeholder="Paste CID, elastos://CID, or gateway URL" autocomplete="off" />
      <button type="submit">Open</button>
    </form>

    <p class="muted">URL format: <code>/s/&lt;cid&gt;/</code></p>
    <ul>
      <li>Health check: <a href="/healthz">/healthz</a></li>
      <li>Site root: <code>/</code> from <code>localhost://MyWebSite</code> or a bound Edge target.</li>
      <li>Publisher objects: <code>/release-head.json</code>, <code>/release.json</code>, <code>/install.sh</code>, <code>/artifacts/...</code></li>
      <li>Content example: <code>/s/bafy.../</code></li>
    </ul>
  </main>

  <script>
    (function () {{
      function extractCid(input) {{
        var s = (input || "").trim().replace(/\/+$/, "");
        if (!s) return "";
        if (s.startsWith("elastos://")) return s.slice("elastos://".length).split("/")[0];
        var m1 = s.match(/\/ipfs\/([^\/?#]+)/);
        if (m1 && m1[1]) return m1[1];
        var m2 = s.match(/^https?:\/\/([^./]+)\.ipfs\./i);
        if (m2 && m2[1]) return m2[1];
        return s;
      }}

      var form = document.getElementById("cid-form");
      var input = document.getElementById("cid-input");
      if (!form || !input) return;
      form.addEventListener("submit", function (e) {{
        e.preventDefault();
        var cid = extractCid(input.value);
        if (!cid) return;
        window.location.href = "/s/" + encodeURIComponent(cid) + "/";
      }});
    }})();
  </script>
</body>
</html>"#,
        GATEWAY_VERSION
    ))
}

#[derive(Debug, Deserialize)]
struct EdgeBinding {
    target: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadPayload {
    schema: String,
    target: String,
    #[serde(default)]
    bundle_cid: Option<String>,
    #[serde(default)]
    release_name: Option<String>,
    #[serde(default)]
    channel_name: Option<String>,
    content_digest: String,
    entry_count: u64,
    total_bytes: u64,
    activated_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SiteHeadEnvelope {
    payload: SiteHeadPayload,
    signature: String,
    signer_did: String,
}

struct ResolvedSiteRoot {
    target: String,
    explicit_binding: bool,
}

#[derive(Debug, Deserialize)]
struct RoomPollBody {
    #[serde(default)]
    since: u64,
}

#[derive(Debug, Deserialize)]
struct RoomSendBody {
    body: String,
}

#[derive(Debug, Deserialize)]
struct RoomUploadStartBody {
    file_name: String,
    #[serde(default)]
    mime_type: String,
    size_bytes: u64,
}

async fn serve_public_root(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, "").await {
        Ok(response) => response,
        Err(status) if resolved.explicit_binding => (status, "Not found").into_response(),
        Err(_) => landing_page().await.into_response(),
    }
}

async fn room_service_summary(State(state): State<GatewayState>) -> Response {
    let data_dir = state.data_dir.clone();
    let identity = tokio::task::spawn_blocking({
        let data_dir = data_dir.clone();
        move || room_service_runtime_identity_profile(&data_dir)
    })
    .await;
    let summary_result = tokio::task::spawn_blocking(move || {
        let mut summary = crate::room_service::load_summary(&data_dir)?;
        let hosted = crate::browser_app_hosts::load_browser_app_hosted_endpoint(
            &data_dir,
            crate::room_service::room_slug(),
        )?;
        let identity: GatewayIdentityProfile = identity.unwrap_or_default();
        summary.canonical_hosted_guest_url = hosted.canonical_url;
        summary.ephemeral_hosted_guest_url = hosted.ephemeral_url;
        let access = crate::room_service::local_runtime_access(&data_dir, identity.did.as_deref())?;
        summary.local_runtime_did = access.runtime_did;
        summary.local_runtime_role = access.member_role;
        summary.pairing_allowed = access.pairing_allowed;
        summary.pairing_block_reason = access.block_reason;
        Ok::<_, anyhow::Error>(summary)
    })
    .await;
    match summary_result {
        Ok(Ok(mut summary)) => {
            summary.transport = room_transport_view(&state, None).await;
            Json(summary).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct GatewayIdentityProfile {
    pub did: Option<String>,
}

pub(crate) fn room_service_runtime_identity_profile(
    data_dir: &std::path::Path,
) -> GatewayIdentityProfile {
    let did = load_existing_gateway_runtime_did(data_dir);
    GatewayIdentityProfile { did }
}

fn load_existing_gateway_runtime_did(data_dir: &std::path::Path) -> Option<String> {
    let device_key = data_dir.join("identity").join("device.key");
    if !device_key.exists() {
        return None;
    }
    elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_signing_key, did)| did)
        .filter(|did| !did.trim().is_empty())
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRuntimeCoords {
    api_url: String,
    attach_secret: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAttachResponse {
    token: String,
}

fn load_runtime_coords(data_dir: &std::path::Path) -> Option<GatewayRuntimeCoords> {
    let path = data_dir.join("runtime-coords.json");
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn attach_client_token_blocking(
    client: &reqwest::blocking::Client,
    coords: &GatewayRuntimeCoords,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/auth/attach", coords.api_url))
        .json(&serde_json::json!({
            "secret": coords.attach_secret,
            "scope": "client",
        }))
        .send()
        .ok()?
        .json()
        .ok()?;
    serde_json::from_value::<GatewayAttachResponse>(body)
        .ok()
        .map(|resp| resp.token)
}

fn request_attached_capability_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    resource: &str,
    action: &str,
) -> Option<String> {
    let body: serde_json::Value = client
        .post(format!("{}/api/capability/request", api))
        .header("Authorization", format!("Bearer {}", client_token))
        .json(&serde_json::json!({
            "resource": resource,
            "action": action,
        }))
        .send()
        .ok()?
        .json()
        .ok()?;

    if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
        return Some(token.to_string());
    }

    let request_id = body.get("request_id").and_then(|r| r.as_str())?;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        let status: serde_json::Value = client
            .get(format!("{}/api/capability/request/{}", api, request_id))
            .header("Authorization", format!("Bearer {}", client_token))
            .send()
            .ok()?
            .json()
            .ok()?;
        if let Some(token) = status.get("token").and_then(|t| t.as_str()) {
            return Some(token.to_string());
        }
        match status.get("status").and_then(|s| s.as_str()) {
            Some("denied") | Some("expired") => return None,
            _ => {}
        }
    }
    None
}

fn did_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    did_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let body: serde_json::Value = client
        .post(format!("{}/api/provider/did/{}", api, op))
        .header("Authorization", format!("Bearer {}", client_token))
        .header("X-Capability-Token", did_cap)
        .json(&body)
        .send()?
        .json()?;
    if body.get("status").and_then(|s| s.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown did-provider error")
        );
    }
    Ok(body)
}

#[derive(Debug, Deserialize)]
struct GatewayGossipMessage {
    sender_id: String,
    content: String,
    ts: u64,
    #[serde(default)]
    signature: Option<String>,
}

struct AttachedRoomRuntimeBlocking {
    client: reqwest::blocking::Client,
    api_url: String,
    client_token: String,
    did_cap: String,
    peer_cap: String,
    did: String,
}

async fn room_transport_view(
    state: &GatewayState,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> crate::room_service::RoomTransportView {
    let data_dir = state.data_dir.clone();
    let topic = room_transport_topic(crate::room_service::room_slug());
    match tokio::task::spawn_blocking(move || sync_room_transport_blocking(&data_dir, outbound))
        .await
    {
        Ok(Ok(view)) => view,
        Ok(Err(err)) => room_transport_error_view(&topic, &err.to_string()),
        Err(err) => {
            room_transport_error_view(&topic, &format!("room transport task failed: {err}"))
        }
    }
}

fn room_transport_topic(room_slug: &str) -> String {
    format!("__elastos_internal/room-sync-v1/{room_slug}")
}

fn room_transport_error_view(topic: &str, detail: &str) -> crate::room_service::RoomTransportView {
    crate::room_service::RoomTransportView {
        available: false,
        connected_peer_count: 0,
        topic: Some(topic.to_string()),
        status: Some(format!("Carrier room sync unavailable: {detail}")),
    }
}

fn sync_room_transport_blocking(
    data_dir: &std::path::Path,
    outbound: Option<crate::room_service::RoomObjectEnvelope>,
) -> anyhow::Result<crate::room_service::RoomTransportView> {
    let topic = room_transport_topic(crate::room_service::room_slug());
    let runtime = attach_room_runtime_blocking(data_dir)?;
    let access = crate::room_service::local_runtime_access(data_dir, Some(&runtime.did))?;
    let Some(_member_role) = access.member_role else {
        return Ok(crate::room_service::RoomTransportView {
            available: false,
            connected_peer_count: 0,
            topic: Some(topic),
            status: Some(access.block_reason.unwrap_or_else(|| {
                "Carrier room sync inactive: this runtime is not an active room member.".to_string()
            })),
        });
    };

    let outbound_envelopes =
        crate::room_service::room_transport_backlog(data_dir, &runtime.did, outbound.as_ref())?;
    join_room_transport_topic_blocking(&runtime, &topic)?;
    for envelope in &outbound_envelopes {
        send_room_object_envelope_blocking(&runtime, &topic, envelope)?;
    }

    let mut imported = 0usize;
    let mut dropped = 0usize;
    for message in recv_room_transport_messages_blocking(&runtime, &topic)? {
        match verify_and_decode_room_message_blocking(&runtime, &message) {
            Some(envelope) => {
                match crate::room_service::ingest_room_object_envelope(data_dir, &envelope) {
                    Ok(Some(_)) => imported += 1,
                    Ok(None) => {}
                    Err(_) => dropped += 1,
                }
            }
            None => dropped += 1,
        }
    }

    let connected_peer_count = list_room_transport_peers_blocking(&runtime, &topic)?.len();
    let mut status = if connected_peer_count > 0 {
        format!(
            "Carrier room sync connected to {} runtime{}.",
            connected_peer_count,
            if connected_peer_count == 1 { "" } else { "s" }
        )
    } else {
        "Carrier room sync ready; waiting for another room member runtime.".to_string()
    };
    if imported > 0 {
        status.push_str(&format!(
            " Imported {} new message{}.",
            imported,
            if imported == 1 { "" } else { "s" }
        ));
    }
    if dropped > 0 {
        status.push_str(&format!(
            " Ignored {} invalid item{}.",
            dropped,
            if dropped == 1 { "" } else { "s" }
        ));
    }

    Ok(crate::room_service::RoomTransportView {
        available: true,
        connected_peer_count,
        topic: Some(topic),
        status: Some(status),
    })
}

fn attach_room_runtime_blocking(
    data_dir: &std::path::Path,
) -> anyhow::Result<AttachedRoomRuntimeBlocking> {
    let coords = load_runtime_coords(data_dir)
        .ok_or_else(|| anyhow::anyhow!("local runtime is not running"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let client_token = attach_client_token_blocking(&client, &coords)
        .ok_or_else(|| anyhow::anyhow!("failed to attach to local runtime"))?;
    let did_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://did/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire DID capability"))?;
    let peer_cap = request_attached_capability_blocking(
        &client,
        &coords.api_url,
        &client_token,
        "elastos://peer/*",
        "execute",
    )
    .ok_or_else(|| anyhow::anyhow!("failed to acquire Carrier peer capability"))?;
    let did = did_provider_request_blocking(
        &client,
        &coords.api_url,
        &client_token,
        &did_cap,
        "get_did",
        serde_json::json!({}),
    )?
    .get("data")
    .and_then(|d| d.get("did"))
    .and_then(|value| value.as_str())
    .map(str::trim)
    .filter(|did| !did.is_empty())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("local runtime DID is unavailable"))?;
    Ok(AttachedRoomRuntimeBlocking {
        client,
        api_url: coords.api_url,
        client_token,
        did_cap,
        peer_cap,
        did,
    })
}

fn peer_provider_request_blocking(
    client: &reqwest::blocking::Client,
    api: &str,
    client_token: &str,
    peer_cap: &str,
    op: &str,
    body: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let response = client
        .post(format!("{api}/api/provider/peer/{op}"))
        .header(AUTHORIZATION, format!("Bearer {client_token}"))
        .header("X-Capability-Token", peer_cap)
        .json(&body)
        .send()?;
    let body: serde_json::Value = response.json()?;
    if body.get("status").and_then(|status| status.as_str()) == Some("error") {
        anyhow::bail!(
            "{}",
            body.get("message")
                .and_then(|message| message.as_str())
                .unwrap_or("unknown peer-provider error")
        );
    }
    Ok(body)
}

fn join_room_transport_topic_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<()> {
    match peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_join",
        serde_json::json!({ "topic": topic }),
    ) {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("already joined") => Ok(()),
        Err(err) => Err(err),
    }
}

fn list_room_transport_peers_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<Vec<String>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "list_topic_peers",
        serde_json::json!({ "topic": topic }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("peers"))
        .and_then(|value| value.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn recv_room_transport_messages_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
) -> anyhow::Result<Vec<GatewayGossipMessage>> {
    let body = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_recv",
        serde_json::json!({
            "topic": topic,
            "limit": 64,
            "consumer_id": ROOM_SYNC_CONSUMER_ID,
            "skip_sender_id": runtime.did,
        }),
    )?;
    Ok(body
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(|value| value.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| {
                    serde_json::from_value::<GatewayGossipMessage>(message.clone()).ok()
                })
                .collect()
        })
        .unwrap_or_default())
}

fn send_room_object_envelope_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    topic: &str,
    envelope: &crate::room_service::RoomObjectEnvelope,
) -> anyhow::Result<()> {
    if envelope.sender_member_did != runtime.did {
        anyhow::bail!(
            "room object signer {} does not match local runtime DID {}",
            envelope.sender_member_did,
            runtime.did
        );
    }
    let message = serde_json::to_string(envelope)?;
    let signature = sign_room_message_blocking(
        runtime,
        &envelope.sender_member_did,
        envelope.created_at,
        &message,
    )?;
    let _ = peer_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.peer_cap,
        "gossip_send",
        serde_json::json!({
            "topic": topic,
            "message": message,
            "sender": envelope.sender,
            "sender_id": envelope.sender_member_did,
            "ts": envelope.created_at,
            "signature": signature,
        }),
    )?;
    Ok(())
}

fn verify_and_decode_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    message: &GatewayGossipMessage,
) -> Option<crate::room_service::RoomObjectEnvelope> {
    if message.sender_id.trim().is_empty() || message.ts == 0 {
        return None;
    }
    let signature = message.signature.as_deref().unwrap_or_default();
    if !verify_room_message_blocking(
        runtime,
        &message.sender_id,
        message.ts,
        &message.content,
        signature,
    ) {
        return None;
    }
    let envelope: crate::room_service::RoomObjectEnvelope =
        serde_json::from_str(&message.content).ok()?;
    if envelope.sender_member_did != message.sender_id || envelope.created_at != message.ts {
        return None;
    }
    Some(envelope)
}

fn sign_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> anyhow::Result<String> {
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "sign",
        serde_json::json!({ "data": payload_hex }),
    )?
    .get("data")
    .and_then(|data| data.get("signature"))
    .and_then(|value| value.as_str())
    .map(ToOwned::to_owned)
    .ok_or_else(|| anyhow::anyhow!("did-provider sign response missing signature"))
}

fn verify_room_message_blocking(
    runtime: &AttachedRoomRuntimeBlocking,
    sender_id: &str,
    ts: u64,
    content: &str,
    signature: &str,
) -> bool {
    if sender_id.trim().is_empty() || signature.trim().is_empty() || ts == 0 {
        return false;
    }
    let payload_hex = elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
    did_provider_request_blocking(
        &runtime.client,
        &runtime.api_url,
        &runtime.client_token,
        &runtime.did_cap,
        "verify",
        serde_json::json!({
            "did": sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    )
    .ok()
    .and_then(|body| {
        body.get("data")
            .and_then(|data| data.get("valid"))
            .and_then(|value| value.as_bool())
    })
    .unwrap_or(false)
}

async fn room_service_session_leave(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let secure = request_uses_tls(&headers);
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || crate::room_service::leave_session(&data_dir, &token))
        .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, None).await;
            let mut response = Json(output).into_response();
            match clear_room_session_cookie_header(secure) {
                Ok(value) => {
                    response.headers_mut().append(SET_COOKIE, value);
                    response
                }
                Err(err) => room_service_error_response(err),
            }
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_poll(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomPollBody>,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    let transport = room_transport_view(&state, None).await;
    match tokio::task::spawn_blocking(move || {
        crate::room_service::room_poll(&data_dir, &token, body.since)
    })
    .await
    {
        Ok(Ok(mut output)) => {
            output.transport = transport;
            Json(output).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_objects_send(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomSendBody>,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_object_with_transport(&data_dir, &token, &body.body)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_start(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<RoomUploadStartBody>,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::start_attachment_upload(
            &data_dir,
            &token,
            &body.file_name,
            &body.mime_type,
            body.size_bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_chunk(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let offset = match upload_offset_from_headers(&headers) {
        Ok(offset) => offset,
        Err(err) => return room_service_error_response(err),
    };
    let bytes = body.to_vec();
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::append_attachment_upload_chunk(
            &data_dir, &token, &upload_id, offset, &bytes,
        )
    })
    .await
    {
        Ok(Ok(output)) => Json(output).into_response(),
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_upload_finish(
    State(state): State<GatewayState>,
    Path(upload_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::finish_attachment_upload(&data_dir, &token, &upload_id)
    })
    .await
    {
        Ok(Ok(output)) => {
            let _ = room_transport_view(&state, output.transport_envelope).await;
            Json(output.object).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

async fn room_service_attachment_get(
    State(state): State<GatewayState>,
    Path(attachment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let token = match room_session_token_from_headers(&headers) {
        Ok(token) => token,
        Err(err) => return room_service_error_response(err),
    };
    let data_dir = state.data_dir.clone();
    match tokio::task::spawn_blocking(move || {
        crate::room_service::read_attachment(&data_dir, &token, &attachment_id)
    })
    .await
    {
        Ok(Ok((attachment, bytes))) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_str(&attachment.mime_type)
                    .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
            );
            let disposition = if attachment.is_image || attachment.is_audio || attachment.is_video {
                "inline"
            } else {
                "attachment"
            };
            let content_disposition = format!(
                "{}; filename=\"{}\"",
                disposition,
                attachment.file_name.replace('"', "")
            );
            headers.insert(
                "content-disposition",
                HeaderValue::from_str(&content_disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            headers.insert("cache-control", HeaderValue::from_static("no-store"));
            (StatusCode::OK, headers, bytes).into_response()
        }
        Ok(Err(err)) => room_service_error_response(err),
        Err(err) => room_service_join_error_response(err),
    }
}

pub(crate) fn request_uses_tls(headers: &HeaderMap) -> bool {
    if headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("https"))
    {
        return true;
    }

    headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("proto=https"))
}

pub(crate) fn set_room_session_cookie_header(
    token: &str,
    max_age_secs: u64,
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value = format!(
        "{ROOM_SESSION_COOKIE}={token}; Max-Age={max_age_secs}; Path=/; HttpOnly; SameSite=Lax"
    );
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

pub(crate) fn clear_room_session_cookie_header(
    secure: bool,
) -> anyhow::Result<HeaderValue> {
    let mut value =
        format!("{ROOM_SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax");
    if secure {
        value.push_str("; Secure");
    }
    HeaderValue::from_str(&value).map_err(|err| anyhow::anyhow!("invalid Set-Cookie header: {err}"))
}

fn room_session_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    if let Ok(token) = bearer_token_from_headers(headers) {
        return Ok(token);
    }
    if let Some(token) = cookie_value_from_headers(headers, ROOM_SESSION_COOKIE) {
        return Ok(token);
    }
    anyhow::bail!(
        "missing room session. Expected Authorization: Bearer <token> or room session cookie"
    )
}

fn cookie_value_from_headers(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie_header| {
            cookie_header.split(';').map(str::trim).find_map(|entry| {
                let (key, value) = entry.split_once('=')?;
                if key.trim() == name {
                    Some(value.trim().to_string())
                } else {
                    None
                }
            })
        })
        .filter(|value| !value.is_empty())
}

fn bearer_token_from_headers(headers: &HeaderMap) -> anyhow::Result<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing Authorization header. Expected: Bearer <token>"))
}

fn upload_offset_from_headers(headers: &HeaderMap) -> anyhow::Result<u64> {
    headers
        .get("x-elastos-upload-offset")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("missing x-elastos-upload-offset header"))?
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid x-elastos-upload-offset header"))
}

fn room_service_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid or expired session") || text.contains("missing room session") {
        StatusCode::UNAUTHORIZED
    } else if text.contains("must not be empty")
        || text.contains("characters or fewer")
        || text.contains("exceeds")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn room_service_join_error_response(err: tokio::task::JoinError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("group chat task failed: {}", err),
    )
        .into_response()
}

async fn resolve_bound_site_root(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<ResolvedSiteRoot, StatusCode> {
    let Some(host) = request_host(headers) else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding_path = edge_binding_path(&state.data_dir, &host);
    let Ok(bytes) = tokio::fs::read(&binding_path).await else {
        return Ok(ResolvedSiteRoot {
            target: MY_WEBSITE_URI.to_string(),
            explicit_binding: false,
        });
    };
    let binding: EdgeBinding =
        serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_GATEWAY)?;
    if rooted_localhost_fs_path(&state.data_dir, &binding.target).is_none() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(ResolvedSiteRoot {
        target: binding.target,
        explicit_binding: true,
    })
}

fn request_host(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))?
        .to_str()
        .ok()?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    if let Some(stripped) = raw.strip_prefix('[') {
        let end = stripped.find(']')?;
        return Some(stripped[..end].to_string());
    }
    Some(raw.split(':').next().unwrap_or("").to_string())
}

async fn healthz() -> &'static str {
    "OK"
}

async fn serve_release_manifest(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_manifest_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release.json not found").into_response(),
    }
}

async fn serve_release_head(State(state): State<GatewayState>) -> Response {
    let path = publisher_release_head_path(&state.data_dir);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "release-head.json not found").into_response(),
    }
}

async fn serve_artifact_file(
    State(state): State<GatewayState>,
    Path(path): Path<String>,
) -> Response {
    if let Err(msg) = validate_file_path(&path) {
        return (StatusCode::BAD_REQUEST, msg).into_response();
    }

    let artifacts_root = publisher_artifacts_path(&state.data_dir);
    let requested = artifacts_root.join(&path);
    let Ok(root_canonical) = tokio::fs::canonicalize(&artifacts_root).await else {
        return (StatusCode::NOT_FOUND, "artifacts not found").into_response();
    };
    let Ok(requested_canonical) = tokio::fs::canonicalize(&requested).await else {
        return (StatusCode::NOT_FOUND, "artifact not found").into_response();
    };
    if !requested_canonical.starts_with(&root_canonical) {
        return (StatusCode::BAD_REQUEST, "Path traversal not allowed").into_response();
    }

    match tokio::fs::read(&requested_canonical).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", content_type(&path))],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "artifact not found").into_response(),
    }
}

async fn serve_install_script(
    State(state): State<GatewayState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let path = publisher_install_script_path(&state.data_dir);
    if let Ok(bytes) = tokio::fs::read(&path).await {
        // Dynamically stamp the publisher gateway URL so `curl <gw>/install.sh | bash`
        // automatically embeds this gateway for future `elastos update`.
        let script = String::from_utf8_lossy(&bytes);
        let stamped = if script.contains("__PUBLISHER_GATEWAY__") {
            if let Some(host) = headers
                .get("x-forwarded-host")
                .or_else(|| headers.get("host"))
                .and_then(|v| v.to_str().ok())
            {
                let scheme = headers
                    .get("x-forwarded-proto")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("https");
                let gateway_url = format!("{}://{}", scheme, host.trim_end_matches('/'));
                script
                    .replace("__PUBLISHER_GATEWAY__", &gateway_url)
                    .into_bytes()
            } else {
                bytes
            }
        } else {
            bytes
        };
        return (
            StatusCode::OK,
            [("content-type", "text/x-shellscript")],
            stamped,
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "install.sh not found").into_response()
}

async fn serve_site_head_document(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    let Some(site_head) = load_site_head(&state, &resolved.target).await else {
        return (StatusCode::NOT_FOUND, "site head not found").into_response();
    };
    match serde_json::to_vec(&site_head) {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "application/json")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "site head encode failed").into_response(),
    }
}

async fn serve_public_site_path(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(path): Path<String>,
) -> Response {
    let resolved = match resolve_bound_site_root(&state, &headers).await {
        Ok(resolved) => resolved,
        Err(status) => return (status, "Bad gateway binding").into_response(),
    };
    match serve_site_file(&state, &resolved.target, &path).await {
        Ok(response) => response,
        Err(status) => (status, "Not found").into_response(),
    }
}

async fn load_site_head(state: &GatewayState, site_root_uri: &str) -> Option<SiteHeadEnvelope> {
    let path = edge_site_head_path(&state.data_dir, site_root_uri);
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn redirect_cid_root(Path(cid): Path<String>) -> Redirect {
    Redirect::permanent(&format!("/s/{}/", cid))
}

async fn serve_cid_root(State(state): State<GatewayState>, Path(cid): Path<String>) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    serve_directory_root(&state, &cid).await
}

async fn serve_ipfs_cid_root(
    State(state): State<GatewayState>,
    Path(cid): Path<String>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    let raw_cache = state.cache_dir.join(format!("{}.raw", cid));
    if let Ok(bytes) = tokio::fs::read(&raw_cache).await {
        return (
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response();
    }

    match fetch_file_inline(&state, &cid, "").await {
        Ok(bytes) => {
            let _ = tokio::fs::create_dir_all(&state.cache_dir).await;
            let _ = tokio::fs::write(&raw_cache, &bytes).await;
            (
                StatusCode::OK,
                [("content-type", "application/octet-stream")],
                bytes,
            )
                .into_response()
        }
        Err(_) => serve_directory_root(&state, &cid).await,
    }
}

async fn serve_directory_root(state: &GatewayState, cid: &str) -> Response {
    match serve_cid_path_result(state, cid, "index.html").await {
        Ok(response) => response,
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found in CID bundle").into_response(),
    }
}

async fn serve_cid_path_result(
    state: &GatewayState,
    cid: &str,
    file_path: &str,
) -> Result<Response, StatusCode> {
    validate_file_path(file_path).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Fast path: check local cache.
    let cid_dir = state.cache_dir.join(cid);
    let requested = cid_dir.join(file_path);
    if cid_dir.is_dir() {
        let canonical_cid_dir = cid_dir.canonicalize().unwrap_or_else(|_| cid_dir.clone());
        let canonical_requested = requested
            .canonicalize()
            .unwrap_or_else(|_| cid_dir.join(file_path));
        if canonical_requested.starts_with(&canonical_cid_dir) {
            if let Ok(bytes) = tokio::fs::read(&requested).await {
                let ct = content_type(file_path);
                return Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response());
            }
        }
    }

    // Cache miss — fetch the individual file inline via cat.
    match fetch_file_inline(state, cid, file_path).await {
        Ok(bytes) => {
            let cache_path = state.cache_dir.join(cid).join(file_path);
            if let Some(parent) = cache_path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&cache_path, &bytes).await;
            let ct = content_type(file_path);
            Ok((StatusCode::OK, [("content-type", ct)], bytes).into_response())
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

fn stamp_site_headers(
    response: &mut Response,
    site_root_uri: &str,
    site_head: Option<&SiteHeadEnvelope>,
) -> Result<(), StatusCode> {
    let site_origin = HeaderValue::from_str(site_root_uri).map_err(|_| StatusCode::BAD_GATEWAY)?;
    response
        .headers_mut()
        .insert("X-Elastos-Site-Origin", site_origin);
    if let Some(site_head) = site_head {
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Schema",
            HeaderValue::from_str(&site_head.payload.schema)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Digest",
            HeaderValue::from_str(&site_head.payload.content_digest)
                .map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        response.headers_mut().insert(
            "X-Elastos-Site-Head-Signer",
            HeaderValue::from_str(&site_head.signer_did).map_err(|_| StatusCode::BAD_GATEWAY)?,
        );
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Cid",
                HeaderValue::from_str(bundle_cid).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(release_name) = site_head.payload.release_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Release",
                HeaderValue::from_str(release_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
        if let Some(channel_name) = site_head.payload.channel_name.as_deref() {
            response.headers_mut().insert(
                "X-Elastos-Site-Head-Channel",
                HeaderValue::from_str(channel_name).map_err(|_| StatusCode::BAD_GATEWAY)?,
            );
        }
    }
    Ok(())
}

async fn serve_site_file(
    state: &GatewayState,
    site_root_uri: &str,
    request_path: &str,
) -> Result<Response, StatusCode> {
    let requested = request_path.trim_start_matches('/');
    if !requested.is_empty() {
        validate_file_path(requested).map_err(|_| StatusCode::BAD_REQUEST)?;
    }

    let site_head = load_site_head(state, site_root_uri).await;
    if let Some(site_head) = site_head.as_ref() {
        if let Some(bundle_cid) = site_head.payload.bundle_cid.as_deref() {
            let bundle_candidates: Vec<String> = if requested.is_empty() {
                vec!["index.html".to_string()]
            } else {
                vec![requested.to_string(), format!("{}/index.html", requested)]
            };
            for bundle_path in bundle_candidates {
                if let Ok(mut response) =
                    serve_cid_path_result(state, bundle_cid, &bundle_path).await
                {
                    stamp_site_headers(&mut response, site_root_uri, Some(site_head))?;
                    return Ok(response);
                }
            }
            return Err(StatusCode::NOT_FOUND);
        }
    }

    let site_root =
        rooted_localhost_fs_path(&state.data_dir, site_root_uri).ok_or(StatusCode::BAD_GATEWAY)?;
    let mut candidates = Vec::new();
    if requested.is_empty() {
        candidates.push(site_root.join("index.html"));
    } else {
        candidates.push(site_root.join(requested));
        candidates.push(site_root.join(requested).join("index.html"));
    }

    let root_canonical = tokio::fs::canonicalize(&site_root)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    for candidate in candidates {
        let Ok(metadata) = tokio::fs::metadata(&candidate).await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(candidate_canonical) = tokio::fs::canonicalize(&candidate).await else {
            continue;
        };
        if !candidate_canonical.starts_with(&root_canonical) {
            return Err(StatusCode::BAD_REQUEST);
        }
        let bytes = tokio::fs::read(&candidate_canonical)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let path_for_type = candidate_canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index.html");
        let mut response = (
            StatusCode::OK,
            [("content-type", content_type(path_for_type))],
            bytes,
        )
            .into_response();
        stamp_site_headers(&mut response, site_root_uri, site_head.as_ref())?;
        return Ok(response);
    }

    Err(StatusCode::NOT_FOUND)
}

async fn serve_cid_file(
    State(state): State<GatewayState>,
    Path((cid, file_path)): Path<(String, String)>,
) -> Response {
    if !is_valid_cid(&cid) {
        return (StatusCode::BAD_REQUEST, "Invalid CID").into_response();
    }

    match serve_cid_path_result(&state, &cid, &file_path).await {
        Ok(response) => response,
        Err(StatusCode::BAD_REQUEST) => {
            (StatusCode::BAD_REQUEST, "Invalid file path").into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "File not found").into_response(),
    }
}

/// Fetch a single file from IPFS inline via the `cat` operation.
/// Returns raw bytes. Works with VM-based providers (data returned over vsock).
async fn fetch_file_inline(state: &GatewayState, cid: &str, path: &str) -> anyhow::Result<Vec<u8>> {
    let req = serde_json::json!({
        "op": "cat",
        "cid": cid,
        "path": path,
    });
    let resp = send_ipfs_raw(state, &req).await?;
    let status = resp
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("error");
    if status != "ok" {
        let msg = resp
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("{}", msg);
    }
    let data_b64 = resp
        .get("data")
        .and_then(|d| d.get("data"))
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!("no data in cat response"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_b64)?;
    if bytes.len() > MAX_GATEWAY_FILE_SIZE {
        anyhow::bail!("file exceeds size limit");
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Path validation
// ---------------------------------------------------------------------------

/// Validate a request file path — reject traversal, absolute paths, backslashes.
pub(super) fn validate_file_path(path: &str) -> Result<(), &'static str> {
    // Reject absolute paths
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("Absolute paths not allowed");
    }
    // Reject backslashes (Windows-style)
    if path.contains('\\') {
        return Err("Backslashes not allowed");
    }
    // Reject traversal (raw and URL-encoded)
    if path.contains("..") {
        return Err("Path traversal not allowed");
    }
    // Check URL-encoded traversal variants
    let decoded = path.replace("%2e", ".").replace("%2E", ".");
    if decoded.contains("..") {
        return Err("Encoded path traversal not allowed");
    }
    let decoded_slash = path.replace("%2f", "/").replace("%2F", "/");
    if decoded_slash.contains("..") {
        return Err("Encoded path traversal not allowed");
    }

    Ok(())
}

/// Get or create a cached ipfs-provider bridge.
async fn get_or_create_bridge(
    state: &GatewayState,
) -> anyhow::Result<Arc<elastos_runtime::provider::ProviderBridge>> {
    let mut guard = state.ipfs_bridge.lock().await;
    if let Some(ref bridge) = *guard {
        return Ok(Arc::clone(bridge));
    }

    let ipfs_binary = state.ipfs_provider_binary.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "ipfs-provider not found. Build: cd capsules/ipfs-provider && cargo build --release"
        )
    })?;

    let bridge = Arc::new(
        elastos_runtime::provider::ProviderBridge::spawn(
            ipfs_binary,
            elastos_runtime::provider::BridgeProviderConfig::default(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("Failed to spawn ipfs-provider: {}", e))?,
    );

    *guard = Some(Arc::clone(&bridge));
    Ok(bridge)
}

/// Send a raw request to ipfs-provider.
///
/// Preferred path: use runtime provider registry (VM-backed provider route).
/// Fallback path: spawn/hold a direct ProviderBridge from ipfs-provider binary.
async fn send_ipfs_raw(
    state: &GatewayState,
    request: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if let Some(registry) = state.provider_registry.as_ref() {
        return registry
            .send_raw("ipfs", request)
            .await
            .map_err(|e| anyhow::anyhow!("provider registry ipfs request failed: {}", e));
    }

    let bridge = get_or_create_bridge(state).await?;
    bridge
        .send_raw(request)
        .await
        .map_err(|e| anyhow::anyhow!("ipfs-provider bridge error: {}", e))
}

// ---------------------------------------------------------------------------
// MIME types
// ---------------------------------------------------------------------------

pub(super) fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("txt" | "sh") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// CID validation (one-liner, avoids depending on main.rs)
// ---------------------------------------------------------------------------

fn is_valid_cid(s: &str) -> bool {
    cid::Cid::try_from(s).is_ok()
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

pub async fn start_gateway_server(
    addr: &str,
    ipfs_provider_binary: Option<PathBuf>,
    provider_registry: Option<Arc<ProviderRegistry>>,
    cache_dir: PathBuf,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    let state = GatewayState {
        ipfs_provider_binary,
        provider_registry,
        cache_dir,
        data_dir,
        ipfs_bridge: Arc::new(Mutex::new(None)),
    };
    let app = gateway_router(state);
    let listener = TcpListener::bind(addr).await?;
    let advertised = advertised_gateway_urls(addr);
    println!("ElastOS Gateway v{}", GATEWAY_VERSION);
    println!("  Bind:      http://{}", addr);
    if let Some(primary) = advertised.first() {
        println!("  Open:      {}", primary);
        println!("  Room:      {}apps/room-browser/", primary);
        println!("  Content:   {}s/<cid>/", primary);
        for extra in advertised.iter().skip(1) {
            println!("  Also:      {}", extra);
        }
    } else {
        println!("  Open:      http://{}", addr);
        println!("  Room:      http://{}/apps/room-browser/", addr);
        println!("  Content:   http://{}/s/<cid>/", addr);
    }
    println!();
    println!("  Cache is unbounded (Tier 1) — delete cache dir to reclaim space");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            println!("\nShutting down gateway...");
        })
        .await?;
    Ok(())
}

pub(crate) fn advertised_gateway_urls(addr: &str) -> Vec<String> {
    let Ok(socket_addr) = addr.parse::<SocketAddr>() else {
        return vec![format!("http://{}/", addr.trim_end_matches('/'))];
    };

    let port = socket_addr.port();
    let host = socket_addr.ip();

    let mut urls = Vec::new();
    match host {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            urls.push(format!("http://127.0.0.1:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(format!("http://{}:{}/", ip, port));
            }
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            urls.push(format!("http://[::1]:{}/", port));
            for ip in detect_advertisable_ips() {
                if ip.is_loopback() {
                    continue;
                }
                urls.push(match ip {
                    IpAddr::V4(ip) => format!("http://{}:{}/", ip, port),
                    IpAddr::V6(ip) => format!("http://[{}]:{}/", ip, port),
                });
            }
        }
        IpAddr::V4(ip) => {
            urls.push(format!("http://{}:{}/", ip, port));
        }
        IpAddr::V6(ip) => {
            urls.push(format!("http://[{}]:{}/", ip, port));
        }
    }

    dedupe_urls(urls)
}

fn detect_advertisable_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for part in stdout.split_whitespace() {
                if let Ok(ip) = part.parse::<IpAddr>() {
                    ips.push(ip);
                }
            }
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".parse().unwrap());
    }
    ips
}

fn dedupe_urls(urls: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for url in urls {
        if seen.insert(url.clone()) {
            deduped.push(url);
        }
    }
    deduped
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate.recv() => {},
            }
        } else {
            ctrl_c.await;
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{save_trusted_sources, TrustedSource, TrustedSourcesConfig};
    use axum::body::Body;
    use axum::extract::{Path as AxumPath, State as AxumState};
    use axum::http::Request;
    use axum::routing::post;
    use axum::Json as AxumJson;
    use ed25519_dalek::{Signer as _, Verifier as _};
    use serde_json::json;
    use std::collections::{BTreeSet, HashMap};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex as TokioMutex;
    use tower::ServiceExt;

    // Real CIDs that pass cid crate validation
    const TEST_CIDV0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";
    const TEST_CIDV1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    fn test_state(cache_dir: &std::path::Path) -> GatewayState {
        GatewayState {
            ipfs_provider_binary: None,
            provider_registry: None,
            cache_dir: cache_dir.to_path_buf(),
            data_dir: cache_dir.to_path_buf(),
            ipfs_bridge: Arc::new(Mutex::new(None)),
        }
    }

    fn room_cookie_header(response: &Response) -> String {
        response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::to_string)
            .expect("room session cookie header")
    }

    #[derive(Default)]
    struct FakePeerBus {
        topic_members: HashMap<String, BTreeSet<String>>,
        topic_messages: HashMap<String, Vec<serde_json::Value>>,
        cursors: HashMap<(String, String, String), usize>,
    }

    #[derive(Clone)]
    struct FakeRuntimeState {
        did: String,
        signing_key: ed25519_dalek::SigningKey,
        attach_secret: String,
        peer_id: String,
        bus: Arc<TokioMutex<FakePeerBus>>,
    }

    struct FakeRuntimeHandle {
        api_url: String,
        _task: tokio::task::JoinHandle<()>,
    }

    fn verifying_key_from_did(did: &str) -> Option<ed25519_dalek::VerifyingKey> {
        let multibase = did.strip_prefix("did:key:z")?;
        let bytes = bs58::decode(multibase).into_vec().ok()?;
        if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
            return None;
        }
        let key_bytes: [u8; 32] = bytes[2..34].try_into().ok()?;
        ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).ok()
    }

    async fn start_fake_runtime(
        data_dir: &std::path::Path,
        bus: Arc<TokioMutex<FakePeerBus>>,
        peer_id: &str,
    ) -> FakeRuntimeHandle {
        let (signing_key, did) = elastos_identity::load_or_create_did(data_dir).unwrap();
        let state = FakeRuntimeState {
            did,
            signing_key,
            attach_secret: format!("attach-{peer_id}"),
            peer_id: peer_id.to_string(),
            bus,
        };
        let app = Router::new()
            .route("/api/auth/attach", post(fake_runtime_attach))
            .route(
                "/api/capability/request",
                post(fake_runtime_capability_request),
            )
            .route("/api/provider/:scheme/:op", post(fake_runtime_provider))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let api_url = format!("http://{}", addr);
        std::fs::write(
            data_dir.join("runtime-coords.json"),
            serde_json::to_vec_pretty(&json!({
                "api_url": api_url,
                "attach_secret": state.attach_secret,
            }))
            .unwrap(),
        )
        .unwrap();
        FakeRuntimeHandle {
            api_url,
            _task: task,
        }
    }

    async fn fake_runtime_attach(
        AxumState(state): AxumState<FakeRuntimeState>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> Response {
        let secret = body
            .get("secret")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if secret != state.attach_secret {
            return (StatusCode::FORBIDDEN, "bad attach secret").into_response();
        }
        AxumJson(json!({
            "token": format!("client-{}", state.peer_id),
        }))
        .into_response()
    }

    async fn fake_runtime_capability_request(
        AxumState(state): AxumState<FakeRuntimeState>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> Response {
        let resource = body
            .get("resource")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let token = if resource.starts_with("elastos://did/") {
            format!("did-cap-{}", state.peer_id)
        } else if resource.starts_with("elastos://peer/") {
            format!("peer-cap-{}", state.peer_id)
        } else {
            format!("cap-{}", state.peer_id)
        };
        AxumJson(json!({ "token": token })).into_response()
    }

    async fn fake_runtime_provider(
        AxumPath((scheme, op)): AxumPath<(String, String)>,
        AxumState(state): AxumState<FakeRuntimeState>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> Response {
        match (scheme.as_str(), op.as_str()) {
            ("did", "get_did") => AxumJson(json!({
                "status": "ok",
                "data": { "did": state.did }
            }))
            .into_response(),
            ("did", "sign") => {
                let data = body
                    .get("data")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let Ok(bytes) = hex::decode(data) else {
                    return AxumJson(json!({"status":"error","message":"invalid hex payload"}))
                        .into_response();
                };
                let signature = state.signing_key.sign(&bytes);
                AxumJson(json!({
                    "status": "ok",
                    "data": { "signature": hex::encode(signature.to_bytes()) }
                }))
                .into_response()
            }
            ("did", "verify") => {
                let did = body
                    .get("did")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let data = body
                    .get("data")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let signature = body
                    .get("signature")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let valid = {
                    let Ok(bytes) = hex::decode(data) else {
                        return AxumJson(json!({"status":"error","message":"invalid hex payload"}))
                            .into_response();
                    };
                    let Ok(sig_bytes) = hex::decode(signature) else {
                        return AxumJson(json!({"status":"error","message":"invalid signature"}))
                            .into_response();
                    };
                    let Ok(sig) = ed25519_dalek::Signature::try_from(sig_bytes.as_slice()) else {
                        return AxumJson(json!({"status":"error","message":"invalid signature"}))
                            .into_response();
                    };
                    verifying_key_from_did(did)
                        .map(|key| key.verify(&bytes, &sig).is_ok())
                        .unwrap_or(false)
                };
                AxumJson(json!({
                    "status": "ok",
                    "data": { "valid": valid }
                }))
                .into_response()
            }
            ("peer", "gossip_join") => {
                let topic = body
                    .get("topic")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let mut bus = state.bus.lock().await;
                bus.topic_members
                    .entry(topic.to_string())
                    .or_default()
                    .insert(state.peer_id.clone());
                AxumJson(json!({
                    "status": "ok",
                    "data": { "topic": topic }
                }))
                .into_response()
            }
            ("peer", "list_topic_peers") => {
                let topic = body
                    .get("topic")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let bus = state.bus.lock().await;
                let peers = bus
                    .topic_members
                    .get(topic)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|peer| peer != &state.peer_id)
                    .collect::<Vec<_>>();
                AxumJson(json!({
                    "status": "ok",
                    "data": { "topic": topic, "peers": peers }
                }))
                .into_response()
            }
            ("peer", "gossip_send") => {
                let topic = body
                    .get("topic")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let mut bus = state.bus.lock().await;
                let peers = bus.topic_members.get(topic).cloned().unwrap_or_default();
                bus.topic_messages
                    .entry(topic.to_string())
                    .or_default()
                    .push(json!({
                        "sender_id": body.get("sender_id").cloned().unwrap_or(serde_json::Value::Null),
                        "sender_nick": body.get("sender").cloned().unwrap_or(serde_json::Value::Null),
                        "content": body.get("message").cloned().unwrap_or(serde_json::Value::Null),
                        "ts": body.get("ts").cloned().unwrap_or(serde_json::Value::from(0u64)),
                        "signature": body.get("signature").cloned().unwrap_or(serde_json::Value::Null),
                    }));
                let mut response = json!({ "status": "ok" });
                if peers.iter().filter(|peer| *peer != &state.peer_id).count() == 0 {
                    response["broadcast"] = serde_json::Value::String("local_only".to_string());
                }
                AxumJson(response).into_response()
            }
            ("peer", "gossip_recv") => {
                let topic = body
                    .get("topic")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let consumer_id = body
                    .get("consumer_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("default");
                let limit = body
                    .get("limit")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(50);
                let skip_sender_id = body
                    .get("skip_sender_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let mut bus = state.bus.lock().await;
                let cursor_key = (
                    state.peer_id.clone(),
                    topic.to_string(),
                    consumer_id.to_string(),
                );
                let start = *bus.cursors.get(&cursor_key).unwrap_or(&0);
                let all = bus.topic_messages.get(topic).cloned().unwrap_or_default();
                let count = all.len().saturating_sub(start).min(limit as usize);
                let selected = all
                    .into_iter()
                    .skip(start)
                    .take(limit as usize)
                    .filter(|message| {
                        skip_sender_id.is_empty()
                            || message
                                .get("sender_id")
                                .and_then(|value| value.as_str())
                                .unwrap_or("")
                                != skip_sender_id
                    })
                    .collect::<Vec<_>>();
                bus.cursors.insert(cursor_key, start + count);
                AxumJson(json!({
                    "status": "ok",
                    "data": { "messages": selected }
                }))
                .into_response()
            }
            _ => AxumJson(json!({
                "status": "error",
                "message": format!("unsupported fake runtime operation {scheme}/{op}"),
            }))
            .into_response(),
        }
    }

    #[test]
    fn test_content_type_mapping() {
        assert_eq!(content_type("index.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("style.css"), "text/css");
        assert_eq!(content_type("app.js"), "application/javascript");
        assert_eq!(content_type("data.json"), "application/json");
        assert_eq!(content_type("README.md"), "text/markdown; charset=utf-8");
        assert_eq!(content_type("image.png"), "image/png");
        assert_eq!(content_type("photo.jpg"), "image/jpeg");
        assert_eq!(content_type("photo.jpeg"), "image/jpeg");
        assert_eq!(content_type("icon.svg"), "image/svg+xml");
        assert_eq!(content_type("module.wasm"), "application/wasm");
        assert_eq!(content_type("unknown.xyz"), "application/octet-stream");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }

    #[test]
    fn test_validate_file_path() {
        assert!(validate_file_path("index.html").is_ok());
        assert!(validate_file_path("sub/dir/file.js").is_ok());
        assert!(validate_file_path("a.b.c.txt").is_ok());

        assert!(validate_file_path("../etc/passwd").is_err());
        assert!(validate_file_path("foo/../../etc/passwd").is_err());
        assert!(validate_file_path("/absolute/path").is_err());
        assert!(validate_file_path("foo\\bar").is_err());
        assert!(validate_file_path("\\windows\\path").is_err());
    }

    #[test]
    fn test_validate_file_path_encoded() {
        assert!(validate_file_path("%2e%2e/etc/passwd").is_err());
        assert!(validate_file_path("%2E%2E/etc/passwd").is_err());
        assert!(validate_file_path("foo%2F..%2Fetc/passwd").is_err());
        assert!(validate_file_path("foo/%2e%2e/bar").is_err());
    }

    #[test]
    fn test_advertised_gateway_urls_for_specific_host() {
        let urls = advertised_gateway_urls("77.42.19.31:18090");
        assert_eq!(urls, vec!["http://77.42.19.31:18090/"]);
    }

    #[test]
    fn test_advertised_gateway_urls_for_wildcard_bind_starts_with_loopback() {
        let urls = advertised_gateway_urls("0.0.0.0:18090");
        assert_eq!(
            urls.first().map(String::as_str),
            Some("http://127.0.0.1:18090/")
        );
    }

    #[tokio::test]
    async fn test_landing_page_200() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("ElastOS Gateway"));
    }

    #[tokio::test]
    async fn test_root_serves_mywebsite_when_staged() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>pc2 site</html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-origin")
                .and_then(|v| v.to_str().ok()),
            Some("localhost://MyWebSite")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>pc2 site</html>");
    }

    #[tokio::test]
    async fn test_healthz_200() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_invalid_cid_400() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/s/not-a-cid/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_cid_without_trailing_slash_redirects() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(location, format!("/s/{}/", TEST_CIDV1));
    }

    #[tokio::test]
    async fn test_ipfs_cid_root_serves_cached_raw_file_without_redirect() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{}.raw", TEST_CIDV1)),
            b"raw-binary",
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/ipfs/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/octet-stream");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"raw-binary");
    }

    #[tokio::test]
    async fn test_ipfs_cid_root_falls_back_to_cached_directory_index() {
        let dir = tempfile::tempdir().unwrap();
        let cid_dir = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cid_dir).unwrap();
        std::fs::write(cid_dir.join("index.html"), "<html>ok</html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/ipfs/{}", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "text/html; charset=utf-8");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>ok</html>");
    }

    #[tokio::test]
    async fn test_traversal_400() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}/../etc/passwd", TEST_CIDV0))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_missing_file_404() {
        let dir = tempfile::tempdir().unwrap();
        // Pre-populate cache so we don't need IPFS
        let cid_dir = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cid_dir).unwrap();
        std::fs::write(cid_dir.join("index.html"), "<html></html>").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/s/{}/no-such-file.txt", TEST_CIDV1))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_release_head_200() {
        let dir = tempfile::tempdir().unwrap();
        let head = r#"{"payload":{"schema":"elastos.release.head/v1"}}"#;
        let publisher_root = publisher_release_head_path(dir.path());
        std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
        std::fs::write(publisher_root, head).unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_release_head_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_release_json_200() {
        let dir = tempfile::tempdir().unwrap();
        let release = r#"{"payload":{"schema":"elastos.release/v1"}}"#;
        let publisher_root = publisher_release_manifest_path(dir.path());
        std::fs::create_dir_all(publisher_root.parent().unwrap()).unwrap();
        std::fs::write(publisher_root, release).unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/release.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_install_sh_200() {
        let dir = tempfile::tempdir().unwrap();
        let install_path = publisher_install_script_path(dir.path());
        std::fs::create_dir_all(install_path.parent().unwrap()).unwrap();
        std::fs::write(install_path, "#!/bin/bash\necho hi").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/install.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "text/x-shellscript");
    }

    #[tokio::test]
    async fn test_install_sh_404() {
        let dir = tempfile::tempdir().unwrap();
        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/install.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_artifact_file_200() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts_dir = publisher_artifacts_path(dir.path());
        std::fs::create_dir_all(&artifacts_dir).unwrap();
        std::fs::write(artifacts_dir.join("components-linux-amd64.json"), "{}").unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/artifacts/components-linux-amd64.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn test_domain_binding_serves_bound_root() {
        let dir = tempfile::tempdir().unwrap();
        let public_site = dir.path().join("Public").join("docs");
        std::fs::create_dir_all(&public_site).unwrap();
        std::fs::write(public_site.join("index.html"), "<html>bound site</html>").unwrap();

        let binding_path = edge_binding_path(dir.path(), "docs.example.com");
        std::fs::create_dir_all(binding_path.parent().unwrap()).unwrap();
        std::fs::write(
            &binding_path,
            r#"{"domain":"docs.example.com","target":"localhost://Public/docs"}"#,
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", "docs.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-origin")
                .and_then(|v| v.to_str().ok()),
            Some("localhost://Public/docs")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>bound site</html>");
    }

    #[tokio::test]
    async fn test_site_head_document_and_headers() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>pc2 site</html>").unwrap();
        let cached_bundle = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cached_bundle).unwrap();
        std::fs::write(
            cached_bundle.join("index.html"),
            "<html>published bundle</html>",
        )
        .unwrap();

        let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
        std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
        std::fs::write(
            &head_path,
            r#"{"payload":{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi","release_name":"v1","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":21,"activated_at":123},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}"#,
        )
        .unwrap();

        let state = test_state(dir.path());
        let app = gateway_router(state);

        let root_resp = app
            .clone()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(root_resp.status(), StatusCode::OK);
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-schema")
                .and_then(|v| v.to_str().ok()),
            Some("elastos.site.head.v1")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-digest")
                .and_then(|v| v.to_str().ok()),
            Some("sha256:abc123")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-cid")
                .and_then(|v| v.to_str().ok()),
            Some(TEST_CIDV1)
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-release")
                .and_then(|v| v.to_str().ok()),
            Some("v1")
        );
        assert_eq!(
            root_resp
                .headers()
                .get("x-elastos-site-head-channel")
                .and_then(|v| v.to_str().ok()),
            Some("live")
        );

        let head_resp = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/elastos/site-head.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head_resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(head_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("\"schema\":\"elastos.site.head.v1\""));
        assert!(text.contains("\"target\":\"localhost://MyWebSite\""));
        assert!(text.contains(&format!("\"bundle_cid\":\"{}\"", TEST_CIDV1)));
        assert!(text.contains("\"release_name\":\"v1\""));
        assert!(text.contains("\"channel_name\":\"live\""));
    }

    #[tokio::test]
    async fn test_active_site_head_prefers_bundle_cid() {
        let dir = tempfile::tempdir().unwrap();
        let site_root = elastos_common::localhost::my_website_root_path(dir.path());
        std::fs::create_dir_all(&site_root).unwrap();
        std::fs::write(site_root.join("index.html"), "<html>working tree</html>").unwrap();

        let cached_bundle = dir.path().join(TEST_CIDV1);
        std::fs::create_dir_all(&cached_bundle).unwrap();
        std::fs::write(
            cached_bundle.join("index.html"),
            "<html>published bundle</html>",
        )
        .unwrap();

        let head_path = edge_site_head_path(dir.path(), MY_WEBSITE_URI);
        std::fs::create_dir_all(head_path.parent().unwrap()).unwrap();
        std::fs::write(
            &head_path,
            format!(
                r#"{{"payload":{{"schema":"elastos.site.head.v1","target":"localhost://MyWebSite","bundle_cid":"{}","release_name":"v2","channel_name":"live","content_digest":"sha256:abc123","entry_count":1,"total_bytes":28,"activated_at":123}},"signature":"deadbeef","signer_did":"did:key:z6Mkexample"}}"#,
                TEST_CIDV1
            ),
        )
        .unwrap();

        let app = gateway_router(test_state(dir.path()));
        let resp = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-cid")
                .and_then(|v| v.to_str().ok()),
            Some(TEST_CIDV1)
        );
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-release")
                .and_then(|v| v.to_str().ok()),
            Some("v2")
        );
        assert_eq!(
            resp.headers()
                .get("x-elastos-site-head-channel")
                .and_then(|v| v.to_str().ok()),
            Some("live")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"<html>published bundle</html>");
    }

    #[tokio::test]
    async fn test_room_service_assets_serve() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/apps/room-browser/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("Room Browser"));

        let wasm = app
            .oneshot(
                Request::builder()
                    .uri("/apps/room-browser/room_browser_ui_bg.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wasm.status(), StatusCode::OK);
        assert_eq!(
            wasm.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/wasm")
        );
    }

    #[tokio::test]
    async fn test_room_service_summary_omits_display_name_suggestion() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/room-browser/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("suggested_display_name").is_none());
    }

    #[tokio::test]
    async fn test_room_service_summary_does_not_create_identity_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/room-browser/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(!dir.path().join("identity").join("device.key").exists());
    }

    #[tokio::test]
    async fn test_room_service_summary_includes_hosted_guest_urls() {
        let dir = tempfile::tempdir().unwrap();
        save_trusted_sources(
            dir.path(),
            &TrustedSourcesConfig {
                schema: "elastos.trusted-sources/v1".to_string(),
                default_source: "default".to_string(),
                sources: vec![TrustedSource {
                    name: "default".to_string(),
                    publisher_dids: vec![],
                    channel: "stable".to_string(),
                    discovery_uri: String::new(),
                    connect_ticket: String::new(),
                    gateways: vec!["https://elastos.elacitylabs.com".to_string()],
                    install_path: String::new(),
                    installed_version: String::new(),
                    head_cid: String::new(),
                    publisher_node_id: String::new(),
                    ipns_name: String::new(),
                }],
            },
        )
        .unwrap();
        crate::browser_app_hosts::record_ephemeral_browser_app_url(
            dir.path(),
            crate::room_service::room_slug(),
            Some("https://quick.trycloudflare.com"),
        )
        .unwrap();
        let app = gateway_router(test_state(dir.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/room-browser/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["canonical_hosted_guest_url"].as_str(),
            Some("https://elastos.elacitylabs.com/apps/room-browser/")
        );
        assert_eq!(
            json["ephemeral_hosted_guest_url"].as_str(),
            Some("https://quick.trycloudflare.com/")
        );
    }

    #[tokio::test]
    async fn test_room_service_summary_blocks_pairing_when_seeded_room_has_no_runtime_member() {
        let dir = tempfile::tempdir().unwrap();
        crate::room_service::seed_room_owner(
            dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let app = gateway_router(test_state(dir.path()));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/apps/room-browser/summary")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["pairing_allowed"].as_bool(), Some(false));
        assert_eq!(json["owner_did"].as_str(), None);
        assert!(json["pairing_block_reason"]
            .as_str()
            .unwrap()
            .contains("no active room member DID available"));
    }

    #[tokio::test]
    async fn test_browser_session_pair_and_status_routes_room_browser() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pair_resp.status(), StatusCode::OK);
        let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
        let request_id = pair_json["request_id"].as_str().unwrap().to_string();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_resp.status(), StatusCode::OK);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert_eq!(status_json["status"].as_str(), Some("pending"));
    }

    #[tokio::test]
    async fn test_browser_session_pair_is_forbidden_when_seeded_room_has_no_runtime_member() {
        let dir = tempfile::tempdir().unwrap();
        crate::room_service::seed_room_owner(
            dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pair_resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_room_service_pairing_and_object_flow() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pair_resp.status(), StatusCode::OK);
        let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
        let request_id = pair_json["request_id"].as_str().unwrap().to_string();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_resp.status(), StatusCode::OK);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert_eq!(status_json["status"].as_str(), Some("pending"));

        let approved = crate::room_service::approve_next_request(dir.path())
            .unwrap()
            .unwrap();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_resp.status(), StatusCode::OK);
        let room_cookie = room_cookie_header(&status_resp);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert_eq!(status_json["status"].as_str(), Some("approved"));
        assert!(status_json["token"].is_null());

        let send_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/objects/send")
                    .header("cookie", &room_cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"body":"Hello room"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(send_resp.status(), StatusCode::OK);

        let feed_resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header("cookie", &room_cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(feed_resp.status(), StatusCode::OK);
        let feed_body = axum::body::to_bytes(feed_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let feed_json: serde_json::Value = serde_json::from_slice(&feed_body).unwrap();
        assert_eq!(feed_json["latest_seq"].as_u64(), Some(2));
        assert_eq!(feed_json["objects"][0]["kind"].as_str(), Some("system"));
        assert_eq!(
            feed_json["objects"][0]["body"].as_str(),
            Some("joined the room")
        );
        assert_eq!(feed_json["objects"][1]["body"].as_str(), Some("Hello room"));
        assert_eq!(feed_json["objects"][1]["sender"].as_str(), Some("Alice"));
        assert_eq!(feed_json["objects"][1]["kind"].as_str(), Some("text"));
        assert_eq!(approved.display_name, "Alice");
    }

    #[tokio::test]
    async fn test_room_service_attachment_upload_and_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
        let request_id = pair_json["request_id"].as_str().unwrap().to_string();

        crate::room_service::approve_next_request(dir.path())
            .unwrap()
            .unwrap();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let room_cookie = room_cookie_header(&status_resp);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert!(status_json["token"].is_null());

        let upload_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/upload/start")
                    .header("cookie", &room_cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"file_name":"photo.png","mime_type":"image/png","size_bytes":{}}}"#,
                        b"png-data".len()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload_resp.status(), StatusCode::OK);
        let upload_body = axum::body::to_bytes(upload_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
        let upload_id = upload_json["upload_id"].as_str().unwrap();

        let chunk_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/apps/room-browser/upload/{}/chunk", upload_id))
                    .header("cookie", &room_cookie)
                    .header("x-elastos-upload-offset", "0")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from("png-data"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chunk_resp.status(), StatusCode::OK);

        let finish_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/apps/room-browser/upload/{}/finish",
                        upload_id
                    ))
                    .header("cookie", &room_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finish_resp.status(), StatusCode::OK);
        let finish_body = axum::body::to_bytes(finish_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let finish_json: serde_json::Value = serde_json::from_slice(&finish_body).unwrap();
        assert_eq!(finish_json["kind"].as_str(), Some("attachment"));
        let attachment_id = finish_json["attachment"]["attachment_id"].as_str().unwrap();

        let fetch_resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/apps/room-browser/attachments/{}",
                        attachment_id
                    ))
                    .header("cookie", &room_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetch_resp.status(), StatusCode::OK);
        assert_eq!(
            fetch_resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("image/png")
        );
        let bytes = axum::body::to_bytes(fetch_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"png-data");
    }

    #[tokio::test]
    async fn test_room_service_audio_attachment_upload_is_inline_media() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
        let request_id = pair_json["request_id"].as_str().unwrap().to_string();

        crate::room_service::approve_next_request(dir.path())
            .unwrap()
            .unwrap();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let room_cookie = room_cookie_header(&status_resp);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert!(status_json["token"].is_null());

        let upload_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/upload/start")
                    .header("cookie", &room_cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"file_name":"voice.ogg","mime_type":"audio/ogg","size_bytes":{}}}"#,
                        b"ogg-data".len()
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload_resp.status(), StatusCode::OK);
        let upload_body = axum::body::to_bytes(upload_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let upload_json: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
        let upload_id = upload_json["upload_id"].as_str().unwrap();

        let chunk_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/apps/room-browser/upload/{}/chunk", upload_id))
                    .header("cookie", &room_cookie)
                    .header("x-elastos-upload-offset", "0")
                    .header("content-type", "application/octet-stream")
                    .body(Body::from("ogg-data"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chunk_resp.status(), StatusCode::OK);

        let finish_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/api/apps/room-browser/upload/{}/finish",
                        upload_id
                    ))
                    .header("cookie", &room_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finish_resp.status(), StatusCode::OK);
        let finish_body = axum::body::to_bytes(finish_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let finish_json: serde_json::Value = serde_json::from_slice(&finish_body).unwrap();
        assert_eq!(finish_json["attachment"]["is_audio"].as_bool(), Some(true));
        let attachment_id = finish_json["attachment"]["attachment_id"].as_str().unwrap();

        let fetch_resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/apps/room-browser/attachments/{}",
                        attachment_id
                    ))
                    .header("cookie", &room_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetch_resp.status(), StatusCode::OK);
        assert_eq!(
            fetch_resp
                .headers()
                .get("content-disposition")
                .and_then(|v| v.to_str().ok()),
            Some("inline; filename=\"voice.ogg\"")
        );
    }

    #[tokio::test]
    async fn test_room_service_session_leave_appends_system_object() {
        let dir = tempfile::tempdir().unwrap();
        let app = gateway_router(test_state(dir.path()));

        let pair_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"app":"room-browser","display_name":"Alice","device_label":"Phone"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let pair_body = axum::body::to_bytes(pair_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let pair_json: serde_json::Value = serde_json::from_slice(&pair_body).unwrap();
        let request_id = pair_json["request_id"].as_str().unwrap().to_string();

        crate::room_service::approve_next_request(dir.path())
            .unwrap()
            .unwrap();

        let status_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browser/session/status")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"app":"room-browser","request_id":"{}"}}"#,
                        request_id
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        let room_cookie = room_cookie_header(&status_resp);
        let status_body = axum::body::to_bytes(status_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status_json: serde_json::Value = serde_json::from_slice(&status_body).unwrap();
        assert!(status_json["token"].is_null());

        let leave_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/session/leave")
                    .header("cookie", &room_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(leave_resp.status(), StatusCode::OK);
        let leave_body = axum::body::to_bytes(leave_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let leave_json: serde_json::Value = serde_json::from_slice(&leave_body).unwrap();
        assert_eq!(leave_json["kind"].as_str(), Some("system"));
        assert_eq!(leave_json["body"].as_str(), Some("left the room"));

        let summary = crate::room_service::load_summary(dir.path()).unwrap();
        assert_eq!(summary.active_session_count, 0);
    }

    #[tokio::test]
    async fn test_room_service_cross_runtime_room_syncs_over_carrier() {
        let owner_dir = tempfile::tempdir().unwrap();
        let guest_dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
        let owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
        let guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

        assert!(owner_runtime.api_url.starts_with("http://127.0.0.1:"));
        assert!(guest_runtime.api_url.starts_with("http://127.0.0.1:"));

        let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
            .unwrap()
            .1;
        let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
            .unwrap()
            .1;

        let _ = crate::room_service::seed_room_owner(
            owner_dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: owner_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = crate::room_service::export_room_invite_envelope(
            owner_dir.path(),
            crate::room_service::RoomInviteInput {
                actor_did: owner_did.clone(),
                invited_did: guest_did.clone(),
                role: crate::room_service::RoomRole::Member,
            },
        )
        .unwrap();
        let invite_json = serde_json::to_vec(&invite).unwrap();
        crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
        crate::room_service::accept_room_invite(
            guest_dir.path(),
            crate::room_service::RoomInviteAcceptInput {
                actor_did: guest_did.clone(),
                invite_id: invite.payload.invite_id.clone(),
            },
        )
        .unwrap();
        let acceptance = crate::room_service::export_room_acceptance_envelope(
            guest_dir.path(),
            &invite.payload.invite_id,
        )
        .unwrap();
        let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
        crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
            .unwrap();

        let owner_request = crate::room_service::request_pairing(
            owner_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Owner".to_string(),
                device_label: "WSL".to_string(),
                member_did: Some(owner_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(owner_dir.path())
            .unwrap()
            .unwrap();
        let owner_token =
            crate::room_service::pairing_status(owner_dir.path(), &owner_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let guest_request = crate::room_service::request_pairing(
            guest_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Guest".to_string(),
                device_label: "Jetson".to_string(),
                member_did: Some(guest_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(guest_dir.path())
            .unwrap()
            .unwrap();
        let guest_token =
            crate::room_service::pairing_status(guest_dir.path(), &guest_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let owner_gateway = gateway_router(test_state(owner_dir.path()));
        let guest_gateway = gateway_router(test_state(guest_dir.path()));

        let send_response = owner_gateway
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/objects/send")
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"body":"hello across runtimes"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(send_response.status(), StatusCode::OK);

        let poll_response = guest_gateway
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll_response.status(), StatusCode::OK);
        let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
        assert_eq!(poll["transport"]["connected_peer_count"].as_u64(), Some(1));
        assert!(poll["transport"]["status"]
            .as_str()
            .unwrap_or_default()
            .contains("Carrier room sync connected to 1 runtime"));
        let participants = poll["participants"].as_array().cloned().unwrap_or_default();
        assert_eq!(participants.len(), 2);
        assert!(participants.iter().any(|participant| {
            participant["member_did"].as_str() == Some(owner_did.as_str())
                && participant["display_name"].as_str() == Some("Owner")
        }));
        assert!(participants.iter().any(|participant| {
            participant["member_did"].as_str() == Some(guest_did.as_str())
                && participant["display_name"].as_str() == Some("Guest")
        }));
        let objects = poll["objects"].as_array().cloned().unwrap_or_default();
        assert!(objects
            .iter()
            .any(|object| object["body"].as_str() == Some("hello across runtimes")));
    }

    #[tokio::test]
    async fn test_room_service_cross_runtime_attachment_syncs_over_carrier() {
        let owner_dir = tempfile::tempdir().unwrap();
        let guest_dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
        let _owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
        let _guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

        let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
            .unwrap()
            .1;
        let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
            .unwrap()
            .1;

        let _ = crate::room_service::seed_room_owner(
            owner_dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: owner_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = crate::room_service::export_room_invite_envelope(
            owner_dir.path(),
            crate::room_service::RoomInviteInput {
                actor_did: owner_did.clone(),
                invited_did: guest_did.clone(),
                role: crate::room_service::RoomRole::Member,
            },
        )
        .unwrap();
        let invite_json = serde_json::to_vec(&invite).unwrap();
        crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
        crate::room_service::accept_room_invite(
            guest_dir.path(),
            crate::room_service::RoomInviteAcceptInput {
                actor_did: guest_did.clone(),
                invite_id: invite.payload.invite_id.clone(),
            },
        )
        .unwrap();
        let acceptance = crate::room_service::export_room_acceptance_envelope(
            guest_dir.path(),
            &invite.payload.invite_id,
        )
        .unwrap();
        let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
        crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
            .unwrap();

        let owner_request = crate::room_service::request_pairing(
            owner_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Owner".to_string(),
                device_label: "WSL".to_string(),
                member_did: Some(owner_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(owner_dir.path())
            .unwrap()
            .unwrap();
        let owner_token =
            crate::room_service::pairing_status(owner_dir.path(), &owner_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let guest_request = crate::room_service::request_pairing(
            guest_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Guest".to_string(),
                device_label: "Jetson".to_string(),
                member_did: Some(guest_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(guest_dir.path())
            .unwrap()
            .unwrap();
        let guest_token =
            crate::room_service::pairing_status(guest_dir.path(), &guest_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let owner_gateway = gateway_router(test_state(owner_dir.path()));
        let guest_gateway = gateway_router(test_state(guest_dir.path()));

        let start_response = owner_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/upload/start")
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"file_name":"photo.png","mime_type":"image/png","size_bytes":8}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start_response.status(), StatusCode::OK);
        let start_body = axum::body::to_bytes(start_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let start_json: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
        let upload_id = start_json["upload_id"].as_str().unwrap().to_string();

        let chunk_response = owner_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/apps/room-browser/upload/{upload_id}/chunk"))
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("x-elastos-upload-offset", "0")
                    .body(Body::from(Vec::from(&b"png-data"[..])))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(chunk_response.status(), StatusCode::OK);

        let finish_response = owner_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/apps/room-browser/upload/{upload_id}/finish"))
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(finish_response.status(), StatusCode::OK);

        let poll_response = guest_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(poll_response.status(), StatusCode::OK);
        let poll_body = axum::body::to_bytes(poll_response.into_body(), usize::MAX)
            .await
            .unwrap();
        let poll: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
        let attachment_object = poll["objects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|object| object["kind"].as_str() == Some("attachment"))
            .cloned()
            .expect("attachment object");
        let attachment_id = attachment_object["attachment"]["attachment_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            attachment_object["attachment"]["file_name"].as_str(),
            Some("photo.png")
        );
        assert_eq!(
            attachment_object["attachment"]["mime_type"].as_str(),
            Some("image/png")
        );

        let attachment_response = guest_gateway
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!(
                        "/api/apps/room-browser/attachments/{attachment_id}"
                    ))
                    .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(attachment_response.status(), StatusCode::OK);
        assert_eq!(
            attachment_response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            Some("image/png")
        );
        let attachment_body = axum::body::to_bytes(attachment_response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(attachment_body.as_ref(), b"png-data");
    }

    #[tokio::test]
    async fn test_room_service_cross_runtime_presence_syncs_join_and_leave() {
        let owner_dir = tempfile::tempdir().unwrap();
        let guest_dir = tempfile::tempdir().unwrap();
        let bus = Arc::new(TokioMutex::new(FakePeerBus::default()));
        let _owner_runtime = start_fake_runtime(owner_dir.path(), bus.clone(), "owner-peer").await;
        let _guest_runtime = start_fake_runtime(guest_dir.path(), bus.clone(), "guest-peer").await;

        let owner_did = elastos_identity::load_or_create_did(owner_dir.path())
            .unwrap()
            .1;
        let guest_did = elastos_identity::load_or_create_did(guest_dir.path())
            .unwrap()
            .1;

        let _ = crate::room_service::seed_room_owner(
            owner_dir.path(),
            crate::room_service::RoomOwnerSeedInput {
                owner_did: owner_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = crate::room_service::export_room_invite_envelope(
            owner_dir.path(),
            crate::room_service::RoomInviteInput {
                actor_did: owner_did.clone(),
                invited_did: guest_did.clone(),
                role: crate::room_service::RoomRole::Member,
            },
        )
        .unwrap();
        let invite_json = serde_json::to_vec(&invite).unwrap();
        crate::room_service::import_room_invite_envelope(guest_dir.path(), &invite_json).unwrap();
        crate::room_service::accept_room_invite(
            guest_dir.path(),
            crate::room_service::RoomInviteAcceptInput {
                actor_did: guest_did.clone(),
                invite_id: invite.payload.invite_id.clone(),
            },
        )
        .unwrap();
        let acceptance = crate::room_service::export_room_acceptance_envelope(
            guest_dir.path(),
            &invite.payload.invite_id,
        )
        .unwrap();
        let acceptance_json = serde_json::to_vec(&acceptance).unwrap();
        crate::room_service::import_room_acceptance_envelope(owner_dir.path(), &acceptance_json)
            .unwrap();

        let owner_request = crate::room_service::request_pairing(
            owner_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Owner".to_string(),
                device_label: "WSL".to_string(),
                member_did: Some(owner_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(owner_dir.path())
            .unwrap()
            .unwrap();
        let owner_token =
            crate::room_service::pairing_status(owner_dir.path(), &owner_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let guest_request = crate::room_service::request_pairing(
            guest_dir.path(),
            crate::room_service::PairRequestInput {
                display_name: "Guest".to_string(),
                device_label: "Jetson".to_string(),
                member_did: Some(guest_did.clone()),
            },
        )
        .unwrap();
        crate::room_service::approve_next_request(guest_dir.path())
            .unwrap()
            .unwrap();
        let guest_token =
            crate::room_service::pairing_status(guest_dir.path(), &guest_request.request_id)
                .unwrap()
                .token
                .unwrap();

        let owner_gateway = gateway_router(test_state(owner_dir.path()));
        let guest_gateway = gateway_router(test_state(guest_dir.path()));

        let owner_first_poll = owner_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_first_poll.status(), StatusCode::OK);

        let guest_first_poll = guest_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(guest_first_poll.status(), StatusCode::OK);
        let guest_body = axum::body::to_bytes(guest_first_poll.into_body(), usize::MAX)
            .await
            .unwrap();
        let guest_poll: serde_json::Value = serde_json::from_slice(&guest_body).unwrap();
        let guest_objects = guest_poll["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(guest_objects.iter().any(|object| {
            object["kind"].as_str() == Some("system")
                && object["sender"].as_str() == Some("Owner")
                && object["body"].as_str() == Some("joined the room")
        }));
        let guest_participants = guest_poll["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(guest_participants.len(), 2);

        let owner_second_poll = owner_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_second_poll.status(), StatusCode::OK);
        let owner_body = axum::body::to_bytes(owner_second_poll.into_body(), usize::MAX)
            .await
            .unwrap();
        let owner_poll: serde_json::Value = serde_json::from_slice(&owner_body).unwrap();
        let owner_objects = owner_poll["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(owner_objects.iter().any(|object| {
            object["kind"].as_str() == Some("system")
                && object["sender"].as_str() == Some("Guest")
                && object["body"].as_str() == Some("joined the room")
        }));
        let owner_participants = owner_poll["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(owner_participants.len(), 2);

        let guest_leave = guest_gateway
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/session/leave")
                    .header(AUTHORIZATION, format!("Bearer {}", guest_token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(guest_leave.status(), StatusCode::OK);

        let owner_after_leave = owner_gateway
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/apps/room-browser/poll")
                    .header(AUTHORIZATION, format!("Bearer {}", owner_token))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"since":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_after_leave.status(), StatusCode::OK);
        let owner_after_leave_body =
            axum::body::to_bytes(owner_after_leave.into_body(), usize::MAX)
                .await
                .unwrap();
        let owner_after_leave_json: serde_json::Value =
            serde_json::from_slice(&owner_after_leave_body).unwrap();
        let owner_after_leave_objects = owner_after_leave_json["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            owner_after_leave_objects.iter().any(|object| {
                object["kind"].as_str() == Some("system")
                    && object["sender"].as_str() == Some("Guest")
                    && object["body"].as_str() == Some("left the room")
            }),
            "owner after leave poll: {owner_after_leave_json}"
        );
        let owner_after_leave_participants = owner_after_leave_json["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(owner_after_leave_participants.len(), 1);
        assert!(owner_after_leave_participants.iter().any(|participant| {
            participant["member_did"].as_str() == Some(owner_did.as_str())
                && participant["display_name"].as_str() == Some("Owner")
        }));
    }
}
