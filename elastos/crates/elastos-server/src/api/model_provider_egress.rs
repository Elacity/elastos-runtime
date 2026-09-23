//! Runtime-owned hosted effects for the confined macOS model provider.
//! The provider names an offer and effect; Runtime selects the destination,
//! checks current owner authority, and adds the stored credential.

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use elastos_runtime::provider::ProviderBridge;
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::Url;

use super::model_provider_config::{
    load_hosted_validate_fixtures, load_model_provider_operator_offers, read_hosted_egress_grants,
    read_hosted_secret,
};

#[derive(Clone, Copy)]
#[cfg_attr(test, allow(dead_code))]
pub(crate) enum ValidationEndpoint {
    OpenRouterModels,
    VeniceAccess,
    VeniceModels,
}

impl ValidationEndpoint {
    fn grant_binding(self) -> (&'static str, &'static str) {
        match self {
            Self::OpenRouterModels => ("validation:openrouter", "validate_models"),
            Self::VeniceAccess => ("validation:venice", "validate_access"),
            Self::VeniceModels => ("validation:venice", "validate_models"),
        }
    }
}

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 68;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_TIMEOUT: Duration = Duration::from_secs(3605);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    schema: String,
    grants: Vec<EgressGrant>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EgressGrant {
    offer_id: String,
    effect: String,
    method: String,
    url: String,
    recipient: String,
    payer: String,
    owner_proof_binding_id: String,
    expires_at_ms: u64,
    active: bool,
}

#[derive(Debug)]
struct EffectRequest {
    offer_id: String,
    effect: String,
    run_id: String,
    request_id: String,
    job_id: Option<String>,
    method: reqwest::Method,
    body: Vec<u8>,
}

struct Destination {
    url: Url,
    grant_url: String,
    credential: Option<String>,
}

pub(crate) async fn fetch_validation(
    data_dir: &Path,
    endpoint: ValidationEndpoint,
    api_key: &str,
) -> anyhow::Result<(reqwest::StatusCode, Vec<u8>)> {
    tokio::time::timeout(
        Duration::from_secs(20),
        fetch_validation_inner(data_dir, endpoint, api_key),
    )
    .await?
}

async fn fetch_validation_inner(
    data_dir: &Path,
    endpoint: ValidationEndpoint,
    api_key: &str,
) -> anyhow::Result<(reqwest::StatusCode, Vec<u8>)> {
    if api_key.trim().is_empty() || api_key.len() > 8192 {
        anyhow::bail!("hosted validation key unavailable");
    }
    let fixtures = load_hosted_validate_fixtures(data_dir)?;
    let raw = match endpoint {
        ValidationEndpoint::OpenRouterModels => fixtures
            .as_ref()
            .map(|value| value.openrouter_models_url.as_str())
            .unwrap_or("https://openrouter.ai/api/v1/models?output_modalities=all"),
        ValidationEndpoint::VeniceAccess => fixtures
            .as_ref()
            .map(|value| value.venice_rate_limits_url.as_str())
            .unwrap_or("https://api.venice.ai/api/v1/api_keys/rate_limits"),
        ValidationEndpoint::VeniceModels => fixtures
            .as_ref()
            .map(|value| value.venice_models_url.as_str())
            .unwrap_or("https://api.venice.ai/api/v1/models?type=text"),
    };
    let destination = Destination {
        url: Url::parse(raw)?,
        grant_url: raw.to_string(),
        credential: Some(api_key.to_string()),
    };
    let (offer_id, effect) = endpoint.grant_binding();
    current_grant_for(data_dir, offer_id, effect, "GET", &destination)?;
    let client = pinned_client(data_dir, &destination.url).await?;
    current_grant_for(data_dir, offer_id, effect, "GET", &destination)?;
    let mut response = client
        .get(destination.url.clone())
        .bearer_auth(api_key)
        .send()
        .await?;
    if response.status().is_redirection() {
        anyhow::bail!("hosted validation redirect denied");
    }
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        current_grant_for(data_dir, offer_id, effect, "GET", &destination)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            anyhow::bail!("hosted validation response too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((status, bytes))
}

pub fn start(
    listener: UnixListener,
    bridge: Arc<ProviderBridge>,
    data_dir: PathBuf,
) -> io::Result<()> {
    let (provider_pid, _) = bridge
        .confined_model_identity()
        .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "model identity changed"))?;
    tokio::spawn(async move {
        let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        let mut monitor = tokio::time::interval(Duration::from_secs(1));
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { break };
                    let Ok(permit) = permits.clone().try_acquire_owned() else { continue };
                    let bridge = bridge.clone();
                    let data_dir = data_dir.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let _ = tokio::time::timeout(
                            STREAM_TIMEOUT,
                            handle(stream, &bridge, provider_pid, &data_dir),
                        ).await;
                    });
                }
                _ = monitor.tick() => {
                    if bridge.confined_model_identity().map(|(pid, _)| pid) != Some(provider_pid) {
                        break;
                    }
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    });
    Ok(())
}

