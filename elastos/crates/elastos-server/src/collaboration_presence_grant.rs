//! A person's grant to be shown in Community: a capability token the Runtime
//! mints for that person (subject = principal id, action `message`, resource
//! `elastos://chat/presence/<network>/<conversation>/always|while-open`) when
//! they answer in Inbox or System → Security (Community section), and validates with all twelve
//! checks on every announcement.
//!
//! The token lives with its holder, the Runtime acting for that person, in one
//! owner-only file in the person's own principal root. No capsule sees it;
//! routes report the mode only. The file is new, so every file an older
//! Runtime reads keeps its exact shape and Undo still loads.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use elastos_runtime::capability::token::TokenId;
use elastos_runtime::capability::{
    Action, CapabilityManager, CapabilityToken, ResourceId, TokenConstraints,
};
use serde::{Deserialize, Serialize};

pub(crate) const PRESENCE_GRANT_SCHEMA: &str = "elastos.community.presence-grant/v1";
const PRESENCE_RESOURCE_PREFIX: &str = "elastos://chat/presence";
const PRESENCE_GRANT_OBJECT: &str = ".AppData/ElastOS/Home/community-presence.json";
const MAX_PRESENCE_GRANT_BYTES: usize = 4 * 1024;
/// A token that keeps validating is audited once per hour; every refusal is
/// audited when it happens.
const SUCCESS_AUDIT_INTERVAL_SECS: u64 = 60 * 60;

/// When a granted person may be announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PresenceMode {
    /// While the Home runs: the Runtime timer announces.
    Always,
    /// Only while Community is open in Chat: Chat's heartbeat announces.
    WhileOpen,
}

impl PresenceMode {
    fn resource_suffix(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::WhileOpen => "while-open",
        }
    }
}

/// An answer that grants nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PresenceStatus {
    /// Chat asked; the Inbox card is open.
    Requested,
    /// Later: no card; Chat offers one "Show me in Community" line.
    Deferred,
    /// Never: no card, no line, until changed in System → Security (Community section).
    Refused,
    /// System's Hidden: the grant was revoked.
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PresenceScope {
    pub(crate) network_id: String,
    pub(crate) conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceGrant {
    scope: PresenceScope,
    mode: PresenceMode,
    /// `CapabilityToken::to_base64`; never leaves the Runtime.
    token: String,
    token_id: String,
    /// The Profile the person granted as; any other Profile revokes.
    profile_did: String,
    issued_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceRequest {
    scope: PresenceScope,
    status: PresenceStatus,
    at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceGrantFile {
    schema: String,
    principal_id: String,
    localhost_root: String,
    grants: Vec<PresenceGrant>,
    requests: Vec<PresenceRequest>,
}

/// The person's answer, in Inbox or System → Security (Community section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresenceDecision {
    Always,
    WhileOpen,
    Later,
    Never,
    /// System's "Hidden": revokes a grant.
    Hidden,
}

/// Who is about to announce a person. Each asks for exactly one resource:
/// the Runtime timer `/always`, Chat's heartbeat `/while-open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresencePublisher {
    /// The Runtime timer, whether or not anyone has Home open.
    Runtime,
    /// Chat, while its Community view is visible.
    CommunityOpen,
}

impl PresencePublisher {
    fn mode(self) -> PresenceMode {
        match self {
            Self::Runtime => PresenceMode::Always,
            Self::CommunityOpen => PresenceMode::WhileOpen,
        }
    }
}

/// The publisher and grant generation that authorized preparation. This holds
/// a digest of the token id, never the bearer token. The existing immutable
/// request id carries it through persistence and restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GrantedPresence {
    pub(crate) mode: PresenceMode,
    generation: String,
}

