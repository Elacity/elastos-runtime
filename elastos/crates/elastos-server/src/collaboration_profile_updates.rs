//! Runtime-owned signed Profile update delivery.
//!
//! An accepted person may change their signed display name or authorized device
//! without becoming a new contact, changing the conversation identity, or
//! requiring another approval. Runtime announces its own signed revision chain
//! to accepted contacts and applies theirs.
//!
//! This is deliberately not a payload on the direct message provider. That
//! provider resolves a contact by endpoint, while a Profile update must resolve
//! by Profile DID because the endpoint is exactly what may be changing, and
//! folding the two together would give Chat identity authority.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use base64::Engine as _;
use elastos_common::collaboration_protocol::{
    canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
    CollaborationMessage, CollaborationRecipient, CollaborationRecipientKind,
    SignedCollaborationMessage, COLLABORATION_MESSAGE_SCHEMA_V1,
    COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1, MAX_COLLABORATION_PAYLOAD_BYTES,
};
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer,
};
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};

use crate::collaboration_contact_store::CollaborationContactStore;
use crate::collaboration_network::VerifiedCollaborationNetworkProfile;
use crate::collaboration_profile_authority::{
    profile_chain_segment, SignedCollaborationProfileDocument,
    VerifiedCollaborationProfileDocument, MAX_RETAINED_PROFILE_REVISIONS,
};
use crate::collaboration_protocol::{
    validate_id, validate_payload_type, verify_collaboration_acceptance_receipt,
    verify_collaboration_message,
};
use crate::crypto::{domain_separated_sign, encode_did_key};
use elastos_logger::log_trace;

const LOG_COMPONENT: &str = "collab";

pub(crate) const PROFILE_UPDATE_PROVIDER_SCHEME: &str = "collaboration-profile";
pub(crate) const PROFILE_UPDATE_PROVIDER_OP: &str = "announce";
pub(crate) const PROFILE_UPDATE_PAYLOAD_TYPE: &str = "elastos.people.profile-update/v1";
const PROFILE_UPDATE_SENDER_SERVICE: &str = "people";
const PROFILE_UPDATE_TTL_SECS: u64 = 2 * 60;
const PROFILE_UPDATE_TIMEOUT_MS: u64 = 5_000;
const MAX_PROFILE_UPDATE_CONTEXTS: usize = 32;
const MAX_PROFILE_UPDATE_WIRE_BASE64_BYTES: usize = 128 * 1024;
const MAX_PROFILE_ANNOUNCEMENTS_PER_PASS: usize = 4;
/// Declared end-of-life for Profile update announcements: envelopes are
/// regenerated from the durable signed head on every pass, so nothing
/// durable ever expires and a restart simply re-announces.
const DECLARED_PROFILE_UPDATE_END_OF_LIFE: crate::collaboration_delivery::DeliveryEndOfLife =
    crate::collaboration_delivery::DeliveryEndOfLife::RegenerateFromTruth;

#[derive(Clone)]
pub(crate) struct CollaborationProfileUpdateService {
    inner: Arc<ProfileUpdateInner>,
}

struct ProfileUpdateInner {
    signing_key: SigningKey,
    network: VerifiedCollaborationNetworkProfile,
    registry: Arc<ProviderRegistry>,
    contexts: Mutex<BTreeMap<String, ProfileUpdateContext>>,
    /// Highest local revision a contact has acknowledged, keyed by local and
    /// remote Profile DID. Announcement is a pure function of the local head and
    /// the accepted contacts, so this only avoids repeat traffic; losing it on
    /// restart re-announces, and the receiver treats that as an idempotent
    /// replay.
    acknowledged: Mutex<BTreeMap<(String, String), u64>>,
}

#[derive(Clone)]
struct ProfileUpdateContext {
    contact_store: Arc<CollaborationContactStore>,
    profile: VerifiedCollaborationProfileDocument,
    authority: ProfileUpdateContextAuthority,
}