async fn handle(
    mut stream: UnixStream,
    bridge: &ProviderBridge,
    provider_pid: u32,
    data_dir: &Path,
) -> io::Result<()> {
    if peer_pid(&stream)? != provider_pid
        || bridge.confined_model_identity().map(|(pid, _)| pid) != Some(provider_pid)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "wrong model client",
        ));
    }
    let request = match tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(request)) => request,
        _ => return deny(&mut stream).await,
    };
    let destination = match resolve_effect(data_dir, &request) {
        Ok(destination) => destination,
        Err(_) => return deny(&mut stream).await,
    };
    if current_grant(data_dir, &request, &destination).is_err() {
        return deny(&mut stream).await;
    }
    let result = {
        let mut monitor = tokio::time::interval(Duration::from_millis(250));
        let outbound = forward(&mut stream, data_dir, &request, &destination);
        tokio::pin!(outbound);
        loop {
            tokio::select! {
                result = &mut outbound => break result,
                _ = monitor.tick() => {
                    if bridge.confined_model_identity().map(|(pid, _)| pid) != Some(provider_pid)
                        || current_grant(data_dir, &request, &destination).is_err()
                    {
                        break Err(io::Error::new(io::ErrorKind::PermissionDenied, "hosted authority ended"));
                    }
                }
            }
        }
    };
    if result.is_err() {
        let _ = stream.shutdown().await;
    }
    result
}

async fn read_request(stream: &mut UnixStream) -> io::Result<EffectRequest> {
    let mut header = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() >= MAX_HEADERS {
            return Err(invalid_request());
        }
        stream.read_exact(&mut byte).await?;
        header.push(byte[0]);
    }
    let header = std::str::from_utf8(&header).map_err(|_| invalid_request())?;
    let mut lines = header.split("\r\n");
    let method = match lines.next() {
        Some("POST /v1/hosted-effect HTTP/1.1") => reqwest::Method::POST,
        Some("GET /v1/hosted-effect HTTP/1.1") => reqwest::Method::GET,
        _ => return Err(invalid_request()),
    };
    let mut fields = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(invalid_request)?;
        let name = name.to_ascii_lowercase();
        if !matches!(
            name.as_str(),
            "host"
                | "accept"
                | "accept-encoding"
                | "user-agent"
                | "content-type"
                | "content-length"
                | "x-elastos-offer-id"
                | "x-elastos-effect"
                | "x-elastos-run-id"
                | "x-elastos-request-id"
                | "x-elastos-job-id"
        ) || fields.insert(name, value.trim().to_string()).is_some()
        {
            return Err(invalid_request());
        }
    }
    if fields.get("host").map(String::as_str) != Some("runtime.invalid") {
        return Err(invalid_request());
    }
    let length = fields
        .get("content-length")
        .map(|value| value.parse::<usize>().map_err(|_| invalid_request()))
        .transpose()?
        .unwrap_or(0);
    if length > MAX_BODY || (method == reqwest::Method::GET && length != 0) {
        return Err(invalid_request());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    let field = |name: &str| -> io::Result<String> {
        let value = fields.get(name).ok_or_else(invalid_request)?;
        if value.is_empty()
            || value.len() > 256
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(invalid_request());
        }
        Ok(value.clone())
    };
    let effect = field("x-elastos-effect")?;
    if !matches!(
        effect.as_str(),
        "text" | "responses" | "decisions" | "job_create" | "job_status" | "job_cancel"
    ) || (method == reqwest::Method::GET) != (effect == "job_status")
    {
        return Err(invalid_request());
    }
    if method == reqwest::Method::POST
        && serde_json::from_slice::<serde_json::Value>(&body).is_err()
    {
        return Err(invalid_request());
    }
    let job_id = fields.get("x-elastos-job-id").cloned();
    if effect == "job_status" && !job_id.as_deref().is_some_and(valid_job_id) {
        return Err(invalid_request());
    }
    Ok(EffectRequest {
        offer_id: field("x-elastos-offer-id")?,
        effect,
        run_id: field("x-elastos-run-id")?,
        request_id: field("x-elastos-request-id")?,
        job_id,
        method,
        body,
    })
}

