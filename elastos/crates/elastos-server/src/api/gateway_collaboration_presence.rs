use super::*;

const HOME_PRESENCE_HEARTBEAT_MS: u64 = 15_000;
const HOME_PRESENCE_REQUEST_ID_DOMAIN: &[u8] = b"elastos.home.presence-heartbeat.v1";
const PEOPLE_PRESENCE_SNAPSHOT_SCHEMA: &str = "elastos.people.presence.snapshot/v1";

#[derive(Serialize)]
struct HomeCollaborationPresenceResult {
    configured: bool,
    queued: bool,
    next_heartbeat_after_ms: u64,
}

#[derive(Serialize)]
struct PeopleCollaborationPresenceSnapshot {
    schema: &'static str,
    configured: bool,
    as_of: u64,
    online_count: usize,
    online: Vec<PeopleCollaborationPresenceRecord>,
}

#[derive(Serialize)]
struct PeopleCollaborationPresenceRecord {
    profile_did: String,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    last_seen_at: u64,
    expires_at: u64,
}

pub(super) struct AuthenticatedPresencePresentation {
    pub(super) principal_id: String,
    pub(super) profile:
        crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    pub(super) display_name: String,
    pub(super) handle: Option<String>,
    pub(super) principal_updated_at: u64,
    pub(super) profile_updated_at: Option<u64>,
}

pub(super) enum HomePresenceAuthenticationError {
    Denied,
    ProfileRequired,
    Internal,
}

/// Chat's heartbeat while its Community view is visible: the only publisher
/// for "Only while Community is open", and a refresh for "Always". The
/// launch token proves Chat asked; that the view is visible is Chat's word.
pub(super) async fn chat_room_presence_heartbeat(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if body.as_ref() != b"{}" {
        return (
            StatusCode::BAD_REQUEST,
            "presence heartbeat body must be exact canonical {}",
        )
            .into_response();
    }
    let presentation =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID)
            .map_err(|_| HomePresenceAuthenticationError::Denied)
            .and_then(|context| authenticated_presence_presentation_for_context(&state, &context))
        {
            Ok(presentation) => presentation,
            Err(HomePresenceAuthenticationError::Denied) => {
                return (StatusCode::FORBIDDEN, "Home presence authorization denied")
                    .into_response();
            }
            Err(HomePresenceAuthenticationError::ProfileRequired) => {
                return (
                    StatusCode::CONFLICT,
                    crate::api::gateway::gateway_home_system::PROFILE_REQUIRED_DISCOVERY_MESSAGE,
                )
                    .into_response();
            }
            Err(HomePresenceAuthenticationError::Internal) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Home presence presentation is unavailable",
                )
                    .into_response();
            }
        };
    let Some(port) = state.collaboration_presence_product_port.as_ref() else {
        return Json(HomeCollaborationPresenceResult {
            configured: false,
            queued: false,
            next_heartbeat_after_ms: HOME_PRESENCE_HEARTBEAT_MS,
        })
        .into_response();
    };
    // Not shown is a successful heartbeat that announces nothing, not an
    // error: the person asked for this, and Chat has no failure to report.
    // Chat presents only a `/while-open` grant; "Always" is the timer's.
    let granted = community_presence_mode(
        &state.data_dir,
        port,
        &presentation.principal_id,
        &presentation.profile,
        crate::collaboration_presence_grant::PresencePublisher::CommunityOpen,
        now_ts(),
    )
    .await;
    let Some(granted) = granted else {
        return Json(HomeCollaborationPresenceResult {
            configured: true,
            queued: false,
            next_heartbeat_after_ms: HOME_PRESENCE_HEARTBEAT_MS,
        })
        .into_response();
    };
    if let Err(error) = prepare_person_presence(port, &presentation, &granted, now_ts()) {
        return home_error_response(error);
    }
    Json(HomeCollaborationPresenceResult {
        configured: true,
        queued: true,
        next_heartbeat_after_ms: HOME_PRESENCE_HEARTBEAT_MS,
    })
    .into_response()
}