/// Why a registered context is allowed to announce and receive. The product
/// path is always a proof-bound Home session, revalidated on every use; the
/// test variant exists so two-runtime proofs can exercise the wire without
/// fabricating browser sessions.
#[derive(Clone)]
enum ProfileUpdateContextAuthority {
    Session {
        session_id: String,
        proof_binding_id: Option<String>,
        grant_id: String,
    },
    /// Registered by the Runtime at startup for the person who owns this
    /// Home, so a contact's rename reaches them whenever their Home is
    /// running. Accepting an update is gated by the signed chain from an
    /// accepted contact, not by whether anyone has a browser open.
    ///
    /// Carries its proof binding so this authority stays revocable: it
    /// outlives every session, so nothing else would ever end it.
    RuntimeOwned { proof_binding_id: String },
    #[cfg(test)]
    VerifiedForTest,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileUpdateRequest {
    op: String,
    message: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileUpdateResponse {
    status: String,
    receipt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileUpdatePayload {
    /// The exact signed chain segment, oldest first, ending at the head.
    pub(crate) signed_profiles: Vec<SignedCollaborationProfileDocument>,
}

impl CollaborationProfileUpdateService {
    fn compare_registered_profile(
        existing: &VerifiedCollaborationProfileDocument,
        incoming: &VerifiedCollaborationProfileDocument,
    ) -> anyhow::Result<std::cmp::Ordering> {
        let ordering = incoming
            .document()
            .revision
            .cmp(&existing.document().revision);
        if ordering == std::cmp::Ordering::Less {
            anyhow::bail!("profile update context Profile revision is stale");
        }
        if ordering == std::cmp::Ordering::Equal
            && crate::collaboration_discovery_runtime::signed_profile_bytes(incoming)?
                != crate::collaboration_discovery_runtime::signed_profile_bytes(existing)?
        {
            anyhow::bail!("profile update context Profile revision conflicts");
        }
        Ok(ordering)
    }

    #[cfg(test)]
    pub(crate) fn context_snapshot_for_test(&self) -> serde_json::Value {
        let Ok(contexts) = self.inner.contexts.lock() else {
            return serde_json::Value::String("poisoned".to_string());
        };
        serde_json::Value::Array(
            contexts
                .iter()
                .map(|(key, context)| {
                    let authority = match &context.authority {
                        ProfileUpdateContextAuthority::Session {
                            session_id,
                            proof_binding_id,
                            grant_id,
                        } => serde_json::json!({
                            "kind": "session",
                            "session_id": session_id,
                            "proof_binding_id": proof_binding_id,
                            "grant_id": grant_id,
                        }),
                        ProfileUpdateContextAuthority::RuntimeOwned { proof_binding_id } => {
                            serde_json::json!({
                                "kind": "runtime_owned",
                                "proof_binding_id": proof_binding_id,
                            })
                        }
                        ProfileUpdateContextAuthority::VerifiedForTest => {
                            serde_json::json!({ "kind": "verified_for_test" })
                        }
                    };
                    let profile_bytes = serde_json::to_vec(context.profile.signed_envelope())
                        .expect("verified Profile-update Profile must serialize");
                    serde_json::json!({
                        "key": key,
                        "profile_did": context.profile.document().profile_did,
                        "profile_revision": context.profile.document().revision,
                        "profile_hash": format!(
                            "sha256:{}",
                            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(profile_bytes))
                        ),
                        "authority": authority,
                    })
                })
                .collect(),
        )
    }

    pub(crate) async fn new(
        signing_key: SigningKey,
        network: VerifiedCollaborationNetworkProfile,
        registry: Arc<ProviderRegistry>,
    ) -> anyhow::Result<Self> {
        let inner = Arc::new(ProfileUpdateInner {
            signing_key,
            network,
            registry: registry.clone(),
            contexts: Mutex::new(BTreeMap::new()),
            acknowledged: Mutex::new(BTreeMap::new()),
        });
        let provider: Arc<dyn Provider> = Arc::new(CollaborationProfileUpdateProvider {
            inner: inner.clone(),
        });
        registry.register(provider.clone()).await;
        registry
            .register_sub_provider(PROFILE_UPDATE_PROVIDER_SCHEME, provider)
            .await?;
        Ok(Self { inner })
    }

    pub(crate) fn register_context(
        &self,
        contact_store: Arc<CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
        session_id: &str,
        proof_binding_id: Option<&str>,
        grant_id: &str,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("profile update context profile does not match the scoped store");
        }
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("profile update context lock is poisoned"))?;
        let key = profile.document().profile_did.clone();
        if !contexts.contains_key(&key) && contexts.len() >= MAX_PROFILE_UPDATE_CONTEXTS {
            anyhow::bail!("profile update context limit reached");
        }
        if let Some(existing) = contexts.get(&key) {
            Self::compare_registered_profile(&existing.profile, &profile)?;
        }
        contexts.insert(
            key,
            ProfileUpdateContext {
                contact_store,
                profile,
                authority: ProfileUpdateContextAuthority::Session {
                    session_id: session_id.to_string(),
                    proof_binding_id: proof_binding_id.map(ToOwned::to_owned),
                    grant_id: grant_id.to_string(),
                },
            },
        );
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn register_verified_context_for_test(
        &self,
        contact_store: Arc<CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("profile update context profile does not match the scoped store");
        }
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("profile update context lock is poisoned"))?;
        contexts.insert(
            profile.document().profile_did.clone(),
            ProfileUpdateContext {
                contact_store,
                profile,
                authority: ProfileUpdateContextAuthority::VerifiedForTest,
            },
        );
        Ok(())
    }

    /// Drops the in-memory acknowledgement cache, as a Runtime restart does.
    /// Announcement is a pure function of the local head and the accepted
    /// contacts, so the only correct behaviour afterwards is a re-announce the
    /// receiver treats as an idempotent replay — which tests prove.
    #[cfg(test)]
    pub(crate) fn forget_acknowledgements_for_test(&self) {
        if let Ok(mut acknowledged) = self.inner.acknowledged.lock() {
            acknowledged.clear();
        }
    }

    /// Announces the local signed chain segment to accepted contacts that have
    /// not acknowledged the current head. Bounded per pass.
    pub(crate) async fn announce_pending(
        &self,
        local_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let context = self.context(local_profile_did, now)?;
        let head_revision = context.profile.document().revision;
        let Some(segment) = profile_chain_segment(
            context.contact_store.data_root(),
            context.contact_store.principal_id(),
            context.contact_store.localhost_root(),
        )?
        else {
            return Ok(());
        };
        if segment.is_empty() || segment.len() > MAX_RETAINED_PROFILE_REVISIONS {
            anyhow::bail!("local profile chain segment is unusable");
        }

        // Nothing durable expires because nothing durable exists: every pass
        // regenerates each envelope from the current signed head and ring.
        debug_assert!(matches!(
            DECLARED_PROFILE_UPDATE_END_OF_LIFE,
            crate::collaboration_delivery::DeliveryEndOfLife::RegenerateFromTruth
        ));
        let contacts = context.contact_store.snapshot()?;
        let mut plan = Vec::new();
        for contact in contacts.contacts() {
            if plan.len() >= MAX_PROFILE_ANNOUNCEMENTS_PER_PASS {
                break;
            }
            let key = (
                local_profile_did.to_string(),
                contact.remote_profile_did().to_string(),
            );
            if self
                .inner
                .acknowledged
                .lock()
                .map_err(|_| anyhow::anyhow!("profile update ack lock is poisoned"))?
                .get(&key)
                .is_some_and(|acked| *acked >= head_revision)
            {
                continue;
            }
            let envelope = prepare_profile_update(
                &self.inner.signing_key,
                &self.inner.network,
                &context.profile,
                contact.conversation_id(),
                contact.remote_profile_did(),
                &segment,
                now,
            )?;
            plan.push(crate::collaboration_delivery::DeliveryPlanItem {
                key: contact.remote_profile_did().to_string(),
                envelope,
                recipient_endpoint_did: contact.remote_presence_device_did().to_string(),
            });
        }
        let service = &self;
        crate::collaboration_delivery::run_bounded_delivery_pass(
            plan,
            |item| async move {
                let outcome = service
                    .deliver(&item.envelope, &item.recipient_endpoint_did, now)
                    .await
                    .map(|()| crate::collaboration_delivery::DeliveryAttempt::Settled);
                (item, outcome)
            },
            |item| {
                self.inner
                    .acknowledged
                    .lock()
                    .map_err(|_| anyhow::anyhow!("profile update ack lock is poisoned"))?
                    .insert(
                        (local_profile_did.to_string(), item.key.clone()),
                        head_revision,
                    );
                Ok(())
            },
        )
        .await
    }

    async fn deliver(
        &self,
        message: &[u8],
        recipient_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let response = self
            .inner
            .registry
            .invoke_provider(ProviderInvocation {
                source: PROFILE_UPDATE_PROVIDER_SCHEME.to_string(),
                target: PROFILE_UPDATE_PROVIDER_SCHEME.to_string(),
                op: PROFILE_UPDATE_PROVIDER_OP.to_string(),
                request: serde_json::to_value(ProfileUpdateRequest {
                    op: PROFILE_UPDATE_PROVIDER_OP.to_string(),
                    message: encode(message),
                })?,
                transfer: ProviderTransfer::Json,
                range: None,
                progress: None,
                transport: ProviderInvocationTransport::Carrier(ProviderCarrierRoute::PeerDid {
                    peer_did: recipient_endpoint_did.to_string(),
                    timeout_ms: Some(PROFILE_UPDATE_TIMEOUT_MS),
                }),
            })
            .await?;
        let mut response = response;
        if let Some(object) = response.as_object_mut() {
            object.remove("_runtime_transfer");
        }
        let response: ProfileUpdateResponse =
            serde_json::from_value(response).context("invalid profile update provider response")?;
        if response.status != "ok" {
            anyhow::bail!("profile update provider rejected the announcement");
        }
        if response.receipt.is_empty()
            || response.receipt.len() > MAX_PROFILE_UPDATE_WIRE_BASE64_BYTES
        {
            anyhow::bail!("profile update receipt has an invalid byte length");
        }
        let receipt = decode(&response.receipt, "profile update receipt")?;
        let verified = verify_profile_update(message, &self.inner.network, now)?;
        let receipt = verify_collaboration_acceptance_receipt(&receipt, &verified.0, now)?;
        if receipt.accepting_endpoint_did() != recipient_endpoint_did {
            anyhow::bail!("profile update receipt came from another endpoint");
        }
        Ok(())
    }

    fn receive(
        &self,
        message: &[u8],
        source_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let raw: SignedCollaborationMessage =
            serde_json::from_slice(message).context("invalid profile update envelope")?;
        if raw.payload.recipient.kind != CollaborationRecipientKind::Profile {
            anyhow::bail!("profile update recipient must be a Profile");
        }
        let contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("profile update context lock is poisoned"))?;
        let context = contexts
            .values()
            .find(|context| context.profile.document().profile_did == raw.payload.recipient.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("profile update recipient is not registered"))?;
        drop(contexts);
        self.revalidate_context(&context, now)?;

        let (verified, payload) = verify_profile_update(message, &self.inner.network, now)?;
        let signed_profiles = payload
            .signed_profiles
            .iter()
            .map(|signed| serde_json::to_vec(signed).map_err(anyhow::Error::from))
            .collect::<anyhow::Result<Vec<_>>>()?;
        context.contact_store.apply_accepted_profile_chain(
            &signed_profiles,
            source_endpoint_did,
            now,
        )?;
        crate::collaboration_direct_messages::acceptance_receipt_for(
            &self.inner.signing_key,
            &verified,
            now,
        )
    }

    fn context(&self, local_profile_did: &str, now: u64) -> anyhow::Result<ProfileUpdateContext> {
        let context = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("profile update context lock is poisoned"))?
            .get(local_profile_did)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("profile update context is not registered"))?;
        self.revalidate_context(&context, now)?;
        Ok(context)
    }

    fn revalidate_context(&self, context: &ProfileUpdateContext, now: u64) -> anyhow::Result<()> {
        match &context.authority {
            ProfileUpdateContextAuthority::Session {
                session_id,
                proof_binding_id,
                grant_id,
            } => crate::collaboration_discovery_runtime::ensure_sync_context_authorized(
                context.contact_store.as_ref(),
                &context.profile,
                context.contact_store.principal_id(),
                session_id,
                proof_binding_id.as_deref(),
                grant_id,
                now,
            ),
            ProfileUpdateContextAuthority::RuntimeOwned { proof_binding_id } => {
                // Read the durable authority from disk rather than
                // comparing this context against itself, so a revoked
                // passkey stops updates the way it stops a session.
                let principal = crate::auth::load_principal_for_proof_binding(
                    context.contact_store.data_root(),
                    proof_binding_id,
                )?;
                crate::auth::ensure_proof_binding_not_revoked(&principal)?;
                if principal.principal_id != context.contact_store.principal_id() {
                    anyhow::bail!("profile update principal no longer owns the contact store");
                }
                Ok(())
            }
            #[cfg(test)]
            ProfileUpdateContextAuthority::VerifiedForTest => Ok(()),
        }
    }

    /// Register the receiving side for a Home's owner without a session, so
    /// a contact's rename lands while the Home is simply running. A
    /// signed-in session knows more and always wins.
    pub(crate) fn register_runtime_owned_context(
        &self,
        contact_store: Arc<CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
        proof_binding_id: &str,
    ) -> anyhow::Result<()> {
        if contact_store.local_profile_did() != profile.document().profile_did {
            anyhow::bail!("profile update context profile does not match the scoped store");
        }
        let mut contexts = self
            .inner
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("profile update context lock is poisoned"))?;
        let key = profile.document().profile_did.clone();
        if let Some(existing) = contexts.get_mut(&key) {
            let ordering = Self::compare_registered_profile(&existing.profile, &profile)?;
            match &existing.authority {
                ProfileUpdateContextAuthority::Session { .. } => {
                    if ordering == std::cmp::Ordering::Greater {
                        existing.contact_store = contact_store;
                        existing.profile = profile;
                    }
                    return Ok(());
                }
                ProfileUpdateContextAuthority::RuntimeOwned { .. } => {
                    if ordering != std::cmp::Ordering::Greater {
                        return Ok(());
                    }
                }
                #[cfg(test)]
                ProfileUpdateContextAuthority::VerifiedForTest => {
                    if ordering == std::cmp::Ordering::Greater {
                        existing.contact_store = contact_store;
                        existing.profile = profile;
                    }
                    return Ok(());
                }
            }
        }
        if !contexts.contains_key(&key) && contexts.len() >= MAX_PROFILE_UPDATE_CONTEXTS {
            anyhow::bail!("profile update context limit reached");
        }
        contexts.insert(
            key,
            ProfileUpdateContext {
                contact_store,
                profile,
                authority: ProfileUpdateContextAuthority::RuntimeOwned {
                    proof_binding_id: proof_binding_id.to_string(),
                },
            },
        );
        Ok(())
    }
}