fn resolve_effect(data_dir: &Path, request: &EffectRequest) -> anyhow::Result<Destination> {
    elastos_model_contract::validate_run_id(&request.run_id)
        .map_err(|_| anyhow::anyhow!("invalid hosted run binding"))?;
    let offers = load_model_provider_operator_offers(data_dir)?;
    let offer = offers
        .iter()
        .find(|offer| offer["id"] == request.offer_id && offer["enabled"] != false)
        .ok_or_else(|| anyhow::anyhow!("hosted offer unavailable"))?;
    let adapter = &offer["adapter"];
    let (raw_url, credential) = match request.effect.as_str() {
        "text" | "responses" | "decisions" => {
            let kind = adapter["kind"].as_str().unwrap_or_default();
            if !matches!(
                (request.effect.as_str(), kind),
                ("text", "open_ai_compatible_text")
                    | ("responses", "open_ai_responses_text")
                    | ("decisions", "open_router_decisions")
            ) {
                anyhow::bail!("hosted effect differs from offer");
            }
            let body: serde_json::Value = serde_json::from_slice(&request.body)?;
            if body["model"] != adapter["model"] {
                anyhow::bail!("hosted model differs from offer");
            }
            let mut url = adapter["api_url"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("hosted URL unavailable"))?
                .to_string();
            if let Some(fixtures) = load_hosted_validate_fixtures(data_dir)? {
                url = match adapter["hosted"]["backend_provider_label"].as_str() {
                    Some("OpenRouter") => fixtures
                        .openrouter_chat_url
                        .as_ref()
                        .map(|v| v.as_str().to_string())
                        .unwrap_or(url),
                    Some("Venice") => fixtures
                        .venice_chat_url
                        .as_ref()
                        .map(|v| v.as_str().to_string())
                        .unwrap_or(url),
                    _ => url,
                };
            }
            (url, read_hosted_secret(data_dir, &request.offer_id)?)
        }
        "job_create" | "job_status" | "job_cancel" => {
            if adapter["kind"] != "http_job_artifact" {
                anyhow::bail!("job effect differs from offer");
            }
            if request.effect == "job_create" {
                let body: serde_json::Value = serde_json::from_slice(&request.body)?;
                if body["offer_id"] != request.offer_id || body["request_id"] != request.request_id
                {
                    anyhow::bail!("job request binding differs from offer");
                }
            }
            let field = match request.effect.as_str() {
                "job_create" => "create_url",
                "job_status" => "status_url",
                _ => "cancel_url",
            };
            let url = adapter[field]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("job URL unavailable"))?
                .to_string();
            let credential = adapter["bearer_token"].as_str().map(ToOwned::to_owned);
            (url, credential)
        }
        _ => anyhow::bail!("effect unavailable"),
    };
    let grant_url = raw_url.clone();
    let mut url = Url::parse(&raw_url)?;
    if request.effect == "job_status" {
        let job_id = request
            .job_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("job id unavailable"))?;
        url.query_pairs_mut().append_pair("job_id", job_id);
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        anyhow::bail!("hosted URL contains forbidden authority");
    }
    Ok(Destination {
        url,
        grant_url,
        credential,
    })
}