impl GrantedPresence {
    fn from_grant(grant: &PresenceGrant) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            mode: grant.mode,
            generation: hex::encode(Sha256::digest(grant.token_id.as_bytes())),
        }
    }

    fn request_prefix(&self) -> String {
        format!(
            "presence:{}:{}:",
            self.mode.resource_suffix(),
            self.generation
        )
    }

    pub(crate) fn bind_request_id(&self, request_id: &str) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "{}{}",
            self.request_prefix(),
            hex::encode(Sha256::digest(request_id.as_bytes()))
        )
    }

    fn authorizes_request(&self, request_id: &str) -> bool {
        let prefix = self.request_prefix();
        request_id
            .strip_prefix(prefix.as_str())
            .is_some_and(|suffix| {
                suffix.len() == 64 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
    }
}

/// The Runtime's capability manager plus when each presence token's success
/// was last audited. Only audit timestamps are remembered: a success never
/// lets a later call skip validation.
pub(crate) struct PresenceAuthority {
    manager: Arc<CapabilityManager>,
    success_audited_at: Mutex<HashMap<String, u64>>,
}

impl PresenceAuthority {
    pub(crate) fn new(manager: Arc<CapabilityManager>) -> Self {
        Self {
            manager,
            success_audited_at: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn manager(&self) -> &Arc<CapabilityManager> {
        &self.manager
    }

    fn success_audit_due(&self, token_id: &str, now: u64) -> bool {
        self.success_audited_at
            .lock()
            .map(|audited| {
                audited
                    .get(token_id)
                    .is_none_or(|at| now.saturating_sub(*at) >= SUCCESS_AUDIT_INTERVAL_SECS)
            })
            .unwrap_or(true)
    }

    fn record_success_audit(&self, token_id: &str, now: u64) {
        if let Ok(mut audited) = self.success_audited_at.lock() {
            audited.insert(token_id.to_string(), now);
        }
    }

    fn forget(&self, token_id: &str) {
        if let Ok(mut audited) = self.success_audited_at.lock() {
            audited.remove(token_id);
        }
    }
}

pub(crate) fn presence_resource(scope: &PresenceScope, mode: PresenceMode) -> ResourceId {
    ResourceId::new(format!(
        "{PRESENCE_RESOURCE_PREFIX}/{}/{}/{}",
        scope.network_id,
        scope.conversation_id,
        mode.resource_suffix()
    ))
}

fn object_uri(principal_id: &str) -> String {
    format!(
        "{}/{PRESENCE_GRANT_OBJECT}",
        crate::auth::principal_localhost_root(principal_id)
    )
}

fn object_path(
    data_dir: &std::path::Path,
    principal_id: &str,
) -> anyhow::Result<std::path::PathBuf> {
    elastos_common::localhost::rooted_localhost_fs_path(data_dir, &object_uri(principal_id))
        .ok_or_else(|| anyhow::anyhow!("invalid Community presence grant root"))
}

fn empty_file(principal_id: &str) -> PresenceGrantFile {
    PresenceGrantFile {
        schema: PRESENCE_GRANT_SCHEMA.to_string(),
        principal_id: principal_id.to_string(),
        localhost_root: crate::auth::principal_localhost_root(principal_id),
        grants: Vec::new(),
        requests: Vec::new(),
    }
}

fn load_file(data_dir: &std::path::Path, principal_id: &str) -> anyhow::Result<PresenceGrantFile> {
    let path = object_path(data_dir, principal_id)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(empty_file(principal_id))
        }
        Err(error) => return Err(error).context("could not inspect Community presence grant"),
        Ok(metadata) if !metadata.is_file() => {
            anyhow::bail!("Community presence grant must be a regular file")
        }
        Ok(_) => {}
    }
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let raw = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &localhost_root,
        &object_uri(principal_id),
        &path,
    )?;
    if raw.len() > MAX_PRESENCE_GRANT_BYTES {
        anyhow::bail!("Community presence grant is too large");
    }
    let file: PresenceGrantFile =
        serde_json::from_slice(&raw).context("invalid Community presence grant")?;
    if file.schema != PRESENCE_GRANT_SCHEMA
        || file.principal_id != principal_id
        || file.localhost_root != localhost_root
    {
        anyhow::bail!("Community presence grant belongs to another schema or person");
    }
    Ok(file)
}

fn save_file(
    data_dir: &std::path::Path,
    principal_id: &str,
    file: &PresenceGrantFile,
) -> anyhow::Result<()> {
    let path = object_path(data_dir, principal_id)?;
    if let Some(parent) = path.parent() {
        crate::auth::create_owner_only_dir_all(data_dir, parent)?;
        crate::collaboration_core::ensure_owner_only_directory(parent)?;
    }
    let raw = serde_json::to_vec(file)?;
    if raw.len() > MAX_PRESENCE_GRANT_BYTES {
        anyhow::bail!("Community presence grant is too large");
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &crate::auth::principal_localhost_root(principal_id),
        &object_uri(principal_id),
        &path,
        &raw,
    )
}