struct CollaborationProfileUpdateProvider {
    inner: Arc<ProfileUpdateInner>,
}

#[async_trait::async_trait]
impl Provider for CollaborationProfileUpdateProvider {
    async fn handle(
        &self,
        _request: elastos_runtime::provider::ResourceRequest,
    ) -> Result<elastos_runtime::provider::ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "profile updates do not expose resource routes".to_string(),
        ))
    }
    fn schemes(&self) -> Vec<&'static str> {
        vec![PROFILE_UPDATE_PROVIDER_SCHEME]
    }
    fn name(&self) -> &'static str {
        "collaboration-profile-update"
    }
    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut request = request.clone();
        let object = request.as_object_mut().ok_or_else(|| {
            ProviderError::Provider("invalid profile update provider request".to_string())
        })?;
        let source_endpoint_did = validate_profile_update_runtime_invocation(
            object.get("_runtime_invocation"),
        )
        .map_err(|_| {
            ProviderError::Provider("invalid profile update provider invocation".to_string())
        })?;
        object.remove("_runtime_invocation");
        let request: ProfileUpdateRequest = serde_json::from_value(request).map_err(|_| {
            ProviderError::Provider("invalid profile update provider request".to_string())
        })?;
        if request.op != PROFILE_UPDATE_PROVIDER_OP
            || request.message.is_empty()
            || request.message.len() > MAX_PROFILE_UPDATE_WIRE_BASE64_BYTES
        {
            return Err(ProviderError::Provider(
                "invalid profile update provider request".to_string(),
            ));
        }
        let service = CollaborationProfileUpdateService {
            inner: self.inner.clone(),
        };
        let receipt = service
            .receive(
                &decode(&request.message, "profile update").map_err(|_| {
                    ProviderError::Provider("invalid profile update provider request".to_string())
                })?,
                &source_endpoint_did,
                now_ts(),
            )
            .map_err(|err| {
                log_trace!(component: LOG_COMPONENT, "profile update rejected: {err}");
                ProviderError::Provider("profile update rejected".to_string())
            })?;
        let receipt = encode(&receipt);
        if receipt.len() > MAX_PROFILE_UPDATE_WIRE_BASE64_BYTES {
            return Err(ProviderError::Provider(
                "profile update receipt is too large".to_string(),
            ));
        }
        Ok(serde_json::to_value(ProfileUpdateResponse {
            status: "ok".to_string(),
            receipt,
        })
        .expect("profile update response serializes"))
    }
}

