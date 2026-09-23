//! Owner Inbox decisions for exact hosted HTTP destinations.
//!
//! A notification displays a pending request. This private record is the
//! authority: only an active admin passkey decision can authorize dispatch.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

use super::model_provider_config::{read_hosted_egress_decisions, write_hosted_egress_decisions};

pub(super) const APPROVE_PREFIX: &str = "model-egress-approve:";
pub(super) const DENY_PREFIX: &str = "model-egress-deny:";
const SCHEMA: &str = "elastos.model.egress-decisions/v1";
const DURATION_MS: u64 = 10 * 60 * 1000;
const MAX_DECISIONS: usize = 128;
static DECISION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(super) struct ActiveDecision {
    pub id: String,
    pub owner_proof_binding_id: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EgressScope {
    pub offer_id: String,
    pub effect: String,
    pub method: String,
    pub url: String,
    pub origin: String,
    pub recipient: String,
    pub payer: String,
    pub provider: String,
    pub purpose: String,
    pub configuration_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DecisionFile {
    schema: String,
    decisions: Vec<EgressDecision>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EgressDecision {
    id: String,
    scope: EgressScope,
    requested_by_proof: Option<String>,
    owner_proof_binding_id: Option<String>,
    requested_at_ms: u64,
    decided_at_ms: Option<u64>,
    expires_at_ms: u64,
    status: DecisionStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DecisionStatus {
    Pending,
    Approved,
    Denied,
    Ended,
}

fn lock() -> &'static Mutex<()> {
    DECISION_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

fn read(data_dir: &Path) -> anyhow::Result<DecisionFile> {
    let Some(bytes) = read_hosted_egress_decisions(data_dir)? else {
        return Ok(DecisionFile {
            schema: SCHEMA.into(),
            decisions: Vec::new(),
        });
    };
    let file: DecisionFile = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        file.schema == SCHEMA && file.decisions.len() <= MAX_DECISIONS,
        "invalid hosted egress decisions"
    );
    Ok(file)
}

fn write(data_dir: &Path, file: &DecisionFile) -> anyhow::Result<()> {
    write_hosted_egress_decisions(data_dir, &serde_json::to_vec(file)?)
}

fn validate_scope(scope: &EgressScope) -> anyhow::Result<()> {
    for field in [
        &scope.offer_id,
        &scope.effect,
        &scope.method,
        &scope.url,
        &scope.origin,
        &scope.recipient,
        &scope.payer,
        &scope.provider,
        &scope.purpose,
        &scope.configuration_id,
    ] {
        anyhow::ensure!(
            !field.is_empty()
                && field.len() <= 1024
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_graphic() || byte == b' '),
            "invalid hosted egress scope"
        );
    }
    let url = Url::parse(&scope.url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.origin().ascii_serialization() == scope.origin
            && url.host_str() == Some(scope.recipient.as_str())
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && scope.payer == "this Home"
            && scope.configuration_id.len() == 64
            && scope
                .configuration_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "invalid hosted egress destination scope"
    );
    Ok(())
}

pub(super) fn request(
    data_dir: &Path,
    scope: &EgressScope,
    requested_by_proof: Option<&str>,
) -> anyhow::Result<String> {
    validate_scope(scope)?;
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let now = now_ms()?;
    let mut file = read(data_dir)?;
    file.decisions
        .retain(|decision| decision.expires_at_ms > now);
    let existing = file.decisions.iter().rev().find(|decision| {
        decision.scope == *scope
            && decision.requested_by_proof.as_deref() == requested_by_proof
            && decision.expires_at_ms > now
    });
    if let Some(existing) = existing {
        if existing.status == DecisionStatus::Approved {
            return Ok(existing.id.clone());
        }
        anyhow::ensure!(
            existing.status == DecisionStatus::Pending,
            "hosted egress request was denied or ended"
        );
        return project_notification(data_dir, scope, &existing.id, now);
    }
    anyhow::ensure!(
        file.decisions.len() < MAX_DECISIONS,
        "hosted egress decisions full"
    );
    let id = format!("model-egress-{:032x}", rand::random::<u128>());
    file.decisions.push(EgressDecision {
        id: id.clone(),
        scope: scope.clone(),
        requested_by_proof: requested_by_proof.map(ToString::to_string),
        owner_proof_binding_id: None,
        requested_at_ms: now,
        decided_at_ms: None,
        expires_at_ms: now + DURATION_MS,
        status: DecisionStatus::Pending,
    });
    write(data_dir, &file)?;
    project_notification(data_dir, scope, &id, now)
}

fn project_notification(
    data_dir: &Path,
    scope: &EgressScope,
    id: &str,
    now: u64,
) -> anyhow::Result<String> {
    let source_app = if scope.offer_id.starts_with("validation:") {
        "system"
    } else {
        "assistant"
    };
    let protocol = if scope.origin.starts_with("https://") {
        "HTTPS"
    } else {
        "HTTP fixture"
    };
    let url = Url::parse(&scope.url)?;
    let title = format!("Approve {} {} access", scope.provider, protocol);
    let body = format!(
        "Provider: {} ({})\nRoute: {} {}{}\nExact URL SHA-256: {}\nEffect: {}\nOrigin: {}\nData recipient: {}\nPayer: {}\nPurpose: {}\nDuration: 10 minutes after approval. Matching requests during that period use separate run grants. This decision is bound to the current key and model configuration. Service access is a separate approval.",
        scope.provider, scope.offer_id, scope.method, scope.origin, url.path(),
        hex::encode(Sha256::digest(scope.url.as_bytes())), scope.effect,
        scope.origin, scope.recipient, scope.payer, scope.purpose,
    );
    crate::notifications::upsert_external_http_request(
        data_dir,
        &id,
        source_app,
        &title,
        &body,
        &format!("{APPROVE_PREFIX}{id}"),
        now / 1000,
    )?;
    Ok(id.to_string())
}

pub(super) fn approve(data_dir: &Path, id: &str, proof: &str) -> anyhow::Result<()> {
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, proof)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    anyhow::ensure!(
        crate::auth::is_admin(&principal) && principal.proof_binding.passkey.is_some(),
        "hosted egress owner unavailable"
    );
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let now = now_ms()?;
    let mut file = read(data_dir)?;
    let decision = file
        .decisions
        .iter_mut()
        .find(|decision| decision.id == id)
        .ok_or_else(|| anyhow::anyhow!("hosted egress request unavailable"))?;
    anyhow::ensure!(
        decision.status == DecisionStatus::Pending
            && decision.expires_at_ms > now
            && decision
                .requested_by_proof
                .as_deref()
                .is_none_or(|requested| requested == proof),
        "hosted egress request is no longer pending"
    );
    validate_scope(&decision.scope)?;
    decision.status = DecisionStatus::Approved;
    decision.owner_proof_binding_id = Some(proof.to_string());
    decision.decided_at_ms = Some(now);
    decision.expires_at_ms = now + DURATION_MS;
    write(data_dir, &file)
}

pub(super) fn deny(data_dir: &Path, id: &str, proof: &str) -> anyhow::Result<()> {
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, proof)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    anyhow::ensure!(
        crate::auth::is_admin(&principal) && principal.proof_binding.passkey.is_some(),
        "hosted egress owner unavailable"
    );
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let mut file = read(data_dir)?;
    let decision = file
        .decisions
        .iter_mut()
        .find(|decision| decision.id == id)
        .ok_or_else(|| anyhow::anyhow!("hosted egress request unavailable"))?;
    anyhow::ensure!(
        decision.status == DecisionStatus::Pending && decision.expires_at_ms > now_ms()?,
        "hosted egress request is no longer pending"
    );
    decision.status = DecisionStatus::Denied;
    write(data_dir, &file)
}