fn current_grant(
    data_dir: &Path,
    request: &EffectRequest,
    destination: &Destination,
) -> anyhow::Result<()> {
    current_grant_for(
        data_dir,
        &request.offer_id,
        &request.effect,
        request.method.as_str(),
        destination,
    )?;
    if request.request_id.is_empty() || request.request_id.len() > 256 {
        anyhow::bail!("invalid hosted request binding");
    }
    Ok(())
}

fn current_grant_for(
    data_dir: &Path,
    offer_id: &str,
    effect: &str,
    method: &str,
    destination: &Destination,
) -> anyhow::Result<()> {
    let bytes = read_hosted_egress_grants(data_dir)?
        .ok_or_else(|| anyhow::anyhow!("hosted egress paused"))?;
    let file: GrantFile = serde_json::from_slice(&bytes)?;
    if file.schema != "elastos.model.egress-grants/v1" || file.grants.len() > 128 {
        anyhow::bail!("invalid hosted egress grants");
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    let host = destination
        .url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("host unavailable"))?;
    let grant = file
        .grants
        .iter()
        .find(|grant| {
            grant.active
                && grant.offer_id == offer_id
                && grant.effect == effect
                && grant.method == method
                && grant.url == destination.grant_url
                && grant.recipient == host
                && grant.payer == "this Home"
                && !grant.owner_proof_binding_id.is_empty()
                && grant.expires_at_ms > now
        })
        .ok_or_else(|| anyhow::anyhow!("hosted egress grant unavailable"))?;
    let _ = grant;
    Ok(())
}

async fn forward(
    stream: &mut UnixStream,
    data_dir: &Path,
    request: &EffectRequest,
    destination: &Destination,
) -> io::Result<()> {
    let url = &destination.url;
    let client = match pinned_client(data_dir, url).await {
        Ok(client) => client,
        Err(_) => return deny(stream).await,
    };
    if current_grant(data_dir, request, destination).is_err() {
        return deny(stream).await;
    }
    let mut outbound = client.request(request.method.clone(), url.clone());
    if let Some(credential) = destination.credential.as_deref() {
        outbound = outbound.bearer_auth(credential);
    }
    if request.method == reqwest::Method::POST {
        outbound = outbound
            .header("content-type", "application/json")
            .body(request.body.clone());
    }
    let mut response = match outbound.send().await {
        Ok(response) if !response.status().is_redirection() => response,
        _ => return unavailable(stream).await,
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ')
        })
        .unwrap_or("application/json");
    stream.write_all(format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        status.as_u16(), status.canonical_reason().unwrap_or("Response"), content_type,
    ).as_bytes()).await?;
    let mut total = 0usize;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let chunk = tokio::select! {
            _ = tick.tick() => {
                if current_grant(data_dir, request, destination).is_err() { return Err(io::Error::new(io::ErrorKind::PermissionDenied, "hosted grant ended")); }
                continue;
            }
            chunk = response.chunk() => chunk.map_err(io::Error::other)?,
        };
        let Some(chunk) = chunk else { break };
        total = total.saturating_add(chunk.len());
        if total > MAX_BODY || current_grant(data_dir, request, destination).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "hosted stream ended",
            ));
        }
        stream
            .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(&chunk).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    Ok(())
}

