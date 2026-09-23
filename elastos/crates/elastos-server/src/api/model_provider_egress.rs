//! Runtime-owned hosted effects for the confined macOS model provider.
//! The provider names an offer and effect; Runtime selects the destination,
//! checks current owner authority, and adds the stored credential.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use elastos_runtime::provider::ProviderBridge;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use url::Url;

use super::model_provider_config::{
    load_hosted_validate_fixtures, load_model_provider_operator_offers, read_hosted_egress_grants,
    read_hosted_job_bindings, read_hosted_secret, write_hosted_egress_grants,
    write_hosted_job_bindings,
};
use super::model_provider_egress_decision::{self, EgressScope};

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

    fn provider(self) -> &'static str {
        match self {
            Self::OpenRouterModels => "OpenRouter",
            Self::VeniceAccess | Self::VeniceModels => "Venice",
        }
    }
}

const MAX_HEADERS: usize = 16 * 1024;
const MAX_BODY: usize = 4 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 68;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_TIMEOUT: Duration = Duration::from_secs(3605);
const MAX_JOB_BINDINGS: usize = 4096;
const MAX_GRANTS: usize = 1024;
const MAX_JOB_BINDINGS_BYTES: usize = 4 * 1024 * 1024;
const JOB_BINDING_LIFETIME_MS: u64 = 2 * 60 * 60 * 1000;
type JobCreateKey = (String, String, String);
static JOB_CREATES: OnceLock<Mutex<HashSet<JobCreateKey>>> = OnceLock::new();
static EGRESS_GRANTS: OnceLock<Mutex<()>> = OnceLock::new();

struct JobCreateReservation(JobCreateKey);