fn validate_profile_update_runtime_invocation(
    value: Option<&serde_json::Value>,
) -> anyhow::Result<String> {
    let runtime = value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("profile update provider invocation is missing"))?;
    let capability = format!(
        "provider:{0}->{0}:{1}",
        PROFILE_UPDATE_PROVIDER_SCHEME, PROFILE_UPDATE_PROVIDER_OP
    );
    for (field, expected) in [
        ("schema", "elastos.provider.invocation/v1"),
        ("source", PROFILE_UPDATE_PROVIDER_SCHEME),
        ("target", PROFILE_UPDATE_PROVIDER_SCHEME),
        ("op", PROFILE_UPDATE_PROVIDER_OP),
        ("capability", capability.as_str()),
        ("transport", "carrier-provider-plane"),
        ("transfer", "json"),
    ] {
        if runtime.get(field).and_then(serde_json::Value::as_str) != Some(expected) {
            anyhow::bail!("profile update provider invocation is invalid");
        }
    }
    crate::collaboration_protocol::authenticated_carrier_source_endpoint(runtime.get("carrier"))
}

pub(crate) fn prepare_profile_update(
    signing_key: &SigningKey,
    network: &VerifiedCollaborationNetworkProfile,
    sender_profile: &VerifiedCollaborationProfileDocument,
    conversation_id: &str,
    recipient_profile_did: &str,
    signed_profiles: &[SignedCollaborationProfileDocument],
    now: u64,
) -> anyhow::Result<Vec<u8>> {
    validate_id(conversation_id, "profile update conversation_id")?;
    crate::crypto::decode_did_key(recipient_profile_did)
        .context("invalid profile update recipient Profile DID")?;
    let signer_did = encode_did_key(&signing_key.verifying_key());
    if !sender_profile.authorizes_signer(
        &signer_did,
        PROFILE_UPDATE_SENDER_SERVICE,
        PROFILE_UPDATE_PAYLOAD_TYPE,
    ) {
        anyhow::bail!("profile update signer is not authorized by the sender Profile");
    }
    if signed_profiles.is_empty() || signed_profiles.len() > MAX_RETAINED_PROFILE_REVISIONS {
        anyhow::bail!("profile update chain segment has an invalid length");
    }
    let payload = serde_json::to_value(ProfileUpdatePayload {
        signed_profiles: signed_profiles.to_vec(),
    })?;
    validate_payload_type(PROFILE_UPDATE_PAYLOAD_TYPE)?;
    if serde_json::to_vec(&payload)?.len() > MAX_COLLABORATION_PAYLOAD_BYTES {
        anyhow::bail!("profile update payload is too large");
    }
    let expires_at = now
        .checked_add(PROFILE_UPDATE_TTL_SECS)
        .context("profile update expiry overflows")?;
    let message = CollaborationMessage {
        schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
        network_id: network.profile().network_id.clone(),
        conversation_id: conversation_id.to_string(),
        message_id: random_hex()?,
        nonce: random_hex()?,
        created_at: now,
        expires_at,
        sender_profile_did: sender_profile.document().profile_did.clone(),
        sender_service: PROFILE_UPDATE_SENDER_SERVICE.to_string(),
        recipient: CollaborationRecipient {
            kind: CollaborationRecipientKind::Profile,
            id: recipient_profile_did.to_string(),
        },
        payload_type: PROFILE_UPDATE_PAYLOAD_TYPE.to_string(),
        payload,
    };
    let payload_bytes = canonical_collaboration_message_bytes(&message)?;
    let (signature, signer_did) = domain_separated_sign(
        signing_key,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
    );
    let envelope = canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
        payload: message,
        signature,
        signer_did,
    })?;
    verify_profile_update(&envelope, network, now)?;
    Ok(envelope)
}

