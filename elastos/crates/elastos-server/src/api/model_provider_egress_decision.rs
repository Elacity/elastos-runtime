//! Owner Inbox decisions for hosted HTTP destinations and connections.
//!
//! A notification displays a pending request. This private record is the
//! authority: only an active admin passkey decision can authorize dispatch.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;

use super::model_provider_config::{
    archive_hosted_egress_decision, read_hosted_egress_decisions, recent_hosted_egress_history,
    write_hosted_egress_decisions,
};

pub(super) const APPROVE_PREFIX: &str = "model-egress-approve:";
pub(super) const DENY_PREFIX: &str = "model-egress-deny:";
pub(super) const END_PREFIX: &str = "model-egress-end:";
const SCHEMA: &str = "elastos.model.egress-decisions/v1";
const DURATION_MS: u64 = 10 * 60 * 1000;
const MAX_DECISIONS: usize = 1024;
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConnectionScope {
    pub version: u8,
    pub offer_id: String,
    pub provider: String,
    pub origin: String,
    pub revision: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    connection: Option<ConnectionScope>,
    requested_by_proof: Option<String>,
    owner_proof_binding_id: Option<String>,
    requested_at_ms: u64,
    decided_at_ms: Option<u64>,
    #[serde(default)]
    ended_at_ms: Option<u64>,
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

#[derive(Serialize)]
pub(super) struct HostedRouteSummary {
    id: String,
    connection: bool,
    provider: String,
    offer_id: String,
    method: String,
    origin: String,
    path: String,
    url_sha256: String,
    recipient: String,
    payer: String,
    purpose: String,
    status: &'static str,
    requested_at: u64,
    decided_at: Option<u64>,
    ended_at: Option<u64>,
    expires_at: u64,
}

#[cfg(test)]
#[derive(Serialize)]
pub(super) struct StagedConnectionSummary {
    id: String,
    provider: String,
    status: &'static str,
}

#[cfg(test)]
pub(super) fn staged_connections(data_dir: &Path) -> anyhow::Result<Vec<StagedConnectionSummary>> {
    let now = now_ms()?;
    let file = read(data_dir)?;
    let mut seen = std::collections::HashSet::new();
    Ok(file
        .decisions
        .iter()
        .rev()
        .filter_map(|decision| {
            let connection = decision.connection.as_ref()?;
            if !seen.insert(connection.offer_id.clone()) {
                return None;
            }
            let status = if decision.expires_at_ms <= now {
                "expired"
            } else {
                match decision.status {
                    DecisionStatus::Pending => "pending",
                    DecisionStatus::Approved => "approved",
                    DecisionStatus::Denied => "denied",
                    DecisionStatus::Ended => "ended",
                }
            };
            Some(StagedConnectionSummary {
                id: connection.offer_id.clone(),
                provider: connection.provider.clone(),
                status,
            })
        })
        .collect())
}

impl HostedRouteSummary {
    pub(super) fn is_pending(&self) -> bool {
        self.status == "pending"
    }
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

fn archive_expired(data_dir: &Path, file: &mut DecisionFile, now: u64) -> anyhow::Result<()> {
    let expired = file
        .decisions
        .iter()
        .filter(|decision| decision.expires_at_ms <= now)
        .collect::<Vec<_>>();
    for decision in &expired {
        archive_hosted_egress_decision(
            data_dir,
            &format!("{:020}-{}.json", decision.requested_at_ms, decision.id),
            &serde_json::to_vec(decision)?,
        )?;
    }
    if !expired.is_empty() {
        file.decisions
            .retain(|decision| decision.expires_at_ms > now);
        write(data_dir, file)?;
    }
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
    archive_expired(data_dir, &mut file, now)?;
    let existing = file.decisions.iter().rev().find(|decision| {
        decision.connection.is_none()
            && decision.scope == *scope
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
        return Ok(existing.id.clone());
    }
    anyhow::ensure!(
        file.decisions.len() < MAX_DECISIONS,
        "hosted egress decisions full"
    );
    let id = format!("model-egress-{:032x}", rand::random::<u128>());
    file.decisions.push(EgressDecision {
        id: id.clone(),
        scope: scope.clone(),
        connection: None,
        requested_by_proof: requested_by_proof.map(ToString::to_string),
        owner_proof_binding_id: None,
        requested_at_ms: now,
        decided_at_ms: None,
        ended_at_ms: None,
        expires_at_ms: now + DURATION_MS,
        status: DecisionStatus::Pending,
    });
    write(data_dir, &file)?;
    Ok(id)
}

fn validate_connection(scope: &EgressScope, connection: &ConnectionScope) -> anyhow::Result<()> {
    validate_scope(scope)?;
    anyhow::ensure!(
        connection.version == 1
            && connection.offer_id == scope.offer_id
            && connection.provider == scope.provider
            && connection.origin == scope.origin
            && connection.origin.starts_with("https://")
            && connection.revision.len() == 64
            && connection
                .revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "invalid hosted connection scope"
    );
    Ok(())
}

pub(super) fn request_connection(
    data_dir: &Path,
    scope: &EgressScope,
    connection: &ConnectionScope,
    proof: Option<&str>,
) -> anyhow::Result<String> {
    validate_connection(scope, connection)?;
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let now = now_ms()?;
    let mut file = read(data_dir)?;
    archive_expired(data_dir, &mut file, now)?;
    if let Some(existing) = file.decisions.iter().rev().find(|decision| {
        decision.connection.as_ref() == Some(connection)
            && (proof.is_none()
                || decision
                    .requested_by_proof
                    .as_deref()
                    .is_none_or(|requested| Some(requested) == proof))
            && matches!(
                decision.status,
                DecisionStatus::Pending | DecisionStatus::Approved | DecisionStatus::Denied
            )
            && decision.expires_at_ms > now
    }) {
        anyhow::ensure!(
            existing.status != DecisionStatus::Denied,
            "hosted connection request was denied"
        );
        if existing.status == DecisionStatus::Pending
            || active_connection(data_dir, scope, connection, proof).is_ok()
        {
            return Ok(existing.id.clone());
        }
    }
    for decision in &mut file.decisions {
        if decision
            .connection
            .as_ref()
            .is_some_and(|old| old.offer_id == connection.offer_id)
            && matches!(
                decision.status,
                DecisionStatus::Pending | DecisionStatus::Approved
            )
        {
            decision.status = DecisionStatus::Ended;
            decision.ended_at_ms = Some(now);
            decision.expires_at_ms = now + DURATION_MS;
        }
    }
    anyhow::ensure!(
        file.decisions.len() < MAX_DECISIONS,
        "hosted egress decisions full"
    );
    let id = format!("model-egress-{:032x}", rand::random::<u128>());
    file.decisions.push(EgressDecision {
        id: id.clone(),
        scope: scope.clone(),
        connection: Some(connection.clone()),
        requested_by_proof: proof.map(ToString::to_string),
        owner_proof_binding_id: None,
        requested_at_ms: now,
        decided_at_ms: None,
        ended_at_ms: None,
        expires_at_ms: now + DURATION_MS,
        status: DecisionStatus::Pending,
    });
    write(data_dir, &file)?;
    Ok(id)
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
    decision.expires_at_ms = if decision.connection.is_some() {
        u64::MAX
    } else {
        now + DURATION_MS
    };
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
    decision.decided_at_ms = Some(now_ms()?);
    write(data_dir, &file)
}

pub(super) fn end_decision(data_dir: &Path, id: &str, proof: &str) -> anyhow::Result<()> {
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
        .ok_or_else(|| anyhow::anyhow!("hosted egress decision unavailable"))?;
    anyhow::ensure!(
        matches!(
            decision.status,
            DecisionStatus::Pending | DecisionStatus::Approved
        ) && decision.expires_at_ms > now,
        "hosted egress decision is no longer active"
    );
    decision.status = DecisionStatus::Ended;
    decision.ended_at_ms = Some(now);
    if decision.connection.is_some() {
        decision.expires_at_ms = now + DURATION_MS;
    }
    write(data_dir, &file)
}

pub(super) fn inbox_history(data_dir: &Path) -> anyhow::Result<Vec<HostedRouteSummary>> {
    let now = now_ms()?;
    let file = read(data_dir)?;
    let mut decisions = file.decisions;
    let mut known = decisions
        .iter()
        .map(|decision| decision.id.clone())
        .collect::<std::collections::HashSet<_>>();
    for bytes in recent_hosted_egress_history(data_dir, 256)? {
        let archived: EgressDecision = serde_json::from_slice(&bytes)?;
        if known.insert(archived.id.clone()) {
            decisions.push(archived);
        }
    }
    decisions.sort_by_key(|decision| decision.requested_at_ms);
    decisions
        .iter()
        .rev()
        .map(|decision| {
            validate_scope(&decision.scope)?;
            let url = Url::parse(&decision.scope.url)?;
            let status = if decision.expires_at_ms <= now
                && matches!(
                    decision.status,
                    DecisionStatus::Pending | DecisionStatus::Approved
                ) {
                "expired"
            } else {
                match decision.status {
                    DecisionStatus::Pending => "pending",
                    DecisionStatus::Approved => "approved",
                    DecisionStatus::Denied => "denied",
                    DecisionStatus::Ended => "ended",
                }
            };
            Ok(HostedRouteSummary {
                id: decision.id.clone(),
                connection: decision.connection.is_some(),
                provider: decision.scope.provider.clone(),
                offer_id: decision.scope.offer_id.clone(),
                method: decision.scope.method.clone(),
                origin: decision.scope.origin.clone(),
                path: url.path().to_string(),
                url_sha256: hex::encode(Sha256::digest(decision.scope.url.as_bytes())),
                recipient: decision.scope.recipient.clone(),
                payer: decision.scope.payer.clone(),
                purpose: decision.scope.purpose.clone(),
                status,
                requested_at: decision.requested_at_ms / 1000,
                decided_at: decision.decided_at_ms.map(|value| value / 1000),
                ended_at: decision.ended_at_ms.map(|value| value / 1000),
                expires_at: if decision.connection.is_some()
                    && decision.status == DecisionStatus::Approved
                {
                    0
                } else {
                    decision.expires_at_ms / 1000
                },
            })
        })
        .collect()
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
            decision.connection.is_none()
                && decision.scope == *scope
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

pub(super) fn active_connection(
    data_dir: &Path,
    scope: &EgressScope,
    connection: &ConnectionScope,
    proof: Option<&str>,
) -> anyhow::Result<ActiveDecision> {
    validate_connection(scope, connection)?;
    let file = read(data_dir)?;
    let now = now_ms()?;
    let decision = file
        .decisions
        .iter()
        .rev()
        .find(|decision| {
            decision.connection.as_ref() == Some(connection)
                && decision.status == DecisionStatus::Approved
                && decision.expires_at_ms > now
                && proof.is_none_or(|proof| {
                    decision
                        .requested_by_proof
                        .as_deref()
                        .is_none_or(|requested| requested == proof)
                })
        })
        .ok_or_else(|| anyhow::anyhow!("hosted connection decision unavailable"))?;
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

pub(super) fn connection_state(
    data_dir: &Path,
    scope: &EgressScope,
    connection: &ConnectionScope,
) -> anyhow::Result<&'static str> {
    validate_connection(scope, connection)?;
    let file = read(data_dir)?;
    let now = now_ms()?;
    let Some(decision) = file.decisions.iter().rev().find(|decision| {
        decision.connection.as_ref() == Some(connection) && decision.expires_at_ms > now
    }) else {
        return Ok("none");
    };
    Ok(match decision.status {
        DecisionStatus::Approved
            if active_connection(data_dir, scope, connection, None).is_ok() =>
        {
            "approved"
        }
        DecisionStatus::Pending => "pending",
        _ => "none",
    })
}

pub(super) fn refusal_state(
    data_dir: &Path,
    scope: &EgressScope,
    connection: Option<&ConnectionScope>,
) -> anyhow::Result<Option<&'static str>> {
    validate_scope(scope)?;
    if let Some(connection) = connection {
        validate_connection(scope, connection)?;
    }
    let now = now_ms()?;
    let file = read(data_dir)?;
    let latest = file.decisions.iter().rev().find(|decision| {
        decision.expires_at_ms > now
            && if let Some(connection) = connection {
                decision.connection.as_ref() == Some(connection)
            } else {
                decision.connection.is_none() && decision.scope == *scope
            }
    });
    Ok(latest.and_then(|decision| match decision.status {
        DecisionStatus::Pending => Some("pending"),
        DecisionStatus::Denied => Some("denied"),
        DecisionStatus::Ended => Some("ended"),
        DecisionStatus::Approved => None,
    }))
}

pub(super) fn end_offer(data_dir: &Path, offer_id: &str) -> anyhow::Result<usize> {
    let _guard = lock().lock().unwrap_or_else(|error| error.into_inner());
    let mut file = read(data_dir)?;
    let mut ended = 0;
    let now = now_ms()?;
    for decision in &mut file.decisions {
        if decision.scope.offer_id == offer_id
            && matches!(
                decision.status,
                DecisionStatus::Pending | DecisionStatus::Approved
            )
        {
            decision.status = DecisionStatus::Ended;
            decision.ended_at_ms = Some(now);
            if decision.connection.is_some() {
                decision.expires_at_ms = now + DURATION_MS;
            }
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
                let approved = if let Some(connection) = &decision.connection {
                    active_connection(
                        data_dir,
                        &decision.scope,
                        connection,
                        decision.requested_by_proof.as_deref(),
                    )
                } else {
                    active(
                        data_dir,
                        &decision.scope,
                        decision.requested_by_proof.as_deref(),
                    )
                };
                if approved.is_ok() {
                    return Ok("approved");
                }
            }
            DecisionStatus::Pending => pending = true,
            DecisionStatus::Denied | DecisionStatus::Ended => {}
        }
    }
    Ok(if pending { "pending" } else { "none" })
}