impl Drop for JobCreateReservation {
    fn drop(&mut self) {
        job_creates()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.0);
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobBindingsFile {
    schema: String,
    bindings: Vec<JobBinding>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JobBinding {
    offer_id: String,
    run_id: String,
    request_id: String,
    // None means create may have reached upstream, but Runtime has no result.
    // Keep that state across restart so an identical request cannot dispatch twice.
    #[serde(default)]
    job_id: Option<String>,
    backend_id: String,
    recorded_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GrantFile {
    schema: String,
    grants: Vec<EgressGrant>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EgressGrant {
    decision_id: String,
    offer_id: String,
    effect: String,
    method: String,
    url: String,
    configuration_id: String,
    recipient: String,
    payer: String,
    owner_proof_binding_id: String,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    expires_at_ms: u64,
    active: bool,
}

#[derive(Debug, Clone)]
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
    job_backend_id: Option<String>,
    provider: String,
    configuration_id: String,
    fixture_ca_pem: Option<String>,
}

fn configuration_id_with_fixture_ca(base: String, ca_pem: Option<&str>) -> String {
    match ca_pem {
        Some(pem) => hex::encode(Sha256::digest(
            serde_json::to_vec(&(base, hex::encode(Sha256::digest(pem.as_bytes()))))
                .expect("configuration identity is serializable"),
        )),
        None => base,
    }
}

pub(crate) async fn fetch_validation(
    data_dir: &Path,
    endpoint: ValidationEndpoint,
    api_key: &str,
    owner_proof_binding_id: &str,
) -> anyhow::Result<(reqwest::StatusCode, Vec<u8>)> {
    tokio::time::timeout(
        Duration::from_secs(20),
        fetch_validation_inner(data_dir, endpoint, api_key, owner_proof_binding_id),
    )
    .await?
}

async fn fetch_validation_inner(
    data_dir: &Path,
    endpoint: ValidationEndpoint,
    api_key: &str,
    owner_proof_binding_id: &str,
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
    let url = Url::parse(raw)?;
    let fixture_ca_pem = if url.scheme() == "https" && url.host_str() == Some("127.0.0.1") {
        fixtures
            .as_ref()
            .and_then(|value| value.loopback_ca_pem.clone())
    } else {
        None
    };
    let destination = Destination {
        url,
        grant_url: raw.to_string(),
        credential: Some(api_key.to_string()),
        job_backend_id: None,
        provider: endpoint.provider().to_string(),
        configuration_id: configuration_id_with_fixture_ca(
            hex::encode(Sha256::digest(serde_json::to_vec(&(
                raw,
                api_key,
                endpoint.grant_binding(),
            ))?)),
            fixture_ca_pem.as_deref(),
        ),
        fixture_ca_pem,
    };
    anyhow::ensure!(
        fixture_destination_allowed(data_dir, &destination.url, &destination.grant_url)?,
        "public hosted HTTPS remains paused"
    );
    let (offer_id, effect) = endpoint.grant_binding();
    let scope = egress_scope(offer_id, effect, "GET", &destination)?;
    if model_provider_egress_decision::active(data_dir, &scope, Some(owner_proof_binding_id))
        .is_err()
    {
        let _ =
            model_provider_egress_decision::request(data_dir, &scope, Some(owner_proof_binding_id));
        anyhow::bail!("hosted egress requires an Inbox decision");
    }
    create_grant_after_decision(
        data_dir,
        offer_id,
        effect,
        "GET",
        &destination,
        Some(owner_proof_binding_id),
        None,
    )?;
    current_grant_for(
        data_dir,
        offer_id,
        effect,
        "GET",
        &destination,
        Some(owner_proof_binding_id),
        None,
    )?;
    let client = pinned_client(
        data_dir,
        &destination.url,
        &destination.grant_url,
        destination.fixture_ca_pem.as_deref(),
    )
    .await?;
    current_grant_for(
        data_dir,
        offer_id,
        effect,
        "GET",
        &destination,
        Some(owner_proof_binding_id),
        None,
    )?;
    let send = client
        .get(destination.url.clone())
        .bearer_auth(api_key)
        .send();
    tokio::pin!(send);
    let mut monitor = tokio::time::interval(Duration::from_millis(250));
    let mut response = loop {
        tokio::select! {
            result = &mut send => break result?,
            _ = monitor.tick() => current_grant_for(
                data_dir, offer_id, effect, "GET", &destination,
                Some(owner_proof_binding_id), None,
            )?,
        }
    };
    if response.status().is_redirection() {
        anyhow::bail!("hosted validation redirect denied");
    }
    let status = response.status();
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            result = response.chunk() => result?,
            _ = monitor.tick() => {
                current_grant_for(
                    data_dir, offer_id, effect, "GET", &destination,
                    Some(owner_proof_binding_id), None,
                )?;
                continue;
            }
        };
        let Some(chunk) = chunk else { break };
        current_grant_for(
            data_dir,
            offer_id,
            effect,
            "GET",
            &destination,
            Some(owner_proof_binding_id),
            None,
        )?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            anyhow::bail!("hosted validation response too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    current_grant_for(
        data_dir,
        offer_id,
        effect,
        "GET",
        &destination,
        Some(owner_proof_binding_id),
        None,
    )?;
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
    if !bridge.confined_model_run_matches(&request.offer_id, &request.run_id, &request.request_id) {
        return deny(&mut stream).await;
    }
    if !fixture_destination_allowed(data_dir, &destination.url, &destination.grant_url)
        .unwrap_or(false)
    {
        return deny(&mut stream).await;
    }
    let scope = match egress_scope(
        &request.offer_id,
        &request.effect,
        request.method.as_str(),
        &destination,
    ) {
        Ok(scope) => scope,
        Err(_) => return deny(&mut stream).await,
    };
    if model_provider_egress_decision::active(data_dir, &scope, None).is_err() {
        let _ = model_provider_egress_decision::request(data_dir, &scope, None);
        return deny(&mut stream).await;
    }
    if create_grant_after_decision(
        data_dir,
        &request.offer_id,
        &request.effect,
        request.method.as_str(),
        &destination,
        None,
        Some((&request.run_id, &request.request_id)),
    )
    .is_err()
        || current_grant(data_dir, &request, &destination).is_err()
    {
        return deny(&mut stream).await;
    }
    let result = {
        let mut monitor = tokio::time::interval(Duration::from_millis(250));
        let outbound = forward(&mut stream, data_dir, &request, &destination, || {
            bridge.confined_model_run_matches(
                &request.offer_id,
                &request.run_id,
                &request.request_id,
            )
        });
        tokio::pin!(outbound);
        loop {
            tokio::select! {
                result = &mut outbound => break result,
                _ = monitor.tick() => {
                    if bridge.confined_model_identity().map(|(pid, _)| pid) != Some(provider_pid)
                        || !bridge.confined_model_run_matches(&request.offer_id, &request.run_id, &request.request_id)
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
    let fixtures = load_hosted_validate_fixtures(data_dir)?;
    let (raw_url, credential, job_backend_id) = match request.effect.as_str() {
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
            if let Some(fixtures) = fixtures.as_ref() {
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
            (url, read_hosted_secret(data_dir, &request.offer_id)?, None)
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
            let routes = ["create_url", "status_url", "cancel_url"]
                .map(|field| adapter[field].as_str())
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| anyhow::anyhow!("job routes unavailable"))?;
            // A credential rotation can select a different upstream account
            // even when all three URLs stay the same.
            let backend_id = hex::encode(Sha256::digest(serde_json::to_vec(&(
                routes,
                credential.as_deref(),
            ))?));
            (url, credential, Some(backend_id))
        }
        _ => anyhow::bail!("effect unavailable"),
    };
    let grant_url = raw_url.clone();
    let mut url = Url::parse(&raw_url)?;
    if matches!(request.effect.as_str(), "job_status" | "job_cancel")
        && url.query_pairs().any(|(key, _)| key == "job_id")
    {
        anyhow::bail!("job URL contains a fixed job id");
    }
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
    let provider = adapter["hosted"]["backend_provider_label"]
        .as_str()
        .filter(|label| !label.is_empty())
        .unwrap_or("Hosted model")
        .to_string();
    let base_configuration_id = hex::encode(Sha256::digest(serde_json::to_vec(&(
        adapter,
        credential.as_deref(),
    ))?));
    let fixture_ca_pem = if url.scheme() == "https" && url.host_str() == Some("127.0.0.1") {
        fixtures
            .as_ref()
            .and_then(|value| value.loopback_ca_pem.clone())
    } else {
        None
    };
    let configuration_id =
        configuration_id_with_fixture_ca(base_configuration_id, fixture_ca_pem.as_deref());
    Ok(Destination {
        url,
        grant_url,
        credential,
        job_backend_id,
        provider,
        configuration_id,
        fixture_ca_pem,
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
        None,
        Some((&request.run_id, &request.request_id)),
    )?;
    if request.request_id.is_empty() || request.request_id.len() > 256 {
        anyhow::bail!("invalid hosted request binding");
    }
    Ok(())
}

fn egress_scope(
    offer_id: &str,
    effect: &str,
    method: &str,
    destination: &Destination,
) -> anyhow::Result<EgressScope> {
    let purpose = match effect {
        "validate_models" => "Load hosted model choices",
        "validate_access" => "Check hosted key access",
        "text" | "responses" => "Send an Assistant prompt",
        "decisions" => "Ask Jev for advice",
        "job_create" => "Create a hosted job",
        "job_status" => "Read a hosted job status",
        "job_cancel" => "Cancel a hosted job",
        _ => anyhow::bail!("hosted effect unavailable"),
    };
    Ok(EgressScope {
        offer_id: offer_id.to_string(),
        effect: effect.to_string(),
        method: method.to_string(),
        url: destination.grant_url.clone(),
        origin: destination.url.origin().ascii_serialization(),
        recipient: destination
            .url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("hosted recipient unavailable"))?
            .to_string(),
        payer: "this Home".into(),
        provider: destination.provider.clone(),
        purpose: purpose.into(),
        configuration_id: destination.configuration_id.clone(),
    })
}

fn grant_file(data_dir: &Path) -> anyhow::Result<GrantFile> {
    let Some(bytes) = read_hosted_egress_grants(data_dir)? else {
        return Ok(GrantFile {
            schema: "elastos.model.egress-grants/v2".into(),
            grants: Vec::new(),
        });
    };
    let file: GrantFile = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        file.schema == "elastos.model.egress-grants/v2" && file.grants.len() <= MAX_GRANTS,
        "invalid hosted egress grants"
    );
    Ok(file)
}

fn create_grant_after_decision(
    data_dir: &Path,
    offer_id: &str,
    effect: &str,
    method: &str,
    destination: &Destination,
    current_admin_proof: Option<&str>,
    run_binding: Option<(&str, &str)>,
) -> anyhow::Result<()> {
    let scope = egress_scope(offer_id, effect, method, destination)?;
    let decision = model_provider_egress_decision::active(data_dir, &scope, current_admin_proof)?;
    let _guard = EGRESS_GRANTS
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut file = grant_file(data_dir)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64;
    file.grants
        .retain(|grant| grant.active && grant.expires_at_ms > now);
    if file.grants.iter().any(|grant| {
        grant.active
            && grant.decision_id == decision.id
            && grant.configuration_id == destination.configuration_id
            && grant.offer_id == offer_id
            && grant.effect == effect
            && grant.method == method
            && grant.url == destination.grant_url
            && grant.run_id.as_deref() == run_binding.map(|binding| binding.0)
            && grant.request_id.as_deref() == run_binding.map(|binding| binding.1)
    }) {
        return Ok(());
    }
    anyhow::ensure!(file.grants.len() < MAX_GRANTS, "hosted egress grants full");
    file.grants.push(EgressGrant {
        decision_id: decision.id,
        offer_id: offer_id.into(),
        effect: effect.into(),
        method: method.into(),
        url: destination.grant_url.clone(),
        configuration_id: destination.configuration_id.clone(),
        recipient: scope.recipient,
        payer: scope.payer,
        owner_proof_binding_id: decision.owner_proof_binding_id,
        run_id: run_binding.map(|binding| binding.0.to_string()),
        request_id: run_binding.map(|binding| binding.1.to_string()),
        expires_at_ms: decision.expires_at_ms,
        active: true,
    });
    write_hosted_egress_grants(data_dir, &serde_json::to_vec(&file)?)
}

fn current_grant_for(
    data_dir: &Path,
    offer_id: &str,
    effect: &str,
    method: &str,
    destination: &Destination,
    current_admin_proof: Option<&str>,
    run_binding: Option<(&str, &str)>,
) -> anyhow::Result<()> {
    let scope = egress_scope(offer_id, effect, method, destination)?;
    let decision = model_provider_egress_decision::active(data_dir, &scope, current_admin_proof)?;
    let file = grant_file(data_dir)?;
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
                && grant.decision_id == decision.id
                && grant.offer_id == offer_id
                && grant.effect == effect
                && grant.method == method
                && grant.url == destination.grant_url
                && grant.configuration_id == destination.configuration_id
                && grant.recipient == host
                && grant.payer == "this Home"
                && grant.owner_proof_binding_id == decision.owner_proof_binding_id
                && current_admin_proof.is_none_or(|proof| grant.owner_proof_binding_id == proof)
                && match run_binding {
                    Some((run_id, request_id)) => {
                        grant.run_id.as_deref() == Some(run_id)
                            && grant.request_id.as_deref() == Some(request_id)
                    }
                    None => grant.run_id.is_none() && grant.request_id.is_none(),
                }
                && grant.expires_at_ms > now
        })
        .ok_or_else(|| anyhow::anyhow!("hosted egress grant unavailable"))?;
    let principal =
        crate::auth::load_principal_for_proof_binding(data_dir, &grant.owner_proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    anyhow::ensure!(
        crate::auth::is_admin(&principal) && principal.proof_binding.passkey.is_some(),
        "hosted egress owner unavailable"
    );
    Ok(())
}

fn job_creates() -> &'static Mutex<HashSet<JobCreateKey>> {
    JOB_CREATES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn job_bindings(data_dir: &Path) -> anyhow::Result<JobBindingsFile> {
    let Some(bytes) = read_hosted_job_bindings(data_dir)? else {
        return Ok(JobBindingsFile {
            schema: "elastos.model.egress-job-bindings/v1".into(),
            bindings: Vec::new(),
        });
    };
    let file: JobBindingsFile = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        file.schema == "elastos.model.egress-job-bindings/v1"
            && file.bindings.len() <= MAX_JOB_BINDINGS
            && file.bindings.iter().all(|binding| {
                binding.job_id.as_deref().is_none_or(valid_job_id)
                    && binding.backend_id.len() == 64
                    && binding
                        .backend_id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
            }),
        "invalid hosted job bindings"
    );
    let mut seen = HashSet::new();
    anyhow::ensure!(
        file.bindings.iter().all(|binding| {
            seen.insert((
                binding.offer_id.as_str(),
                binding.run_id.as_str(),
                binding.request_id.as_str(),
            ))
        }),
        "duplicate hosted job binding"
    );
    Ok(file)
}

fn job_binding_time_ms() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

fn job_binding_id(request: &EffectRequest) -> anyhow::Result<String> {
    match request.effect.as_str() {
        "job_status" => request
            .job_id
            .as_deref()
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("job id unavailable")),
        "job_cancel" => {
            let body: serde_json::Value = serde_json::from_slice(&request.body)?;
            body.get("job_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("job id unavailable"))
        }
        _ => anyhow::bail!("job binding unavailable"),
    }
}

fn current_job_binding(
    data_dir: &Path,
    request: &EffectRequest,
    backend_id: &str,
) -> anyhow::Result<()> {
    let job_id = job_binding_id(request)?;
    anyhow::ensure!(valid_job_id(&job_id), "invalid hosted job id");
    let now = job_binding_time_ms()?;
    let file = job_bindings(data_dir)?;
    anyhow::ensure!(
        file.bindings.iter().any(|binding| {
            binding.offer_id == request.offer_id
                && binding.run_id == request.run_id
                && binding.request_id == request.request_id
                && binding.job_id.as_deref() == Some(job_id.as_str())
                && binding.backend_id == backend_id
                && binding.recorded_at_ms <= now
                && now - binding.recorded_at_ms <= JOB_BINDING_LIFETIME_MS
        }),
        "hosted job result unavailable"
    );
    Ok(())
}

fn recorded_job_create(
    data_dir: &Path,
    request: &EffectRequest,
    backend_id: &str,
) -> anyhow::Result<Option<String>> {
    let now = job_binding_time_ms()?;
    let file = job_bindings(data_dir)?;
    let Some(binding) = file.bindings.iter().find(|binding| {
        binding.offer_id == request.offer_id
            && binding.run_id == request.run_id
            && binding.request_id == request.request_id
    }) else {
        return Ok(None);
    };
    let job_id = binding
        .job_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("hosted job create settlement unknown"))?;
    anyhow::ensure!(
        binding.recorded_at_ms <= now
            && now - binding.recorded_at_ms <= JOB_BINDING_LIFETIME_MS
            && binding.backend_id == backend_id,
        "hosted job result expired or backend changed"
    );
    Ok(Some(job_id.to_string()))
}

fn job_create_has_capacity(
    data_dir: &Path,
    request: &EffectRequest,
    pending: usize,
) -> anyhow::Result<()> {
    let now = job_binding_time_ms()?;
    let mut file = job_bindings(data_dir)?;
    anyhow::ensure!(
        !file.bindings.iter().any(|binding| {
            binding.offer_id == request.offer_id
                && binding.run_id == request.run_id
                && binding.request_id == request.request_id
        }),
        "hosted job create already recorded"
    );
    // Unknown attempts stay until upstream settlement can be reconciled. The
    // bounded journal fails closed rather than allowing a duplicate create.
    file.bindings.retain(|binding| {
        binding.job_id.is_none()
            || (binding.recorded_at_ms <= now
                && now - binding.recorded_at_ms <= JOB_BINDING_LIFETIME_MS)
    });
    anyhow::ensure!(
        file.bindings.len().saturating_add(pending) < MAX_JOB_BINDINGS,
        "hosted job bindings full"
    );
    // All incoming identifiers are bounded to 256 bytes and job IDs to 512.
    anyhow::ensure!(
        serde_json::to_vec(&file)?
            .len()
            .saturating_add(1600usize.saturating_mul(pending.saturating_add(1)))
            <= MAX_JOB_BINDINGS_BYTES,
        "hosted job bindings full"
    );
    Ok(())
}

fn reserve_job_create(
    data_dir: &Path,
    request: &EffectRequest,
) -> anyhow::Result<JobCreateReservation> {
    let key = (
        request.offer_id.clone(),
        request.run_id.clone(),
        request.request_id.clone(),
    );
    let mut pending = job_creates()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    anyhow::ensure!(
        !pending.contains(&key),
        "hosted job create already in flight"
    );
    job_create_has_capacity(data_dir, request, pending.len())?;
    pending.insert(key.clone());
    Ok(JobCreateReservation(key))
}

fn mark_job_create_attempt(
    data_dir: &Path,
    request: &EffectRequest,
    backend_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(backend_id.len() == 64, "invalid hosted job backend");
    let _pending = job_creates()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    job_create_has_capacity(data_dir, request, 0)?;
    let mut file = job_bindings(data_dir)?;
    let now = job_binding_time_ms()?;
    file.bindings.retain(|binding| {
        binding.job_id.is_none()
            || (binding.recorded_at_ms <= now
                && now - binding.recorded_at_ms <= JOB_BINDING_LIFETIME_MS)
    });
    file.bindings.push(JobBinding {
        offer_id: request.offer_id.clone(),
        run_id: request.run_id.clone(),
        request_id: request.request_id.clone(),
        job_id: None,
        backend_id: backend_id.to_string(),
        recorded_at_ms: now,
    });
    write_hosted_job_bindings(data_dir, &serde_json::to_vec(&file)?)
}

fn record_job_create(
    data_dir: &Path,
    request: &EffectRequest,
    backend_id: &str,
    body: &[u8],
) -> anyhow::Result<()> {
    let response: serde_json::Value = serde_json::from_slice(body)?;
    let job_id = response
        .get("job_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("hosted create result has no job id"))?;
    anyhow::ensure!(valid_job_id(job_id), "invalid hosted create job id");
    anyhow::ensure!(backend_id.len() == 64, "invalid hosted job backend");
    let _pending = job_creates()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut file = job_bindings(data_dir)?;
    let now = job_binding_time_ms()?;
    let binding = file
        .bindings
        .iter_mut()
        .find(|binding| {
            binding.offer_id == request.offer_id
                && binding.run_id == request.run_id
                && binding.request_id == request.request_id
        })
        .ok_or_else(|| anyhow::anyhow!("hosted job create attempt unavailable"))?;
    anyhow::ensure!(
        binding.job_id.is_none() && binding.backend_id == backend_id,
        "hosted job create attempt changed"
    );
    binding.job_id = Some(job_id.to_string());
    binding.recorded_at_ms = now;
    write_hosted_job_bindings(data_dir, &serde_json::to_vec(&file)?)
}

async fn forward(
    stream: &mut UnixStream,
    data_dir: &Path,
    request: &EffectRequest,
    destination: &Destination,
    run_authorized: impl Fn() -> bool,
) -> io::Result<()> {
    let backend_id = destination.job_backend_id.as_deref().unwrap_or_default();
    let _create_reservation = if request.effect == "job_create" {
        if backend_id.is_empty()
            || !run_authorized()
            || current_grant(data_dir, request, destination).is_err()
        {
            return deny(stream).await;
        }
        match recorded_job_create(data_dir, request, backend_id) {
            Ok(Some(job_id)) => return write_job_replay(stream, &job_id).await,
            Ok(None) => {}
            Err(_) => return deny(stream).await,
        }
        match reserve_job_create(data_dir, request) {
            Ok(reservation) => Some(reservation),
            Err(_) => return deny(stream).await,
        }
    } else {
        None
    };
    if matches!(request.effect.as_str(), "job_status" | "job_cancel")
        && current_job_binding(data_dir, request, backend_id).is_err()
    {
        return deny(stream).await;
    }
    let url = &destination.url;
    let client = match pinned_client(
        data_dir,
        url,
        &destination.grant_url,
        destination.fixture_ca_pem.as_deref(),
    )
    .await
    {
        Ok(client) => client,
        Err(_) => return deny(stream).await,
    };
    if !run_authorized()
        || current_grant(data_dir, request, destination).is_err()
        || (matches!(request.effect.as_str(), "job_status" | "job_cancel")
            && current_job_binding(data_dir, request, backend_id).is_err())
    {
        return deny(stream).await;
    }
    let mut outbound = client.request(request.method.clone(), url.clone());
    if let Some(credential) = destination.credential.as_deref() {
        outbound = outbound.bearer_auth(credential);
    }
    if request.method == reqwest::Method::POST {
        // Forward the same JSON meaning that Runtime checked. Duplicate keys
        // in the provider's raw body must not select a different upstream ID.
        let body: serde_json::Value = match serde_json::from_slice(&request.body) {
            Ok(body) => body,
            Err(_) => return deny(stream).await,
        };
        outbound = outbound
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).map_err(io::Error::other)?);
    }
    if request.effect == "job_create"
        && mark_job_create_attempt(data_dir, request, backend_id).is_err()
    {
        return deny(stream).await;
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
        .unwrap_or("application/json")
        .to_string();
    if request.effect == "job_create" && status.is_success() {
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(io::Error::other)? {
            if body.len().saturating_add(chunk.len()) > MAX_BODY
                || !run_authorized()
                || current_grant(data_dir, request, destination).is_err()
            {
                return unavailable(stream).await;
            }
            body.extend_from_slice(&chunk);
        }
        if !run_authorized()
            || current_grant(data_dir, request, destination).is_err()
            || record_job_create(data_dir, request, backend_id, &body).is_err()
        {
            return unavailable(stream).await;
        }
        write_response_head(stream, status, &content_type).await?;
        stream
            .write_all(format!("{:X}\r\n", body.len()).as_bytes())
            .await?;
        stream.write_all(&body).await?;
        stream.write_all(b"\r\n0\r\n\r\n").await?;
        return Ok(());
    }
    write_response_head(stream, status, &content_type).await?;
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

async fn write_job_replay(stream: &mut UnixStream, job_id: &str) -> io::Result<()> {
    let body = serde_json::json!({"job_id": job_id}).to_string();
    write_response_head(stream, reqwest::StatusCode::OK, "application/json").await?;
    stream
        .write_all(format!("{:X}\r\n{body}\r\n0\r\n\r\n", body.len()).as_bytes())
        .await
}

async fn write_response_head(
    stream: &mut UnixStream,
    status: reqwest::StatusCode,
    content_type: &str,
) -> io::Result<()> {
    stream.write_all(format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        status.as_u16(), status.canonical_reason().unwrap_or("Response"), content_type,
    ).as_bytes()).await
}

async fn pinned_client(
    data_dir: &Path,
    url: &Url,
    grant_url: &str,
    ca_pem: Option<&str>,
) -> io::Result<reqwest::Client> {
    let fixture =
        fixture_destination_allowed(data_dir, url, grant_url).map_err(io::Error::other)?;
    // Public destinations stay paused until owner grant UI and installed
    // revocation/replay proof cover every hosted effect.
    if !fixture {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "public hosted HTTPS remains paused",
        ));
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
        || addresses
            .iter()
            .any(|addr| addr.ip() != IpAddr::from([127, 0, 0, 1]))
    {
        return Err(invalid_request());
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(5))
        .timeout(STREAM_TIMEOUT);
    if url.scheme() == "https" {
        let pem = ca_pem.ok_or_else(invalid_request)?;
        let certificate = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid fixture CA"))?;
        builder = builder
            .tls_built_in_root_certs(false)
            .add_root_certificate(certificate);
    }
    builder.build().map_err(io::Error::other)
}