/// Remove `token_id`'s grant if it is still the one on file; a newer
/// decision written meanwhile is kept.
fn drop_grant(data_dir: &std::path::Path, principal_id: &str, token_id: &str) {
    let Ok(mut file) = load_file(data_dir, principal_id) else {
        return;
    };
    let before = file.grants.len();
    file.grants.retain(|grant| grant.token_id != token_id);
    if file.grants.len() != before {
        if let Err(error) = save_file(data_dir, principal_id, &file) {
            tracing::warn!("could not remove a failed Community presence grant: {error:#}");
        }
    }
}

/// Product state for Chat and System: `requested`, `deferred`, `refused`,
/// `hidden`, `always`, `while_open`, or `None` (not asked yet, or a grant
/// that stopped validating and was removed, so Chat asks again).
pub(crate) fn state(
    data_dir: &std::path::Path,
    principal_id: &str,
    scope: &PresenceScope,
) -> anyhow::Result<Option<&'static str>> {
    let file = load_file(data_dir, principal_id)?;
    if let Some(grant) = file.grants.iter().find(|grant| &grant.scope == scope) {
        return Ok(Some(match grant.mode {
            PresenceMode::Always => "always",
            PresenceMode::WhileOpen => "while_open",
        }));
    }
    Ok(file
        .requests
        .iter()
        .find(|request| &request.scope == scope)
        .map(|request| match request.status {
            PresenceStatus::Requested => "requested",
            PresenceStatus::Deferred => "deferred",
            PresenceStatus::Refused => "refused",
            PresenceStatus::Hidden => "hidden",
        }))
}

/// When the open Inbox request for `scope` was filed, if one is open.
pub(crate) fn open_request_at(
    data_dir: &std::path::Path,
    principal_id: &str,
    scope: &PresenceScope,
) -> anyhow::Result<Option<u64>> {
    let file = load_file(data_dir, principal_id)?;
    if file.grants.iter().any(|grant| &grant.scope == scope) {
        return Ok(None);
    }
    Ok(file
        .requests
        .iter()
        .find(|request| &request.scope == scope && request.status == PresenceStatus::Requested)
        .map(|request| request.at))
}

/// Chat asks once: a first Community open, or the person's own "Show me in
/// Community" after Later. Never, Hidden and any grant are kept. It never
/// grants anything.
pub(crate) fn request(
    data_dir: &std::path::Path,
    principal_id: &str,
    scope: &PresenceScope,
    asked_by_person: bool,
    now: u64,
) -> anyhow::Result<()> {
    let mut file = load_file(data_dir, principal_id)?;
    if file.grants.iter().any(|grant| &grant.scope == scope) {
        return Ok(());
    }
    let ask = match file.requests.iter().find(|request| &request.scope == scope) {
        None => true,
        Some(request) => request.status == PresenceStatus::Deferred && asked_by_person,
    };
    if !ask {
        return Ok(());
    }
    file.requests.retain(|request| &request.scope != scope);
    file.requests.push(PresenceRequest {
        scope: scope.clone(),
        status: PresenceStatus::Requested,
        at: now,
    });
    save_file(data_dir, principal_id, &file)
}