async fn pinned_client(data_dir: &Path, url: &Url) -> io::Result<reqwest::Client> {
    let fixtures = load_hosted_validate_fixtures(data_dir).map_err(io::Error::other)?;
    let fixture = fixtures.as_ref().is_some_and(|fixtures| {
        [
            Some(fixtures.openrouter_models_url.as_str()),
            Some(fixtures.venice_rate_limits_url.as_str()),
            Some(fixtures.venice_models_url.as_str()),
            fixtures
                .openrouter_chat_url
                .as_ref()
                .map(|url| url.as_str()),
            fixtures.venice_chat_url.as_ref().map(|url| url.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(|pinned| pinned == url.as_str())
    }) && url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1");
    // Public destinations stay paused until owner grant UI and installed
    // revocation/replay proof cover every hosted effect.
    if !fixture {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "public hosted HTTPS remains paused",
        ));
    }
    if !fixture
        && (url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !matches!(url.host(), Some(url::Host::Domain(_))))
    {
        return Err(invalid_request());
    }
    let host = url.host_str().ok_or_else(invalid_request)?;
    let port = url.port_or_known_default().ok_or_else(invalid_request)?;
    let addresses: Vec<SocketAddr> = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::lookup_host((host, port)),
    )
    .await??
    .collect();
    if addresses.is_empty()
        || addresses.iter().any(|addr| {
            if fixture {
                addr.ip() != IpAddr::from([127, 0, 0, 1])
            } else {
                !public_ip(addr.ip())
            }
        })
    {
        return Err(invalid_request());
    }
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(5))
        .timeout(STREAM_TIMEOUT)
        .build()
        .map_err(io::Error::other)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_multicast()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || o[0] == 0
                || o[0] >= 224
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && !v.is_multicast()
                && !v.is_unique_local()
                && !v.is_unicast_link_local()
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
        }
    }
}

fn valid_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && id.trim() == id && !id.chars().any(char::is_control)
}

fn peer_pid(stream: &UnixStream) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut size,
        )
    };
    if status != 0 || pid <= 1 || size as usize != std::mem::size_of::<libc::pid_t>() {
        return Err(io::Error::last_os_error());
    }
    Ok(pid as u32)
}

fn invalid_request() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid hosted effect")
}

async fn deny(stream: &mut UnixStream) -> io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
}