pub(crate) fn verify_profile_update(
    bytes: &[u8],
    network: &VerifiedCollaborationNetworkProfile,
    now: u64,
) -> anyhow::Result<(
    crate::collaboration_protocol::VerifiedCollaborationMessage,
    ProfileUpdatePayload,
)> {
    let message = verify_collaboration_message(bytes, network, PROFILE_UPDATE_SENDER_SERVICE, now)?;
    let envelope = &message.envelope().payload;
    if envelope.payload_type != PROFILE_UPDATE_PAYLOAD_TYPE
        || envelope.recipient.kind != CollaborationRecipientKind::Profile
    {
        anyhow::bail!("profile update envelope is invalid");
    }
    let payload: ProfileUpdatePayload = serde_json::from_value(envelope.payload.clone())
        .context("invalid profile update payload")?;
    if payload.signed_profiles.is_empty()
        || payload.signed_profiles.len() > MAX_RETAINED_PROFILE_REVISIONS
    {
        anyhow::bail!("profile update chain segment has an invalid length");
    }
    let signed_head = payload
        .signed_profiles
        .last()
        .expect("validated profile update chain is nonempty");
    let head = crate::collaboration_profile_authority::verify_signed_profile_document(signed_head)
        .context("profile update head is invalid")?;
    if head.document().profile_did != envelope.sender_profile_did {
        anyhow::bail!("profile update sender does not match the signed Profile head");
    }
    if !head.authorizes_signer(
        &message.envelope().signer_did,
        PROFILE_UPDATE_SENDER_SERVICE,
        PROFILE_UPDATE_PAYLOAD_TYPE,
    ) {
        anyhow::bail!("profile update signer is not authorized by the signed Profile head");
    }
    Ok((message, payload))
}