pub(super) async fn people_collaboration_presence(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, PEOPLE_CAPSULE_ID) {
            Ok(context) => context,
            Err(error) => return home_error_response(error),
        };
    let presentation = match authenticated_presence_presentation_for_context(&state, &context) {
        Ok(presentation) => presentation,
        Err(HomePresenceAuthenticationError::Denied) => {
            return (
                StatusCode::FORBIDDEN,
                "People presence authorization denied",
            )
                .into_response();
        }
        Err(HomePresenceAuthenticationError::ProfileRequired) => {
            return (
                StatusCode::CONFLICT,
                crate::api::gateway::gateway_home_system::PROFILE_REQUIRED_DISCOVERY_MESSAGE,
            )
                .into_response();
        }
        Err(HomePresenceAuthenticationError::Internal) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "People Profile is unavailable",
            )
                .into_response();
        }
    };
    let now = now_ts();
    let Some(port) = state.collaboration_presence_product_port.as_ref() else {
        return Json(PeopleCollaborationPresenceSnapshot {
            schema: PEOPLE_PRESENCE_SNAPSHOT_SCHEMA,
            configured: false,
            as_of: now,
            online_count: 0,
            online: Vec::new(),
        })
        .into_response();
    };
    let local_profile_did = &presentation.profile.document().profile_did;
    let snapshot = match port.authorized_snapshot(now).await {
        Ok(snapshot) => snapshot,
        Err(error) => return home_error_response(error),
    };
    let online = snapshot
        .records()
        .iter()
        .filter(|record| record.sender_profile_did() != local_profile_did)
        .map(|record| PeopleCollaborationPresenceRecord {
            profile_did: record.sender_profile_did().to_string(),
            display_name: record.display_name().to_string(),
            handle: record.handle().map(str::to_string),
            last_seen_at: record.last_seen_at(),
            expires_at: record.expires_at(),
        })
        .collect::<Vec<_>>();
    Json(PeopleCollaborationPresenceSnapshot {
        schema: PEOPLE_PRESENCE_SNAPSHOT_SCHEMA,
        configured: true,
        as_of: now,
        online_count: online.len(),
        online,
    })
    .into_response()
}

pub(super) fn authenticated_presence_presentation_for_context(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Result<AuthenticatedPresencePresentation, HomePresenceAuthenticationError> {
    let proof_binding_id = context
        .proof_binding_id
        .as_deref()
        .ok_or(HomePresenceAuthenticationError::Denied)?;
    let auth_state = crate::auth::load_auth_state(&state.data_dir)
        .map_err(|_| HomePresenceAuthenticationError::Internal)?;
    let principal = auth_state
        .principals
        .into_iter()
        .find(|principal| principal.proof_binding_id == proof_binding_id)
        .ok_or(HomePresenceAuthenticationError::Denied)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)
        .map_err(|_| HomePresenceAuthenticationError::Denied)?;
    if principal.principal_id != context.principal_id {
        return Err(HomePresenceAuthenticationError::Denied);
    }
    let localhost_root = crate::auth::principal_localhost_root(&context.principal_id);
    let profile = crate::collaboration_profile_authority::load_profile_authority(
        &state.data_dir,
        &context.principal_id,
        &localhost_root,
    )
    .map_err(|_| HomePresenceAuthenticationError::Internal)?;
    let profile = profile.ok_or(HomePresenceAuthenticationError::ProfileRequired)?;
    let display_name = profile.document().display_name.trim().to_string();
    if display_name.is_empty() {
        return Err(HomePresenceAuthenticationError::Internal);
    }
    Ok(AuthenticatedPresencePresentation {
        principal_id: principal.principal_id,
        profile: profile.clone(),
        display_name,
        handle: profile.document().handle.clone(),
        principal_updated_at: principal.updated_at,
        profile_updated_at: Some(profile.document().updated_at),
    })
}