fn fixture_destination_allowed(
    data_dir: &Path,
    url: &Url,
    grant_url: &str,
) -> anyhow::Result<bool> {
    let fixtures = load_hosted_validate_fixtures(data_dir)?;
    Ok(fixtures.as_ref().is_some_and(|fixtures| {
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
        // Status adds the Runtime-bound job_id after resolving the configured
        // route. The private fixture pins that route, before the added query.
        .any(|pinned| pinned == grant_url)
    }) && matches!(url.scheme(), "http" | "https")
        && url.host_str() == Some("127.0.0.1"))
}

#[cfg(test)]
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

    fn admin_proof(dir: &Path) -> String {
        let mut identity = elastos_identity::IdentityManager::new(dir.to_path_buf()).unwrap();
        let secret = crate::auth::random_secret_hex();
        let owner = crate::auth::OwnerAdmission {
            origin: "http://localhost:61971",
            rp_id: "localhost",
            claimant: &secret,
            loopback: true,
        };
        let (_, options) = crate::auth::begin_owner_enrollment(
            dir,
            &mut identity,
            &owner,
            "owner",
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        let attestation = crate::auth::owner_attestation_for_test(
            &options.unwrap().public_key.challenge,
            owner.rp_id,
            owner.origin,
        );
        crate::auth::complete_owner_enrollment(
            dir,
            &mut identity,
            &owner,
            "owner",
            Some(&attestation),
            None,
            crate::auth::now_ts(),
        )
        .unwrap()
        .unwrap();
        crate::auth::active_passkey_principals(dir)
            .unwrap()
            .into_iter()
            .find(crate::auth::is_admin)
            .unwrap()
            .proof_binding_id
    }

    fn approve_fixture(
        data_dir: &Path,
        offer_id: &str,
        effect: &str,
        method: &str,
        destination: &Destination,
        proof: &str,
        run_binding: Option<(&str, &str)>,
    ) -> String {
        let scope = egress_scope(offer_id, effect, method, destination).unwrap();
        let request_proof = run_binding.is_none().then_some(proof);
        let id = model_provider_egress_decision::request(data_dir, &scope, request_proof).unwrap();
        model_provider_egress_decision::approve(data_dir, &id, proof).unwrap();
        create_grant_after_decision(
            data_dir,
            offer_id,
            effect,
            method,
            destination,
            request_proof,
            run_binding,
        )
        .unwrap();
        id
    }

    fn update_fixture_grant(data_dir: &Path, change: impl FnOnce(&mut serde_json::Value)) {
        let path = data_dir.join("providers/model-provider/egress-grants.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut value);
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn validation_fixture(url: &str, key: &str, endpoint: ValidationEndpoint) -> Destination {
        Destination {
            url: Url::parse(url).unwrap(),
            grant_url: url.into(),
            credential: Some(key.into()),
            job_backend_id: None,
            provider: endpoint.provider().into(),
            configuration_id: hex::encode(Sha256::digest(
                serde_json::to_vec(&(url, key, endpoint.grant_binding())).unwrap(),
            )),
            fixture_ca_pem: None,
        }
    }

    #[test]
    fn fixture_ca_rotation_requires_a_new_exact_decision() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let proof = admin_proof(dir.path());
        let mut destination = validation_fixture(
            "https://127.0.0.1:44321/models",
            "fixture-key",
            ValidationEndpoint::OpenRouterModels,
        );
        let base = destination.configuration_id.clone();
        destination.configuration_id =
            configuration_id_with_fixture_ca(base.clone(), Some("ca-one"));
        let original = egress_scope(
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
        )
        .unwrap();
        let id =
            model_provider_egress_decision::request(dir.path(), &original, Some(&proof)).unwrap();
        model_provider_egress_decision::approve(dir.path(), &id, &proof).unwrap();
        assert!(
            model_provider_egress_decision::active(dir.path(), &original, Some(&proof)).is_ok()
        );
        destination.configuration_id = configuration_id_with_fixture_ca(base, Some("ca-two"));
        let rotated = egress_scope(
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
        )
        .unwrap();
        assert!(
            model_provider_egress_decision::active(dir.path(), &rotated, Some(&proof)).is_err()
        );
        assert_ne!(
            model_provider_egress_decision::request(dir.path(), &rotated, Some(&proof)).unwrap(),
            id
        );
    }

    #[test]
    fn owner_inbox_decision_precedes_exact_grant_and_end_stays_closed() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let proof = admin_proof(dir.path());
        let destination = validation_fixture(
            "http://127.0.0.1:9999/models",
            "fixture-key",
            ValidationEndpoint::OpenRouterModels,
        );
        let scope = egress_scope(
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
        )
        .unwrap();
        let id = model_provider_egress_decision::request(dir.path(), &scope, Some(&proof)).unwrap();
        assert!(crate::notifications::load_summary(dir.path())
            .unwrap()
            .entries
            .is_empty());
        let history = model_provider_egress_decision::inbox_history(dir.path()).unwrap();
        assert_eq!(history.len(), 1);
        assert!(create_grant_after_decision(
            dir.path(),
            &scope.offer_id,
            &scope.effect,
            &scope.method,
            &destination,
            Some(&proof),
            None
        )
        .is_err());
        assert!(read_hosted_egress_grants(dir.path()).unwrap().is_none());
        assert!(
            model_provider_egress_decision::approve(dir.path(), "model-egress-stale", &proof)
                .is_err()
        );
        assert!(model_provider_egress_decision::approve(dir.path(), &id, "wrong-proof").is_err());
        model_provider_egress_decision::deny(dir.path(), &id, &proof).unwrap();
        assert!(model_provider_egress_decision::approve(dir.path(), &id, &proof).is_err());
        assert!(model_provider_egress_decision::request(dir.path(), &scope, Some(&proof)).is_err());

        let mut rotated = destination;
        rotated.configuration_id = "b".repeat(64);
        let rotated_scope =
            egress_scope("validation:openrouter", "validate_models", "GET", &rotated).unwrap();
        let approved_id =
            model_provider_egress_decision::request(dir.path(), &rotated_scope, Some(&proof))
                .unwrap();
        model_provider_egress_decision::approve(dir.path(), &approved_id, &proof).unwrap();
        create_grant_after_decision(
            dir.path(),
            &rotated_scope.offer_id,
            &rotated_scope.effect,
            &rotated_scope.method,
            &rotated,
            Some(&proof),
            None,
        )
        .unwrap();
        current_grant_for(
            dir.path(),
            &rotated_scope.offer_id,
            &rotated_scope.effect,
            &rotated_scope.method,
            &rotated,
            Some(&proof),
            None,
        )
        .unwrap();
        assert!(current_grant_for(
            dir.path(),
            &scope.offer_id,
            &scope.effect,
            &scope.method,
            &validation_fixture(
                "http://127.0.0.1:9999/models",
                "fixture-key",
                ValidationEndpoint::OpenRouterModels
            ),
            Some(&proof),
            None
        )
        .is_err());
        model_provider_egress_decision::end_offer(dir.path(), "validation:openrouter").unwrap();
        assert!(current_grant_for(
            dir.path(),
            &rotated_scope.offer_id,
            &rotated_scope.effect,
            &rotated_scope.method,
            &rotated,
            Some(&proof),
            None
        )
        .is_err());
        assert!(model_provider_egress_decision::approve(dir.path(), &approved_id, &proof).is_err());
        assert!(
            model_provider_egress_decision::request(dir.path(), &rotated_scope, Some(&proof))
                .is_err()
        );
    }

    #[test]
    fn approved_route_can_bind_more_than_128_distinct_runs() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let proof = admin_proof(dir.path());
        let destination = Destination {
            url: Url::parse("http://127.0.0.1:9999/chat").unwrap(),
            grant_url: "http://127.0.0.1:9999/chat".into(),
            credential: None,
            job_backend_id: None,
            provider: "Fixture".into(),
            configuration_id: "a".repeat(64),
            fixture_ca_pem: None,
        };
        let scope = egress_scope(
            "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "text",
            "POST",
            &destination,
        )
        .unwrap();
        let id = model_provider_egress_decision::request(dir.path(), &scope, None).unwrap();
        model_provider_egress_decision::approve(dir.path(), &id, &proof).unwrap();
        for index in 0..130 {
            let run_id = format!("run:sha256:{index:064x}");
            let request_id = format!("request-{index}");
            create_grant_after_decision(
                dir.path(),
                &scope.offer_id,
                &scope.effect,
                &scope.method,
                &destination,
                None,
                Some((&run_id, &request_id)),
            )
            .unwrap();
            current_grant_for(
                dir.path(),
                &scope.offer_id,
                &scope.effect,
                &scope.method,
                &destination,
                None,
                Some((&run_id, &request_id)),
            )
            .unwrap();
        }
        assert_eq!(grant_file(dir.path()).unwrap().grants.len(), 130);
    }

    #[tokio::test]
    async fn public_validation_stays_paused_without_creating_an_inbox_request() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let proof = admin_proof(dir.path());
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
            &proof,
        )
        .await
        .is_err());
        assert!(
            super::super::model_provider_config::read_hosted_egress_decisions(dir.path())
                .unwrap()
                .is_none()
        );
        assert!(crate::notifications::load_summary(dir.path())
            .unwrap()
            .entries
            .is_empty());
    }

    #[tokio::test]
    async fn hosted_job_effects_require_the_persisted_create_result() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let create = EffectRequest {
            offer_id: "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            effect: "job_create".into(),
            run_id: format!("run:sha256:{}", "0".repeat(64)),
            request_id: "request-1".into(),
            job_id: None,
            method: reqwest::Method::POST,
            body: json!({"offer_id":"model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","request_id":"request-1"}).to_string().into_bytes(),
        };
        let backend_id = "a".repeat(64);
        let mut status = EffectRequest {
            effect: "job_status".into(),
            job_id: Some("job-1".into()),
            method: reqwest::Method::GET,
            body: Vec::new(),
            ..create.clone()
        };
        assert!(current_job_binding(dir.path(), &status, &backend_id).is_err());
        assert!(record_job_create(dir.path(), &create, &backend_id, br#"{}"#).is_err());
        assert!(read_hosted_job_bindings(dir.path()).unwrap().is_none());
        mark_job_create_attempt(dir.path(), &create, &backend_id).unwrap();
        assert!(recorded_job_create(dir.path(), &create, &backend_id).is_err());
        record_job_create(dir.path(), &create, &backend_id, br#"{"job_id":"job-1"}"#).unwrap();
        assert!(job_create_has_capacity(dir.path(), &create, 0).is_err());
        assert_eq!(
            recorded_job_create(dir.path(), &create, &backend_id)
                .unwrap()
                .as_deref(),
            Some("job-1")
        );
        assert!(recorded_job_create(dir.path(), &create, &"b".repeat(64)).is_err());
        assert!(current_job_binding(dir.path(), &status, &backend_id).is_ok());
        assert!(current_job_binding(dir.path(), &status, &"b".repeat(64)).is_err());
        let mut cancel = EffectRequest {
            effect: "job_cancel".into(),
            job_id: None,
            method: reqwest::Method::POST,
            body: br#"{"job_id":"job-1"}"#.to_vec(),
            ..status.clone()
        };
        assert!(current_job_binding(dir.path(), &cancel, &backend_id).is_ok());
        cancel.body = br#"{"job_id":"job-other"}"#.to_vec();
        assert!(current_job_binding(dir.path(), &cancel, &backend_id).is_err());
        status = EffectRequest {
            effect: "job_status".into(),
            job_id: Some("job-other".into()),
            method: reqwest::Method::GET,
            body: Vec::new(),
            ..cancel
        };
        assert!(current_job_binding(dir.path(), &status, &backend_id).is_err());
        status.job_id = Some("job-1".into());
        status.request_id = "request-other".into();
        assert!(current_job_binding(dir.path(), &status, &backend_id).is_err());
        status.request_id = create.request_id.clone();
        status.run_id = format!("run:sha256:{}", "1".repeat(64));
        assert!(current_job_binding(dir.path(), &status, &backend_id).is_err());

        let proof = admin_proof(dir.path());
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://127.0.0.1:{}/create",
            sink.local_addr().unwrap().port()
        );
        let destination = Destination {
            url: Url::parse(&url).unwrap(),
            grant_url: url,
            credential: None,
            job_backend_id: Some(backend_id),
            provider: "Fixture".into(),
            configuration_id: "a".repeat(64),
            fixture_ca_pem: None,
        };
        approve_fixture(
            dir.path(),
            &create.offer_id,
            "job_create",
            "POST",
            &destination,
            &proof,
            Some((&create.run_id, &create.request_id)),
        );
        let (mut broker, mut provider) = UnixStream::pair().unwrap();
        forward(&mut broker, dir.path(), &create, &destination, || true)
            .await
            .unwrap();
        broker.shutdown().await.unwrap();
        let mut replay = String::new();
        provider.read_to_string(&mut replay).await.unwrap();
        assert!(replay.starts_with("HTTP/1.1 200 OK"));
        assert!(replay.contains("\"job_id\":\"job-1\""));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), sink.accept())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn hosted_job_create_with_lost_response_blocks_identical_retry() {
        let dir = tempfile::tempdir().unwrap();
        let proof = admin_proof(dir.path());
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = sink.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/create");
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let root = dir.path().join("providers/model-provider");
        write_private(
            &root.join("validate-fixtures.json"),
            json!({
                "openrouter_models_url":format!("http://127.0.0.1:{port}/models"),
                "venice_rate_limits_url":format!("http://127.0.0.1:{port}/limits"),
                "venice_models_url":format!("http://127.0.0.1:{port}/venice-models"),
                "openrouter_chat_url":url
            })
            .to_string()
            .as_bytes(),
        );
        let request = EffectRequest {
            offer_id: "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            effect: "job_create".into(),
            run_id: format!("run:sha256:{}", "0".repeat(64)),
            request_id: "request-lost".into(),
            job_id: None,
            method: reqwest::Method::POST,
            body: br#"{"request_id":"request-lost"}"#.to_vec(),
        };
        let backend_id = "a".repeat(64);
        let destination = Destination {
            url: Url::parse(&url).unwrap(),
            grant_url: url,
            credential: None,
            job_backend_id: Some(backend_id.clone()),
            provider: "Fixture".into(),
            configuration_id: "a".repeat(64),
            fixture_ca_pem: None,
        };
        approve_fixture(
            dir.path(),
            &request.offer_id,
            "job_create",
            "POST",
            &destination,
            &proof,
            Some((&request.run_id, &request.request_id)),
        );
        let sink_task = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut request_bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0 && request_bytes.len() + count <= 4096);
                request_bytes.extend_from_slice(&chunk[..count]);
                let Some(header_end) = request_bytes
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request_bytes[..header_end]);
                assert!(headers.starts_with("POST /create "));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.parse().ok())
                    })
                    .unwrap();
                if request_bytes.len() >= header_end + 4 + length {
                    assert_eq!(
                        serde_json::from_slice::<serde_json::Value>(
                            &request_bytes[header_end + 4..header_end + 4 + length]
                        )
                        .unwrap()["request_id"],
                        "request-lost"
                    );
                    break;
                }
            }
            drop(socket); // Upstream accepted create but lost its response.
            sink
        });
        let (mut broker, mut provider) = UnixStream::pair().unwrap();
        forward(&mut broker, dir.path(), &request, &destination, || true)
            .await
            .unwrap();
        broker.shutdown().await.unwrap();
        let mut first_response = String::new();
        provider.read_to_string(&mut first_response).await.unwrap();
        assert!(
            first_response.starts_with("HTTP/1.1 502"),
            "{first_response}"
        );
        let sink = sink_task.await.unwrap();
        assert!(job_bindings(dir.path()).unwrap().bindings[0]
            .job_id
            .is_none());
        assert!(recorded_job_create(dir.path(), &request, &backend_id).is_err());

        let (mut broker, mut provider) = UnixStream::pair().unwrap();
        forward(&mut broker, dir.path(), &request, &destination, || true)
            .await
            .unwrap();
        broker.shutdown().await.unwrap();
        let mut retry_response = String::new();
        provider.read_to_string(&mut retry_response).await.unwrap();
        assert!(retry_response.starts_with("HTTP/1.1 403"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), sink.accept())
                .await
                .is_err()
        );
    }

    #[test]
    fn hosted_job_backend_identity_covers_all_three_routes() {
        let dir = tempfile::tempdir().unwrap();
        let request = EffectRequest {
            offer_id: "model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            effect: "job_create".into(),
            run_id: format!("run:sha256:{}", "0".repeat(64)),
            request_id: "request-1".into(),
            job_id: None,
            method: reqwest::Method::POST,
            body: br#"{"offer_id":"model:hosted-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","request_id":"request-1"}"#.to_vec(),
        };
        let seed = |cancel_url: &str, bearer_token: &str| {
            super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
                dir.path(),
                vec![json!({
                    "id":request.offer_id,"enabled":true,
                    "adapter":{
                        "kind":"http_job_artifact",
                        "create_url":"https://jobs.example/create",
                        "status_url":"https://jobs.example/status",
                        "cancel_url":cancel_url,
                        "bearer_token":bearer_token
                    }
                })],
            )
            .unwrap();
        };
        seed("https://jobs.example/cancel", "account-a");
        let create = resolve_effect(dir.path(), &request).unwrap();
        let status = resolve_effect(
            dir.path(),
            &EffectRequest {
                effect: "job_status".into(),
                job_id: Some("job-1".into()),
                method: reqwest::Method::GET,
                body: Vec::new(),
                ..request.clone()
            },
        )
        .unwrap();
        assert_eq!(create.job_backend_id, status.job_backend_id);
        seed("https://jobs.example/cancel-v2", "account-a");
        let changed = resolve_effect(dir.path(), &request).unwrap();
        assert_ne!(create.job_backend_id, changed.job_backend_id);
        seed("https://jobs.example/cancel", "account-b");
        let changed_account = resolve_effect(dir.path(), &request).unwrap();
        assert_ne!(create.job_backend_id, changed_account.job_backend_id);
    }

    #[tokio::test]
    async fn exact_fixture_grant_routes_one_text_effect_and_revoke_denies_next_effect() {
        let dir = tempfile::tempdir().unwrap();
        let proof = admin_proof(dir.path());
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
            body: br#"{"model":"wrong/model","model":"fixture/model","messages":[]}"#.to_vec(),
        };
        let destination = resolve_effect(dir.path(), &request).unwrap();
        assert_eq!(destination.url.as_str(), path);
        assert!(current_grant(dir.path(), &request, &destination).is_err());
        approve_fixture(
            dir.path(),
            &request.offer_id,
            "text",
            "POST",
            &destination,
            &proof,
            Some((&request.run_id, &request.request_id)),
        );
        let grant_path = root.join("egress-grants.json");
        let valid_grant = fs::read(&grant_path).unwrap();
        update_fixture_grant(dir.path(), |file| {
            file["grants"][0].as_object_mut().unwrap().remove("run_id");
        });
        assert!(current_grant(dir.path(), &request, &destination).is_err());
        let (mut denied, mut denied_peer) = UnixStream::pair().unwrap();
        forward(&mut denied, dir.path(), &request, &destination, || true)
            .await
            .unwrap();
        denied.shutdown().await.unwrap();
        let mut denial = String::new();
        denied_peer.read_to_string(&mut denial).await.unwrap();
        assert!(denial.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), sink.accept())
                .await
                .is_err()
        );
        fs::write(&grant_path, &valid_grant).unwrap();
        current_grant(dir.path(), &request, &destination).unwrap();
        let (mut denied, mut denied_peer) = UnixStream::pair().unwrap();
        forward(&mut denied, dir.path(), &request, &destination, || false)
            .await
            .unwrap();
        denied.shutdown().await.unwrap();
        let mut denial = String::new();
        denied_peer.read_to_string(&mut denial).await.unwrap();
        assert!(denial.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), sink.accept())
                .await
                .is_err()
        );
        let sink_task = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut bytes = vec![0u8; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
            String::from_utf8_lossy(&bytes[..count]).to_string()
        });
        let (mut broker_side, mut provider_side) = UnixStream::pair().unwrap();
        forward(&mut broker_side, dir.path(), &request, &destination, || {
            true
        })
        .await
        .unwrap();
        broker_side.shutdown().await.unwrap();
        let mut response = String::new();
        provider_side.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        let observed = sink_task.await.unwrap();
        assert!(observed.starts_with("POST /openrouter/api/v1/chat/completions "));
        assert!(!observed.contains("wrong/model"));
        assert!(
            observed.contains("authorization: Bearer fixture-secret")
                || observed.contains("Authorization: Bearer fixture-secret")
        );
        let continued = EffectRequest {
            run_id: format!("run:sha256:{}", "1".repeat(64)),
            request_id: "fixture-continue".into(),
            ..request.clone()
        };
        assert!(current_grant(dir.path(), &continued, &destination).is_err());
        create_grant_after_decision(
            dir.path(),
            &continued.offer_id,
            "text",
            "POST",
            &destination,
            None,
            Some((&continued.run_id, &continued.request_id)),
        )
        .unwrap();
        current_grant(dir.path(), &continued, &destination).unwrap();
        model_provider_egress_decision::end_offer(dir.path(), &request.offer_id).unwrap();
        assert!(current_grant(dir.path(), &request, &destination).is_err());
        assert!(current_grant(dir.path(), &continued, &destination).is_err());
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
    async fn job_status_fixture_pins_the_configured_route_before_job_id() {
        let dir = tempfile::tempdir().unwrap();
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let root = dir.path().join("providers/model-provider");
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = sink.local_addr().unwrap().port();
        let route = format!("http://127.0.0.1:{port}/jobs/status");
        write_private(
            &root.join("validate-fixtures.json"),
            json!({
                "openrouter_models_url":route,
                "venice_rate_limits_url":format!("http://127.0.0.1:{port}/limits"),
                "venice_models_url":format!("http://127.0.0.1:{port}/models"),
            })
            .to_string()
            .as_bytes(),
        );
        let status = Url::parse(&format!("{route}?job_id=job-1")).unwrap();
        assert!(pinned_client(dir.path(), &status, &route, None)
            .await
            .is_ok());
        assert!(
            pinned_client(dir.path(), &status, &format!("{route}-other"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn validation_uses_the_same_exact_grant_and_stops_after_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let proof = admin_proof(dir.path());
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
            "fixture-key",
            &proof,
        )
        .await
        .is_err());
        let destination = validation_fixture(
            &models_url,
            "fixture-key",
            ValidationEndpoint::OpenRouterModels,
        );
        approve_fixture(
            dir.path(),
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
            &proof,
            None,
        );
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
            "proof:wrong-admin",
        )
        .await
        .is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), sink.accept())
                .await
                .is_err()
        );
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
            &proof,
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
        model_provider_egress_decision::end_offer(dir.path(), "validation:openrouter").unwrap();
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
            &proof,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn validation_redirect_never_contacts_unapproved_origin() {
        let dir = tempfile::tempdir().unwrap();
        let proof = admin_proof(dir.path());
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let approved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unapproved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = approved.local_addr().unwrap().port();
        let approved_url = format!("http://127.0.0.1:{port}/models");
        let unapproved_url = format!(
            "http://127.0.0.1:{}/stolen",
            unapproved.local_addr().unwrap().port()
        );
        let root = dir.path().join("providers/model-provider");
        write_private(
            &root.join("validate-fixtures.json"),
            json!({
                "openrouter_models_url": approved_url,
                "venice_rate_limits_url": format!("http://127.0.0.1:{port}/limits"),
                "venice_models_url": format!("http://127.0.0.1:{port}/venice-models"),
            })
            .to_string()
            .as_bytes(),
        );
        let destination = validation_fixture(
            &approved_url,
            "fixture-key",
            ValidationEndpoint::OpenRouterModels,
        );
        approve_fixture(
            dir.path(),
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
            &proof,
            None,
        );
        let approved_task = tokio::spawn(async move {
            let mut observed = Vec::new();
            let responses = [
                format!("HTTP/1.1 302 Found\r\nLocation: {unapproved_url}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes(),
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}".to_vec(),
            ];
            for response in responses {
                let (mut socket, _) = approved.accept().await.unwrap();
                let mut bytes = [0u8; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                observed.push(String::from_utf8_lossy(&bytes[..count]).to_string());
                socket.write_all(&response).await.unwrap();
            }
            observed
        });
        assert!(fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
            &proof
        )
        .await
        .is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), unapproved.accept())
                .await
                .is_err()
        );
        let (status, body) = fetch_validation(
            dir.path(),
            ValidationEndpoint::OpenRouterModels,
            "fixture-key",
            &proof,
        )
        .await
        .unwrap();
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(body, br#"{"data":[]}"#);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), unapproved.accept())
                .await
                .is_err()
        );
        let observed = approved_task.await.unwrap();
        assert_eq!(observed.len(), 2);
        assert!(observed
            .iter()
            .all(|request| request.starts_with("GET /models ")));
    }

    #[tokio::test]
    async fn active_validation_stops_while_upstream_waits_after_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let proof = admin_proof(dir.path());
        super::super::model_provider_config::seed_model_provider_operator_offers_for_test(
            dir.path(),
            vec![],
        )
        .unwrap();
        let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = sink.local_addr().unwrap().port();
        let models_url = format!("http://127.0.0.1:{port}/models");
        let root = dir.path().join("providers/model-provider");
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
        let destination = validation_fixture(
            &models_url,
            "fixture-key",
            ValidationEndpoint::OpenRouterModels,
        );
        approve_fixture(
            dir.path(),
            "validation:openrouter",
            "validate_models",
            "GET",
            &destination,
            &proof,
            None,
        );
        let (accepted, observed) = tokio::sync::oneshot::channel();
        let sink_task = tokio::spawn(async move {
            let (mut socket, _) = sink.accept().await.unwrap();
            let mut bytes = [0u8; 4096];
            let _ = socket.read(&mut bytes).await.unwrap();
            let _ = accepted.send(());
            std::future::pending::<()>().await;
        });
        let data_dir = dir.path().to_path_buf();
        let proof_for_request = proof.clone();
        let validation = tokio::spawn(async move {
            fetch_validation(
                &data_dir,
                ValidationEndpoint::OpenRouterModels,
                "fixture-key",
                &proof_for_request,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(3), observed)
            .await
            .unwrap()
            .unwrap();
        model_provider_egress_decision::end_offer(dir.path(), "validation:openrouter").unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), validation)
            .await
            .expect("revoked validation must stop before its 20-second timeout")
            .unwrap();
        assert!(result.is_err());
        sink_task.abort();
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