fn random_hex() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).context("OS randomness unavailable for profile update ID")?;
    Ok(hex::encode(bytes))
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode(value: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .with_context(|| format!("invalid base64 {label}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration_discovery_runtime::tests::signed_profile;
    use crate::collaboration_profile_authority::signed_profile_document_for_test;
    use elastos_runtime::signature::generate_keypair;

    const NETWORK: &str = "elastos.community.test";

    fn network() -> VerifiedCollaborationNetworkProfile {
        let (trusted, _) = generate_keypair();
        signed_profile(NETWORK, &trusted, vec![])
    }

    fn chain(
        signer: &SigningKey,
        device: &SigningKey,
        revisions: u64,
    ) -> Vec<SignedCollaborationProfileDocument> {
        let mut segment: Vec<SignedCollaborationProfileDocument> = Vec::new();
        let mut previous: Option<String> = None;
        for revision in 1..=revisions {
            let verified = signed_profile_document_for_test(
                signer,
                &format!("Person {revision}"),
                None,
                revision,
                previous.as_deref(),
                1_785_900_000 + revision,
                vec![encode_did_key(&device.verifying_key())],
            )
            .unwrap();
            let signed = verified.signed_envelope().clone();
            previous = Some(format!(
                "sha256:{}",
                hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
                    serde_json::to_vec(&signed).unwrap()
                ))
            ));
            segment.push(signed);
        }
        segment
    }

    /// Recovery keeps accepted contacts, end to end: Bob's Full Recovery
    /// Bundle identity (profile signing seed, revision ring, contact store)
    /// restores onto a genuinely fresh machine with a different device key,
    /// the restore mints the next signed revision authorizing that device,
    /// and one announcement over the live provider plane teaches Alice's
    /// store the new delivery endpoint — same Profile DID, same conversation,
    /// same signed name.
    #[tokio::test]
    async fn recovered_identity_rebinds_a_fresh_device_and_contacts_learn_it() {
        let temp = tempfile::tempdir().unwrap();
        let pair =
            crate::collaboration_discovery_runtime::tests::durable_profile_peer_pair(temp.path())
                .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let now = crate::auth::now_ts();
        let b_root = temp.path().join("b");
        let profile_b_did = pair.identity_b.profile.document().profile_did.clone();

        // What the Full Recovery Bundle carries for Bob.
        let profile_bundle =
            crate::collaboration_profile_authority::export_profile_authority_bundle_for_recovery(
                &b_root,
                &pair.identity_b.principal_id,
                &pair.identity_b.localhost_root,
            )
            .unwrap()
            .expect("saved Profile exports");
        let store_state =
            crate::collaboration_contact_store::export_contact_store_state_for_recovery(
                &b_root,
                &pair.identity_b.principal_id,
                &pair.identity_b.localhost_root,
            )
            .unwrap()
            .expect("contact store exports");

        // A genuinely fresh machine: same recovered principal root, new
        // device key, new passkey proof binding.
        let b2_root = temp.path().join("b2");
        std::fs::create_dir_all(&b2_root).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&b2_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (b2_device_key, _) = elastos_identity::load_or_create_did(&b2_root).unwrap();
        let b2_device_did = encode_did_key(&b2_device_key.verifying_key());
        assert_ne!(
            b2_device_did,
            encode_did_key(&pair.identity_b.device_key.verifying_key())
        );
        crate::auth::store_test_principal_root_protection(&b2_root, &pair.identity_b.principal_id);
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: "credential:recovered-b2".to_string(),
                public_key: "recovered-device-test-public-key".to_string(),
                sign_count: 1,
                user_verified: true,
                origin: "https://elastos.elacitylabs.com".to_string(),
                rp_id: "elastos.elacitylabs.com".to_string(),
                created_at: now,
                last_used_at: now,
                revoked_at: None,
            },
        );
        let proof_binding_id = crate::auth::upsert_principal_for_binding_as_role_named(
            &b2_root,
            binding,
            pair.identity_b.principal_id.clone(),
            crate::auth::RuntimePrincipalRole::Admin,
            Some("Bob"),
            now,
        )
        .unwrap()
        .proof_binding_id;

        // The restore the recovery import performs.
        let restored =
            crate::collaboration_profile_authority::restore_profile_authority_bundle_for_recovery(
                &b2_root,
                &pair.identity_b.principal_id,
                &pair.identity_b.localhost_root,
                &profile_bundle,
            )
            .unwrap();
        assert_eq!(restored.document().profile_did, profile_b_did);
        assert!(!restored.authorizes_endpoint(&b2_device_did));
        crate::collaboration_contact_store::restore_contact_store_state_for_recovery(
            &b2_root,
            &pair.identity_b.principal_id,
            &pair.identity_b.localhost_root,
            &store_state,
            &profile_b_did,
        )
        .unwrap();
        let rebound = crate::collaboration_profile_authority::update_profile_authority(
            &b2_root,
            &pair.identity_b.principal_id,
            &pair.identity_b.localhost_root,
            &proof_binding_id,
            &restored.document().display_name,
            restored.document().handle.as_deref(),
            now + 1,
        )
        .unwrap();
        assert_eq!(rebound.document().profile_did, profile_b_did);
        assert_eq!(
            rebound.document().revision,
            restored.document().revision + 1
        );
        assert!(rebound.authorizes_endpoint(&b2_device_did));
        assert_eq!(rebound.document().display_name, "Bob");

        // The recovered identity comes alive on the fresh machine and
        // announces. Alice's store — never touched by recovery — verifies the
        // chain and switches Bob's delivery endpoint to the new device.
        let (_registry_b2, service_b2, store_b2, _node_b2) =
            crate::collaboration_discovery_runtime::tests::discovery_service_with_identity(
                &b2_root,
                &pair.trusted,
                &b2_device_key,
                vec![],
                &pair.identity_b.principal_id,
                &pair.identity_b.localhost_root,
                rebound.clone(),
            )
            .await;
        let recovered_contacts = store_b2.snapshot().unwrap();
        assert_eq!(recovered_contacts.contacts().len(), 1);
        assert_eq!(
            recovered_contacts.contacts()[0].conversation_id(),
            pair.conversation_id
        );
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        service_b2
            .profile_update_service()
            .register_verified_context_for_test(store_b2.clone(), rebound.clone())
            .unwrap();
        pair.service_a
            .profile_update_service()
            .register_verified_context_for_test(
                pair.store_a.clone(),
                pair.identity_a.profile.clone(),
            )
            .unwrap();
        service_b2
            .profile_update_service()
            .announce_pending(&profile_b_did, now + 2)
            .await
            .unwrap();

        let alice_view = pair.store_a.snapshot().unwrap();
        assert_eq!(alice_view.contacts().len(), 1);
        assert_eq!(alice_view.contacts()[0].remote_display_name(), "Bob");
        assert_eq!(
            alice_view.contacts()[0].conversation_id(),
            pair.conversation_id
        );
        assert_eq!(
            alice_view.contacts()[0].remote_presence_device_did(),
            b2_device_did
        );
    }

    /// The product path across two live runtimes: a person renames twice
    /// while their contact hears nothing, one announcement carries the exact
    /// signed chain segment over the Runtime provider plane, and the
    /// receiving store applies it under the strict next-revision and
    /// chain-hash rules. A restart loses only the in-memory acknowledgement
    /// cache, and the forced re-announce is an idempotent replay.
    #[tokio::test]
    async fn profile_update_announce_delivers_the_signed_chain_across_runtimes() {
        let temp = tempfile::tempdir().unwrap();
        let pair =
            crate::collaboration_discovery_runtime::tests::durable_profile_peer_pair(temp.path())
                .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let now = crate::auth::now_ts();
        let root_b = temp.path().join("b");

        let before = pair.store_a.snapshot().unwrap();
        assert_eq!(before.contacts()[0].remote_display_name(), "Bob");
        let endpoint_before = before.contacts()[0]
            .remote_presence_device_did()
            .to_string();

        // Two renames while Alice hears nothing: the announcement must carry
        // the exact chain segment, not just the head.
        crate::collaboration_profile_authority::update_profile_authority(
            &root_b,
            &pair.identity_b.principal_id,
            &pair.identity_b.localhost_root,
            &pair.identity_b.proof_binding_id,
            "Bob Renamed",
            Some("bob"),
            now + 1,
        )
        .unwrap();
        let head_b = crate::collaboration_profile_authority::update_profile_authority(
            &root_b,
            &pair.identity_b.principal_id,
            &pair.identity_b.localhost_root,
            &pair.identity_b.proof_binding_id,
            "Bob Final",
            Some("bob"),
            now + 2,
        )
        .unwrap();
        assert_eq!(head_b.document().revision, 3);

        pair.service_b
            .profile_update_service()
            .register_verified_context_for_test(pair.store_b.clone(), head_b.clone())
            .unwrap();
        pair.service_a
            .profile_update_service()
            .register_verified_context_for_test(
                pair.store_a.clone(),
                pair.identity_a.profile.clone(),
            )
            .unwrap();

        pair.service_b
            .profile_update_service()
            .announce_pending(&head_b.document().profile_did, now + 3)
            .await
            .unwrap();

        let after = pair.store_a.snapshot().unwrap();
        assert_eq!(after.contacts().len(), 1);
        assert_eq!(after.contacts()[0].remote_display_name(), "Bob Final");
        assert_eq!(after.contacts()[0].conversation_id(), pair.conversation_id);
        assert_eq!(
            after.contacts()[0].remote_presence_device_did(),
            endpoint_before
        );
        // The other direction is untouched: Bob still sees Alice's name.
        assert_eq!(
            pair.store_b.snapshot().unwrap().contacts()[0].remote_display_name(),
            "Alice"
        );

        // Acknowledged head: a second pass is a no-op that stays consistent.
        pair.service_b
            .profile_update_service()
            .announce_pending(&head_b.document().profile_did, now + 4)
            .await
            .unwrap();
        // A restart loses the cache; the re-announce is an idempotent replay.
        pair.service_b
            .profile_update_service()
            .forget_acknowledgements_for_test();
        pair.service_b
            .profile_update_service()
            .announce_pending(&head_b.document().profile_did, now + 5)
            .await
            .unwrap();
        let replayed = pair.store_a.snapshot().unwrap();
        assert_eq!(replayed.contacts().len(), 1);
        assert_eq!(replayed.contacts()[0].remote_display_name(), "Bob Final");
        assert_eq!(
            replayed.contacts()[0].conversation_id(),
            pair.conversation_id
        );
    }

    #[tokio::test]
    async fn profile_update_between_same_runtime_profiles_uses_strict_carrier_loopback() {
        let temp = tempfile::tempdir().unwrap();
        let mut pair =
            crate::collaboration_discovery_runtime::tests::same_runtime_profile_pair(temp.path())
                .await;
        let now = crate::auth::now_ts();
        let endpoint_did = encode_did_key(&pair.endpoint_key.verifying_key());
        let previous = crate::collaboration_discovery_runtime::tests::profile_hash(&pair.profile_b);
        let head_b = signed_profile_document_for_test(
            &pair.profile_key_b,
            "Bob Same Runtime",
            Some("bob"),
            2,
            Some(&previous),
            now + 1,
            vec![endpoint_did.clone()],
        )
        .unwrap();
        let updates = pair.service.profile_update_service();
        updates
            .register_verified_context_for_test(pair.store_b.clone(), head_b.clone())
            .unwrap();
        let envelope = prepare_profile_update(
            &pair.endpoint_key,
            &pair.service.network_profile(),
            &head_b,
            &pair.conversation_id,
            &pair.profile_a.document().profile_did,
            &[head_b.signed_envelope().clone()],
            now + 2,
        )
        .unwrap();

        // A closed Carrier endpoint makes any socket path impossible. The
        // update must still pass collaboration-profile's strict Carrier-plane
        // validator and authenticate this Runtime endpoint as its source.
        pair.node
            .take()
            .expect("fixture Carrier node")
            .shutdown()
            .await;
        updates
            .deliver(&envelope, &endpoint_did, now + 2)
            .await
            .unwrap();

        let alice_view = pair.store_a.snapshot().unwrap();
        assert_eq!(alice_view.contacts().len(), 1);
        assert_eq!(
            alice_view.contacts()[0].remote_profile_did(),
            pair.profile_b.document().profile_did
        );
        assert_eq!(
            alice_view.contacts()[0].remote_display_name(),
            "Bob Same Runtime"
        );
        assert_eq!(
            alice_view.contacts()[0].remote_presence_device_did(),
            endpoint_did
        );
    }

    #[test]
    fn profile_update_envelope_round_trips_and_stays_runtime_internal() {
        let network = network();
        let (sender_device, _) = generate_keypair();
        let (profile_signer, _) = generate_keypair();
        let (recipient_device, _) = generate_keypair();
        let segment = chain(&profile_signer, &sender_device, 3);
        let sender_profile =
            crate::collaboration_profile_authority::verify_signed_profile_document(
                segment.last().unwrap(),
            )
            .unwrap();
        let now = crate::auth::now_ts();

        let envelope = prepare_profile_update(
            &sender_device,
            &network,
            &sender_profile,
            "direct:sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
            &encode_did_key(&recipient_device.verifying_key()),
            &segment,
            now,
        )
        .unwrap();

        let (message, payload) = verify_profile_update(&envelope, &network, now).unwrap();
        assert_eq!(payload.signed_profiles, segment);
        assert_eq!(
            message.envelope().payload.payload_type,
            PROFILE_UPDATE_PAYLOAD_TYPE
        );
        assert_eq!(
            message.envelope().payload.sender_service,
            PROFILE_UPDATE_SENDER_SERVICE
        );
        assert_eq!(
            message.envelope().payload.recipient.kind,
            CollaborationRecipientKind::Profile
        );
        // Runtime resolves the peer; the envelope never carries a route or ticket.
        let rendered = String::from_utf8_lossy(&envelope);
        assert!(!rendered.contains("connect_ticket"));
        assert!(!rendered.contains("127.0.0.1") && !rendered.contains("localhost"));
    }

    #[test]
    fn profile_update_signer_and_carrier_endpoint_are_independent_profile_roles() {
        let network = network();
        let (profile_key, _) = generate_keypair();
        let (endpoint_key, _) = generate_keypair();
        let (message_key, _) = generate_keypair();
        let (recipient_key, _) = generate_keypair();
        let endpoint_did = encode_did_key(&endpoint_key.verifying_key());
        let message_signer_did = encode_did_key(&message_key.verifying_key());
        let sender_profile = crate::collaboration_profile_authority::
            signed_profile_document_with_authority_for_test(
                &profile_key,
                "Alice",
                None,
                1,
                None,
                1_800_000_000,
                crate::collaboration_profile_authority::ProfileAuthorityForTest {
                    endpoint_dids: vec![endpoint_did.clone()],
                    signer_dids: vec![message_signer_did.clone()],
                },
            )
            .unwrap();
        let segment = vec![sender_profile.signed_envelope().clone()];
        let now = 1_800_000_100;
        let envelope = prepare_profile_update(
            &message_key,
            &network,
            &sender_profile,
            "direct:sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899",
            &encode_did_key(&recipient_key.verifying_key()),
            &segment,
            now,
        )
        .unwrap();

        let (verified, _) = verify_profile_update(&envelope, &network, now).unwrap();
        assert_eq!(verified.envelope().signer_did, message_signer_did);
        assert_ne!(verified.envelope().signer_did, endpoint_did);
        assert_eq!(sender_profile.sole_endpoint_did().unwrap(), endpoint_did);
    }

    #[test]
    fn profile_update_provider_requires_authenticated_carrier_source() {
        let source_endpoint_did = encode_did_key(&generate_keypair().0.verifying_key());
        let valid = serde_json::json!({
            "schema": "elastos.provider.invocation/v1",
            "source": PROFILE_UPDATE_PROVIDER_SCHEME,
            "target": PROFILE_UPDATE_PROVIDER_SCHEME,
            "op": PROFILE_UPDATE_PROVIDER_OP,
            "capability": format!(
                "provider:{0}->{0}:{1}",
                PROFILE_UPDATE_PROVIDER_SCHEME,
                PROFILE_UPDATE_PROVIDER_OP,
            ),
            "transport": "carrier-provider-plane",
            "transfer": "json",
            "carrier": { "source_endpoint_did": source_endpoint_did },
        });
        assert_eq!(
            validate_profile_update_runtime_invocation(Some(&valid)).unwrap(),
            source_endpoint_did
        );

        let mut caller_asserted = valid.clone();
        caller_asserted["carrier"]["peer_did"] = serde_json::json!(source_endpoint_did);
        assert!(validate_profile_update_runtime_invocation(Some(&caller_asserted)).is_err());
        let mut missing = valid;
        missing["carrier"] = serde_json::Value::Null;
        assert!(validate_profile_update_runtime_invocation(Some(&missing)).is_err());
    }

    #[test]
    fn profile_update_rejects_unusable_segments_and_foreign_services() {
        let network = network();
        let (sender_device, _) = generate_keypair();
        let (profile_signer, _) = generate_keypair();
        let recipient = encode_did_key(&generate_keypair().0.verifying_key());
        let sender_segment = chain(&profile_signer, &sender_device, 1);
        let sender_profile =
            crate::collaboration_profile_authority::verify_signed_profile_document(
                sender_segment.last().unwrap(),
            )
            .unwrap();
        let conversation =
            "direct:sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899";
        let now = crate::auth::now_ts();

        assert!(prepare_profile_update(
            &sender_device,
            &network,
            &sender_profile,
            conversation,
            &recipient,
            &[],
            now
        )
        .is_err());

        let oversized = chain(&profile_signer, &sender_device, 4)
            .into_iter()
            .cycle()
            .take(MAX_RETAINED_PROFILE_REVISIONS + 1)
            .collect::<Vec<_>>();
        assert!(prepare_profile_update(
            &sender_device,
            &network,
            &sender_profile,
            conversation,
            &recipient,
            &oversized,
            now
        )
        .is_err());

        // A well-formed envelope from another service must not verify here.
        let segment = chain(&profile_signer, &sender_device, 1);
        let envelope = prepare_profile_update(
            &sender_device,
            &network,
            &sender_profile,
            conversation,
            &recipient,
            &segment,
            now,
        )
        .unwrap();
        assert!(crate::collaboration_protocol::verify_collaboration_message(
            &envelope, &network, "chat", now
        )
        .is_err());
    }
}