const COMMUNITY_PRESENCE_SCHEMA: &str = "elastos.community-presence/v1";
/// Inbox card actions: `community-presence-<always|open|later|never>:<request>`.
pub(super) const COMMUNITY_PRESENCE_ACTIONS: [(
    &str,
    crate::collaboration_presence_grant::PresenceDecision,
); 4] = [
    (
        "community-presence-always:",
        crate::collaboration_presence_grant::PresenceDecision::Always,
    ),
    (
        "community-presence-open:",
        crate::collaboration_presence_grant::PresenceDecision::WhileOpen,
    ),
    (
        "community-presence-later:",
        crate::collaboration_presence_grant::PresenceDecision::Later,
    ),
    (
        "community-presence-never:",
        crate::collaboration_presence_grant::PresenceDecision::Never,
    ),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommunityPresenceModeRequest {
    mode: String,
}

type PresenceSubject<'a> = (
    &'a crate::collaboration_presence::CollaborationPresenceProductPort,
    String,
    crate::collaboration_presence_grant::PresenceScope,
);

/// The caller's own Profile DID and Community scope, when Community exists.
fn presence_subject<'a>(
    state: &'a GatewayState,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<PresenceSubject<'a>>> {
    let Some(port) = state.collaboration_presence_product_port.as_ref() else {
        return Ok(None);
    };
    let Some(profile) =
        gateway_home_system::load_profile_authority_for_context(&state.data_dir, context)?
    else {
        anyhow::bail!("Create your Profile before choosing how Community shows you.");
    };
    Ok(Some((
        port,
        profile.document().profile_did.clone(),
        port.presence_scope(),
    )))
}

/// The caller's presence state for Chat and System: `requested`,
/// `deferred`, `refused`, `hidden`, `always`, `while_open`, or none. The
/// mode only; the token never leaves the Runtime.
pub(super) fn community_presence_state(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Option<&'static str> {
    let (_, _, scope) = presence_subject(state, context).ok()??;
    crate::collaboration_presence_grant::state(&state.data_dir, &context.principal_id, &scope)
        .ok()?
}

/// Chat asks on Community open (`asked_by_person` false), or when the person
/// taps "Show me in Community" after Later. It never grants anything.
pub(super) fn request_community_presence(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    asked_by_person: bool,
) -> anyhow::Result<()> {
    let Some((port, _, scope)) = presence_subject(state, context)? else {
        return Ok(());
    };
    if !port.community_joined() || port.presence_authority().is_none() {
        return Ok(());
    }
    crate::collaboration_presence_grant::request(
        &state.data_dir,
        &context.principal_id,
        &scope,
        asked_by_person,
        now_ts(),
    )
}

/// Chat's "Show me in Community" line: opens the Inbox card again.
pub(super) async fn chat_room_community_presence_request(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, CHAT_ROOM_CAPSULE_ID) {
            Ok(context) => context,
            Err(error) => return home_error_response(error),
        };
    if let Err(error) = request_community_presence(&state, &context, true) {
        return home_error_response(error);
    }
    community_presence_response(&state, &context)
}

pub(super) async fn system_community_presence_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
        Ok(context) => community_presence_response(&state, &context),
        Err(error) => home_error_response(error),
    }
}

/// System → Community, any role, own grant only: `always`, `while_open` or
/// `hidden`.
pub(super) async fn system_community_presence_update(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(body): Json<CommunityPresenceModeRequest>,
) -> Response {
    use crate::collaboration_presence_grant::PresenceDecision;
    let decision = match body.mode.as_str() {
        "always" => PresenceDecision::Always,
        "while_open" => PresenceDecision::WhileOpen,
        "hidden" => PresenceDecision::Hidden,
        _ => return (StatusCode::BAD_REQUEST, "unknown Community presence mode").into_response(),
    };
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, SYSTEM_CAPSULE_ID) {
            Ok(context) => context,
            Err(error) => return home_error_response(error),
        };
    if let Err(error) = set_community_presence(&state, &context, decision).await {
        return home_error_response(error);
    }
    community_presence_response(&state, &context)
}

/// The only place a presence token is minted or revoked: the person's own
/// answer in Inbox or System, for the principal of that launch token.
async fn set_community_presence(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    decision: crate::collaboration_presence_grant::PresenceDecision,
) -> anyhow::Result<()> {
    let (port, profile_did, scope) = presence_subject(state, context)?
        .ok_or_else(|| anyhow::anyhow!("Community is not configured on this Home."))?;
    let authority = port
        .presence_authority()
        .ok_or_else(|| anyhow::anyhow!("Community presence is not available on this Home."))?;
    crate::collaboration_presence_grant::set_presence_grant(
        authority,
        &state.data_dir,
        &context.principal_id,
        &profile_did,
        &scope,
        decision,
        now_ts(),
    )
    .await?;
    // Each answer retires the previous projection and queued announcements.
    // The selected publisher prepares fresh presence under the current grant.
    port.retire_own_presence(&context.principal_id, &profile_did)?;
    Ok(())
}