/// May `publisher` announce this person in `scope` now? Validates the held
/// token against the live capability store on every call, for the one
/// resource this publisher may ask for. `None`: no grant for this publisher,
/// not decided, unreadable, or the token failed (it is then removed, so Chat
/// asks again). A Profile other than the one granted revokes the token.
pub(crate) async fn presence_grant(
    authority: &PresenceAuthority,
    data_dir: &std::path::Path,
    principal_id: &str,
    profile_did: &str,
    scope: &PresenceScope,
    publisher: PresencePublisher,
    now: u64,
) -> Option<GrantedPresence> {
    let file = load_file(data_dir, principal_id).ok()?;
    let grant = file
        .grants
        .into_iter()
        .find(|grant| &grant.scope == scope)?;
    // A token is presented only to the publisher its recorded mode names;
    // the recorded mode is not trusted beyond that, the token's signed
    // resource decides below.
    if grant.mode != publisher.mode() {
        return None;
    }
    let token = match CapabilityToken::from_base64(&grant.token) {
        Ok(token) if token.id().to_string() == grant.token_id => token,
        _ => {
            authority.forget(&grant.token_id);
            drop_grant(data_dir, principal_id, &grant.token_id);
            return None;
        }
    };
    if grant.profile_did != profile_did {
        authority
            .manager
            .revoke(*token.id(), "profile changed")
            .await;
        authority.forget(&grant.token_id);
        drop_grant(data_dir, principal_id, &grant.token_id);
        return None;
    }
    if validate_presence_grant(authority, principal_id, scope, &grant, &token, now).await {
        Some(GrantedPresence::from_grant(&grant))
    } else {
        authority.forget(&grant.token_id);
        drop_grant(data_dir, principal_id, &grant.token_id);
        None
    }
}

/// Check the live capability without changing the holder's grant file.
async fn validate_presence_grant(
    authority: &PresenceAuthority,
    principal_id: &str,
    scope: &PresenceScope,
    grant: &PresenceGrant,
    token: &CapabilityToken,
    now: u64,
) -> bool {
    let audit_success = authority.success_audit_due(&grant.token_id, now);
    match authority
        .manager
        .validate_recurring(
            token,
            principal_id,
            Action::Message,
            &presence_resource(scope, grant.mode),
            None,
            audit_success,
        )
        .await
    {
        Ok(()) => {
            if audit_success {
                authority.record_success_audit(&grant.token_id, now);
            }
            true
        }
        Err(error) => {
            tracing::info!("Community presence grant no longer valid: {error}");
            false
        }
    }
}

/// Publisher maintenance checks the current grant and retires a failed holder.
/// Effects use `own_presence_operation_authorized` with their original binding.
pub(crate) async fn current_presence_grant_usable(
    authority: &PresenceAuthority,
    data_dir: &std::path::Path,
    principal_id: &str,
    profile_did: &str,
    scope: &PresenceScope,
    now: u64,
) -> bool {
    let Some(mode) = load_file(data_dir, principal_id).ok().and_then(|file| {
        file.grants
            .into_iter()
            .find(|grant| &grant.scope == scope)
            .map(|grant| grant.mode)
    }) else {
        return false;
    };
    let publisher = match mode {
        PresenceMode::Always => PresencePublisher::Runtime,
        PresenceMode::WhileOpen => PresencePublisher::CommunityOpen,
    };
    presence_grant(
        authority,
        data_dir,
        principal_id,
        profile_did,
        scope,
        publisher,
        now,
    )
    .await
    .is_some()
}

/// An effect presents the generation and publisher that prepared its operation.
/// A newer valid grant cannot revive work from an older generation. Snapshot
/// checks pass false for retirement; only lifecycle/effect calls change state.
pub(crate) async fn own_presence_operation_authorized(
    authority: &PresenceAuthority,
    data_dir: &std::path::Path,
    operation: &crate::esp_binding::EspRequestBinding,
    profile_did: &str,
    scope: &PresenceScope,
    now: u64,
    retire_denied: bool,
) -> bool {
    let principal_id = operation.principal.as_str();
    let Some(grant) = load_file(data_dir, principal_id)
        .ok()
        .and_then(|file| file.grants.into_iter().find(|grant| &grant.scope == scope))
    else {
        return false;
    };
    if !GrantedPresence::from_grant(&grant).authorizes_request(&operation.request_id) {
        return false;
    }
    let token = match CapabilityToken::from_base64(&grant.token) {
        Ok(token) if token.id().to_string() == grant.token_id => token,
        _ => {
            if retire_denied {
                authority.forget(&grant.token_id);
                drop_grant(data_dir, principal_id, &grant.token_id);
            }
            return false;
        }
    };
    let allowed = if grant.profile_did != profile_did {
        if retire_denied {
            authority
                .manager
                .revoke(*token.id(), "profile changed")
                .await;
        }
        false
    } else {
        validate_presence_grant(authority, principal_id, scope, &grant, &token, now).await
    };
    if !allowed && retire_denied {
        authority.forget(&grant.token_id);
        drop_grant(data_dir, principal_id, &grant.token_id);
    }
    allowed
}