async fn unavailable(stream: &mut UnixStream) -> io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::{self, OpenOptions};
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    fn write_private(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true).mode(0o600);
        options.open(path).unwrap().write_all(bytes).unwrap();
    }

    #[tokio::test]
    async fn exact_fixture_grant_routes_one_text_effect_and_revoke_denies_next_effect() {
        let dir = tempfile::tempdir().unwrap();
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = sink.local_addr().unwrap().port();
        let path = format!("http://127.0.0.1:{port}/openrouter/api/v1/chat/completions");
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![json!({
                "id":"model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "title":"Fixture",
                "operation":"text.generate",
                "input_modalities":["text/plain"],
                "output_modalities":["text/plain"],
                "enabled":true,
                "adapter":{
                    "kind":"open_ai_compatible_text",
                    "api_url":"https://openrouter.ai/api/v1/chat/completions",
                    "api_key":"fixture-secret",
                    "model":"fixture/model",
                    "hosted":{
                        "backend_provider_label":"OpenRouter",
                        "selection_mode":"pinned",
                        "privacy_policy_ref":"fixture:privacy:v1",
                        "terms_ref":"fixture:terms:v1",
                        "upstream_routing_fallback_assertion":"operator_asserted_disabled"
                    }
                }
            })],
        )
        .unwrap();
        let root = dir.path().join("providers/model-provider");
        write_private(
            &root.join("validate-fixtures.json"),
            json!({
                "openrouter_models_url":format!("http://127.0.0.1:{port}/models"),
                "venice_rate_limits_url":format!("http://127.0.0.1:{port}/limits"),
                "venice_models_url":format!("http://127.0.0.1:{port}/venice-models"),
                "openrouter_chat_url":path,
            })
            .to_string()
            .as_bytes(),
        );
        let request = EffectRequest {
            offer_id: "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            effect: "text".into(),
            run_id: format!("run:sha256:{}", "0".repeat(64)),
            request_id: "fixture-request".into(),
            job_id: None,
            method: reqwest::Method::POST,
            body: json!({"model":"fixture/model","messages":[]})
                .to_string()
                .into_bytes(),
        };
        let destination = resolve_effect(dir.path(), &request).unwrap();
        assert_eq!(destination.url.as_str(), path);
        assert!(current_grant(dir.path(), &request, &destination).is_err());
        let grant_path = root.join("egress-grants.json");
        let grant = |active| {
            json!({
                "schema":"elastos.model.egress-grants/v1",
                "grants":[{
                    "offer_id":request.offer_id,
                    "effect":"text",
                    "method":"POST",
                    "url":path,
                    "recipient":"127.0.0.1",
                    "payer":"this Home",
                    "owner_proof_binding_id":"proof:fixture-owner",
                    "expires_at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000,
                    "active":active
                }]
            })
        };
        write_private(&grant_path, grant(true).to_string().as_bytes());
        current_grant(dir.path(), &request, &destination).unwrap();
        let sink_task = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut bytes = vec![0u8; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
            String::from_utf8_lossy(&bytes[..count]).to_string()
        });
        let (mut broker_side, mut provider_side) = UnixStream::pair().unwrap();
        forward(&mut broker_side, dir.path(), &request, &destination)
            .await
            .unwrap();
        broker_side.shutdown().await.unwrap();
        let mut response = String::new();
        provider_side.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        let observed = sink_task.await.unwrap();
        assert!(observed.starts_with("POST /openrouter/api/v1/chat/completions "));
        assert!(
            observed.contains("authorization: Bearer fixture-secret")
                || observed.contains("Authorization: Bearer fixture-secret")
        );
        fs::write(&grant_path, grant(false).to_string()).unwrap();
        assert!(current_grant(dir.path(), &request, &destination).is_err());
        let mut other = EffectRequest {
            effect: "decisions".into(),
            ..request
        };
        assert!(current_grant(dir.path(), &other, &destination).is_err());
        other.effect = "text".into();
        other.offer_id = "model:hosted-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        assert!(current_grant(dir.path(), &other, &destination).is_err());
    }

    #[tokio::test]
    async fn validation_uses_the_same_exact_grant_and_stops_after_revoke() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let root = dir.path().join("providers/model-provider");
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = sink.local_addr().unwrap().port();
        let models_url = format!("http://127.0.0.1:{port}/models");
        write_private(
            &root.join("validate-fixtures.json"),
            json!({
                "openrouter_models_url":models_url,
                "venice_rate_limits_url":format!("http://127.0.0.1:{port}/limits"),
                "venice_models_url":format!("http://127.0.0.1:{port}/venice-models")
            })
            .to_string()
            .as_bytes(),
        );
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key"
        )
        .await
        .is_err());
        let grant_path = root.join("egress-grants.json");
        let grant = |active| {
            json!({
                "schema":"elastos.model.egress-grants/v1",
                "grants":[{
                    "offer_id":"validation:openrouter",
                    "effect":"validate_models",
                    "method":"GET",
                    "url":models_url,
                    "recipient":"127.0.0.1",
                    "payer":"this Home",
                    "owner_proof_binding_id":"proof:fixture-owner",
                    "expires_at_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 60_000,
                    "active":active
                }]
            })
        };
        write_private(&grant_path, grant(true).to_string().as_bytes());
        let sink_task = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut bytes = vec![0u8; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.unwrap();
            String::from_utf8_lossy(&bytes[..count]).to_string()
        });
        let (status, body) = fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
        )
        .await
        .unwrap();
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(body, br#"{"data":[]}"#);
        let observed = sink_task.await.unwrap();
        assert!(observed.starts_with("GET /models "));
        assert!(
            observed.contains("authorization: Bearer fixture-key")
                || observed.contains("Authorization: Bearer fixture-key")
        );
        fs::write(&grant_path, grant(false).to_string()).unwrap();
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key"
        )
        .await
        .is_err());
    }

    #[test]
    fn public_address_filter_denies_local_and_special_ranges() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.1.1",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "::1",
            "fe80::1",
            "2001:db8::1",
            "2002::1",
        ] {
            assert!(!public_ip(address.parse().unwrap()), "{address}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