fn community_presence_response(state: &GatewayState, context: &HomeLaunchTokenContext) -> Response {
    let joined = state
        .collaboration_presence_product_port
        .as_ref()
        .map(|port| port.community_joined());
    Json(serde_json::json!({
        "schema": COMMUNITY_PRESENCE_SCHEMA,
        "configured": joined.is_some(),
        "joined": joined.unwrap_or(false),
        "community_presence": community_presence_state(state, context),
    }))
    .into_response()
}

/// The Inbox card's answer, for the person signed in to Inbox only: the
/// record is that person's by construction.
pub(super) async fn answer_community_presence_request(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    action_id: &str,
) -> Option<anyhow::Result<String>> {
    use crate::collaboration_presence_grant::PresenceDecision;
    let (decision, request) = COMMUNITY_PRESENCE_ACTIONS
        .iter()
        .find_map(|(prefix, decision)| Some((*decision, action_id.strip_prefix(prefix)?)))?;
    let answer = async {
        let (_, _, scope) = presence_subject(state, context)?
            .ok_or_else(|| anyhow::anyhow!("Community is not configured on this Home."))?;
        let open = crate::collaboration_presence_grant::open_request_at(
            &state.data_dir,
            &context.principal_id,
            &scope,
        )?;
        if open.map(|at| at.to_string()).as_deref() != Some(request) {
            anyhow::bail!("That Community request is no longer open.");
        }
        set_community_presence(state, context, decision).await?;
        Ok(match decision {
            PresenceDecision::Always => "Community shows you while your Home is online.",
            PresenceDecision::WhileOpen => "Community shows you while Community is open.",
            PresenceDecision::Later => "Later. Show me in Community, in Chat, asks again.",
            _ => "Hidden. Change it in System → Community.",
        }
        .to_string())
    };
    Some(answer.await)
}

/// The one Inbox card a person with an open request sees.
pub(super) fn community_presence_request_notification(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
) -> Option<HomeNotificationEntrySummary> {
    let (_, _, scope) = presence_subject(state, context).ok()??;
    let issued_at = crate::collaboration_presence_grant::open_request_at(
        &state.data_dir,
        &context.principal_id,
        &scope,
    )
    .ok()??;
    Some(HomeNotificationEntrySummary {
        id: format!("community-presence-request:{issued_at}"),
        source_app: CHAT_ROOM_CAPSULE_ID.to_string(),
        kind: "community_presence_request".to_string(),
        title: "Show yourself in Community?".to_string(),
        body: "Community members would see your name, your Profile ID, this Home's device \
               address, and that your Home is online, announced every 15 seconds while you are \
               shown. Every Home in Community sees it, including the seed's Community node. It \
               lets members see who is online and lets you load earlier posts. Change it any \
               time in System → Community."
            .to_string(),
        action_ref: Some(HomeNotificationActionSummary {
            app: INBOX_CAPSULE_ID.to_string(),
            action_id: format!("{}{issued_at}", COMMUNITY_PRESENCE_ACTIONS[0].0),
        }),
        severity: "attention".to_string(),
        read: false,
        created_at: issued_at,
    })
}

fn prepare_person_presence(
    port: &crate::collaboration_presence::CollaborationPresenceProductPort,
    presentation: &AuthenticatedPresencePresentation,
    grant: &crate::collaboration_presence_grant::GrantedPresence,
    now: u64,
) -> anyhow::Result<()> {
    let request_id = grant.bind_request_id(&presence_heartbeat_request_id(presentation, now)?);
    let operation = crate::collaboration_presence::presence_request_binding(
        &request_id,
        &presentation.principal_id,
        &presentation.profile,
    )?;
    port.prepare_presence(operation, &presentation.profile, now)?;
    Ok(())
}