/// Records the person's own answer, from Inbox or System → Security (Community section) only.
/// Always and Only-while-open mint a token for `principal_id`; any change
/// revokes the token it replaces (persisted before this returns).
pub(crate) async fn set_presence_grant(
    authority: &PresenceAuthority,
    data_dir: &std::path::Path,
    principal_id: &str,
    profile_did: &str,
    scope: &PresenceScope,
    decision: PresenceDecision,
    now: u64,
) -> anyhow::Result<()> {
    let mut file = load_file(data_dir, principal_id)?;
    let mode = match decision {
        PresenceDecision::Always => Some(PresenceMode::Always),
        PresenceDecision::WhileOpen => Some(PresenceMode::WhileOpen),
        PresenceDecision::Later | PresenceDecision::Never | PresenceDecision::Hidden => None,
    };
    if let Some(current) = file.grants.iter().find(|grant| &grant.scope == scope) {
        if Some(current.mode) == mode && current.profile_did == profile_did {
            return Ok(());
        }
        let token_id = TokenId::from_hex(&current.token_id)
            .map_err(|error| anyhow::anyhow!("invalid Community presence token id: {error}"))?;
        authority
            .manager
            .revoke(token_id, "community presence changed")
            .await;
        authority.forget(&current.token_id);
    }
    file.grants.retain(|grant| &grant.scope != scope);
    file.requests.retain(|request| &request.scope != scope);
    match mode {
        Some(mode) => {
            let token = authority.manager.grant(
                principal_id,
                presence_resource(scope, mode),
                Action::Message,
                TokenConstraints::default(),
                None,
            );
            file.grants.push(PresenceGrant {
                scope: scope.clone(),
                mode,
                token: token
                    .to_base64()
                    .map_err(|error| anyhow::anyhow!("could not encode presence token: {error}"))?,
                token_id: token.id().to_string(),
                profile_did: profile_did.to_string(),
                issued_at: now,
            });
        }
        None => file.requests.push(PresenceRequest {
            scope: scope.clone(),
            status: match decision {
                PresenceDecision::Later => PresenceStatus::Deferred,
                PresenceDecision::Never => PresenceStatus::Refused,
                _ => PresenceStatus::Hidden,
            },
            at: now,
        }),
    }
    save_file(data_dir, principal_id, &file)
}

/// Store an isolated fixture's actual signed Profile through the protected
/// principal-root path used by the production reader.
#[cfg(test)]
pub(crate) fn store_test_profile(
    data_dir: &std::path::Path,
    principal_id: &str,
    key: &elastos_runtime::signature::SigningKey,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
) {
    let root = crate::auth::principal_localhost_root(principal_id);
    let path =
        crate::collaboration_profile_authority::profile_authority_path(data_dir, &root).unwrap();
    crate::auth::create_owner_only_dir_all(data_dir, path.parent().unwrap()).unwrap();
    let bundle = serde_json::json!({
        "schema": crate::collaboration_profile_authority::COLLABORATION_PROFILE_AUTHORITY_BUNDLE_SCHEMA_V1,
        "profile_signing_seed_hex": hex::encode(key.to_bytes()),
        "previous_profiles": [],
        "signed_profile": profile.signed_envelope(),
    });
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &crate::collaboration_profile_authority::profile_authority_object_uri(&root),
        &path,
        &serde_json::to_vec(&bundle).unwrap(),
    )
    .unwrap();
}

