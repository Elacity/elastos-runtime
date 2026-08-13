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

pub(super) async fn home_collaboration_presence(
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
    let presentation = match authenticated_home_presence_presentation(&state, &headers) {
        Ok(presentation) => presentation,
        Err(HomePresenceAuthenticationError::Denied) => {
            return (StatusCode::FORBIDDEN, "Home presence authorization denied").into_response();
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
    // A tab publishes on behalf of the same person the Runtime does, to the
    // same open topic, so it obeys the same switch. Discovery off is a
    // successful heartbeat that announces nothing, not an error: the person
    // asked for this, and the tab has no failure to report.
    let local_device_did =
        crate::collaboration_profile_authority::load_existing_device_did(&state.data_dir)
            .ok()
            .flatten();
    if !presence_opted_in(
        &state.data_dir,
        state.collaboration_discovery_service.as_ref(),
        &presentation.principal_id,
        &presentation.profile,
        local_device_did.as_deref(),
    ) {
        return Json(HomeCollaborationPresenceResult {
            configured: true,
            queued: false,
            next_heartbeat_after_ms: HOME_PRESENCE_HEARTBEAT_MS,
        })
        .into_response();
    }
    let now = now_ts();
    let request_id = match presence_heartbeat_request_id(&presentation, now) {
        Ok(request_id) => request_id,
        Err(error) => return home_error_response(error),
    };
    let operation = match crate::collaboration_presence::presence_request_binding(
        &request_id,
        &presentation.principal_id,
        &presentation.profile,
    ) {
        Ok(operation) => operation,
        Err(error) => return home_error_response(error),
    };
    if let Err(error) = port.prepare_presence(operation, &presentation.profile, now) {
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
    let snapshot = match port.snapshot(now) {
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

pub(super) fn authenticated_home_presence_presentation(
    state: &GatewayState,
    headers: &HeaderMap,
) -> Result<AuthenticatedPresencePresentation, HomePresenceAuthenticationError> {
    let context = require_home_token_context(&state.data_dir, headers)
        .map_err(|_| HomePresenceAuthenticationError::Denied)?;
    authenticated_presence_presentation_for_context(state, &context)
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

/// Announce the people who live on this Home, from the Runtime.
///
/// Presence is a gossip announcement with a lifetime, so it has to be
/// refreshed — but it was refreshed by a browser tab, once per tab, and it
/// vanished when the tab closed, so two running Homes showed each other
/// Offline. Everything the announcement carries is durable: the principal
/// record and its signed Profile. The session only ever said which person
/// was asking. This publishes for every Profile on the Home, once, no
/// matter how many tabs are open or whether any are.
pub fn publish_runtime_owned_presence(
    data_dir: &std::path::Path,
    port: &crate::collaboration_presence::CollaborationPresenceProductPort,
    discovery_service: Option<
        &crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    >,
    now: u64,
) -> anyhow::Result<usize> {
    let local_device_did =
        crate::collaboration_profile_authority::load_existing_device_did(data_dir)?;
    let mut published = 0usize;
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
        // Announcing from the Runtime means announcing whenever this Home is
        // up, not only while someone is looking at it — so it must obey the
        // switch the person actually set. A Home whose owner has Discovery
        // off says nothing, exactly as they asked.
        if !presence_opted_in(
            data_dir,
            discovery_service,
            &principal.principal_id,
            &profile,
            local_device_did.as_deref(),
        ) {
            continue;
        }
        let presentation = AuthenticatedPresencePresentation {
            principal_id: principal.principal_id.clone(),
            profile: profile.clone(),
            display_name: display_name.clone(),
            handle: profile.document().handle.clone(),
            principal_updated_at: principal.updated_at,
            profile_updated_at: Some(profile.document().updated_at),
        };
        let request_id = presence_heartbeat_request_id(&presentation, now)?;
        let operation = crate::collaboration_presence::presence_request_binding(
            &request_id,
            &presentation.principal_id,
            &presentation.profile,
        )?;
        port.prepare_presence(operation, &presentation.profile, now)?;
        published = published.saturating_add(1);
    }
    Ok(published)
}

/// Has this person opted in to being announced? Presence rides the same
/// bounded Discovery choice the product already offers; absent a contact
/// store to ask, the Home says nothing rather than assuming consent.
///
/// Both publishers ask this. The browser's heartbeat and the Runtime's
/// timer announce the same person to the same open topic, so gating one
/// and not the other would leave the person announced whenever they had
/// their Home open — which is exactly the leak, only quieter.
fn presence_opted_in(
    data_dir: &std::path::Path,
    discovery_service: Option<
        &crate::collaboration_discovery_runtime::CollaborationDiscoveryService,
    >,
    principal_id: &str,
    profile: &crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument,
    local_device_did: Option<&str>,
) -> bool {
    let (Some(service), Some(device_did)) = (discovery_service, local_device_did) else {
        return false;
    };
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let Ok(store) = crate::collaboration_contact_store::CollaborationContactStore::new(
        data_dir,
        principal_id,
        &localhost_root,
        service.network_profile(),
        profile,
        device_did,
    ) else {
        return false;
    };
    store.discovery_enabled().unwrap_or(false)
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
        let signer_did = crate::crypto::encode_signing_key_did(&signing_key);
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
    /// so the Discovery switch is the only thing standing between a person
    /// and being broadcast around the clock. A Home that publishes for
    /// someone who never turned Discovery on is a privacy defect, not a
    /// missing feature, so this asserts the silence directly.
    #[tokio::test]
    async fn runtime_presence_says_nothing_until_discovery_is_switched_on() {
        const NETWORK: &str = "presence-runtime-opt-in";
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (runtime_device_key, _) = elastos_identity::load_or_create_did(temp.path()).unwrap();

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
        let discovery_service =
            crate::collaboration_discovery_runtime::CollaborationDiscoveryService::new(
                SigningKey::from_bytes(&runtime_device_key.to_bytes()),
                network_profile.clone(),
                std::sync::Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
            )
            .await
            .unwrap();

        // A Home with a Profile and a running Runtime, whose owner has not
        // asked to be found: it stays quiet.
        assert!(
            !presence_opted_in(
                temp.path(),
                Some(&discovery_service),
                &principal.principal_id,
                &profile,
                Some(local_device_did.as_str()),
            ),
            "a Home announces nobody who has not switched Discovery on"
        );

        // Without a discovery service, or without a device identity, consent
        // cannot be established at all — so it is not assumed.
        assert!(
            !presence_opted_in(
                temp.path(),
                None,
                &principal.principal_id,
                &profile,
                Some(local_device_did.as_str()),
            ),
            "no discovery service means no announcement, not an assumed yes"
        );
        assert!(
            !presence_opted_in(
                temp.path(),
                Some(&discovery_service),
                &principal.principal_id,
                &profile,
                None,
            ),
            "no device identity means no announcement, not an assumed yes"
        );

        // The person opens People and switches Discovery on. Only now does
        // the Runtime speak for them.
        let store = crate::collaboration_contact_store::CollaborationContactStore::new(
            temp.path(),
            &principal.principal_id,
            &protection.localhost_root,
            network_profile,
            &profile,
            &local_device_did,
        )
        .unwrap();
        store
            .set_discovery_enabled(true, crate::auth::now_ts())
            .unwrap();
        assert!(
            presence_opted_in(
                temp.path(),
                Some(&discovery_service),
                &principal.principal_id,
                &profile,
                Some(local_device_did.as_str()),
            ),
            "a person who switched Discovery on is announced without a browser open"
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