/// Announce the people who live on this Home, from the Runtime.
///
/// Presence is a gossip announcement with a lifetime, so it has to be
/// refreshed. Everything the announcement carries is durable: the principal
/// record and its signed Profile. This publishes, once per pass, for every
/// person on the Home whose grant says "Always while Home is online", after
/// validating that person's token against the live capability store.
pub async fn publish_runtime_owned_presence(
    data_dir: &std::path::Path,
    port: &crate::collaboration_presence::CollaborationPresenceProductPort,
    now: u64,
) -> anyhow::Result<usize> {
    let pass = publish_runtime_owned_presence_pass(data_dir, port, now).await;
    // History asks the person to show themselves only after a pass that
    // read every person and found no grant; any error leaves it unknown.
    port.record_local_publication(match &pass {
        Ok((published, _)) if *published > 0 => Some(true),
        Ok((_, true)) => Some(false),
        _ => None,
    });
    pass.map(|(published, _)| published)
}

/// Returns how many people were announced and whether nobody here has any
/// presence grant.
async fn publish_runtime_owned_presence_pass(
    data_dir: &std::path::Path,
    port: &crate::collaboration_presence::CollaborationPresenceProductPort,
    now: u64,
) -> anyhow::Result<(usize, bool)> {
    if !port.community_joined() {
        return Ok((0, false));
    }
    let mut published = 0usize;
    let mut nobody_granted = true;
    let scope = port.presence_scope();
    for principal in crate::auth::active_passkey_principals(data_dir)? {
        let localhost_root = crate::auth::principal_localhost_root(&principal.principal_id);
        let Some(profile) = crate::collaboration_profile_authority::load_profile_authority(
            data_dir,
            &principal.principal_id,
            &localhost_root,
        )?
        else {
            continue;
        };
        let display_name = profile.document().display_name.trim().to_string();
        if display_name.is_empty() {
            continue;
        }
        let granted = community_presence_mode(
            data_dir,
            port,
            &principal.principal_id,
            &profile,
            crate::collaboration_presence_grant::PresencePublisher::Runtime,
            now,
        )
        .await;
        let Some(granted) = granted else {
            // Someone shown only while Community is open still counts as a
            // grant on this Home for the History hint.
            if crate::collaboration_presence_grant::state(data_dir, &principal.principal_id, &scope)
                .ok()
                .flatten()
                == Some("while_open")
            {
                nobody_granted = false;
            }
            continue;
        };
        nobody_granted = false;
        let presentation = AuthenticatedPresencePresentation {
            principal_id: principal.principal_id.clone(),
            profile: profile.clone(),
            display_name: display_name.clone(),
            handle: profile.document().handle.clone(),
            principal_updated_at: principal.updated_at,
            profile_updated_at: Some(profile.document().updated_at),
        };
        prepare_person_presence(port, &presentation, &granted, now)?;
        published = published.saturating_add(1);
    }
    Ok((published, nobody_granted))
}

/// The one question every publisher asks: may this publisher announce this
/// person in this Community now? `None`: not joined, no grant for this
/// publisher, not decided, unreadable, or the token no longer validates, so
/// the Home says nothing. Leaving Community stops it for everyone; Discovery
/// is a different choice and is not consulted.
pub(super) async fn community_presence_mode(
    data_dir: &std::path::Path,
    port: &crate::collaboration_presence::CollaborationPresenceProductPort,
    principal_id: &str,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    publisher: crate::collaboration_presence_grant::PresencePublisher,
    now: u64,
) -> Option<crate::collaboration_presence_grant::GrantedPresence> {
    let granted = match port.presence_authority() {
        Some(authority) if port.community_joined() => {
            crate::collaboration_presence_grant::presence_grant(
                authority,
                data_dir,
                principal_id,
                &profile.document().profile_did,
                &port.presence_scope(),
                publisher,
                now,
            )
            .await
        }
        _ => None,
    };
    if granted.is_none() {
        // A publisher-mode mismatch can leave the other publisher's grant
        // valid. The common use gate retires state only when that grant also
        // fails, including Hidden, revocation and an epoch change.
        port.retire_denied_current_presence(principal_id, &profile.document().profile_did, now)
            .await;
    }
    granted
}