#[cfg(test)]
pub(crate) fn test_authority() -> Arc<PresenceAuthority> {
    Arc::new(PresenceAuthority::new(Arc::new(CapabilityManager::new(
        Arc::new(elastos_runtime::capability::CapabilityStore::new()),
        Arc::new(elastos_runtime::primitives::audit::AuditLog::new()),
        Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new()),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::primitives::audit::AuditEvent;

    const ME: &str = "did:key:zMe";

    fn home() -> (tempfile::TempDir, String) {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let principal = "person:local:presence-grant-test".to_string();
        for person in [principal.as_str(), SECOND, OTHER] {
            crate::auth::store_test_principal_root_protection(temp.path(), person);
        }
        (temp, principal)
    }

    const SECOND: &str = "person:local:presence-grant-second";
    const OTHER: &str = "person:local:someone-else";

    fn files(root: &std::path::Path) -> std::collections::BTreeSet<std::path::PathBuf> {
        let mut written = std::collections::BTreeSet::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    written.insert(path);
                }
            }
        }
        written
    }

    fn scope() -> PresenceScope {
        PresenceScope {
            network_id: "elastos.community".to_string(),
            conversation_id: "community".to_string(),
        }
    }

    fn successes(authority: &PresenceAuthority) -> usize {
        authority
            .manager()
            .audit_log()
            .recent_events(100_000)
            .into_iter()
            .filter(|event| matches!(event, AuditEvent::CapabilityUse { success: true, .. }))
            .count()
    }

    async fn gate(
        authority: &PresenceAuthority,
        data: &std::path::Path,
        principal: &str,
        profile: &str,
        publisher: PresencePublisher,
        now: u64,
    ) -> Option<PresenceMode> {
        presence_grant(
            authority,
            data,
            principal,
            profile,
            &scope(),
            publisher,
            now,
        )
        .await
        .map(|grant| grant.mode)
    }

    fn held_token(data: &std::path::Path, principal: &str) -> CapabilityToken {
        let file = load_file(data, principal).unwrap();
        CapabilityToken::from_base64(&file.grants[0].token).unwrap()
    }

    #[tokio::test]
    async fn requests_and_answers_follow_the_contract() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        assert_eq!(state(data, &principal, &scope()).unwrap(), None);

        // Chat asks once; asking again keeps the open request.
        request(data, &principal, &scope(), false, 10).unwrap();
        request(data, &principal, &scope(), false, 11).unwrap();
        assert_eq!(
            state(data, &principal, &scope()).unwrap(),
            Some("requested")
        );
        assert_eq!(
            open_request_at(data, &principal, &scope()).unwrap(),
            Some(10)
        );

        // Later: no new card on open, but the person's own line asks again.
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Later,
            12,
        )
        .await
        .unwrap();
        request(data, &principal, &scope(), false, 13).unwrap();
        assert_eq!(state(data, &principal, &scope()).unwrap(), Some("deferred"));
        request(data, &principal, &scope(), true, 14).unwrap();
        assert_eq!(
            open_request_at(data, &principal, &scope()).unwrap(),
            Some(14)
        );

        // Never: no card, no line; only System changes it.
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Never,
            15,
        )
        .await
        .unwrap();
        for asked_by_person in [false, true] {
            request(data, &principal, &scope(), asked_by_person, 16).unwrap();
            assert_eq!(state(data, &principal, &scope()).unwrap(), Some("refused"));
        }

        // Granting mints a token whose subject is the principal and whose
        // resource carries the mode.
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            17,
        )
        .await
        .unwrap();
        assert_eq!(state(data, &principal, &scope()).unwrap(), Some("always"));
        let token = held_token(data, &principal);
        assert_eq!(token.capsule(), principal);
        assert_eq!(token.action(), Action::Message);
        assert_eq!(
            token.resource().to_string(),
            "elastos://chat/presence/elastos.community/community/always"
        );
        assert!(token.expiry().is_none());
        request(data, &principal, &scope(), true, 18).unwrap();
        assert_eq!(state(data, &principal, &scope()).unwrap(), Some("always"));

        // Hidden revokes the token and is kept as an answer.
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Hidden,
            19,
        )
        .await
        .unwrap();
        assert_eq!(state(data, &principal, &scope()).unwrap(), Some("hidden"));
        assert!(authority
            .manager()
            .validate(
                &token,
                &principal,
                Action::Message,
                &presence_resource(&scope(), PresenceMode::Always),
                None
            )
            .await
            .is_err());
        request(data, &principal, &scope(), true, 20).unwrap();
        assert_eq!(state(data, &principal, &scope()).unwrap(), Some("hidden"));
    }

    #[tokio::test]
    async fn mode_is_in_the_resource_and_each_publisher_asks_for_its_own() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        let runtime = PresencePublisher::Runtime;
        let chat = PresencePublisher::CommunityOpen;

        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::WhileOpen,
            1,
        )
        .await
        .unwrap();
        assert_eq!(
            gate(&authority, data, &principal, ME, runtime, 2).await,
            None
        );
        assert_eq!(
            gate(&authority, data, &principal, ME, chat, 2).await,
            Some(PresenceMode::WhileOpen)
        );
        // The /while-open token cannot drive the timer even if presented.
        let open = held_token(data, &principal);
        assert!(matches!(
            authority
                .manager()
                .validate(
                    &open,
                    &principal,
                    Action::Message,
                    &presence_resource(&scope(), PresenceMode::Always),
                    None
                )
                .await,
            Err(elastos_runtime::capability::manager::ValidationError::WrongResource { .. })
        ));

        // A mode change revokes the old token and mints a new one.
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            3,
        )
        .await
        .unwrap();
        assert_eq!(
            gate(&authority, data, &principal, ME, runtime, 4).await,
            Some(PresenceMode::Always)
        );
        assert_eq!(gate(&authority, data, &principal, ME, chat, 4).await, None);
        assert!(authority
            .manager()
            .validate(
                &open,
                &principal,
                Action::Message,
                &presence_resource(&scope(), PresenceMode::WhileOpen),
                None
            )
            .await
            .is_err());

        // Another person's token is refused for this person.
        let file = load_file(data, &principal).unwrap();
        let other = OTHER;
        let mut forged = empty_file(other);
        forged.grants = file.grants.clone();
        save_file(data, other, &forged).unwrap();
        assert_eq!(gate(&authority, data, other, ME, runtime, 5).await, None);
        assert!(load_file(data, other).unwrap().grants.is_empty());
    }

    #[tokio::test]
    async fn undecided_and_refused_are_silent() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        for publisher in [PresencePublisher::Runtime, PresencePublisher::CommunityOpen] {
            assert_eq!(
                gate(&authority, data, &principal, ME, publisher, 1).await,
                None
            );
        }
        for decision in [
            PresenceDecision::Later,
            PresenceDecision::Never,
            PresenceDecision::Hidden,
        ] {
            request(data, &principal, &scope(), true, 1).unwrap();
            set_presence_grant(&authority, data, &principal, ME, &scope(), decision, 2)
                .await
                .unwrap();
            for publisher in [PresencePublisher::Runtime, PresencePublisher::CommunityOpen] {
                assert_eq!(
                    gate(&authority, data, &principal, ME, publisher, 3).await,
                    None
                );
            }
        }
    }

    #[tokio::test]
    async fn revoke_and_epoch_are_effective_on_the_next_tick() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        let runtime = PresencePublisher::Runtime;
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            1,
        )
        .await
        .unwrap();
        assert!(gate(&authority, data, &principal, ME, runtime, 15)
            .await
            .is_some());

        // Revoked directly in the store: the record is untouched, the next
        // tick validates against the live store and stops.
        authority
            .manager()
            .revoke(*held_token(data, &principal).id(), "test")
            .await;
        assert_eq!(
            gate(&authority, data, &principal, ME, runtime, 30).await,
            None
        );
        // The dead grant is removed so Chat asks again.
        assert_eq!(state(data, &principal, &scope()).unwrap(), None);

        // An epoch bump silences every grant at once.
        let second = SECOND;
        for person in [principal.as_str(), second] {
            set_presence_grant(
                &authority,
                data,
                person,
                ME,
                &scope(),
                PresenceDecision::Always,
                40,
            )
            .await
            .unwrap();
            assert!(gate(&authority, data, person, ME, runtime, 45)
                .await
                .is_some());
        }
        authority.manager().revoke_all("test epoch");
        for person in [principal.as_str(), second] {
            assert_eq!(gate(&authority, data, person, ME, runtime, 60).await, None);
            assert_eq!(state(data, person, &scope()).unwrap(), None);
        }
    }

    #[tokio::test]
    async fn a_profile_change_revokes_the_grant() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            1,
        )
        .await
        .unwrap();
        let token = held_token(data, &principal);
        assert_eq!(
            gate(
                &authority,
                data,
                &principal,
                "did:key:zNewProfile",
                PresencePublisher::Runtime,
                2
            )
            .await,
            None
        );
        assert_eq!(state(data, &principal, &scope()).unwrap(), None);
        // Revoked in the store, not only forgotten: the old Profile cannot
        // bring it back.
        assert!(authority
            .manager()
            .validate(
                &token,
                &principal,
                Action::Message,
                &presence_resource(&scope(), PresenceMode::Always),
                None
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn audit_volume_is_bounded_over_an_hour_of_ticks() {
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            0,
        )
        .await
        .unwrap();
        let before = successes(&authority);
        // One hour of 15-second ticks, then the first tick of the next hour.
        for tick in 0..=240u64 {
            assert!(gate(
                &authority,
                data,
                &principal,
                ME,
                PresencePublisher::Runtime,
                tick * 15
            )
            .await
            .is_some());
        }
        assert_eq!(successes(&authority) - before, 2);
    }

    #[tokio::test]
    async fn the_grant_is_one_owner_only_file_that_holds_the_token() {
        // Undo to alpha.22 reads only the files it knows; the grant writes
        // nothing else, so their shapes stay exactly as alpha.22 wrote them.
        let (temp, principal) = home();
        let data = temp.path();
        let authority = test_authority();
        let before = files(data);
        let snapshot = |paths: &std::collections::BTreeSet<std::path::PathBuf>| {
            paths
                .iter()
                .map(|path| (path.clone(), std::fs::read(path).unwrap()))
                .collect::<Vec<_>>()
        };
        let before_bytes = snapshot(&before);
        request(data, &principal, &scope(), false, 1).unwrap();
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            2,
        )
        .await
        .unwrap();
        let path = object_path(data, &principal).unwrap();
        let after = files(data);
        assert_eq!(
            after.difference(&before).cloned().collect::<Vec<_>>(),
            vec![path.clone()]
        );
        assert_eq!(snapshot(&before), before_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let write = |value: serde_json::Value| {
            crate::auth::write_principal_root_object(
                data,
                &principal,
                &crate::auth::principal_localhost_root(&principal),
                &object_uri(&principal),
                &path,
                &serde_json::to_vec(&value).unwrap(),
            )
            .unwrap();
        };
        let root = crate::auth::principal_localhost_root(&principal);
        write(serde_json::json!({
            "schema": PRESENCE_GRANT_SCHEMA, "principal_id": principal,
            "localhost_root": root, "grants": [], "requests": [], "extra": 1,
        }));
        assert!(state(data, &principal, &scope()).is_err());
        write(serde_json::json!({
            "schema": PRESENCE_GRANT_SCHEMA, "principal_id": "someone-else",
            "localhost_root": root, "grants": [], "requests": [],
        }));
        assert!(state(data, &principal, &scope()).is_err());
        assert_eq!(
            gate(
                &authority,
                data,
                &principal,
                ME,
                PresencePublisher::Runtime,
                3
            )
            .await,
            None
        );
    }

    #[tokio::test]
    async fn revoked_ids_persist_in_the_store_files_alpha22_reads() {
        // Undo: the capability store gains only revoked ids; the same loader
        // reads it back, with the grant and with a refusal on file.
        let (temp, principal) = home();
        let data = temp.path();
        let store_dir = temp.path().join("capability-store");
        let store = Arc::new(
            elastos_runtime::capability::CapabilityStore::with_persistence(&store_dir)
                .await
                .unwrap(),
        );
        let authority = PresenceAuthority::new(Arc::new(CapabilityManager::new(
            store,
            Arc::new(elastos_runtime::primitives::audit::AuditLog::new()),
            Arc::new(elastos_runtime::primitives::metrics::MetricsManager::new()),
        )));
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Always,
            1,
        )
        .await
        .unwrap();
        let token = held_token(data, &principal);
        set_presence_grant(
            &authority,
            data,
            &principal,
            ME,
            &scope(),
            PresenceDecision::Never,
            2,
        )
        .await
        .unwrap();
        let reloaded = elastos_runtime::capability::CapabilityStore::with_persistence(&store_dir)
            .await
            .unwrap();
        assert!(reloaded.is_token_revoked(token.id()).await);
        assert_eq!(reloaded.current_epoch(), 0);
    }
}