pub(super) fn active(
    data_dir: &Path,
    scope: &EgressScope,
    proof: Option<&str>,
) -> anyhow::Result<ActiveDecision> {
    validate_scope(scope)?;
    let file = read(data_dir)?;
    let now = now_ms()?;
    let decision = file
        .decisions
        .iter()
        .find(|decision| {
            decision.scope == *scope
                && decision.status == DecisionStatus::Approved
                && decision.expires_at_ms > now
                && decision
                    .requested_by_proof
                    .as_deref()
                    .is_none_or(|requested| Some(requested) == proof)
        })
        .ok_or_else(|| anyhow::anyhow!("hosted egress decision unavailable"))?;
    let owner = decision
        .owner_proof_binding_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("hosted egress owner unavailable"))?;
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, owner)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    anyhow::ensure!(
        crate::auth::is_admin(&principal) && principal.proof_binding.passkey.is_some(),
        "hosted egress owner unavailable"
    );
    Ok(ActiveDecision {
        id: decision.id.clone(),
        owner_proof_binding_id: owner.to_string(),
        expires_at_ms: decision.expires_at_ms,
    })
}

pub(super) fn end_offer(data_dir: &Path, offer_id: &str) -> anyhow::Result<usize> {
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let mut file = read(data_dir)?;
    let mut ended = 0;
    for decision in &mut file.decisions {
        if decision.scope.offer_id == offer_id
            && matches!(
                decision.status,
                DecisionStatus::Pending | DecisionStatus::Approved
            )
        {
            decision.status = DecisionStatus::Ended;
            ended += 1;
        }
    }
    if ended > 0 {
        write(data_dir, &file)?;
    }
    Ok(ended)
}

pub(super) fn offer_state(data_dir: &Path, offer_id: &str) -> anyhow::Result<&'static str> {
    let now = now_ms()?;
    let file = read(data_dir)?;
    let mut pending = false;
    for decision in file.decisions.iter().rev() {
        if decision.scope.offer_id != offer_id || decision.expires_at_ms <= now {
            continue;
        }
        match decision.status {
            DecisionStatus::Approved => {
                if active(
                    data_dir,
                    &decision.scope,
                    decision.requested_by_proof.as_deref(),
                )
                .is_ok()
                {
                    return Ok("approved");
                }
            }
            DecisionStatus::Pending => pending = true,
            DecisionStatus::Denied | DecisionStatus::Ended => {}
        }
    }
    Ok(if pending { "pending" } else { "none" })
}