fn presence_heartbeat_request_id(
    presentation: &AuthenticatedPresencePresentation,
    now: u64,
) -> anyhow::Result<String> {
    let value = serde_json::json!({
        "bucket": now / (HOME_PRESENCE_HEARTBEAT_MS / 1_000),
        "display_name": presentation.display_name,
        "handle": presentation.handle,
        "principal_id": presentation.principal_id,
        "principal_updated_at": presentation.principal_updated_at,
        "profile_updated_at": presentation.profile_updated_at,
    });
    let mut digest = Sha256::new();
    digest.update(HOME_PRESENCE_REQUEST_ID_DOMAIN);
    digest.update([0]);
    digest.update(serde_json::to_vec(&value)?);
    Ok(format!("presence:{}", hex::encode(digest.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration_profile_authority::signed_profile_document_for_test;
    use ed25519_dalek::SigningKey;
    use elastos_runtime::signature::generate_keypair;

    fn test_profile(
        display_name: &str,
        handle: Option<&str>,
    ) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        let profile_key = generate_keypair().0;
        let device_key = generate_keypair().0;
        signed_profile_document_for_test(
            &profile_key,
            display_name,
            handle,
            1,
            None,
            1,
            vec![crate::crypto::encode_signing_key_did(&device_key)],
        )
        .unwrap()
    }

    #[test]
    fn runtime_presence_publishes_from_durable_state_without_a_session() {
        // Presence used to require a signed-in browser tab, so two running
        // Homes showed each other Offline with nobody looking. Everything
        // the announcement carries is on disk, so a Home with no Profile
        // has nothing to say and a Home with one says it unprompted.
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        elastos_identity::load_or_create_did(temp.path()).unwrap();

        // No principals yet: nothing to announce, and no error.
        let none = crate::auth::active_passkey_principals(temp.path()).unwrap();
        assert!(none.is_empty());

        let now_created = crate::auth::now_ts();
        let rp_id = "elastos.elacitylabs.com";
        let credential_id = "presence-runtime-passkey";
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: credential_id.to_string(),
                public_key: "presence-runtime-public-key".to_string(),
                sign_count: 1,
                user_verified: true,
                origin: "https://elastos.elacitylabs.com".to_string(),
                rp_id: rp_id.to_string(),
                created_at: now_created,
                last_used_at: now_created,
                revoked_at: None,
            },
        );
        let principal_id =
            crate::auth::passkey_credential_principal_id(rp_id, credential_id).unwrap();
        let principal = crate::auth::upsert_principal_for_binding_as_role_named(
            temp.path(),
            binding,
            principal_id.clone(),
            crate::auth::RuntimePrincipalRole::Admin,
            Some("Runtime Announced"),
            now_created,
        )
        .unwrap();
        let protection =
            crate::auth::store_test_principal_root_protection(temp.path(), &principal.principal_id);
        crate::collaboration_profile_authority::update_profile_authority(
            temp.path(),
            &principal.principal_id,
            &protection.localhost_root,
            &principal.proof_binding_id,
            "Runtime Announced",
            None,
            crate::auth::now_ts(),
        )
        .unwrap();
        let principal_id = principal.principal_id.as_str();

        // The presentation the Runtime would announce comes entirely from
        // the stored principal and its signed Profile.
        let principals = crate::auth::active_passkey_principals(temp.path()).unwrap();
        assert_eq!(principals.len(), 1);
        let profile = crate::collaboration_profile_authority::load_profile_authority(
            temp.path(),
            principal_id,
            &protection.localhost_root,
        )
        .unwrap()
        .expect("the Profile is on disk");
        assert_eq!(profile.document().display_name, "Runtime Announced");

        let presentation = AuthenticatedPresencePresentation {
            principal_id: principals[0].principal_id.clone(),
            profile: profile.clone(),
            display_name: profile.document().display_name.clone(),
            handle: profile.document().handle.clone(),
            principal_updated_at: principals[0].updated_at,
            profile_updated_at: Some(profile.document().updated_at),
        };
        let now = crate::auth::now_ts();
        let request_id = presence_heartbeat_request_id(&presentation, now).unwrap();
        crate::collaboration_presence::presence_request_binding(
            &request_id,
            &presentation.principal_id,
            &presentation.profile,
        )
        .expect("a Runtime-built presence announcement binds like a browser's");
    }

    fn presence_test_network_profile(
        signing_key: &SigningKey,
        network_id: &str,
    ) -> crate::collaboration_network::VerifiedCollaborationNetworkProfile {
        let signer_did = crate::crypto::encode_signing_key_did(signing_key);
        let payload = crate::collaboration_network::CollaborationNetworkProfile {
            schema: crate::collaboration_network::COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: None,
        };
        let payload_bytes =
            crate::collaboration_network::canonical_collaboration_network_profile_payload_bytes(
                &payload,
            )
            .unwrap();
        let (signature, envelope_signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            crate::collaboration_network::COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let bytes = serde_json::to_vec(
            &serde_json::to_value(
                crate::collaboration_network::SignedCollaborationNetworkProfile {
                    payload,
                    signature,
                    signer_did: envelope_signer_did,
                },
            )
            .unwrap(),
        )
        .unwrap();
        match crate::collaboration_network::validate_collaboration_network_profile(
            Some(&bytes),
            network_id,
            &[signer_did],
            None,
        )
        .unwrap()
        {
            crate::collaboration_network::CollaborationNetworkProfileMode::Configured(profile) => {
                profile
            }
            crate::collaboration_network::CollaborationNetworkProfileMode::Isolated => {
                panic!("configured discovery profile expected")
            }
        }
    }

    /// Announcing from the Runtime means announcing whenever this Home is up,
    /// so the person's own Community choice is what stands between them and
    /// being announced around the clock. Joining Community is automatic, so
    /// it is not that choice; Discovery is a different one. Leaving Community
    /// stops it for everyone on the Home.
    #[tokio::test]
    async fn runtime_presence_follows_the_persons_community_choice_not_discovery() {
        const NETWORK: &str = "presence-runtime-opt-in";
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        elastos_identity::load_or_create_did(temp.path()).unwrap();

        let now_created = crate::auth::now_ts();
        let rp_id = "elastos.elacitylabs.com";
        let credential_id = "presence-opt-in-passkey";
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: credential_id.to_string(),
                public_key: "presence-opt-in-public-key".to_string(),
                sign_count: 1,
                user_verified: true,
                origin: "https://elastos.elacitylabs.com".to_string(),
                rp_id: rp_id.to_string(),
                created_at: now_created,
                last_used_at: now_created,
                revoked_at: None,
            },
        );
        let principal_id =
            crate::auth::passkey_credential_principal_id(rp_id, credential_id).unwrap();
        let principal = crate::auth::upsert_principal_for_binding_as_role_named(
            temp.path(),
            binding,
            principal_id,
            crate::auth::RuntimePrincipalRole::Admin,
            Some("Quiet By Default"),
            now_created,
        )
        .unwrap();
        let protection =
            crate::auth::store_test_principal_root_protection(temp.path(), &principal.principal_id);
        crate::collaboration_profile_authority::update_profile_authority(
            temp.path(),
            &principal.principal_id,
            &protection.localhost_root,
            &principal.proof_binding_id,
            "Quiet By Default",
            None,
            crate::auth::now_ts(),
        )
        .unwrap();
        let profile = crate::collaboration_profile_authority::load_profile_authority(
            temp.path(),
            &principal.principal_id,
            &protection.localhost_root,
        )
        .unwrap()
        .expect("the Profile is on disk");
        let local_device_did =
            crate::collaboration_profile_authority::load_existing_device_did(temp.path())
                .unwrap()
                .expect("the Home has a device identity");

        let (trusted_key, _) = generate_keypair();
        let network_profile = presence_test_network_profile(&trusted_key, NETWORK);
        let chat =
            crate::collaboration_product::test_chat_product_port(temp.path(), NETWORK, "community");
        let authority = crate::collaboration_presence_grant::test_authority();
        let port = crate::collaboration_presence::CollaborationPresenceProductPort::new(
            chat.test_core().clone(),
        )
        .unwrap()
        .with_presence_authority(authority.clone());
        let scope = port.presence_scope();
        let decide = |decision| {
            crate::collaboration_presence_grant::set_presence_grant(
                &authority,
                temp.path(),
                &principal.principal_id,
                &profile.document().profile_did,
                &scope,
                decision,
                crate::auth::now_ts(),
            )
        };
        use crate::collaboration_presence_grant::PresenceDecision;
        let now = crate::auth::now_ts();

        // A joined Home whose person has not chosen yet stays quiet, and
        // History learns that nobody here chose to be shown.
        assert_eq!(port.local_publication(), None);
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now)
                .await
                .unwrap(),
            0
        );
        assert_eq!(port.local_publication(), Some(false));

        // Discovery is a different choice: switching it on announces nobody.
        let store = crate::collaboration_contact_store::CollaborationContactStore::new(
            temp.path(),
            &principal.principal_id,
            &protection.localhost_root,
            network_profile,
            &profile,
            &local_device_did,
        )
        .unwrap();
        store.set_discovery_enabled(true, now).unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now)
                .await
                .unwrap(),
            0
        );

        // Only while Community is open: the timer still says nothing, but a
        // grant exists on this Home.
        store.set_discovery_enabled(false, now).unwrap();
        decide(PresenceDecision::WhileOpen).await.unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now)
                .await
                .unwrap(),
            0
        );
        assert_eq!(port.local_publication(), None);

        // Always, with Discovery off: the Runtime now announces them
        // without a browser open.
        decide(PresenceDecision::Always).await.unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now)
                .await
                .unwrap(),
            1
        );
        assert_eq!(port.local_publication(), Some(true));
        let prepared = port.pending_outgoing_presences(now).unwrap();
        assert_eq!(prepared.len(), 1);
        assert_eq!(
            prepared[0].sender_profile_did(),
            profile.document().profile_did
        );
        assert!(prepared[0].expires_at() <= now + crate::collaboration_presence::PRESENCE_TTL_SECS);
        port.project_prepared_presence(&prepared[0], now).unwrap();
        assert_eq!(port.snapshot(now).unwrap().records().len(), 1);
        // Presence is not a post: it leaves no replay tombstone behind.
        assert_eq!(chat.test_core().summary().unwrap().replay_tombstones, 0);

        // The next publisher pass retires both local and queued presence.
        decide(PresenceDecision::Hidden).await.unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now + 15)
                .await
                .unwrap(),
            0
        );
        assert!(port.snapshot(now + 15).unwrap().records().is_empty());
        assert!(chat
            .test_core()
            .pending_outgoing(now + 15)
            .unwrap()
            .is_empty());
        decide(PresenceDecision::Always).await.unwrap();

        // Without the Runtime's capability manager no grant is honoured.
        let unmanaged = crate::collaboration_presence::CollaborationPresenceProductPort::new(
            chat.test_core().clone(),
        )
        .unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &unmanaged, now + 15)
                .await
                .unwrap(),
            0
        );

        // Leaving Community stops it for everyone, whatever they chose.
        chat.test_core().set_community_joined(false).unwrap();
        assert_eq!(
            publish_runtime_owned_presence(temp.path(), &port, now + 30)
                .await
                .unwrap(),
            0
        );
    }

    #[test]
    fn request_id_coalesces_one_profile_per_bucket_and_changes_with_presentation() {
        let first = AuthenticatedPresencePresentation {
            principal_id: "principal:one".to_string(),
            profile: test_profile("Alice", Some("alice")),
            display_name: "Alice".to_string(),
            handle: Some("alice".to_string()),
            principal_updated_at: 7,
            profile_updated_at: Some(11),
        };
        let inside_bucket = presence_heartbeat_request_id(&first, 30).unwrap();
        assert_eq!(
            inside_bucket,
            presence_heartbeat_request_id(&first, 44).unwrap()
        );
        assert_ne!(
            inside_bucket,
            presence_heartbeat_request_id(&first, 45).unwrap()
        );
        let changed = AuthenticatedPresencePresentation {
            principal_id: first.principal_id.clone(),
            profile: test_profile("Alice Updated", Some("alice")),
            display_name: "Alice Updated".to_string(),
            handle: first.handle.clone(),
            principal_updated_at: first.principal_updated_at,
            profile_updated_at: Some(12),
        };
        assert_ne!(
            inside_bucket,
            presence_heartbeat_request_id(&changed, 44).unwrap()
        );
    }
}
