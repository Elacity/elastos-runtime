//! Runtime-owned discovery relay path for signed advertisements and contact requests.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use base64::Engine as _;
use elastos_common::collaboration_protocol::{
    canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
    collaboration_message_envelope_sha256, CollaborationMessage, CollaborationRecipient,
    CollaborationRecipientKind, SignedCollaborationMessage, COLLABORATION_MESSAGE_SCHEMA_V1,
    COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1, MAX_COLLABORATION_CLOCK_SKEW_SECS,
};
use elastos_runtime::provider::{
    Provider, ProviderCarrierRoute, ProviderError, ProviderInvocation, ProviderInvocationTransport,
    ProviderRegistry, ProviderTransfer,
};
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};

use crate::collaboration_contact_store::{
    CollaborationContactStore, ContactStoreWrite, PendingIncomingContactRequest,
};
use crate::collaboration_core::random_hex_128;
use crate::collaboration_direct_messages::CollaborationDirectMessageService;
use crate::collaboration_discovery::{
    canonical_signed_collaboration_contact_decision_receipt_bytes,
    verify_collaboration_contact_decision_receipt, verify_collaboration_contact_request,
    verify_collaboration_discovery_advertisement, verify_collaboration_discovery_mailbox_poll,
    verify_collaboration_discovery_withdrawal, CollaborationContactDecision,
    CollaborationContactDecisionReceipt, CollaborationContactRequestPayload,
    CollaborationDiscoveryAdvertisementPayload, CollaborationDiscoveryMailboxKind,
    CollaborationDiscoveryMailboxPollPayload, CollaborationDiscoveryWithdrawalPayload,
    SignedCollaborationContactDecisionReceipt, VerifiedCollaborationContactRequest,
    VerifiedCollaborationDiscoveryAdvertisement, COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1,
    COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
    COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
    COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS, COLLABORATION_DISCOVERY_CONTACT_ID,
    COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
    COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS, COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
    COLLABORATION_DISCOVERY_DIRECTORY_ID, COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
    COLLABORATION_DISCOVERY_SERVICE, COLLABORATION_DISCOVERY_WITHDRAWAL_PAYLOAD_TYPE,
    MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES,
};
use crate::collaboration_network::{
    CollaborationBootstrapPeer, VerifiedCollaborationNetworkProfile,
};
use crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument;
use crate::collaboration_protocol::validate_id;
use crate::crypto::{domain_separated_sign, encode_did_key};

const COLLABORATION_DISCOVERY_PROVIDER_SCHEME: &str = "collaboration";
const MAX_DISCOVERY_CLIENT_STATES: usize = 32;
const MAX_DISCOVERY_QUERY_RESULTS: usize = 32;
const MAX_DISCOVERY_REQUESTS_PER_RECIPIENT: usize = 32;
const MAX_DISCOVERY_RECEIPTS_PER_RECIPIENT: usize = 32;
const MAX_DISCOVERY_TOTAL_ADVERTISEMENTS: usize = 64;
const MAX_DISCOVERY_TOTAL_REQUESTS: usize = 64;
const MAX_DISCOVERY_TOTAL_REQUEST_MAILBOX_ENTRIES: usize = 64;
const MAX_DISCOVERY_TOTAL_DECISION_MAILBOX_ENTRIES: usize = 64;
const DISCOVERY_PROVIDER_TIMEOUT_MS: u64 = 5_000;
const DISCOVERY_ADVERTISEMENT_RENEWAL_WINDOW_SECS: u64 = 60;
const MAX_DISCOVERY_SYNC_CONTEXTS: usize = 32;
// A worker pass may make several bounded provider calls per Profile context.
// Keep the batch deliberately small so one unavailable relay cannot occupy the
// dedicated Runtime sync task for an unbounded run of contexts.
const MAX_DISCOVERY_SYNC_WORK_PER_WAKE: usize = 1;
const DISCOVERY_SYNC_ENABLED_CADENCE_SECS: u64 = 15;
const DISCOVERY_SYNC_IDLE_CADENCE_SECS: u64 = 30;
const DIRECT_SYNC_CADENCE_SECS: u64 = 15;
const PROFILE_SYNC_CADENCE_SECS: u64 = 15;
/// Declared end-of-life for signed contact revocations: the exact removal
/// fact is re-minted into a fresh envelope and delivery continues until the
/// removed peer's device acknowledges it.
const DECLARED_CONTACT_REVOCATION_END_OF_LIFE: crate::collaboration_delivery::DeliveryEndOfLife =
    crate::collaboration_delivery::DeliveryEndOfLife::RemintExact;
const DISCOVERY_SYNC_BASE_BACKOFF_SECS: u64 = 5;
const DISCOVERY_SYNC_MAX_BACKOFF_SECS: u64 = 60;
const MAX_DISCOVERY_OUTBOX_SENDS_PER_SYNC: usize = 4;

#[derive(Clone)]
pub struct CollaborationDiscoveryService {
    authority: Arc<CollaborationDiscoveryAuthority>,
    registry: Arc<ProviderRegistry>,
    bootstrap_peers: Arc<Vec<CollaborationBootstrapPeer>>,
    state: Arc<Mutex<BTreeMap<String, DiscoveryClientState>>>,
    sync_contexts: Arc<Mutex<BTreeMap<DiscoverySyncContextKey, DiscoverySyncContext>>>,
    sync_pass_lock: Arc<tokio::sync::Mutex<()>>,
    intent_mutex: Arc<Mutex<()>>,
    direct_messages: CollaborationDirectMessageService,
    profile_updates: crate::collaboration_profile_updates::CollaborationProfileUpdateService,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationDiscoveryStatus {
    available: bool,
    enabled: bool,
    expires_at: Option<u64>,
    remote_visibility_may_remain_until: Option<u64>,
    visible_people: Vec<DiscoveryVisiblePerson>,
    incoming_requests: Vec<PendingIncomingContactRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveryVisiblePerson {
    advertisement_id: String,
    display_name: String,
    handle: Option<String>,
    last_seen_at: u64,
    expires_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderAdvertisementRequest {
    op: String,
    advertisement: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderQueryRequest {
    op: String,
    advertisement: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderWithdrawalRequest {
    op: String,
    withdrawal: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderContactRequest {
    op: String,
    request: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderMailboxPollRequest {
    op: String,
    poll: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderDecisionReceiptRequest {
    op: String,
    receipt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderAdvertisementResponse {
    status: String,
    data: DiscoveryProviderAdvertisementData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderAdvertisementData {
    advertisements: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderStatusResponse {
    status: String,
    #[serde(rename = "data")]
    _data: DiscoveryProviderEmptyData,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderEmptyData {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderMailboxResponse {
    status: String,
    data: DiscoveryProviderMailboxData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryProviderMailboxData {
    requests: Vec<String>,
    decisions: Vec<String>,
}

#[derive(Clone)]
struct DiscoveryClientState {
    current_advertisement: Option<CachedAdvertisement>,
    visible_advertisements: BTreeMap<String, CachedAdvertisement>,
    observed_profile_heads: BTreeMap<String, ObservedProfileHead>,
    remote_visibility_may_remain_until: Option<u64>,
    transport_available: bool,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DiscoverySyncContextKey {
    principal_id: String,
    localhost_root: String,
    profile_did: String,
}

#[derive(Clone)]
struct DiscoverySyncContext {
    store: Arc<CollaborationContactStore>,
    profile: VerifiedCollaborationProfileDocument,
    session_id: String,
    proof_binding_id: Option<String>,
    grant_id: String,
    discovery_next_wake_at: u64,
    discovery_failures: u8,
    direct_next_wake_at: u64,
    direct_failures: u8,
    profile_next_wake_at: u64,
    profile_failures: u8,
    #[cfg(test)]
    verified_for_test: bool,
}

impl Default for DiscoveryClientState {
    fn default() -> Self {
        Self {
            current_advertisement: None,
            visible_advertisements: BTreeMap::new(),
            observed_profile_heads: BTreeMap::new(),
            remote_visibility_may_remain_until: None,
            transport_available: true,
        }
    }
}

#[derive(Clone)]
struct CachedAdvertisement {
    envelope_bytes: Vec<u8>,
    verified: VerifiedCollaborationDiscoveryAdvertisement,
}

#[derive(Clone)]
struct ObservedProfileHead {
    revision: u64,
    profile_envelope_sha256: String,
    observed_at: u64,
}

struct CollaborationDiscoveryAuthority {
    signing_key: SigningKey,
    profile: VerifiedCollaborationNetworkProfile,
}

#[derive(Clone, Copy)]
struct CollaborationMessageTiming {
    created_at: u64,
    ttl_secs: u64,
}

struct CollaborationDiscoveryRelayProvider {
    profile: VerifiedCollaborationNetworkProfile,
    state: Mutex<DiscoveryRelayState>,
}

struct DiscoveryRelayState {
    advertisements: HashMap<String, CachedAdvertisement>,
    requests: HashMap<String, RelayContactRequest>,
    request_mailboxes: HashMap<String, Vec<String>>,
    decision_mailboxes: HashMap<String, Vec<RelayDecision>>,
}

#[derive(Clone)]
struct RelayContactRequest {
    envelope_bytes: Vec<u8>,
    verified: VerifiedCollaborationContactRequest,
    advertisement: CachedAdvertisement,
}

#[derive(Clone)]
struct RelayDecision {
    envelope_bytes: Vec<u8>,
    request_hash: String,
}

impl CollaborationDiscoveryService {
    pub(crate) async fn new(
        signing_key: SigningKey,
        profile: VerifiedCollaborationNetworkProfile,
        registry: Arc<ProviderRegistry>,
    ) -> anyhow::Result<Self> {
        let provider: Arc<dyn Provider> =
            Arc::new(CollaborationDiscoveryRelayProvider::new(profile.clone()));
        registry.register(provider).await;
        let direct_messages = CollaborationDirectMessageService::new(
            SigningKey::from_bytes(&signing_key.to_bytes()),
            profile.clone(),
            registry.clone(),
        )
        .await?;
        let profile_updates =
            crate::collaboration_profile_updates::CollaborationProfileUpdateService::new(
                SigningKey::from_bytes(&signing_key.to_bytes()),
                profile.clone(),
                registry.clone(),
            )
            .await?;
        Ok(Self {
            authority: Arc::new(CollaborationDiscoveryAuthority::new(
                signing_key,
                profile.clone(),
            )),
            registry,
            bootstrap_peers: Arc::new(profile.profile().bootstrap_peers.clone()),
            state: Arc::new(Mutex::new(BTreeMap::new())),
            sync_contexts: Arc::new(Mutex::new(BTreeMap::new())),
            sync_pass_lock: Arc::new(tokio::sync::Mutex::new(())),
            intent_mutex: Arc::new(Mutex::new(())),
            direct_messages,
            profile_updates,
        })
    }

    pub(crate) fn network_profile(&self) -> VerifiedCollaborationNetworkProfile {
        self.authority.profile.clone()
    }

    pub(crate) fn direct_message_service(&self) -> CollaborationDirectMessageService {
        self.direct_messages.clone()
    }

    #[cfg(test)]
    pub(crate) fn profile_update_service(
        &self,
    ) -> crate::collaboration_profile_updates::CollaborationProfileUpdateService {
        self.profile_updates.clone()
    }

    /// Registers runtime-owned receive contexts for every verified Profile on
    /// the running Home. This uses only existing durable authority and never
    /// creates contact, Profile, or device state.
    pub(crate) fn register_runtime_owned_contexts(&self, data_dir: &Path) -> anyhow::Result<usize> {
        let local_device_did =
            match crate::collaboration_profile_authority::load_existing_device_did(data_dir)? {
                Some(did) => did,
                None => return Ok(0),
            };
        let mut registered = 0usize;
        for principal in crate::auth::active_passkey_principals(data_dir)? {
            let Some(profile) = crate::collaboration_profile_authority::load_profile_authority(
                data_dir,
                &principal.principal_id,
                &principal.localhost_root,
            )?
            else {
                continue;
            };
            let registered_for_principal = (|| -> anyhow::Result<()> {
                let store = Arc::new(CollaborationContactStore::new(
                    data_dir,
                    &principal.principal_id,
                    &principal.localhost_root,
                    self.network_profile(),
                    &profile,
                    &local_device_did,
                )?);
                self.direct_messages.register_runtime_owned_context(
                    store.clone(),
                    profile.clone(),
                    &principal.proof_binding_id,
                )?;
                self.profile_updates.register_runtime_owned_context(
                    store,
                    profile,
                    &principal.proof_binding_id,
                )?;
                Ok(())
            })();
            match registered_for_principal {
                Ok(()) => registered = registered.saturating_add(1),
                Err(err) => {
                    tracing::warn!(
                        principal_id = %principal.principal_id,
                        error = %err,
                        "collaboration registration skipped for this Profile"
                    );
                }
            }
        }
        Ok(registered)
    }

    /// Registers a verified principal-scoped context with the Runtime-owned
    /// collaboration worker. It only retains already-validated objects; it
    /// never creates contact, Profile, or device state.
    pub(crate) fn register_sync_context(
        &self,
        store: Arc<CollaborationContactStore>,
        profile: VerifiedCollaborationProfileDocument,
        session_id: &str,
        proof_binding_id: Option<&str>,
        grant_id: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        self.require_profile_store_match(store.as_ref(), &profile)?;
        ensure_sync_context_authorized(
            store.as_ref(),
            &profile,
            store.principal_id(),
            session_id,
            proof_binding_id,
            grant_id,
            now,
        )?;
        let key = DiscoverySyncContextKey {
            principal_id: store.principal_id().to_string(),
            localhost_root: store.localhost_root().to_string(),
            profile_did: store.local_profile_did().to_string(),
        };
        let mut contexts = self
            .sync_contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery sync context lock is poisoned"))?;
        if let Some(existing_key) = contexts.keys().find(|existing| {
            existing.principal_id == key.principal_id
                && existing.localhost_root == key.localhost_root
                && existing.profile_did != key.profile_did
        }) {
            anyhow::bail!(
                "discovery sync registration profile conflicts with {}",
                existing_key.profile_did
            );
        }
        if !contexts.contains_key(&key) && contexts.len() >= MAX_DISCOVERY_SYNC_CONTEXTS {
            anyhow::bail!("discovery sync context limit reached");
        }
        if let Some(existing) = contexts.get_mut(&key) {
            let existing_revision = existing.profile.document().revision;
            let incoming_revision = profile.document().revision;
            if incoming_revision < existing_revision {
                anyhow::bail!("discovery sync registration profile revision is stale");
            }
            let same_profile =
                signed_profile_bytes(&existing.profile)? == signed_profile_bytes(&profile)?;
            if incoming_revision == existing_revision && !same_profile {
                anyhow::bail!("discovery sync registration profile revision conflicts");
            }
            existing.store = store;
            existing.profile = profile;
            existing.session_id = session_id.to_string();
            existing.proof_binding_id = proof_binding_id.map(ToOwned::to_owned);
            existing.grant_id = grant_id.to_string();
            if incoming_revision > existing_revision {
                existing.discovery_next_wake_at = now;
                existing.discovery_failures = 0;
                existing.direct_next_wake_at = now;
                existing.direct_failures = 0;
                existing.profile_next_wake_at = now;
                existing.profile_failures = 0;
            }
            self.direct_messages.register_context(
                existing.store.clone(),
                existing.profile.clone(),
                session_id,
                proof_binding_id,
                grant_id,
                now,
            )?;
            self.profile_updates.register_context(
                existing.store.clone(),
                existing.profile.clone(),
                session_id,
                proof_binding_id,
                grant_id,
            )?;
            return Ok(());
        }
        self.direct_messages.register_context(
            store.clone(),
            profile.clone(),
            session_id,
            proof_binding_id,
            grant_id,
            now,
        )?;
        self.profile_updates.register_context(
            store.clone(),
            profile.clone(),
            session_id,
            proof_binding_id,
            grant_id,
        )?;
        contexts.insert(
            key,
            DiscoverySyncContext {
                store,
                profile,
                session_id: session_id.to_string(),
                proof_binding_id: proof_binding_id.map(ToOwned::to_owned),
                grant_id: grant_id.to_string(),
                discovery_next_wake_at: now,
                discovery_failures: 0,
                direct_next_wake_at: now,
                direct_failures: 0,
                profile_next_wake_at: now,
                profile_failures: 0,
                #[cfg(test)]
                verified_for_test: false,
            },
        );
        Ok(())
    }

    /// A local UI wake never performs delivery itself. The collaboration
    /// worker will handle the next bounded sync pass.
    pub(crate) fn wake_registered_sync(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<()> {
        if self.wake_registered_sync_if_present(store, profile, now)? {
            return Ok(());
        }
        anyhow::bail!("discovery sync context is not registered");
    }

    pub(crate) fn wake_registered_sync_if_present(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<bool> {
        self.require_profile_store_match(store, profile)?;
        let key = DiscoverySyncContextKey {
            principal_id: store.principal_id().to_string(),
            localhost_root: store.localhost_root().to_string(),
            profile_did: store.local_profile_did().to_string(),
        };
        let mut contexts = self
            .sync_contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery sync context lock is poisoned"))?;
        let Some(context) = contexts.get_mut(&key) else {
            return Ok(false);
        };
        if context.discovery_failures == 0 {
            context.discovery_next_wake_at = now;
        }
        if context.direct_failures == 0 {
            context.direct_next_wake_at = now;
        }
        if context.profile_failures == 0 {
            context.profile_next_wake_at = now;
        }
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn registered_context_snapshot_for_test(&self) -> serde_json::Value {
        let sync = match self.sync_contexts.lock() {
            Ok(contexts) => serde_json::Value::Array(
                contexts
                    .iter()
                    .map(|(key, context)| {
                        let profile_bytes = serde_json::to_vec(context.profile.signed_envelope())
                            .expect("verified discovery Profile must serialize");
                        serde_json::json!({
                            "key": {
                                "principal_id": key.principal_id,
                                "localhost_root": key.localhost_root,
                                "profile_did": key.profile_did,
                            },
                            "profile_revision": context.profile.document().revision,
                            "profile_hash": format!(
                                "sha256:{}",
                                hex::encode(<sha2::Sha256 as sha2::Digest>::digest(profile_bytes))
                            ),
                            "authority": {
                                "kind": if context.verified_for_test {
                                    "verified_for_test"
                                } else {
                                    "session"
                                },
                                "session_id": context.session_id,
                                "proof_binding_id": context.proof_binding_id,
                                "grant_id": context.grant_id,
                            },
                            "discovery_next_wake_at": context.discovery_next_wake_at,
                            "discovery_failures": context.discovery_failures,
                            "direct_next_wake_at": context.direct_next_wake_at,
                            "direct_failures": context.direct_failures,
                            "profile_next_wake_at": context.profile_next_wake_at,
                            "profile_failures": context.profile_failures,
                        })
                    })
                    .collect(),
            ),
            Err(_) => serde_json::Value::String("poisoned".to_string()),
        };
        serde_json::json!({
            "sync": sync,
            "direct": self.direct_messages.context_snapshot_for_test(),
            "profile_updates": self.profile_updates.context_snapshot_for_test(),
        })
    }

    /// Called only by the Runtime-owned discovery sync task cadence.
    pub(crate) async fn sync_registered_contexts_once(&self, now: u64) {
        let Ok(_sync_guard) = self.sync_pass_lock.try_lock() else {
            return;
        };
        let due = match self.sync_contexts.lock() {
            Ok(contexts) => due_sync_contexts(&contexts, now),
            Err(_) => {
                tracing::warn!("discovery sync context lock is poisoned");
                return;
            }
        };
        for (key, context) in due {
            #[cfg(test)]
            let authorized = context.verified_for_test
                || ensure_sync_context_authorized(
                    context.store.as_ref(),
                    &context.profile,
                    &key.principal_id,
                    &context.session_id,
                    context.proof_binding_id.as_deref(),
                    &context.grant_id,
                    now,
                )
                .is_ok();
            #[cfg(not(test))]
            let authorized = ensure_sync_context_authorized(
                context.store.as_ref(),
                &context.profile,
                &key.principal_id,
                &context.session_id,
                context.proof_binding_id.as_deref(),
                &context.grant_id,
                now,
            )
            .is_ok();
            if !authorized {
                if let Ok(mut contexts) = self.sync_contexts.lock() {
                    contexts.remove(&key);
                }
                continue;
            }
            let generation_profile = match signed_profile_bytes(&context.profile) {
                Ok(bytes) => bytes,
                Err(_) => {
                    if let Ok(mut contexts) = self.sync_contexts.lock() {
                        contexts.remove(&key);
                    }
                    continue;
                }
            };
            let discovery_result = if context.discovery_next_wake_at <= now {
                Some(
                    self.refresh(context.store.as_ref(), &context.profile, now)
                        .await
                        .is_ok(),
                )
            } else {
                None
            };
            let direct_result = if context.direct_next_wake_at <= now {
                let retried = self
                    .direct_messages
                    .retry_pending(&key.profile_did, now)
                    .await
                    .is_ok();
                // Unsettled removals ride the same cadence: deliver the exact
                // signed revocation until the peer's device acknowledges it,
                // re-minting only once an envelope's own lifetime lapses.
                let revoked = self
                    .retry_contact_revocations(context.store.as_ref(), &context.profile, now)
                    .await
                    .is_ok();
                Some(retried && revoked)
            } else {
                None
            };
            let profile_result = if context.profile_next_wake_at <= now {
                Some(
                    self.profile_updates
                        .announce_pending(&key.profile_did, now)
                        .await
                        .is_ok(),
                )
            } else {
                None
            };
            let Ok(mut contexts) = self.sync_contexts.lock() else {
                tracing::warn!("discovery sync context lock is poisoned");
                return;
            };
            let Some(current) = contexts.get_mut(&key) else {
                continue;
            };
            let current_profile = signed_profile_bytes(&current.profile);
            if current.session_id != context.session_id
                || current.proof_binding_id != context.proof_binding_id
                || current.grant_id != context.grant_id
                || current_profile.as_ref().ok() != Some(&generation_profile)
            {
                continue;
            }
            if let Some(success) = discovery_result {
                let cadence = match current.store.discovery_enabled() {
                    Ok(true) => DISCOVERY_SYNC_ENABLED_CADENCE_SECS,
                    Ok(false) => DISCOVERY_SYNC_IDLE_CADENCE_SECS,
                    Err(_) => DISCOVERY_SYNC_MAX_BACKOFF_SECS,
                };
                update_sync_schedule(
                    &mut current.discovery_next_wake_at,
                    &mut current.discovery_failures,
                    success,
                    cadence,
                    now,
                );
            }
            if let Some(success) = direct_result {
                update_sync_schedule(
                    &mut current.direct_next_wake_at,
                    &mut current.direct_failures,
                    success,
                    DIRECT_SYNC_CADENCE_SECS,
                    now,
                );
            }
            if let Some(success) = profile_result {
                update_sync_schedule(
                    &mut current.profile_next_wake_at,
                    &mut current.profile_failures,
                    success,
                    PROFILE_SYNC_CADENCE_SECS,
                    now,
                );
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn status(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        self.local_status(store, profile, now)
    }

    pub(crate) fn local_status(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        self.require_profile_store_match(store, profile)?;
        self.status_from_state(store, now)
    }

    /// Derives one summary projection without inserting, pruning, or updating
    /// the principal's transient discovery client state.
    pub(crate) fn read_only_status(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        let profile_did = self.require_profile_store_match(store, profile)?;
        let enabled = store.discovery_enabled()?;
        let stored_current = self.stored_published_local_advertisement(store, now)?;
        let states = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
        let mut state = states.get(profile_did).cloned().unwrap_or_default();
        normalize_client_state_for_status(&mut state, enabled, stored_current.as_ref(), now);
        project_discovery_status(&state, enabled, store)
    }

    #[cfg(test)]
    pub(crate) fn client_state_snapshot_for_test(&self) -> serde_json::Value {
        let Ok(states) = self.state.lock() else {
            return serde_json::Value::String("poisoned".to_string());
        };
        serde_json::Value::Array(
            states
                .iter()
                .map(|(profile_did, state)| {
                    serde_json::json!({
                        "profile_did": profile_did,
                        "current": state.current_advertisement.as_ref().map(|cached| {
                            cached.verified.message().envelope_sha256()
                        }),
                        "visible": state.visible_advertisements.iter().map(|(key, cached)| {
                            (key, cached.verified.message().envelope_sha256())
                        }).collect::<Vec<_>>(),
                        "heads": state.observed_profile_heads.iter().map(|(key, head)| {
                            (key, head.revision, &head.profile_envelope_sha256, head.observed_at)
                        }).collect::<Vec<_>>(),
                        "remote_visibility_may_remain_until": state.remote_visibility_may_remain_until,
                        "transport_available": state.transport_available,
                    })
                })
                .collect(),
        )
    }

    pub(crate) async fn set_enabled(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        enabled: bool,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        let profile_did = self.require_profile_store_match(store, profile)?;
        if enabled {
            store.set_discovery_enabled(true, now)?;
            let mut states = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
            let state = client_state_mut(&mut states, profile_did)?;
            state.current_advertisement = None;
            state.remote_visibility_may_remain_until = None;
            state.transport_available = false;
        } else {
            store.set_discovery_enabled(false, now)?;
            let current = {
                let mut states = self
                    .state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
                let state = client_state_mut(&mut states, profile_did)?;
                let current = state.current_advertisement.take();
                state.visible_advertisements.clear();
                current
            }
            .or(self.stored_published_local_advertisement(store, now)?);
            let remote_visibility_may_remain_until = current
                .as_ref()
                .map(|current| current.verified.message().envelope().payload.expires_at);
            let mut states = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
            let state = client_state_mut(&mut states, profile_did)?;
            state.remote_visibility_may_remain_until = remote_visibility_may_remain_until;
            state.transport_available = false;
        }
        self.status_from_state(store, now)
    }

    pub(crate) async fn refresh(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        let profile_did = self.require_profile_store_match(store, profile)?;
        let enabled = store.discovery_enabled()?;
        // Expiry ends the bounded relay exposure. Keep any referenced signed
        // advertisement as immutable request/decision evidence, but clear the
        // published/pending pointer before deciding whether to renew it.
        store.clear_expired_published_local_advertisement(now)?;
        let stored_current = self.stored_published_local_advertisement(store, now)?;
        let has_pending_outbox = !store
            .resendable_outgoing_contact_requests(now, 1)?
            .is_empty()
            || !store.resendable_contact_decisions(now, 1)?.is_empty();
        if !enabled && stored_current.is_none() && !has_pending_outbox {
            return self.status_from_state(store, now);
        }
        let outbox_result = if has_pending_outbox || enabled {
            self.resubmit_pending_outbox(store, now).await
        } else {
            Ok(())
        };
        let mailbox_result = if has_pending_outbox || enabled || stored_current.is_some() {
            self.poll_mailboxes(store, profile, now).await
        } else {
            Ok(())
        };
        if let Err(err) = outbox_result {
            self.set_transport_available(profile_did, false)?;
            return Err(err.context("discovery relay outbox delivery failed"));
        }
        if let Err(err) = mailbox_result {
            self.set_transport_available(profile_did, false)?;
            return Err(err.context("discovery relay mailbox delivery failed"));
        }
        let maybe_current =
            self.current_or_renewed_local_advertisement_if_enabled(store, profile, now)?;
        if let Some(cached) = maybe_current {
            let response = self
                .invoke_bootstrap(
                    "query",
                    serde_json::to_value(DiscoveryProviderQueryRequest {
                        op: "query".to_string(),
                        advertisement: encode_bytes(&cached.envelope_bytes),
                    })?,
                )
                .await;
            match response {
                Ok(response) => {
                    let query: DiscoveryProviderAdvertisementResponse =
                        match serde_json::from_value(response) {
                            Ok(query) => query,
                            Err(_) => {
                                self.mark_query_unavailable(profile_did, cached.clone())?;
                                anyhow::bail!("invalid discovery query response");
                            }
                        };
                    if query.status != "ok" {
                        self.mark_query_unavailable(profile_did, cached.clone())?;
                        anyhow::bail!("discovery query returned an error");
                    }
                    let advertisements =
                        match decode_advertisement_response(&self.authority.profile, &query, now) {
                            Ok(advertisements) => advertisements,
                            Err(_) => {
                                self.mark_query_unavailable(profile_did, cached.clone())?;
                                anyhow::bail!("invalid discovery query response");
                            }
                        };
                    let mut states = self
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
                    let state = client_state_mut(&mut states, profile_did)?;
                    state.current_advertisement = Some(cached);
                    apply_decoded_advertisements(state, advertisements, true)?;
                }
                Err(_) => {
                    let mut states = self
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
                    let state = client_state_mut(&mut states, profile_did)?;
                    state.current_advertisement = Some(cached);
                    state.transport_available = false;
                    anyhow::bail!("discovery relay query failed");
                }
            }
        } else if let Some(current) = stored_current {
            let withdrawal = self.authority.prepare_withdrawal(&current.verified, now)?;
            match self
                .invoke_bootstrap(
                    "withdraw",
                    serde_json::to_value(DiscoveryProviderWithdrawalRequest {
                        op: "withdraw".to_string(),
                        withdrawal: encode_bytes(&withdrawal),
                    })?,
                )
                .await
            {
                Ok(response) => {
                    match serde_json::from_value(response) {
                        Ok(DiscoveryProviderStatusResponse { status, .. }) if status == "ok" => {}
                        _ => {
                            self.mark_withdrawal_unavailable(profile_did, &current)?;
                            anyhow::bail!("discovery relay withdrawal failed");
                        }
                    }
                    store.settle_published_local_advertisement_withdrawal(
                        current.verified.message().envelope_sha256(),
                        now,
                    )?;
                    let mut states = self
                        .state
                        .lock()
                        .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
                    let state = client_state_mut(&mut states, profile_did)?;
                    state.remote_visibility_may_remain_until = None;
                    state.transport_available = true;
                }
                Err(_) => {
                    self.mark_withdrawal_unavailable(profile_did, &current)?;
                    anyhow::bail!("discovery relay withdrawal failed");
                }
            }
        }
        self.status_from_state(store, now)
    }

    pub(crate) async fn send_contact_request(
        &self,
        store: &CollaborationContactStore,
        advertisement_id: &str,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<()> {
        let profile_did = profile.document().profile_did.as_str();
        let advertisement = {
            let states = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
            states
                .get(profile_did)
                .and_then(|state| {
                    state
                        .visible_advertisements
                        .values()
                        .find(|advertisement| {
                            advertisement.verified.message().envelope_sha256() == advertisement_id
                        })
                        .cloned()
                })
                .ok_or_else(|| anyhow::anyhow!("discovery advertisement is not visible"))?
        };
        {
            let _guard = self
                .intent_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("discovery intent lock is poisoned"))?;
            match store.stored_outgoing_contact_request(
                advertisement.verified.message().envelope_sha256(),
                advertisement.verified.profile_did(),
                now,
            )? {
                Some(_) => {}
                None => {
                    let request = self.authority.prepare_contact_request(
                        &advertisement.verified,
                        profile,
                        now,
                    )?;
                    store.record_outgoing_contact_request(
                        &request,
                        &advertisement.envelope_bytes,
                        now,
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn submit_contact_decision(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        request_hash: &str,
        decision: CollaborationContactDecision,
        now: u64,
    ) -> anyhow::Result<()> {
        {
            let _guard = self
                .intent_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("discovery intent lock is poisoned"))?;
            match store.stored_contact_decision_receipt(request_hash)? {
                Some(receipt) => {
                    let stored_receipt: SignedCollaborationContactDecisionReceipt =
                        serde_json::from_slice(&receipt).map_err(|err| {
                            anyhow::anyhow!("stored contact decision receipt is invalid: {err}")
                        })?;
                    if stored_receipt.payload.decision != decision {
                        anyhow::bail!("contact request already has a different terminal decision");
                    }
                }
                None => {
                    let request = store
                        .pending_incoming_request(request_hash, now)?
                        .ok_or_else(|| {
                            anyhow::anyhow!("discovery request is not waiting for a decision")
                        })?;
                    let receipt = self
                        .authority
                        .prepare_contact_decision_receipt(&request, profile, decision, now)?;
                    store.record_contact_decision_receipt(&receipt, now)?;
                }
            }
        }
        Ok(())
    }

    /// Removes an accepted contact. The revocation is minted and durably
    /// recorded in this call, so local removal is immediate; delivery to the
    /// peer's device rides the sync loop until acknowledged. Inbox stays the
    /// only Accept/Decline surface — removal is not a decision on a pending
    /// request, it ends an accepted relationship.
    pub(crate) async fn remove_contact(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        remote_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let _guard = self
            .intent_mutex
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery intent lock is poisoned"))?;
        let snapshot = store.snapshot()?;
        if snapshot
            .removed()
            .iter()
            .any(|removed| removed.remote_profile_did() == remote_profile_did)
        {
            return Ok(());
        }
        let contact = snapshot
            .contacts()
            .iter()
            .find(|contact| contact.remote_profile_did() == remote_profile_did)
            .ok_or_else(|| anyhow::anyhow!("people contact not found"))?;
        let envelope = self.authority.prepare_contact_revocation(
            profile,
            remote_profile_did,
            contact.conversation_id(),
            now,
        )?;
        store.record_local_contact_revocation(&envelope, profile, now)?;
        Ok(())
    }

    async fn retry_contact_revocations(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<()> {
        // A removal must eventually reach the removed peer, however long
        // they stay offline: end-of-life re-mints the exact signed fact.
        debug_assert!(matches!(
            DECLARED_CONTACT_REVOCATION_END_OF_LIFE,
            crate::collaboration_delivery::DeliveryEndOfLife::RemintExact
        ));
        let resendable = store.resendable_contact_revocations(MAX_DISCOVERY_QUERY_RESULTS)?;
        let mut plan = Vec::new();
        for pending in resendable {
            let envelope = if pending.expires_at <= now {
                // The peer stayed offline past one envelope lifetime. The
                // removal fact and its timestamp are unchanged; only the
                // delivery envelope is renewed, and the store checks that.
                let fresh = self.authority.prepare_contact_revocation_at(
                    profile,
                    &pending.remote_profile_did,
                    &crate::collaboration_contact_store::stable_direct_conversation_id(
                        &self.authority.profile.profile().network_id,
                        &profile.document().profile_did,
                        &pending.remote_profile_did,
                    )?,
                    pending.removed_at,
                    now,
                )?;
                store.refresh_local_contact_revocation(&pending.remote_profile_did, &fresh, now)?;
                fresh
            } else {
                pending.envelope
            };
            plan.push(crate::collaboration_delivery::DeliveryPlanItem {
                key: pending.remote_profile_did.clone(),
                envelope,
                recipient_endpoint_did: pending.recipient_endpoint_did.clone(),
            });
        }
        let direct = &self.direct_messages;
        crate::collaboration_delivery::run_bounded_delivery_pass(
            plan,
            |item| async move {
                let outcome = direct
                    .deliver_contact_revocation(&item.envelope, &item.recipient_endpoint_did, now)
                    .await
                    .map(|()| crate::collaboration_delivery::DeliveryAttempt::Settled);
                (item, outcome)
            },
            |item| store.settle_local_contact_revocation(&item.key, now),
        )
        .await
    }

    fn current_or_renewed_local_advertisement_if_enabled(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<Option<CachedAdvertisement>> {
        if !store.discovery_enabled()? {
            return Ok(None);
        }
        let profile_did = profile.document().profile_did.as_str();
        let maybe_current = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?
            .get(profile_did)
            .and_then(|state| state.current_advertisement.clone());
        let maybe_current = match maybe_current {
            Some(current) => Some(current),
            None => self.stored_published_local_advertisement(store, now)?,
        };
        match maybe_current {
            Some(current)
                if same_signed_profile(&current.verified, profile)?
                    && current.verified.message().envelope().payload.expires_at
                        > now.saturating_add(DISCOVERY_ADVERTISEMENT_RENEWAL_WINDOW_SECS) =>
            {
                Ok(Some(current))
            }
            Some(_) => Ok(Some(
                self.prepare_and_store_local_advertisement(store, profile, now)?,
            )),
            None => Ok(Some(
                self.prepare_and_store_local_advertisement(store, profile, now)?,
            )),
        }
    }

    fn prepare_and_store_local_advertisement(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<CachedAdvertisement> {
        let envelope_bytes = self.authority.prepare_advertisement(profile, now)?;
        store.store_local_advertisement(&envelope_bytes, now)?;
        let verified = verify_collaboration_discovery_advertisement(
            &envelope_bytes,
            &self.authority.profile,
            now,
        )?;
        Ok(CachedAdvertisement {
            envelope_bytes,
            verified,
        })
    }

    fn stored_published_local_advertisement(
        &self,
        store: &CollaborationContactStore,
        now: u64,
    ) -> anyhow::Result<Option<CachedAdvertisement>> {
        let Some(envelope_bytes) = store.published_local_advertisement(now)? else {
            return Ok(None);
        };
        let verified = verify_collaboration_discovery_advertisement(
            &envelope_bytes,
            &self.authority.profile,
            now,
        )?;
        Ok(Some(CachedAdvertisement {
            envelope_bytes,
            verified,
        }))
    }

    fn require_profile_store_match<'a>(
        &self,
        store: &'a CollaborationContactStore,
        profile: &'a VerifiedCollaborationProfileDocument,
    ) -> anyhow::Result<&'a str> {
        let profile_did = profile.document().profile_did.as_str();
        if store.local_profile_did() != profile_did {
            anyhow::bail!("discovery profile does not match the scoped contact authority");
        }
        Ok(profile_did)
    }

    async fn resubmit_pending_outbox(
        &self,
        store: &CollaborationContactStore,
        now: u64,
    ) -> anyhow::Result<()> {
        let mut requests = VecDeque::from(
            store.resendable_outgoing_contact_requests(now, MAX_DISCOVERY_QUERY_RESULTS)?,
        );
        let mut decisions =
            VecDeque::from(store.resendable_contact_decisions(now, MAX_DISCOVERY_QUERY_RESULTS)?);
        let mut prefer_requests = true;
        for _ in 0..MAX_DISCOVERY_OUTBOX_SENDS_PER_SYNC {
            let next_is_request = match (requests.is_empty(), decisions.is_empty()) {
                (true, true) => break,
                (false, true) => true,
                (true, false) => false,
                (false, false) => prefer_requests,
            };
            if next_is_request {
                let request = requests
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("pending contact request queue underflow"))?;
                let response = self
                    .invoke_bootstrap(
                        "send_contact_request",
                        serde_json::to_value(DiscoveryProviderContactRequest {
                            op: "send_contact_request".to_string(),
                            request: encode_bytes(&request),
                        })?,
                    )
                    .await?;
                require_discovery_provider_success(response, "contact request submission")?;
            } else {
                let receipt = decisions
                    .pop_front()
                    .ok_or_else(|| anyhow::anyhow!("pending contact decision queue underflow"))?;
                let response = self
                    .invoke_bootstrap(
                        "submit_contact_decision_receipt",
                        serde_json::to_value(DiscoveryProviderDecisionReceiptRequest {
                            op: "submit_contact_decision_receipt".to_string(),
                            receipt: encode_bytes(&receipt),
                        })?,
                    )
                    .await?;
                require_discovery_provider_success(response, "contact decision submission")?;
            }
            if !requests.is_empty() && !decisions.is_empty() {
                prefer_requests = !prefer_requests;
            } else {
                prefer_requests = !requests.is_empty();
            }
        }
        Ok(())
    }

    fn set_transport_available(&self, profile_did: &str, available: bool) -> anyhow::Result<()> {
        let mut states = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
        client_state_mut(&mut states, profile_did)?.transport_available = available;
        Ok(())
    }

    fn mark_query_unavailable(
        &self,
        profile_did: &str,
        current_advertisement: CachedAdvertisement,
    ) -> anyhow::Result<()> {
        let mut states = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
        let state = client_state_mut(&mut states, profile_did)?;
        state.current_advertisement = Some(current_advertisement);
        state.transport_available = false;
        Ok(())
    }

    fn mark_withdrawal_unavailable(
        &self,
        profile_did: &str,
        current_advertisement: &CachedAdvertisement,
    ) -> anyhow::Result<()> {
        let mut states = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
        let state = client_state_mut(&mut states, profile_did)?;
        state.remote_visibility_may_remain_until = Some(
            current_advertisement
                .verified
                .message()
                .envelope()
                .payload
                .expires_at,
        );
        state.transport_available = false;
        Ok(())
    }

    async fn poll_mailboxes(
        &self,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<()> {
        let request_poll = self.authority.prepare_mailbox_poll(
            profile,
            CollaborationDiscoveryMailboxKind::Requests,
            now,
        )?;
        let request_response = self
            .invoke_bootstrap(
                "poll_requests",
                serde_json::to_value(DiscoveryProviderMailboxPollRequest {
                    op: "poll_requests".to_string(),
                    poll: encode_bytes(&request_poll),
                })?,
            )
            .await?;
        let request_response: DiscoveryProviderMailboxResponse =
            serde_json::from_value(request_response)
                .context("invalid discovery request mailbox response")?;
        if request_response.status != "ok" {
            anyhow::bail!("discovery request mailbox returned an error");
        }
        for request in request_response.data.requests {
            let request_bytes = decode_bytes(&request, "discovery request mailbox entry")?;
            match store.record_incoming_contact_request(&request_bytes, now)? {
                ContactStoreWrite::Recorded | ContactStoreWrite::Replayed => {}
            }
        }

        let decision_poll = self.authority.prepare_mailbox_poll(
            profile,
            CollaborationDiscoveryMailboxKind::Decisions,
            now,
        )?;
        let decision_response = self
            .invoke_bootstrap(
                "poll_decisions",
                serde_json::to_value(DiscoveryProviderMailboxPollRequest {
                    op: "poll_decisions".to_string(),
                    poll: encode_bytes(&decision_poll),
                })?,
            )
            .await?;
        let decision_response: DiscoveryProviderMailboxResponse =
            serde_json::from_value(decision_response)
                .context("invalid discovery decision mailbox response")?;
        if decision_response.status != "ok" {
            anyhow::bail!("discovery decision mailbox returned an error");
        }
        for receipt in decision_response.data.decisions {
            let receipt_bytes = decode_bytes(&receipt, "discovery decision mailbox entry")?;
            match store.record_contact_decision_receipt(&receipt_bytes, now)? {
                ContactStoreWrite::Recorded | ContactStoreWrite::Replayed => {}
            }
        }
        Ok(())
    }

    fn status_from_state(
        &self,
        store: &CollaborationContactStore,
        now: u64,
    ) -> anyhow::Result<CollaborationDiscoveryStatus> {
        let enabled = store.discovery_enabled()?;
        let stored_current = self.stored_published_local_advertisement(store, now)?;
        let mut states = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery client state lock is poisoned"))?;
        let state = client_state_mut(&mut states, store.local_profile_did())?;
        normalize_client_state_for_status(state, enabled, stored_current.as_ref(), now);
        project_discovery_status(state, enabled, store)
    }

    async fn invoke_bootstrap(
        &self,
        op: &str,
        request: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let mut errors = Vec::new();
        for peer in self.bootstrap_peers.iter() {
            match self
                .registry
                .invoke_provider(ProviderInvocation {
                    source: "collaboration-discovery".to_string(),
                    target: COLLABORATION_DISCOVERY_PROVIDER_SCHEME.to_string(),
                    op: op.to_string(),
                    request: {
                        let mut request = request.clone();
                        request["op"] = serde_json::Value::String(op.to_string());
                        request
                    },
                    transfer: ProviderTransfer::Json,
                    range: None,
                    progress: None,
                    transport: ProviderInvocationTransport::Carrier(
                        ProviderCarrierRoute::ConnectTicket {
                            connect_ticket: peer.connect_ticket.clone(),
                            peer_did: Some(peer.node_id.clone()),
                            timeout_ms: Some(DISCOVERY_PROVIDER_TIMEOUT_MS),
                        },
                    ),
                })
                .await
            {
                Ok(mut response) => {
                    if let Some(object) = response.as_object_mut() {
                        object.remove("_runtime_transfer");
                    }
                    return Ok(response);
                }
                Err(err) => errors.push(err.to_string()),
            }
        }
        anyhow::bail!("discovery relay invocation failed: {}", errors.join(" | "))
    }
}

fn normalize_client_state_for_status(
    state: &mut DiscoveryClientState,
    enabled: bool,
    stored_current: Option<&CachedAdvertisement>,
    now: u64,
) {
    let stored_current = stored_current
        .filter(|current| current.verified.message().envelope().payload.expires_at > now);
    if state
        .current_advertisement
        .as_ref()
        .is_some_and(|current| current.verified.message().envelope().payload.expires_at <= now)
    {
        state.current_advertisement = None;
        state.visible_advertisements.clear();
    }
    if enabled {
        if state.current_advertisement.is_none() {
            state.current_advertisement = stored_current.cloned();
        }
    } else {
        state.current_advertisement = None;
        state.visible_advertisements.clear();
    }
    if !enabled && state.remote_visibility_may_remain_until.is_none() {
        state.remote_visibility_may_remain_until = stored_current.map(|advertisement| {
            advertisement
                .verified
                .message()
                .envelope()
                .payload
                .expires_at
        });
    }
    if state
        .remote_visibility_may_remain_until
        .is_some_and(|expires_at| expires_at <= now)
    {
        state.remote_visibility_may_remain_until = None;
    }
}

fn project_discovery_status(
    state: &DiscoveryClientState,
    enabled: bool,
    store: &CollaborationContactStore,
) -> anyhow::Result<CollaborationDiscoveryStatus> {
    let visible_people = if state.current_advertisement.is_some() {
        state
            .visible_advertisements
            .values()
            .map(|cached| DiscoveryVisiblePerson {
                advertisement_id: cached.verified.message().envelope_sha256().to_string(),
                display_name: cached.verified.display_name().to_string(),
                handle: cached.verified.handle().map(str::to_string),
                last_seen_at: cached.verified.message().envelope().payload.created_at,
                expires_at: cached.verified.message().envelope().payload.expires_at,
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(CollaborationDiscoveryStatus {
        available: state.transport_available,
        enabled,
        expires_at: state
            .current_advertisement
            .as_ref()
            .map(|current| current.verified.message().envelope().payload.expires_at),
        remote_visibility_may_remain_until: state.remote_visibility_may_remain_until,
        visible_people,
        incoming_requests: store.pending_incoming_requests()?,
    })
}

pub(crate) fn signed_profile_bytes(
    profile: &VerifiedCollaborationProfileDocument,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(profile.signed_envelope())?)
}

/// Revalidates the existing Home-launch authority on each Runtime worker pass.
/// The sync context retains identifiers only; the session grant and proof
/// record remain the sole source of truth in the existing auth store.
struct ProfileContextAuthority<'a> {
    principal_id: &'a str,
    session_id: &'a str,
    proof_binding_id: Option<&'a str>,
    grant_id: &'a str,
    required_app: &'a str,
}

pub(crate) fn ensure_sync_context_authorized(
    store: &CollaborationContactStore,
    profile: &VerifiedCollaborationProfileDocument,
    principal_id: &str,
    session_id: &str,
    proof_binding_id: Option<&str>,
    grant_id: &str,
    now: u64,
) -> anyhow::Result<()> {
    ensure_profile_context_authorized(
        store,
        profile,
        ProfileContextAuthority {
            principal_id,
            session_id,
            proof_binding_id,
            grant_id,
            required_app: "home",
        },
        now,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ensure_direct_context_authorized(
    store: &CollaborationContactStore,
    profile: &VerifiedCollaborationProfileDocument,
    principal_id: &str,
    session_id: &str,
    proof_binding_id: Option<&str>,
    grant_id: &str,
    authority_app: &str,
    now: u64,
) -> anyhow::Result<()> {
    // Session grants enumerate authority actors, and browser chat windows
    // act under Home authority — the caller passes the authority actor its
    // validated launch token already proved against the grant.
    ensure_profile_context_authorized(
        store,
        profile,
        ProfileContextAuthority {
            principal_id,
            session_id,
            proof_binding_id,
            grant_id,
            required_app: authority_app,
        },
        now,
    )
}

fn ensure_profile_context_authorized(
    store: &CollaborationContactStore,
    profile: &VerifiedCollaborationProfileDocument,
    authority: ProfileContextAuthority<'_>,
    now: u64,
) -> anyhow::Result<()> {
    if store.principal_id() != authority.principal_id {
        anyhow::bail!("Profile context principal does not match the scoped store");
    }
    if store.local_profile_did() != profile.document().profile_did {
        anyhow::bail!("Profile context Profile does not match the scoped store");
    }
    let proof_binding_id = authority
        .proof_binding_id
        .filter(|binding| !binding.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("Profile context requires a proof-bound session"))?;
    let auth_data_dir = crate::api::gateway::home_launch_auth_data_dir(store.data_root());
    let grant = crate::auth::load_active_session_grant(&auth_data_dir, authority.session_id, now)
        .map_err(|_| anyhow::anyhow!("Profile context session is not active"))?;
    if grant.principal_id != authority.principal_id
        || grant.proof_binding_id != proof_binding_id
        || grant.grant_id != authority.grant_id
        || !grant.apps.iter().any(|app| app == authority.required_app)
    {
        anyhow::bail!("Profile context authority mismatch");
    }
    let principal =
        crate::auth::load_principal_for_proof_binding(&auth_data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if principal.principal_id != authority.principal_id {
        anyhow::bail!("Profile context proof binding does not match the active principal");
    }
    let current_profile = crate::collaboration_profile_authority::load_profile_authority(
        store.data_root(),
        authority.principal_id,
        store.localhost_root(),
    )?
    .ok_or_else(|| anyhow::anyhow!("Profile context Profile is no longer available"))?;
    // Identity is the Profile DID, and it does not move when a person
    // renames themselves. Comparing whole signed documents asked whether the
    // Profile had changed at all, so editing your own name read as a
    // different authority and your Home stopped accepting your contacts'
    // mail. What actually threatens this context is a different Profile
    // taking over the store, or a rolled-back revision replaying an older
    // truth; a newer revision of the same DID is the same person, saying
    // something new about themselves.
    if current_profile.document().profile_did != profile.document().profile_did {
        anyhow::bail!("Profile context Profile no longer matches the registered authority");
    }
    if current_profile.document().revision < profile.document().revision {
        anyhow::bail!("Profile context Profile revision rolled back");
    }
    Ok(())
}

impl CollaborationDiscoveryStatus {
    pub(crate) fn available(&self) -> bool {
        self.available
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }

    #[cfg(test)]
    pub(crate) fn remote_visibility_may_remain(&self) -> bool {
        self.remote_visibility_may_remain_until.is_some()
    }

    pub(crate) fn remote_visibility_may_remain_until(&self) -> Option<u64> {
        self.remote_visibility_may_remain_until
    }

    pub(crate) fn visible_people(&self) -> &[DiscoveryVisiblePerson] {
        &self.visible_people
    }

    pub(crate) fn incoming_requests(&self) -> &[PendingIncomingContactRequest] {
        &self.incoming_requests
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        available: bool,
        enabled: bool,
        expires_at: Option<u64>,
        remote_visibility_may_remain_until: Option<u64>,
        visible_people: Vec<DiscoveryVisiblePerson>,
        incoming_requests: Vec<PendingIncomingContactRequest>,
    ) -> Self {
        Self {
            available,
            enabled,
            expires_at,
            remote_visibility_may_remain_until,
            visible_people,
            incoming_requests,
        }
    }
}

impl DiscoveryVisiblePerson {
    pub(crate) fn advertisement_id(&self) -> &str {
        &self.advertisement_id
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    pub(crate) fn last_seen_at(&self) -> u64 {
        self.last_seen_at
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        advertisement_id: impl Into<String>,
        display_name: impl Into<String>,
        handle: Option<String>,
        last_seen_at: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            advertisement_id: advertisement_id.into(),
            display_name: display_name.into(),
            handle,
            last_seen_at,
            expires_at,
        }
    }
}

impl CollaborationDiscoveryAuthority {
    fn new(signing_key: SigningKey, profile: VerifiedCollaborationNetworkProfile) -> Self {
        Self {
            signing_key,
            profile,
        }
    }

    fn prepare_advertisement(
        &self,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let payload = CollaborationDiscoveryAdvertisementPayload {
            signed_profile: profile.signed_envelope().clone(),
        };
        let envelope = self.sign_message(
            &profile.document().profile_did,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            canonical_payload_value(&payload)?,
            CollaborationMessageTiming {
                created_at: now,
                ttl_secs: COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
            },
        )?;
        verify_collaboration_discovery_advertisement(&envelope, &self.profile, now)?;
        Ok(envelope)
    }

    fn prepare_withdrawal(
        &self,
        current_advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let payload = CollaborationDiscoveryWithdrawalPayload {
            advertisement_envelope_sha256: current_advertisement
                .message()
                .envelope_sha256()
                .to_string(),
        };
        let envelope = self.sign_message(
            current_advertisement.profile_did(),
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_WITHDRAWAL_PAYLOAD_TYPE,
            serde_json::to_value(payload)?,
            CollaborationMessageTiming {
                created_at: now,
                ttl_secs: COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
            },
        )?;
        verify_collaboration_discovery_withdrawal(
            &envelope,
            &self.profile,
            current_advertisement,
            now,
        )?;
        Ok(envelope)
    }

    fn prepare_mailbox_poll(
        &self,
        profile: &VerifiedCollaborationProfileDocument,
        mailbox_kind: CollaborationDiscoveryMailboxKind,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let local_endpoint_did = self.local_device_did();
        if !profile.authorizes_endpoint(&local_endpoint_did) {
            anyhow::bail!("local endpoint is not authorized by the signed Profile");
        }
        let envelope = self.sign_message(
            &profile.document().profile_did,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: profile.document().profile_did.clone(),
            },
            COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
            serde_json::to_value(CollaborationDiscoveryMailboxPollPayload {
                mailbox_kind,
                profile_did: profile.document().profile_did.clone(),
            })?,
            CollaborationMessageTiming {
                created_at: now,
                ttl_secs: COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
            },
        )?;
        verify_collaboration_discovery_mailbox_poll(&envelope, &self.profile, now)?;
        Ok(envelope)
    }

    fn prepare_contact_request(
        &self,
        advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let payload = CollaborationContactRequestPayload {
            advertisement_envelope_sha256: advertisement.message().envelope_sha256().to_string(),
            signed_profile: profile.signed_envelope().clone(),
        };
        let envelope = self.sign_message(
            &profile.document().profile_did,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: advertisement
                    .message()
                    .envelope()
                    .payload
                    .sender_profile_did
                    .clone(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            canonical_payload_value(&payload)?,
            CollaborationMessageTiming {
                created_at: now,
                ttl_secs: COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
            },
        )?;
        verify_collaboration_contact_request(&envelope, &self.profile, advertisement, now)?;
        Ok(envelope)
    }

    fn prepare_contact_decision_receipt(
        &self,
        request: &VerifiedCollaborationContactRequest,
        profile: &VerifiedCollaborationProfileDocument,
        decision: CollaborationContactDecision,
        decided_at: u64,
    ) -> anyhow::Result<Vec<u8>> {
        if request.message().envelope().payload.recipient.id != profile.document().profile_did {
            anyhow::bail!("contact decision recipient does not match this Profile");
        }
        let local_endpoint_did = self.local_device_did();
        if !profile.authorizes_endpoint(&local_endpoint_did) {
            anyhow::bail!("contact decision Profile does not authorize this Runtime endpoint");
        }
        let payload = CollaborationContactDecisionReceipt {
            schema: COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1.to_string(),
            network_id: request.message().envelope().payload.network_id.clone(),
            request_envelope_sha256: request.message().envelope_sha256().to_string(),
            conversation_id: request.message().envelope().payload.conversation_id.clone(),
            requester_profile_did: request.requester_profile_did().to_string(),
            requester_endpoint_did: request.route_endpoint_did()?.to_string(),
            request_message_id: request.message().envelope().payload.message_id.clone(),
            request_message_nonce: request.message().envelope().payload.nonce.clone(),
            recipient_profile_did: profile.document().profile_did.clone(),
            recipient_endpoint_did: local_endpoint_did,
            decision,
            decided_at,
        };
        let payload_bytes = serde_json::to_vec(&serde_json::to_value(&payload)?)?;
        let (signature, signer_did) = domain_separated_sign(
            &self.signing_key,
            COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        Ok(
            canonical_signed_collaboration_contact_decision_receipt_bytes(
                &SignedCollaborationContactDecisionReceipt {
                    payload,
                    signature,
                    signer_did,
                },
            )?,
        )
    }

    fn prepare_contact_revocation(
        &self,
        profile: &VerifiedCollaborationProfileDocument,
        remote_profile_did: &str,
        conversation_id: &str,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        self.prepare_contact_revocation_at(profile, remote_profile_did, conversation_id, now, now)
    }

    fn prepare_contact_revocation_at(
        &self,
        profile: &VerifiedCollaborationProfileDocument,
        remote_profile_did: &str,
        conversation_id: &str,
        removed_at: u64,
        now: u64,
    ) -> anyhow::Result<Vec<u8>> {
        if !profile.authorizes_endpoint(&self.local_device_did()) {
            anyhow::bail!("contact revocation Profile does not authorize this Runtime endpoint");
        }
        let payload = crate::collaboration_discovery::CollaborationContactRevocationPayload {
            revoking_profile_did: profile.document().profile_did.clone(),
            revoked_profile_did: remote_profile_did.to_string(),
            end_verb: crate::collaboration_discovery::COLLABORATION_CONTACT_END_VERB_REMOVE
                .to_string(),
            removed_at,
        };
        let envelope = self.sign_message(
            &profile.document().profile_did,
            conversation_id,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: remote_profile_did.to_string(),
            },
            crate::collaboration_discovery::COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE,
            canonical_payload_value(&payload)?,
            CollaborationMessageTiming {
                created_at: now,
                ttl_secs: crate::collaboration_discovery::COLLABORATION_CONTACT_REVOCATION_TTL_SECS,
            },
        )?;
        crate::collaboration_discovery::verify_collaboration_contact_revocation(
            &envelope,
            &self.profile,
            now,
        )?;
        Ok(envelope)
    }

    fn sign_message(
        &self,
        sender_profile_did: &str,
        conversation_id: &str,
        recipient: CollaborationRecipient,
        payload_type: &str,
        payload: serde_json::Value,
        timing: CollaborationMessageTiming,
    ) -> anyhow::Result<Vec<u8>> {
        validate_id(conversation_id, "collaboration discovery conversation_id")?;
        let expires_at = timing
            .created_at
            .checked_add(timing.ttl_secs)
            .context("collaboration discovery TTL overflows its timestamp")?;
        let message = CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: self.profile.profile().network_id.clone(),
            conversation_id: conversation_id.to_string(),
            message_id: random_hex_128()?,
            nonce: random_hex_128()?,
            created_at: timing.created_at,
            expires_at,
            sender_profile_did: sender_profile_did.to_string(),
            sender_service: COLLABORATION_DISCOVERY_SERVICE.to_string(),
            recipient,
            payload_type: payload_type.to_string(),
            payload,
        };
        let payload_bytes = canonical_collaboration_message_bytes(&message)?;
        let (signature, signer_did) = domain_separated_sign(
            &self.signing_key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        Ok(canonical_signed_collaboration_message_bytes(
            &SignedCollaborationMessage {
                payload: message,
                signature,
                signer_did,
            },
        )?)
    }

    fn local_device_did(&self) -> String {
        encode_did_key(&self.signing_key.verifying_key())
    }
}

impl CollaborationDiscoveryRelayProvider {
    fn new(profile: VerifiedCollaborationNetworkProfile) -> Self {
        Self {
            profile,
            state: Mutex::new(DiscoveryRelayState {
                advertisements: HashMap::new(),
                requests: HashMap::new(),
                request_mailboxes: HashMap::new(),
                decision_mailboxes: HashMap::new(),
            }),
        }
    }

    fn prune(&self, now: u64, state: &mut DiscoveryRelayState) {
        state.advertisements.retain(|_, advertisement| {
            advertisement
                .verified
                .message()
                .envelope()
                .payload
                .expires_at
                > now
        });
        state.requests.retain(|request_hash, request| {
            let keep = request.verified.message().envelope().payload.expires_at > now;
            if !keep {
                for mailbox in state.request_mailboxes.values_mut() {
                    mailbox.retain(|candidate| candidate != request_hash);
                }
                for mailbox in state.decision_mailboxes.values_mut() {
                    mailbox.retain(|receipt| receipt.request_hash != *request_hash);
                }
            }
            keep
        });
        for mailbox in state.request_mailboxes.values_mut() {
            mailbox.retain(|request_hash| state.requests.contains_key(request_hash));
            if mailbox.len() > MAX_DISCOVERY_REQUESTS_PER_RECIPIENT {
                mailbox.drain(0..mailbox.len() - MAX_DISCOVERY_REQUESTS_PER_RECIPIENT);
            }
        }
        for mailbox in state.decision_mailboxes.values_mut() {
            mailbox.retain(|receipt| {
                state
                    .requests
                    .get(&receipt.request_hash)
                    .is_some_and(|request| {
                        request
                            .verified
                            .message()
                            .envelope()
                            .payload
                            .expires_at
                            .saturating_add(MAX_COLLABORATION_CLOCK_SKEW_SECS)
                            > now
                    })
            });
            if mailbox.len() > MAX_DISCOVERY_RECEIPTS_PER_RECIPIENT {
                mailbox.drain(0..mailbox.len() - MAX_DISCOVERY_RECEIPTS_PER_RECIPIENT);
            }
        }
        state
            .request_mailboxes
            .retain(|_, mailbox| !mailbox.is_empty());
        state
            .decision_mailboxes
            .retain(|_, mailbox| !mailbox.is_empty());
    }
}

fn total_request_mailbox_entries(state: &DiscoveryRelayState) -> usize {
    state.request_mailboxes.values().map(Vec::len).sum()
}

fn total_decision_mailbox_entries(state: &DiscoveryRelayState) -> usize {
    state.decision_mailboxes.values().map(Vec::len).sum()
}

fn parse_discovery_provider_request<T: serde::de::DeserializeOwned>(
    request: &serde_json::Value,
    label: &str,
) -> Result<T, ProviderError> {
    let mut sanitized = request.clone();
    let object = sanitized.as_object_mut().ok_or_else(|| {
        ProviderError::Provider(format!(
            "invalid {label}: provider request must be an object"
        ))
    })?;
    object.remove("_runtime_invocation");
    serde_json::from_value(sanitized)
        .map_err(|err| ProviderError::Provider(format!("invalid {label}: {err}")))
}

#[async_trait::async_trait]
impl Provider for CollaborationDiscoveryRelayProvider {
    async fn handle(
        &self,
        _request: elastos_runtime::provider::ResourceRequest,
    ) -> Result<elastos_runtime::provider::ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "collaboration discovery relay does not expose resource routes".to_string(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec![COLLABORATION_DISCOVERY_PROVIDER_SCHEME]
    }

    fn name(&self) -> &'static str {
        "collaboration-discovery-relay"
    }

    async fn send_raw(
        &self,
        request: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let op = request
            .get("op")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let now = current_timestamp();
        let mut state = self.state.lock().map_err(|_| {
            ProviderError::Provider("discovery relay state lock is poisoned".to_string())
        })?;
        self.prune(now, &mut state);
        match op {
            "advertise" => {
                let request: DiscoveryProviderAdvertisementRequest =
                    parse_discovery_provider_request(request, "discovery advertise request")?;
                let envelope_bytes =
                    decode_bytes(&request.advertisement, "discovery advertisement")
                        .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let verified = verify_collaboration_discovery_advertisement(
                    &envelope_bytes,
                    &self.profile,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                if !state.advertisements.contains_key(verified.profile_did())
                    && state.advertisements.len() >= MAX_DISCOVERY_TOTAL_ADVERTISEMENTS
                {
                    return Err(ProviderError::Provider(
                        "discovery relay advertisement capacity is full".to_string(),
                    ));
                }
                merge_profile_scoped_advertisement(
                    &mut state.advertisements,
                    CachedAdvertisement {
                        envelope_bytes,
                        verified,
                    },
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                Ok(serde_json::json!({"status":"ok","data":{}}))
            }
            "query" => {
                let request: DiscoveryProviderQueryRequest =
                    parse_discovery_provider_request(request, "discovery query request")?;
                let envelope_bytes =
                    decode_bytes(&request.advertisement, "discovery advertisement")
                        .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let verified = verify_collaboration_discovery_advertisement(
                    &envelope_bytes,
                    &self.profile,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let caller_profile_did = verified.profile_did().to_string();
                if !state.advertisements.contains_key(&caller_profile_did)
                    && state.advertisements.len() >= MAX_DISCOVERY_TOTAL_ADVERTISEMENTS
                {
                    return Err(ProviderError::Provider(
                        "discovery relay advertisement capacity is full".to_string(),
                    ));
                }
                merge_profile_scoped_advertisement(
                    &mut state.advertisements,
                    CachedAdvertisement {
                        envelope_bytes,
                        verified,
                    },
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let mut advertisements = state
                    .advertisements
                    .values()
                    .filter(|advertisement| {
                        advertisement.verified.profile_did() != caller_profile_did
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                advertisements.sort_by(compare_cached_advertisements);
                let advertisements = advertisements
                    .into_iter()
                    .rev()
                    .take(MAX_DISCOVERY_QUERY_RESULTS)
                    .map(|advertisement| encode_bytes(&advertisement.envelope_bytes))
                    .collect();
                Ok(
                    serde_json::to_value(DiscoveryProviderAdvertisementResponse {
                        status: "ok".to_string(),
                        data: DiscoveryProviderAdvertisementData { advertisements },
                    })
                    .expect("discovery query response is serializable"),
                )
            }
            "withdraw" => {
                let request: DiscoveryProviderWithdrawalRequest =
                    parse_discovery_provider_request(request, "discovery withdrawal request")?;
                let envelope_bytes = decode_bytes(&request.withdrawal, "discovery withdrawal")
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let withdrawal_sender = sender_profile_did_from_message(&envelope_bytes)
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let advertisement_hash =
                    advertisement_hash_from_discovery_withdrawal(&envelope_bytes)
                        .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let current = state
                    .advertisements
                    .values()
                    .find(|advertisement| {
                        advertisement.verified.message().envelope_sha256() == advertisement_hash
                    })
                    .cloned()
                    .ok_or_else(|| {
                        ProviderError::Provider(
                            "discovery withdrawal advertisement is not active".to_string(),
                        )
                    })?;
                if current
                    .verified
                    .message()
                    .envelope()
                    .payload
                    .sender_profile_did
                    != withdrawal_sender
                {
                    return Err(ProviderError::Provider(
                        "discovery withdrawal advertisement signer does not match the active advertisement"
                            .to_string(),
                    ));
                }
                verify_collaboration_discovery_withdrawal(
                    &envelope_bytes,
                    &self.profile,
                    &current.verified,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                state.advertisements.remove(current.verified.profile_did());
                Ok(serde_json::json!({"status":"ok","data":{}}))
            }
            "send_contact_request" => {
                let request: DiscoveryProviderContactRequest =
                    parse_discovery_provider_request(request, "contact request submission")?;
                let envelope_bytes = decode_bytes(&request.request, "contact request")
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let request_hash = collaboration_message_envelope_sha256(&envelope_bytes);
                if let Some(existing) = state.requests.get(&request_hash) {
                    if existing.envelope_bytes != envelope_bytes {
                        return Err(ProviderError::Provider(
                            "contact request replay bytes do not match the stored request"
                                .to_string(),
                        ));
                    }
                    return Ok(serde_json::json!({"status":"ok","data":{}}));
                }
                let advertisement_hash = advertisement_hash_from_contact_request(&envelope_bytes)
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let advertisement = state
                    .advertisements
                    .values()
                    .find(|advertisement| {
                        advertisement.verified.message().envelope_sha256() == advertisement_hash
                    })
                    .cloned()
                    .ok_or_else(|| {
                        ProviderError::Provider("discovery advertisement is not active".to_string())
                    })?;
                let verified = verify_collaboration_contact_request(
                    &envelope_bytes,
                    &self.profile,
                    &advertisement.verified,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let request_hash = verified.message().envelope_sha256().to_string();
                let recipient_profile_did = advertisement.verified.profile_did().to_string();
                let request_exists = state.requests.contains_key(&request_hash);
                let mailbox_contains = state
                    .request_mailboxes
                    .get(&recipient_profile_did)
                    .is_some_and(|mailbox| mailbox.contains(&request_hash));
                let mailbox_len = state
                    .request_mailboxes
                    .get(&recipient_profile_did)
                    .map_or(0, Vec::len);
                if !request_exists && state.requests.len() >= MAX_DISCOVERY_TOTAL_REQUESTS {
                    return Err(ProviderError::Provider(
                        "discovery relay request capacity is full".to_string(),
                    ));
                }
                if !mailbox_contains {
                    if mailbox_len >= MAX_DISCOVERY_REQUESTS_PER_RECIPIENT {
                        return Err(ProviderError::Provider(
                            "discovery request mailbox is full".to_string(),
                        ));
                    }
                    if total_request_mailbox_entries(&state)
                        >= MAX_DISCOVERY_TOTAL_REQUEST_MAILBOX_ENTRIES
                    {
                        return Err(ProviderError::Provider(
                            "discovery relay request mailbox capacity is full".to_string(),
                        ));
                    }
                }
                if !request_exists {
                    state.requests.insert(
                        request_hash.clone(),
                        RelayContactRequest {
                            envelope_bytes: envelope_bytes.clone(),
                            verified: verified.clone(),
                            advertisement: advertisement.clone(),
                        },
                    );
                }
                if !mailbox_contains {
                    state
                        .request_mailboxes
                        .entry(recipient_profile_did)
                        .or_default()
                        .push(request_hash);
                }
                Ok(serde_json::json!({"status":"ok","data":{}}))
            }
            "poll_requests" | "poll_decisions" => {
                let request: DiscoveryProviderMailboxPollRequest =
                    parse_discovery_provider_request(request, "discovery mailbox poll request")?;
                let envelope_bytes = decode_bytes(&request.poll, "discovery mailbox poll")
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let verified = verify_collaboration_discovery_mailbox_poll(
                    &envelope_bytes,
                    &self.profile,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let polling_endpoint_did = verified.message().envelope().signer_did.clone();
                let mailbox_profile_did = verified.profile_did();
                if verified.message().envelope().payload.sender_profile_did != mailbox_profile_did {
                    return Err(ProviderError::Provider(
                        "discovery mailbox poll Profile mismatch".to_string(),
                    ));
                }
                let data = match verified.mailbox_kind() {
                    CollaborationDiscoveryMailboxKind::Requests if op == "poll_requests" => {
                        let mut requests = Vec::new();
                        if let Some(mailbox) = state.request_mailboxes.get(mailbox_profile_did) {
                            for request_hash in mailbox {
                                let request =
                                    state.requests.get(request_hash).ok_or_else(|| {
                                        ProviderError::Provider(
                                            "discovery request mailbox state is invalid"
                                                .to_string(),
                                        )
                                    })?;
                                if request.advertisement.verified.profile_did()
                                    != mailbox_profile_did
                                {
                                    return Err(ProviderError::Provider(
                                        "discovery request mailbox state is invalid".to_string(),
                                    ));
                                }
                                if request
                                    .advertisement
                                    .verified
                                    .profile_authorizes_device(&polling_endpoint_did)
                                {
                                    requests.push(encode_bytes(&request.envelope_bytes));
                                }
                            }
                        }
                        DiscoveryProviderMailboxData {
                            requests,
                            decisions: Vec::new(),
                        }
                    }
                    CollaborationDiscoveryMailboxKind::Decisions if op == "poll_decisions" => {
                        let mut decisions = Vec::new();
                        if let Some(mailbox) = state.decision_mailboxes.get(mailbox_profile_did) {
                            for decision in mailbox {
                                let request = state
                                    .requests
                                    .get(&decision.request_hash)
                                    .ok_or_else(|| {
                                        ProviderError::Provider(
                                            "discovery decision mailbox state is invalid"
                                                .to_string(),
                                        )
                                    })?;
                                if request.verified.requester_profile_did() != mailbox_profile_did {
                                    return Err(ProviderError::Provider(
                                        "discovery decision mailbox state is invalid".to_string(),
                                    ));
                                }
                                if request
                                    .verified
                                    .profile_authorizes_device(&polling_endpoint_did)
                                {
                                    decisions.push(encode_bytes(&decision.envelope_bytes));
                                }
                            }
                        }
                        DiscoveryProviderMailboxData {
                            requests: Vec::new(),
                            decisions,
                        }
                    }
                    _ => {
                        return Err(ProviderError::Provider(
                            "discovery mailbox poll kind does not match the requested mailbox"
                                .to_string(),
                        ))
                    }
                };
                Ok(serde_json::to_value(DiscoveryProviderMailboxResponse {
                    status: "ok".to_string(),
                    data,
                })
                .expect("discovery mailbox response is serializable"))
            }
            "submit_contact_decision_receipt" => {
                let request: DiscoveryProviderDecisionReceiptRequest =
                    parse_discovery_provider_request(request, "contact decision submission")?;
                let receipt_bytes = decode_bytes(&request.receipt, "contact decision receipt")
                    .map_err(|err| ProviderError::Provider(err.to_string()))?;
                if receipt_bytes.len() > MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES {
                    return Err(ProviderError::Provider(
                        "contact decision receipt exceeds the protocol byte limit".to_string(),
                    ));
                }
                let raw_receipt: SignedCollaborationContactDecisionReceipt =
                    serde_json::from_slice(&receipt_bytes).map_err(|err| {
                        ProviderError::Provider(format!(
                            "invalid contact decision receipt envelope: {err}"
                        ))
                    })?;
                let request_record = state
                    .requests
                    .get(&raw_receipt.payload.request_envelope_sha256)
                    .cloned()
                    .ok_or_else(|| {
                        ProviderError::Provider("contact request is not available".to_string())
                    })?;
                verify_collaboration_contact_decision_receipt(
                    &receipt_bytes,
                    &request_record.verified,
                    &request_record.advertisement.verified,
                    now,
                )
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
                let requester_profile_did =
                    request_record.verified.requester_profile_did().to_string();
                let existing_decision = state
                    .decision_mailboxes
                    .get(&requester_profile_did)
                    .and_then(|mailbox| {
                        mailbox.iter().find(|receipt| {
                            receipt.request_hash == raw_receipt.payload.request_envelope_sha256
                        })
                    });
                if let Some(existing_decision) = existing_decision {
                    if existing_decision.envelope_bytes != receipt_bytes {
                        return Err(ProviderError::Provider(
                            "contact request already has a different terminal decision".to_string(),
                        ));
                    }
                }
                let mailbox_contains = existing_decision.is_some();
                let mailbox_len = state
                    .decision_mailboxes
                    .get(&requester_profile_did)
                    .map_or(0, Vec::len);
                if !mailbox_contains {
                    if mailbox_len >= MAX_DISCOVERY_RECEIPTS_PER_RECIPIENT {
                        return Err(ProviderError::Provider(
                            "discovery decision mailbox is full".to_string(),
                        ));
                    }
                    if total_decision_mailbox_entries(&state)
                        >= MAX_DISCOVERY_TOTAL_DECISION_MAILBOX_ENTRIES
                    {
                        return Err(ProviderError::Provider(
                            "discovery relay decision mailbox capacity is full".to_string(),
                        ));
                    }
                }
                if !mailbox_contains {
                    state
                        .decision_mailboxes
                        .entry(requester_profile_did)
                        .or_default()
                        .push(RelayDecision {
                            envelope_bytes: receipt_bytes,
                            request_hash: raw_receipt.payload.request_envelope_sha256.clone(),
                        });
                }
                if let Some(request_mailbox) = state
                    .request_mailboxes
                    .get_mut(request_record.advertisement.verified.profile_did())
                {
                    request_mailbox.retain(|request_hash| {
                        request_hash != &raw_receipt.payload.request_envelope_sha256
                    });
                    if request_mailbox.is_empty() {
                        state
                            .request_mailboxes
                            .remove(request_record.advertisement.verified.profile_did());
                    }
                }
                Ok(serde_json::json!({"status":"ok","data":{}}))
            }
            _ => Err(ProviderError::Provider(
                "unsupported collaboration discovery operation".to_string(),
            )),
        }
    }
}

fn decode_advertisement_response(
    profile: &VerifiedCollaborationNetworkProfile,
    response: &DiscoveryProviderAdvertisementResponse,
    now: u64,
) -> anyhow::Result<Vec<CachedAdvertisement>> {
    if response.data.advertisements.len() > MAX_DISCOVERY_QUERY_RESULTS {
        anyhow::bail!("discovery query returned too many advertisements");
    }
    let mut seen = HashSet::new();
    let mut advertisements_by_profile = HashMap::new();
    for entry in &response.data.advertisements {
        let envelope_bytes = decode_bytes(entry, "discovery advertisement response entry")?;
        let verified = verify_collaboration_discovery_advertisement(&envelope_bytes, profile, now)?;
        let advertisement_id = verified.message().envelope_sha256().to_string();
        if !seen.insert(advertisement_id) {
            continue;
        }
        merge_profile_scoped_advertisement(
            &mut advertisements_by_profile,
            CachedAdvertisement {
                envelope_bytes,
                verified,
            },
        )?;
    }
    let mut advertisements = advertisements_by_profile.into_values().collect::<Vec<_>>();
    advertisements.sort_by(compare_cached_advertisements);
    advertisements.reverse();
    Ok(advertisements)
}

fn same_signed_profile(
    advertisement: &VerifiedCollaborationDiscoveryAdvertisement,
    profile: &VerifiedCollaborationProfileDocument,
) -> anyhow::Result<bool> {
    Ok(serde_json::to_vec(advertisement.signed_profile())?
        == serde_json::to_vec(profile.signed_envelope())?)
}

fn compare_cached_advertisements(
    left: &CachedAdvertisement,
    right: &CachedAdvertisement,
) -> std::cmp::Ordering {
    left.verified
        .message()
        .envelope()
        .payload
        .created_at
        .cmp(&right.verified.message().envelope().payload.created_at)
        .then(
            left.verified
                .message()
                .envelope_sha256()
                .cmp(right.verified.message().envelope_sha256()),
        )
}

fn merge_profile_scoped_advertisement(
    advertisements_by_profile: &mut HashMap<String, CachedAdvertisement>,
    advertisement: CachedAdvertisement,
) -> anyhow::Result<()> {
    let profile_did = advertisement.verified.profile_did().to_string();
    let merged = match advertisements_by_profile.get(&profile_did) {
        Some(existing) => merge_two_profile_scoped_advertisements(existing, advertisement)?,
        None => advertisement,
    };
    advertisements_by_profile.insert(profile_did, merged);
    Ok(())
}

fn merge_two_profile_scoped_advertisements(
    existing: &CachedAdvertisement,
    incoming: CachedAdvertisement,
) -> anyhow::Result<CachedAdvertisement> {
    match incoming
        .verified
        .profile_revision()
        .cmp(&existing.verified.profile_revision())
    {
        std::cmp::Ordering::Greater => Ok(incoming),
        std::cmp::Ordering::Less => Ok(existing.clone()),
        std::cmp::Ordering::Equal => {
            if incoming.verified.profile_envelope_sha256()?
                != existing.verified.profile_envelope_sha256()?
            {
                anyhow::bail!("conflicting discovery profile revision for the same profile DID");
            }
            if compare_cached_advertisements(&incoming, existing).is_gt() {
                Ok(incoming)
            } else {
                Ok(existing.clone())
            }
        }
    }
}

fn advertisement_allowed_by_profile_heads(
    heads: &BTreeMap<String, ObservedProfileHead>,
    advertisement: &CachedAdvertisement,
) -> anyhow::Result<bool> {
    let Some(head) = heads.get(advertisement.verified.profile_did()) else {
        return Ok(true);
    };
    match advertisement
        .verified
        .profile_revision()
        .cmp(&head.revision)
    {
        std::cmp::Ordering::Greater => Ok(true),
        std::cmp::Ordering::Less => Ok(false),
        std::cmp::Ordering::Equal => {
            if advertisement.verified.profile_envelope_sha256()? != head.profile_envelope_sha256 {
                anyhow::bail!("conflicting discovery profile revision for the same profile DID");
            }
            Ok(true)
        }
    }
}

fn remember_profile_head(
    heads: &mut BTreeMap<String, ObservedProfileHead>,
    advertisement: &CachedAdvertisement,
) -> anyhow::Result<()> {
    let profile_did = advertisement.verified.profile_did().to_string();
    let observed_at = advertisement
        .verified
        .message()
        .envelope()
        .payload
        .created_at;
    let incoming = ObservedProfileHead {
        revision: advertisement.verified.profile_revision(),
        profile_envelope_sha256: advertisement.verified.profile_envelope_sha256()?,
        observed_at,
    };
    match heads.get(&profile_did) {
        None => {
            heads.insert(profile_did, incoming);
        }
        Some(existing) => match incoming.revision.cmp(&existing.revision) {
            std::cmp::Ordering::Greater => {
                heads.insert(profile_did, incoming);
            }
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                if incoming.profile_envelope_sha256 != existing.profile_envelope_sha256 {
                    anyhow::bail!(
                        "conflicting discovery profile revision for the same profile DID"
                    );
                }
                if incoming.observed_at >= existing.observed_at {
                    heads.insert(profile_did, incoming);
                }
            }
        },
    }
    Ok(())
}

fn prune_observed_profile_heads(heads: &mut BTreeMap<String, ObservedProfileHead>) {
    while heads.len() > MAX_DISCOVERY_QUERY_RESULTS {
        let Some(evict_profile_did) = heads
            .iter()
            .min_by(|left, right| {
                left.1
                    .observed_at
                    .cmp(&right.1.observed_at)
                    .then(left.0.cmp(right.0))
            })
            .map(|(profile_did, _)| profile_did.clone())
        else {
            break;
        };
        heads.remove(&evict_profile_did);
    }
}

fn client_state_mut<'a>(
    states: &'a mut BTreeMap<String, DiscoveryClientState>,
    profile_did: &str,
) -> anyhow::Result<&'a mut DiscoveryClientState> {
    if !states.contains_key(profile_did) && states.len() >= MAX_DISCOVERY_CLIENT_STATES {
        if let Some(disposable_profile_did) = states.iter().find_map(|(candidate, state)| {
            (state.current_advertisement.is_none()
                && state.visible_advertisements.is_empty()
                && state.observed_profile_heads.is_empty()
                && state.remote_visibility_may_remain_until.is_none()
                && state.transport_available)
                .then(|| candidate.clone())
        }) {
            states.remove(&disposable_profile_did);
        } else {
            anyhow::bail!("discovery client state capacity is full");
        }
    }
    Ok(states.entry(profile_did.to_string()).or_default())
}

fn apply_decoded_advertisements(
    state: &mut DiscoveryClientState,
    advertisements: Vec<CachedAdvertisement>,
    transport_available: bool,
) -> anyhow::Result<()> {
    let mut candidate_heads = state.observed_profile_heads.clone();
    let mut candidate_visible = BTreeMap::new();
    for advertisement in advertisements {
        if !advertisement_allowed_by_profile_heads(&candidate_heads, &advertisement)? {
            continue;
        }
        remember_profile_head(&mut candidate_heads, &advertisement)?;
        candidate_visible.insert(
            advertisement.verified.profile_did().to_string(),
            advertisement,
        );
    }
    prune_observed_profile_heads(&mut candidate_heads);
    state.observed_profile_heads = candidate_heads;
    state.visible_advertisements = candidate_visible;
    state.transport_available = transport_available;
    Ok(())
}

fn advertisement_hash_from_contact_request(envelope_bytes: &[u8]) -> anyhow::Result<String> {
    let envelope: SignedCollaborationMessage =
        serde_json::from_slice(envelope_bytes).context("invalid contact request envelope")?;
    let payload: CollaborationContactRequestPayload =
        serde_json::from_value(envelope.payload.payload)
            .context("invalid contact request payload")?;
    Ok(payload.advertisement_envelope_sha256)
}

fn advertisement_hash_from_discovery_withdrawal(envelope_bytes: &[u8]) -> anyhow::Result<String> {
    let envelope: SignedCollaborationMessage =
        serde_json::from_slice(envelope_bytes).context("invalid discovery withdrawal envelope")?;
    let payload: CollaborationDiscoveryWithdrawalPayload =
        serde_json::from_value(envelope.payload.payload)
            .context("invalid discovery withdrawal payload")?;
    Ok(payload.advertisement_envelope_sha256)
}

fn sender_profile_did_from_message(envelope_bytes: &[u8]) -> anyhow::Result<String> {
    let envelope: SignedCollaborationMessage =
        serde_json::from_slice(envelope_bytes).context("invalid collaboration message envelope")?;
    Ok(envelope.payload.sender_profile_did)
}

fn encode_bytes(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn canonical_payload_value<T: Serialize>(payload: &T) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::from_slice(&serde_json::to_vec(payload)?)?)
}

fn decode_bytes(value: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .with_context(|| format!("invalid {label} base64"))?;
    if encode_bytes(&bytes) != value {
        anyhow::bail!("{label} base64 is not canonical");
    }
    Ok(bytes)
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn due_sync_contexts(
    contexts: &BTreeMap<DiscoverySyncContextKey, DiscoverySyncContext>,
    now: u64,
) -> Vec<(DiscoverySyncContextKey, DiscoverySyncContext)> {
    select_due_sync_keys(
        contexts.iter().map(|(key, context)| {
            (
                key.clone(),
                context
                    .discovery_next_wake_at
                    .min(context.direct_next_wake_at)
                    .min(context.profile_next_wake_at),
            )
        }),
        now,
    )
    .into_iter()
    .filter_map(|key| contexts.get(&key).cloned().map(|context| (key, context)))
    .collect()
}

fn update_sync_schedule(
    next_wake_at: &mut u64,
    failures: &mut u8,
    success: bool,
    success_cadence: u64,
    now: u64,
) {
    if success {
        *failures = 0;
        *next_wake_at = now.saturating_add(success_cadence);
        return;
    }
    *failures = failures.saturating_add(1).min(6);
    let exponent = u32::from(failures.saturating_sub(1));
    let delay = DISCOVERY_SYNC_BASE_BACKOFF_SECS
        .saturating_mul(1_u64 << exponent)
        .min(DISCOVERY_SYNC_MAX_BACKOFF_SECS);
    *next_wake_at = now.saturating_add(delay);
}

fn select_due_sync_keys<K>(entries: impl Iterator<Item = (K, u64)>, now: u64) -> Vec<K>
where
    K: Ord,
{
    entries
        .filter(|(_, next_wake_at)| *next_wake_at <= now)
        .min_by(|(left_key, left_wake_at), (right_key, right_wake_at)| {
            left_wake_at
                .cmp(right_wake_at)
                .then_with(|| left_key.cmp(right_key))
        })
        .map(|(key, _)| key)
        .into_iter()
        .take(MAX_DISCOVERY_SYNC_WORK_PER_WAKE)
        .collect()
}

fn require_discovery_provider_success(
    response: serde_json::Value,
    operation: &str,
) -> anyhow::Result<()> {
    match serde_json::from_value(response) {
        Ok(DiscoveryProviderStatusResponse { status, .. }) if status == "ok" => Ok(()),
        _ => anyhow::bail!("{operation} failed"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use elastos_common::collaboration_protocol::collaboration_message_envelope_sha256;
    use elastos_runtime::signature::generate_keypair;
    use sha2::Digest;
    use tokio::time::sleep;

    use crate::carrier::{start_carrier_node_with_registry, CarrierGossipProvider};
    use crate::collaboration_contact_store::CollaborationContactStoreSnapshot;
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationBootstrapPeer,
        CollaborationNetworkProfile, CollaborationNetworkProfileMode,
        SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_profile_authority::{
        signed_profile_document_for_test, VerifiedCollaborationProfileDocument,
    };

    const NETWORK: &str = "elastos.community.test";
    const TEST_MAX_REQUESTS_PER_SENDER: usize = 14;
    const TEST_MAX_DECISIONS_PER_SENDER: usize = 16;

    struct ControllableRelayProvider {
        inner: CollaborationDiscoveryRelayProvider,
        reject_request_submissions: AtomicBool,
        reject_decision_submissions: AtomicBool,
        reject_withdrawals: AtomicBool,
        reject_mailboxes: AtomicBool,
        withdrawal_response: Mutex<Option<serde_json::Value>>,
        request_submission_response: Mutex<Option<serde_json::Value>>,
        submission_log: Mutex<Vec<LoggedRelayOperation>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum LoggedRelayOperation {
        Request(String),
        Decision(String),
    }

    impl ControllableRelayProvider {
        fn new(profile: VerifiedCollaborationNetworkProfile) -> Self {
            Self {
                inner: CollaborationDiscoveryRelayProvider::new(profile),
                reject_request_submissions: AtomicBool::new(false),
                reject_decision_submissions: AtomicBool::new(false),
                reject_withdrawals: AtomicBool::new(false),
                reject_mailboxes: AtomicBool::new(false),
                withdrawal_response: Mutex::new(None),
                request_submission_response: Mutex::new(None),
                submission_log: Mutex::new(Vec::new()),
            }
        }

        fn reject_requests(&self) {
            self.reject_request_submissions
                .store(true, Ordering::SeqCst);
        }

        fn allow_requests(&self) {
            self.reject_request_submissions
                .store(false, Ordering::SeqCst);
        }

        fn reject_decisions(&self) {
            self.reject_decision_submissions
                .store(true, Ordering::SeqCst);
        }

        fn allow_decisions(&self) {
            self.reject_decision_submissions
                .store(false, Ordering::SeqCst);
        }

        fn reject_withdrawals(&self) {
            self.reject_withdrawals.store(true, Ordering::SeqCst);
        }

        fn allow_withdrawals(&self) {
            self.reject_withdrawals.store(false, Ordering::SeqCst);
        }

        fn set_withdrawal_response(&self, response: serde_json::Value) {
            *self.withdrawal_response.lock().unwrap() = Some(response);
        }

        fn set_request_submission_response(&self, response: serde_json::Value) {
            *self.request_submission_response.lock().unwrap() = Some(response);
        }

        fn clear_request_submission_response(&self) {
            *self.request_submission_response.lock().unwrap() = None;
        }

        fn reject_mailboxes(&self) {
            self.reject_mailboxes.store(true, Ordering::SeqCst);
        }

        fn submission_log(&self) -> Vec<LoggedRelayOperation> {
            self.submission_log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Provider for ControllableRelayProvider {
        async fn handle(
            &self,
            request: elastos_runtime::provider::ResourceRequest,
        ) -> Result<elastos_runtime::provider::ResourceResponse, ProviderError> {
            self.inner.handle(request).await
        }

        fn schemes(&self) -> Vec<&'static str> {
            self.inner.schemes()
        }

        fn name(&self) -> &'static str {
            self.inner.name()
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            let op = request.get("op").and_then(|value| value.as_str());
            match op {
                Some("send_contact_request")
                    if self.reject_request_submissions.load(Ordering::SeqCst) =>
                {
                    return Err(ProviderError::Provider(
                        "discovery request submission is temporarily unavailable".to_string(),
                    ));
                }
                Some("submit_contact_decision_receipt")
                    if self.reject_decision_submissions.load(Ordering::SeqCst) =>
                {
                    return Err(ProviderError::Provider(
                        "discovery decision submission is temporarily unavailable".to_string(),
                    ));
                }
                Some("withdraw") if self.reject_withdrawals.load(Ordering::SeqCst) => {
                    return Err(ProviderError::Provider(
                        "discovery withdrawal is temporarily unavailable".to_string(),
                    ));
                }
                Some("withdraw") => {
                    if let Some(response) = self.withdrawal_response.lock().unwrap().clone() {
                        return Ok(response);
                    }
                }
                Some("send_contact_request") => {
                    if let Some(response) = self.request_submission_response.lock().unwrap().clone()
                    {
                        return Ok(response);
                    }
                }
                Some("poll_requests" | "poll_decisions")
                    if self.reject_mailboxes.load(Ordering::SeqCst) =>
                {
                    return Err(ProviderError::Provider(
                        "discovery mailbox polling is temporarily unavailable".to_string(),
                    ));
                }
                _ => {}
            }
            let response = self.inner.send_raw(request).await?;
            match op {
                Some("send_contact_request") => {
                    let envelope = request
                        .get("request")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| {
                            ProviderError::Provider(
                                "missing request bytes in contact request submission".to_string(),
                            )
                        })?;
                    let bytes = decode_bytes(envelope, "logged contact request")
                        .map_err(|err| ProviderError::Provider(err.to_string()))?;
                    let request_hash = collaboration_message_envelope_sha256(&bytes);
                    self.submission_log
                        .lock()
                        .unwrap()
                        .push(LoggedRelayOperation::Request(request_hash));
                }
                Some("submit_contact_decision_receipt") => {
                    let envelope = request
                        .get("receipt")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| {
                            ProviderError::Provider(
                                "missing receipt bytes in contact decision submission".to_string(),
                            )
                        })?;
                    let bytes = decode_bytes(envelope, "logged contact decision receipt")
                        .map_err(|err| ProviderError::Provider(err.to_string()))?;
                    let receipt: SignedCollaborationContactDecisionReceipt =
                        serde_json::from_slice(&bytes).map_err(|err| {
                            ProviderError::Provider(format!(
                                "invalid contact decision receipt envelope: {err}"
                            ))
                        })?;
                    self.submission_log
                        .lock()
                        .unwrap()
                        .push(LoggedRelayOperation::Decision(
                            receipt.payload.request_envelope_sha256,
                        ));
                }
                _ => {}
            }
            Ok(response)
        }
    }

    pub(crate) fn signed_profile(
        network_id: &str,
        trusted_signing_key: &SigningKey,
        bootstrap_peers: Vec<CollaborationBootstrapPeer>,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = encode_did_key(&trusted_signing_key.verifying_key());
        let profile = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers,
            default_conversation: None,
        };
        let payload = canonical_collaboration_network_profile_payload_bytes(&profile).unwrap();
        let (signature, signer_did) = domain_separated_sign(
            trusted_signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload,
        );
        match validate_collaboration_network_profile(
            Some(
                &serde_json::to_vec(
                    &serde_json::to_value(SignedCollaborationNetworkProfile {
                        payload: profile,
                        signature,
                        signer_did,
                    })
                    .unwrap(),
                )
                .unwrap(),
            ),
            network_id,
            &[encode_did_key(&trusted_signing_key.verifying_key())],
            None,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    async fn ticket_from_peer_provider(registry: &ProviderRegistry) -> CollaborationBootstrapPeer {
        let response = registry
            .send_raw("peer", &serde_json::json!({"op":"get_ticket"}))
            .await
            .unwrap();
        CollaborationBootstrapPeer {
            node_id: response["data"]["node_id"].as_str().unwrap().to_string(),
            connect_ticket: response["data"]["ticket"].as_str().unwrap().to_string(),
        }
    }

    pub(crate) async fn discovery_service(
        root: &Path,
        trusted_signing_key: &SigningKey,
        device_signing_key: &SigningKey,
        bootstrap_peers: Vec<CollaborationBootstrapPeer>,
    ) -> (
        Arc<ProviderRegistry>,
        CollaborationDiscoveryService,
        Arc<CollaborationContactStore>,
        crate::carrier::CarrierNode,
    ) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let did = encode_did_key(&device_signing_key.verifying_key());
        let principal_id = format!("person:local:{}", &did[8..16]);
        let protection = crate::auth::store_test_principal_root_protection(root, &principal_id);
        let local_profile =
            verified_discovery_profile(device_signing_key, "Bootstrap", Some("bootstrap"));
        discovery_service_with_identity(
            root,
            trusted_signing_key,
            device_signing_key,
            bootstrap_peers,
            &principal_id,
            &protection.localhost_root,
            local_profile,
        )
        .await
    }

    /// The same live runtime as [`discovery_service`], scoped to an identity
    /// the caller prepared — used by fixtures whose Profile is the durable
    /// on-disk authority rather than an ad-hoc signed document.
    pub(crate) async fn discovery_service_with_identity(
        root: &Path,
        trusted_signing_key: &SigningKey,
        device_signing_key: &SigningKey,
        bootstrap_peers: Vec<CollaborationBootstrapPeer>,
        principal_id: &str,
        localhost_root: &str,
        local_profile: VerifiedCollaborationProfileDocument,
    ) -> (
        Arc<ProviderRegistry>,
        CollaborationDiscoveryService,
        Arc<CollaborationContactStore>,
        crate::carrier::CarrierNode,
    ) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let profile = signed_profile(NETWORK, trusted_signing_key, bootstrap_peers);
        let registry = Arc::new(ProviderRegistry::new());
        let did = encode_did_key(&device_signing_key.verifying_key());
        let node = start_carrier_node_with_registry(
            device_signing_key,
            &did,
            root.to_path_buf(),
            Some(Arc::downgrade(&registry)),
        )
        .await
        .unwrap();
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    node.endpoint.clone(),
                ),
            ))
            .await;
        registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let store = Arc::new(
            CollaborationContactStore::new(
                root,
                principal_id,
                localhost_root,
                profile.clone(),
                &local_profile,
                &did,
            )
            .unwrap(),
        );
        let service = CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&device_signing_key.to_bytes()),
            profile,
            registry.clone(),
        )
        .await
        .unwrap();
        (registry, service, store, node)
    }

    pub(crate) struct DirectPeerPair {
        pub(crate) key_a: SigningKey,
        pub(crate) key_b: SigningKey,
        pub(crate) registry_a: Arc<ProviderRegistry>,
        pub(crate) registry_b: Arc<ProviderRegistry>,
        pub(crate) service_a: CollaborationDiscoveryService,
        pub(crate) service_b: CollaborationDiscoveryService,
        pub(crate) store_a: Arc<CollaborationContactStore>,
        pub(crate) store_b: Arc<CollaborationContactStore>,
        pub(crate) profile_a: VerifiedCollaborationProfileDocument,
        pub(crate) profile_b: VerifiedCollaborationProfileDocument,
        pub(crate) conversation_id: String,
        pub(crate) _node_a: crate::carrier::CarrierNode,
        pub(crate) _node_b: crate::carrier::CarrierNode,
    }

    /// A real passkey principal on disk for a fixture's principal id, so a
    /// Runtime-owned context can prove its authority the way production
    /// does — and so revoking that passkey can be seen to stop delivery.
    pub(crate) fn fixture_passkey_proof_binding(
        root: &Path,
        principal_id: &str,
        label: &str,
    ) -> String {
        let now = crate::auth::now_ts();
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: format!("{label}-credential"),
                public_key: format!("{label}-public-key"),
                sign_count: 1,
                user_verified: true,
                origin: "https://elastos.elacitylabs.com".to_string(),
                rp_id: "elastos.elacitylabs.com".to_string(),
                created_at: now,
                last_used_at: now,
                revoked_at: None,
            },
        );
        crate::auth::upsert_principal_for_binding_as_role_named(
            root,
            binding,
            principal_id.to_string(),
            crate::auth::RuntimePrincipalRole::Admin,
            Some(label),
            now,
        )
        .unwrap()
        .proof_binding_id
    }

    pub(crate) async fn direct_peer_pair(root: &Path) -> DirectPeerPair {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let key_a = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let key_b = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let (registry_a, service_a, store_a, node_a) =
            discovery_service(&root.join("a"), &trusted, &key_a, vec![]).await;
        let (registry_b, service_b, store_b, node_b) =
            discovery_service(&root.join("b"), &trusted, &key_b, vec![]).await;
        let profile_a = service_profile(&service_a, "Alice", Some("alice"));
        let profile_b = service_profile(&service_b, "Bob", Some("bob"));
        let now = current_timestamp();
        let advertisement = service_b
            .authority
            .prepare_advertisement(&profile_b, now)
            .unwrap();
        store_b
            .store_local_advertisement(&advertisement, now)
            .unwrap();
        let verified_advertisement = verify_collaboration_discovery_advertisement(
            &advertisement,
            &service_b.authority.profile,
            now,
        )
        .unwrap();
        let request = service_a
            .authority
            .prepare_contact_request(&verified_advertisement, &profile_a, now)
            .unwrap();
        store_a
            .record_outgoing_contact_request(&request, &advertisement, now)
            .unwrap();
        store_b
            .record_incoming_contact_request(&request, now)
            .unwrap();
        let verified_request = verify_collaboration_contact_request(
            &request,
            &service_a.authority.profile,
            &verified_advertisement,
            now,
        )
        .unwrap();
        let decision = service_b
            .authority
            .prepare_contact_decision_receipt(
                &verified_request,
                &profile_b,
                CollaborationContactDecision::Accepted,
                now,
            )
            .unwrap();
        store_b
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        store_a
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        let contacts_a = store_a.snapshot().unwrap();
        let contacts_b = store_b.snapshot().unwrap();
        assert_eq!(contacts_a.contacts().len(), 1);
        assert_eq!(contacts_b.contacts().len(), 1);
        let conversation_id = contacts_a.contacts()[0].conversation_id().to_string();
        assert_eq!(conversation_id, contacts_b.contacts()[0].conversation_id());
        service_a
            .direct_messages
            .register_verified_context_for_test(store_a.clone(), profile_a.clone())
            .unwrap();
        service_b
            .direct_messages
            .register_verified_context_for_test(store_b.clone(), profile_b.clone())
            .unwrap();
        DirectPeerPair {
            key_a,
            key_b,
            registry_a,
            registry_b,
            service_a,
            service_b,
            store_a,
            store_b,
            profile_a,
            profile_b,
            conversation_id,
            _node_a: node_a,
            _node_b: node_b,
        }
    }

    /// A runtime identity whose Profile is the durable on-disk authority: a
    /// real device key, a protected principal root, a passkey proof binding,
    /// and a signed Profile bundle written through `update_profile_authority`.
    pub(crate) struct DurableProfileIdentity {
        pub(crate) device_key: SigningKey,
        pub(crate) principal_id: String,
        pub(crate) localhost_root: String,
        pub(crate) proof_binding_id: String,
        pub(crate) profile: VerifiedCollaborationProfileDocument,
    }

    pub(crate) struct DurableHomeSessionGrant {
        pub(crate) session_id: String,
        pub(crate) grant_id: String,
    }

    pub(crate) fn durable_profile_identity(
        root: &Path,
        display_name: &str,
        handle: Option<&str>,
        now: u64,
    ) -> DurableProfileIdentity {
        std::fs::create_dir_all(root).unwrap();
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (device_key, _) = elastos_identity::load_or_create_did(root).unwrap();
        let did = encode_did_key(&device_key.verifying_key());
        let principal_id = format!("person:local:{}", &did[8..16]);
        let protection = crate::auth::store_test_principal_root_protection(root, &principal_id);
        let binding = elastos_runtime::auth::ProofBinding::passkey_webauthn(
            elastos_runtime::auth::PasskeyWebAuthnBinding {
                credential_id: format!("credential:{}", &did[8..16]),
                public_key: "durable-profile-test-public-key".to_string(),
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
            root,
            binding,
            principal_id.clone(),
            crate::auth::RuntimePrincipalRole::Admin,
            Some(display_name),
            now,
        )
        .unwrap()
        .proof_binding_id;
        let profile = crate::collaboration_profile_authority::update_profile_authority(
            root,
            &principal_id,
            &protection.localhost_root,
            &proof_binding_id,
            display_name,
            handle,
            now,
        )
        .unwrap();
        DurableProfileIdentity {
            device_key,
            principal_id,
            localhost_root: protection.localhost_root,
            proof_binding_id,
            profile,
        }
    }

    pub(crate) fn store_home_session_grant_for_test(
        data_dir: &Path,
        identity: &DurableProfileIdentity,
        label: &str,
        now: u64,
    ) -> DurableHomeSessionGrant {
        let grant = elastos_runtime::auth::AuthSessionGrantV1 {
            schema: elastos_runtime::auth::AuthSessionGrantV1::SCHEMA.to_string(),
            grant_id: format!("grant:{label}:{now}"),
            session_id: format!("auth:{label}:{now}"),
            principal_id: identity.principal_id.clone(),
            proof_binding_id: identity.proof_binding_id.clone(),
            issued_at: now,
            expires_at: now + 12 * 60 * 60,
            apps: vec!["home".to_string()],
        };
        crate::auth::store_session_grant(data_dir, grant.clone()).unwrap();
        DurableHomeSessionGrant {
            session_id: grant.session_id,
            grant_id: grant.grant_id,
        }
    }

    pub(crate) struct DurableProfilePeerPair {
        pub(crate) trusted: SigningKey,
        pub(crate) identity_a: DurableProfileIdentity,
        pub(crate) identity_b: DurableProfileIdentity,
        pub(crate) service_a: CollaborationDiscoveryService,
        pub(crate) service_b: CollaborationDiscoveryService,
        pub(crate) store_a: Arc<CollaborationContactStore>,
        pub(crate) store_b: Arc<CollaborationContactStore>,
        pub(crate) conversation_id: String,
        pub(crate) _node_a: crate::carrier::CarrierNode,
        pub(crate) _node_b: crate::carrier::CarrierNode,
    }

    /// Two live runtimes with an accepted pair whose Profiles are the durable
    /// on-disk authorities — the state a Profile update announcement reads.
    pub(crate) async fn durable_profile_peer_pair(root: &Path) -> DurableProfilePeerPair {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let now = current_timestamp();
        let identity_a = durable_profile_identity(&root.join("a"), "Alice", Some("alice"), now);
        let identity_b = durable_profile_identity(&root.join("b"), "Bob", Some("bob"), now);
        let (_registry_a, service_a, store_a, node_a) = discovery_service_with_identity(
            &root.join("a"),
            &trusted,
            &identity_a.device_key,
            vec![],
            &identity_a.principal_id,
            &identity_a.localhost_root,
            identity_a.profile.clone(),
        )
        .await;
        let (_registry_b, service_b, store_b, node_b) = discovery_service_with_identity(
            &root.join("b"),
            &trusted,
            &identity_b.device_key,
            vec![],
            &identity_b.principal_id,
            &identity_b.localhost_root,
            identity_b.profile.clone(),
        )
        .await;
        let advertisement = service_b
            .authority
            .prepare_advertisement(&identity_b.profile, now)
            .unwrap();
        store_b
            .store_local_advertisement(&advertisement, now)
            .unwrap();
        let verified_advertisement = verify_collaboration_discovery_advertisement(
            &advertisement,
            &service_b.authority.profile,
            now,
        )
        .unwrap();
        let request = service_a
            .authority
            .prepare_contact_request(&verified_advertisement, &identity_a.profile, now)
            .unwrap();
        store_a
            .record_outgoing_contact_request(&request, &advertisement, now)
            .unwrap();
        store_b
            .record_incoming_contact_request(&request, now)
            .unwrap();
        let verified_request = verify_collaboration_contact_request(
            &request,
            &service_a.authority.profile,
            &verified_advertisement,
            now,
        )
        .unwrap();
        let decision = service_b
            .authority
            .prepare_contact_decision_receipt(
                &verified_request,
                &identity_b.profile,
                CollaborationContactDecision::Accepted,
                now,
            )
            .unwrap();
        store_b
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        store_a
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        let contacts_a = store_a.snapshot().unwrap();
        let contacts_b = store_b.snapshot().unwrap();
        assert_eq!(contacts_a.contacts().len(), 1);
        assert_eq!(contacts_b.contacts().len(), 1);
        let conversation_id = contacts_a.contacts()[0].conversation_id().to_string();
        assert_eq!(conversation_id, contacts_b.contacts()[0].conversation_id());
        DurableProfilePeerPair {
            trusted,
            identity_a,
            identity_b,
            service_a,
            service_b,
            store_a,
            store_b,
            conversation_id,
            _node_a: node_a,
            _node_b: node_b,
        }
    }

    pub(crate) fn add_offline_accepted_contact(pair: &DirectPeerPair) -> (String, String) {
        let key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&key.to_bytes()),
            pair.service_a.authority.profile.clone(),
        );
        let profile = verified_discovery_profile(&key, "Offline", Some("offline"));
        let now = current_timestamp();
        let advertisement = authority.prepare_advertisement(&profile, now).unwrap();
        let verified_advertisement = verify_collaboration_discovery_advertisement(
            &advertisement,
            &pair.service_a.authority.profile,
            now,
        )
        .unwrap();
        let request = pair
            .service_a
            .authority
            .prepare_contact_request(&verified_advertisement, &pair.profile_a, now)
            .unwrap();
        pair.store_a
            .record_outgoing_contact_request(&request, &advertisement, now)
            .unwrap();
        let verified_request = verify_collaboration_contact_request(
            &request,
            &pair.service_a.authority.profile,
            &verified_advertisement,
            now,
        )
        .unwrap();
        let decision = authority
            .prepare_contact_decision_receipt(
                &verified_request,
                &profile,
                CollaborationContactDecision::Accepted,
                now,
            )
            .unwrap();
        pair.store_a
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        let conversation_id = pair
            .store_a
            .snapshot()
            .unwrap()
            .contacts()
            .iter()
            .find(|contact| contact.remote_profile_did() == profile.document().profile_did)
            .unwrap()
            .conversation_id()
            .to_string();
        (profile.document().profile_did.clone(), conversation_id)
    }

    pub(crate) struct DirectGatewayPeerFixture {
        pub(crate) service: CollaborationDiscoveryService,
        pub(crate) store: Arc<CollaborationContactStore>,
        pub(crate) remote_key: SigningKey,
        pub(crate) remote_registry: Arc<ProviderRegistry>,
        pub(crate) conversation_id: String,
        pub(crate) _local_node: crate::carrier::CarrierNode,
        pub(crate) _remote_node: crate::carrier::CarrierNode,
    }

    pub(crate) async fn direct_gateway_peer_fixture(
        data_root: &Path,
        principal_id: &str,
        localhost_root: &str,
        local_key: &SigningKey,
        local_profile: &VerifiedCollaborationProfileDocument,
    ) -> DirectGatewayPeerFixture {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let remote_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let remote_root = data_root.join("direct-api-remote");
        let (remote_registry, remote_service, remote_store, remote_node) =
            discovery_service(&remote_root, &trusted, &remote_key, vec![]).await;
        let remote_profile = service_profile(&remote_service, "Remote Person", Some("remote"));
        let network = remote_service.network_profile();
        let registry = Arc::new(ProviderRegistry::new());
        let local_did = encode_did_key(&local_key.verifying_key());
        let local_node = start_carrier_node_with_registry(
            local_key,
            &local_did,
            data_root.join("direct-api-carrier"),
            Some(Arc::downgrade(&registry)),
        )
        .await
        .unwrap();
        registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    local_node.endpoint.clone(),
                ),
            ))
            .await;
        registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(local_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let store = Arc::new(
            CollaborationContactStore::new(
                data_root,
                principal_id,
                localhost_root,
                network.clone(),
                local_profile,
                &local_did,
            )
            .unwrap(),
        );
        let service = CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&local_key.to_bytes()),
            network,
            registry,
        )
        .await
        .unwrap();
        let now = current_timestamp();
        let advertisement = remote_service
            .authority
            .prepare_advertisement(&remote_profile, now)
            .unwrap();
        remote_store
            .store_local_advertisement(&advertisement, now)
            .unwrap();
        let verified_advertisement = verify_collaboration_discovery_advertisement(
            &advertisement,
            &remote_service.authority.profile,
            now,
        )
        .unwrap();
        let request = service
            .authority
            .prepare_contact_request(&verified_advertisement, local_profile, now)
            .unwrap();
        store
            .record_outgoing_contact_request(&request, &advertisement, now)
            .unwrap();
        remote_store
            .record_incoming_contact_request(&request, now)
            .unwrap();
        let verified_request = verify_collaboration_contact_request(
            &request,
            &service.authority.profile,
            &verified_advertisement,
            now,
        )
        .unwrap();
        let decision = remote_service
            .authority
            .prepare_contact_decision_receipt(
                &verified_request,
                &remote_profile,
                CollaborationContactDecision::Accepted,
                now,
            )
            .unwrap();
        remote_store
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        store
            .record_contact_decision_receipt(&decision, now)
            .unwrap();
        let conversation_id = store.snapshot().unwrap().contacts()[0]
            .conversation_id()
            .to_string();
        service
            .direct_messages
            .register_verified_context_for_test(store.clone(), local_profile.clone())
            .unwrap();
        remote_service
            .direct_messages
            .register_verified_context_for_test(remote_store, remote_profile.clone())
            .unwrap();
        assert!(remote_registry.schemes().await.iter().any(|scheme| {
            scheme == crate::collaboration_direct_messages::DIRECT_MESSAGE_PROVIDER_SCHEME
        }));
        DirectGatewayPeerFixture {
            service,
            store,
            remote_key,
            remote_registry,
            conversation_id,
            _local_node: local_node,
            _remote_node: remote_node,
        }
    }

    fn discovery_profile_signing_key(device_signing_key: &SigningKey) -> SigningKey {
        let mut seed = device_signing_key.to_bytes();
        seed[0] ^= 0xA5;
        seed[31] ^= 0x5A;
        SigningKey::from_bytes(&seed)
    }

    pub(crate) fn verified_discovery_profile(
        device_signing_key: &SigningKey,
        display_name: &str,
        handle: Option<&str>,
    ) -> VerifiedCollaborationProfileDocument {
        verified_discovery_profile_with(
            &discovery_profile_signing_key(device_signing_key),
            device_signing_key,
            display_name,
            handle,
            1,
            None,
            1,
        )
    }

    /// The same person after a rename: same Profile DID and signing key,
    /// a later revision, a different name.
    pub(crate) fn renamed_discovery_profile(
        device_signing_key: &SigningKey,
        display_name: &str,
        handle: Option<&str>,
        previous: &VerifiedCollaborationProfileDocument,
    ) -> VerifiedCollaborationProfileDocument {
        verified_discovery_profile_with(
            &discovery_profile_signing_key(device_signing_key),
            device_signing_key,
            display_name,
            handle,
            previous.document().revision + 1,
            Some(profile_hash(previous).as_str()),
            1,
        )
    }

    fn verified_discovery_profile_with(
        profile_signing_key: &SigningKey,
        device_signing_key: &SigningKey,
        display_name: &str,
        handle: Option<&str>,
        revision: u64,
        previous_profile_sha256: Option<&str>,
        updated_at: u64,
    ) -> VerifiedCollaborationProfileDocument {
        signed_profile_document_for_test(
            profile_signing_key,
            display_name,
            handle,
            revision,
            previous_profile_sha256,
            updated_at,
            vec![encode_did_key(&device_signing_key.verifying_key())],
        )
        .unwrap()
    }

    fn service_profile(
        service: &CollaborationDiscoveryService,
        display_name: &str,
        handle: Option<&str>,
    ) -> VerifiedCollaborationProfileDocument {
        verified_discovery_profile(&service.authority.signing_key, display_name, handle)
    }

    fn profile_hash(profile: &VerifiedCollaborationProfileDocument) -> String {
        let bytes = serde_json::to_vec(profile.signed_envelope()).unwrap();
        format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)))
    }

    fn cached_advertisement(
        authority: &CollaborationDiscoveryAuthority,
        profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> CachedAdvertisement {
        let envelope_bytes = authority.prepare_advertisement(profile, now).unwrap();
        let verified =
            verify_collaboration_discovery_advertisement(&envelope_bytes, &authority.profile, now)
                .unwrap();
        CachedAdvertisement {
            envelope_bytes,
            verified,
        }
    }

    #[tokio::test]
    async fn read_only_status_matches_effective_stored_current_without_mutating_client_state() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let local_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let (_registry, service, store, _node) =
            discovery_service(temp.path(), &trusted, &local_key, Vec::new()).await;
        let local_profile = service_profile(&service, "Bootstrap", Some("bootstrap"));
        let now = current_timestamp();
        store.set_discovery_enabled(true, now).unwrap();
        let local_current = cached_advertisement(&service.authority, &local_profile, now);
        store
            .store_local_advertisement(&local_current.envelope_bytes, now)
            .unwrap();

        let remote_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let remote_profile = verified_discovery_profile(&remote_key, "Remote", Some("remote"));
        let remote_authority =
            CollaborationDiscoveryAuthority::new(remote_key, service.network_profile());
        let remote = cached_advertisement(&remote_authority, &remote_profile, now);
        {
            let mut states = service.state.lock().unwrap();
            let state =
                client_state_mut(&mut states, local_profile.document().profile_did.as_str())
                    .unwrap();
            state.current_advertisement = None;
            state
                .visible_advertisements
                .insert(remote_profile.document().profile_did.clone(), remote);
        }

        let before = service.client_state_snapshot_for_test();
        let status = service
            .read_only_status(store.as_ref(), &local_profile, now)
            .unwrap();
        assert_eq!(status.visible_people().len(), 1);
        assert_eq!(status.visible_people()[0].display_name(), "Remote");
        assert_eq!(
            status.expires_at(),
            Some(
                local_current
                    .verified
                    .message()
                    .envelope()
                    .payload
                    .expires_at
            )
        );
        assert_eq!(service.client_state_snapshot_for_test(), before);

        let expired = service
            .read_only_status(
                store.as_ref(),
                &local_profile,
                local_current
                    .verified
                    .message()
                    .envelope()
                    .payload
                    .expires_at,
            )
            .unwrap();
        assert!(expired.visible_people().is_empty());
        assert_eq!(expired.expires_at(), None);
        assert_eq!(service.client_state_snapshot_for_test(), before);
    }

    async fn wait_for_visible_peer(
        service: &CollaborationDiscoveryService,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        expected_peer: &str,
    ) -> CollaborationDiscoveryStatus {
        for _ in 0..30 {
            let status = service
                .refresh(store, profile, current_timestamp())
                .await
                .unwrap();
            if status
                .visible_people()
                .iter()
                .any(|peer| peer.display_name() == expected_peer)
            {
                return status;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for visible discovery peer");
    }

    async fn wait_for_transport_available(
        service: &CollaborationDiscoveryService,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
    ) -> CollaborationDiscoveryStatus {
        for _ in 0..30 {
            let status = service
                .refresh(store, profile, current_timestamp())
                .await
                .unwrap();
            if status.available() {
                return status;
            }
            sleep(Duration::from_millis(100)).await;
        }
        let cached = service
            .state
            .lock()
            .unwrap()
            .get(profile.document().profile_did.as_str())
            .and_then(|state| state.current_advertisement.clone())
            .expect("discovery transport wait requires a current advertisement");
        let error = service
            .invoke_bootstrap(
                "query",
                serde_json::to_value(DiscoveryProviderQueryRequest {
                    op: "query".to_string(),
                    advertisement: encode_bytes(&cached.envelope_bytes),
                })
                .unwrap(),
            )
            .await
            .err()
            .map(|err| format!("{err:#}"))
            .unwrap_or_else(|| {
                "transport remained unavailable without a bootstrap error".to_string()
            });
        panic!("timed out waiting for discovery transport availability: {error}");
    }

    async fn wait_for_incoming_request(
        service: &CollaborationDiscoveryService,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        expected_display_name: &str,
    ) -> Vec<PendingIncomingContactRequest> {
        for _ in 0..30 {
            let requests = service
                .refresh(store, profile, current_timestamp())
                .await
                .unwrap()
                .incoming_requests()
                .to_vec();
            if requests
                .iter()
                .any(|request| request.display_name() == expected_display_name)
            {
                return requests;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for incoming discovery request");
    }

    async fn wait_for_contact_count(
        service: &CollaborationDiscoveryService,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        expected_count: usize,
    ) -> CollaborationContactStoreSnapshot {
        for _ in 0..30 {
            let _ = service
                .refresh(store, profile, current_timestamp())
                .await
                .unwrap();
            let snapshot = store.snapshot().unwrap();
            if snapshot.contacts().len() == expected_count {
                return snapshot;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for accepted discovery contacts");
    }

    async fn wait_for_removed_contact_count(
        service: &CollaborationDiscoveryService,
        store: &CollaborationContactStore,
        profile: &VerifiedCollaborationProfileDocument,
        expected_count: usize,
    ) -> CollaborationContactStoreSnapshot {
        for _ in 0..30 {
            let _ = service
                .refresh(store, profile, current_timestamp())
                .await
                .unwrap();
            let snapshot = store.snapshot().unwrap();
            if snapshot.removed().len() == expected_count {
                return snapshot;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for removed discovery contacts");
    }

    #[tokio::test]
    async fn discovery_relay_advertise_query_and_contact_decision_flow() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider: Arc<dyn Provider> = Arc::new(CollaborationDiscoveryRelayProvider::new(
            seed_profile.clone(),
        ));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));
        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let _ =
            wait_for_transport_available(&remote_service, remote_store.as_ref(), &remote_profile)
                .await;
        let refreshed = local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        assert_eq!(refreshed.visible_people().len(), 0);
        let enabled = local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        assert!(enabled.enabled());
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        assert_eq!(refreshed.visible_people().len(), 1);
        assert_eq!(refreshed.visible_people()[0].display_name(), "Remote");
        assert_eq!(refreshed.incoming_requests().len(), 0);

        local_service
            .send_contact_request(
                local_store.as_ref(),
                refreshed.visible_people()[0].advertisement_id(),
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        assert_eq!(remote_requests.len(), 1);
        assert_eq!(remote_requests[0].display_name(), "Local");
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                remote_requests[0].request_hash(),
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts =
            wait_for_contact_count(&remote_service, remote_store.as_ref(), &remote_profile, 1)
                .await;
        assert_eq!(local_contacts.contacts().len(), 1);
        assert_eq!(remote_contacts.contacts().len(), 1);
        assert_eq!(
            local_contacts.contacts()[0].conversation_id(),
            remote_contacts.contacts()[0].conversation_id()
        );
        assert_eq!(local_contacts.contacts()[0].remote_display_name(), "Remote");
    }

    #[tokio::test]
    async fn bilateral_removal_can_readd_the_same_profile_contact_with_one_stable_conversation() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider: Arc<dyn Provider> = Arc::new(CollaborationDiscoveryRelayProvider::new(
            seed_profile.clone(),
        ));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let _ =
            wait_for_transport_available(&remote_service, remote_store.as_ref(), &remote_profile)
                .await;
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        let advertisement_id = refreshed.visible_people()[0].advertisement_id().to_string();
        local_service
            .send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                remote_requests[0].request_hash(),
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();

        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts =
            wait_for_contact_count(&remote_service, remote_store.as_ref(), &remote_profile, 1)
                .await;
        let conversation_id = local_contacts.contacts()[0].conversation_id().to_string();
        assert_eq!(
            conversation_id,
            remote_contacts.contacts()[0].conversation_id()
        );

        remote_service
            .remove_contact(
                remote_store.as_ref(),
                &remote_profile,
                local_profile.document().profile_did.as_str(),
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .remove_contact(
                local_store.as_ref(),
                &local_profile,
                remote_profile.document().profile_did.as_str(),
                current_timestamp(),
            )
            .await
            .unwrap();

        let local_removed =
            wait_for_removed_contact_count(&local_service, local_store.as_ref(), &local_profile, 1)
                .await;
        let remote_removed = wait_for_removed_contact_count(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            1,
        )
        .await;
        assert!(local_removed.contacts().is_empty());
        assert!(remote_removed.contacts().is_empty());
        assert_eq!(
            local_removed.removed()[0].conversation_id(),
            conversation_id
        );
        assert_eq!(
            remote_removed.removed()[0].conversation_id(),
            conversation_id
        );

        let refreshed = wait_for_visible_peer(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        let readd_advertisement_id = refreshed.visible_people()[0].advertisement_id().to_string();
        remote_service
            .send_contact_request(
                remote_store.as_ref(),
                &readd_advertisement_id,
                &remote_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        let local_requests = wait_for_incoming_request(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        local_service
            .submit_contact_decision(
                local_store.as_ref(),
                &local_profile,
                local_requests[0].request_hash(),
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();

        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts =
            wait_for_contact_count(&remote_service, remote_store.as_ref(), &remote_profile, 1)
                .await;
        assert!(local_contacts.removed().is_empty());
        assert!(remote_contacts.removed().is_empty());
        assert_eq!(
            local_contacts.contacts()[0].conversation_id(),
            conversation_id
        );
        assert_eq!(
            remote_contacts.contacts()[0].conversation_id(),
            conversation_id
        );
    }

    #[tokio::test]
    async fn unilateral_contact_removal_propagates_after_sync_wake() {
        let temp = tempfile::tempdir().unwrap();
        let pair = durable_profile_peer_pair(temp.path()).await;
        let root_a = temp.path().join("a");
        let root_b = temp.path().join("b");
        let now = crate::auth::now_ts();
        let grant_a =
            store_home_session_grant_for_test(&root_a, &pair.identity_a, "alice-home", now);
        let grant_b = store_home_session_grant_for_test(&root_b, &pair.identity_b, "bob-home", now);
        pair.service_a
            .register_sync_context(
                pair.store_a.clone(),
                pair.identity_a.profile.clone(),
                &grant_a.session_id,
                Some(&pair.identity_a.proof_binding_id),
                &grant_a.grant_id,
                now,
            )
            .unwrap();
        pair.service_b
            .register_sync_context(
                pair.store_b.clone(),
                pair.identity_b.profile.clone(),
                &grant_b.session_id,
                Some(&pair.identity_b.proof_binding_id),
                &grant_b.grant_id,
                now,
            )
            .unwrap();

        pair.service_b
            .remove_contact(
                pair.store_b.as_ref(),
                &pair.identity_b.profile,
                pair.identity_a.profile.document().profile_did.as_str(),
                now,
            )
            .await
            .unwrap();
        pair.service_b
            .wake_registered_sync(pair.store_b.as_ref(), &pair.identity_b.profile, now)
            .unwrap();

        for offset in 0..30 {
            pair.service_b
                .sync_registered_contexts_once(now + offset)
                .await;
            let local_snapshot = pair.store_a.snapshot().unwrap();
            if local_snapshot.removed().len() == 1 {
                assert!(local_snapshot.contacts().is_empty());
                assert_eq!(
                    local_snapshot.removed()[0].remote_profile_did(),
                    pair.identity_b.profile.document().profile_did
                );
                assert_eq!(
                    local_snapshot.removed()[0].conversation_id(),
                    pair.conversation_id
                );
                let remote_snapshot = pair.store_b.snapshot().unwrap();
                assert!(remote_snapshot.contacts().is_empty());
                assert_eq!(remote_snapshot.removed().len(), 1);
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for unilateral contact removal propagation");
    }

    #[tokio::test]
    async fn unilateral_contact_removal_still_propagates_after_profile_rename() {
        let temp = tempfile::tempdir().unwrap();
        let pair = durable_profile_peer_pair(temp.path()).await;
        let root_a = temp.path().join("a");
        let root_b = temp.path().join("b");
        let now = crate::auth::now_ts();
        let grant_a =
            store_home_session_grant_for_test(&root_a, &pair.identity_a, "alice-home", now);
        let grant_b = store_home_session_grant_for_test(&root_b, &pair.identity_b, "bob-home", now);
        pair.service_a
            .register_sync_context(
                pair.store_a.clone(),
                pair.identity_a.profile.clone(),
                &grant_a.session_id,
                Some(&pair.identity_a.proof_binding_id),
                &grant_a.grant_id,
                now,
            )
            .unwrap();
        pair.service_b
            .register_sync_context(
                pair.store_b.clone(),
                pair.identity_b.profile.clone(),
                &grant_b.session_id,
                Some(&pair.identity_b.proof_binding_id),
                &grant_b.grant_id,
                now,
            )
            .unwrap();

        let renamed_profile = crate::collaboration_profile_authority::update_profile_authority(
            &root_a,
            &pair.identity_a.principal_id,
            &pair.identity_a.localhost_root,
            &pair.identity_a.proof_binding_id,
            "Alice Renamed",
            Some("alice"),
            now + 1,
        )
        .unwrap();
        pair.service_a
            .register_sync_context(
                pair.store_a.clone(),
                renamed_profile.clone(),
                &grant_a.session_id,
                Some(&pair.identity_a.proof_binding_id),
                &grant_a.grant_id,
                now + 1,
            )
            .unwrap();

        for offset in 0..30 {
            pair.service_a
                .sync_registered_contexts_once(now + offset)
                .await;
            pair.service_b
                .sync_registered_contexts_once(now + offset)
                .await;
            let remote_snapshot = pair.store_b.snapshot().unwrap();
            let renamed = remote_snapshot
                .contacts()
                .iter()
                .any(|contact| contact.remote_display_name() == "Alice Renamed");
            if renamed {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        let remote_snapshot = pair.store_b.snapshot().unwrap();
        assert!(
            remote_snapshot
                .contacts()
                .iter()
                .any(|contact| contact.remote_display_name() == "Alice Renamed"),
            "timed out waiting for rename propagation before removal"
        );

        pair.service_b
            .remove_contact(
                pair.store_b.as_ref(),
                &pair.identity_b.profile,
                renamed_profile.document().profile_did.as_str(),
                now + 2,
            )
            .await
            .unwrap();
        pair.service_b
            .wake_registered_sync(pair.store_b.as_ref(), &pair.identity_b.profile, now + 2)
            .unwrap();

        for offset in 0..30 {
            pair.service_b
                .sync_registered_contexts_once(now + 2 + offset)
                .await;
            pair.service_a
                .sync_registered_contexts_once(now + 2 + offset)
                .await;
            let local_snapshot = pair.store_a.snapshot().unwrap();
            if local_snapshot.removed().len() == 1 {
                assert!(local_snapshot.contacts().is_empty());
                assert_eq!(
                    local_snapshot.removed()[0].remote_profile_did(),
                    pair.identity_b.profile.document().profile_did
                );
                assert_eq!(
                    local_snapshot.removed()[0].conversation_id(),
                    pair.conversation_id
                );
                let remote_snapshot = pair.store_b.snapshot().unwrap();
                assert!(remote_snapshot.contacts().is_empty());
                assert_eq!(remote_snapshot.removed().len(), 1);
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out waiting for unilateral contact removal after rename propagation");
    }

    #[tokio::test]
    async fn contact_decision_verifies_against_request_bound_advertisement_after_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile_v1 = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile_v1,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(
                remote_store.as_ref(),
                &remote_profile_v1,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        let advertisement_id = refreshed.visible_people()[0].advertisement_id().to_string();

        local_service
            .send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile_v1,
            "Local",
        )
        .await;
        let request_hash = remote_requests[0].request_hash().to_string();

        let remote_profile_v2 = verified_discovery_profile_with(
            &discovery_profile_signing_key(&remote_service.authority.signing_key),
            &remote_service.authority.signing_key,
            "Remote Renamed",
            Some("remote-renamed"),
            2,
            Some(&profile_hash(&remote_profile_v1)),
            remote_profile_v1.document().updated_at + 1,
        );
        let replacement_now = current_timestamp().saturating_add(1);
        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile_v2,
                true,
                replacement_now,
            )
            .await
            .unwrap();
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile_v2,
                &request_hash,
                CollaborationContactDecision::Accepted,
                replacement_now,
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile_v2, replacement_now)
            .await
            .unwrap();

        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts = wait_for_contact_count(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile_v2,
            1,
        )
        .await;
        assert_eq!(local_contacts.contacts().len(), 1);
        assert_eq!(remote_contacts.contacts().len(), 1);
    }

    #[tokio::test]
    async fn contact_decision_verifies_against_request_bound_advertisement_after_withdrawal() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        let advertisement_id = refreshed.visible_people()[0].advertisement_id().to_string();
        let remote_profile_did = remote_profile.document().profile_did.clone();
        let advertisement_hash = advertisement_id.clone();

        local_service
            .send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        let request_hash = remote_requests[0].request_hash().to_string();

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                false,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Declined,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();

        for _ in 0..30 {
            let _ = local_service
                .refresh(local_store.as_ref(), &local_profile, current_timestamp())
                .await
                .unwrap();
            if local_store
                .stored_outgoing_contact_request(
                    &advertisement_hash,
                    &remote_profile_did,
                    current_timestamp(),
                )
                .unwrap()
                .is_none()
            {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        assert!(local_store.snapshot().unwrap().contacts().is_empty());
        assert!(remote_store.snapshot().unwrap().contacts().is_empty());
        assert!(local_store
            .stored_outgoing_contact_request(
                &advertisement_hash,
                &remote_profile_did,
                current_timestamp(),
            )
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn contact_decision_reuses_stored_receipt_after_outage_then_retry() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_provider.reject_decisions();
        seed_registry.register(seed_provider.clone()).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let _ =
            wait_for_transport_available(&remote_service, remote_store.as_ref(), &remote_profile)
                .await;
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        local_service
            .send_contact_request(
                local_store.as_ref(),
                refreshed.visible_people()[0].advertisement_id(),
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();

        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        let request_hash = remote_requests[0].request_hash().to_string();
        let (first, second) = tokio::join!(
            remote_service.submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp()
            ),
            remote_service.submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp()
            )
        );
        first.unwrap();
        second.unwrap();

        let stored_receipt = remote_store
            .stored_contact_decision_receipt(&request_hash)
            .unwrap()
            .expect("receipt must remain stored for retry");
        assert!(remote_store.pending_incoming_requests().unwrap().is_empty());
        assert!(local_store.snapshot().unwrap().contacts().is_empty());

        seed_provider.allow_decisions();
        let (first, second) = tokio::join!(
            remote_service.submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp()
            ),
            remote_service.submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp()
            )
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            remote_store
                .stored_contact_decision_receipt(&request_hash)
                .unwrap()
                .unwrap(),
            stored_receipt
        );
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts =
            wait_for_contact_count(&remote_service, remote_store.as_ref(), &remote_profile, 1)
                .await;
        assert_eq!(local_contacts.contacts().len(), 1);
        assert_eq!(remote_contacts.contacts().len(), 1);
        assert_eq!(
            local_contacts.contacts()[0].conversation_id(),
            remote_contacts.contacts()[0].conversation_id()
        );

        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        assert_eq!(local_store.snapshot().unwrap().contacts().len(), 1);
        assert_eq!(remote_store.snapshot().unwrap().contacts().len(), 1);
    }

    #[tokio::test]
    async fn submit_contact_decision_rejects_different_terminal_decision_for_same_request() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_provider.reject_decisions();
        seed_registry.register(seed_provider.clone()).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let _ =
            wait_for_transport_available(&remote_service, remote_store.as_ref(), &remote_profile)
                .await;
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        local_service
            .send_contact_request(
                local_store.as_ref(),
                refreshed.visible_people()[0].advertisement_id(),
                &local_profile,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();

        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        let request_hash = remote_requests[0].request_hash().to_string();
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        let stored_receipt = remote_store
            .stored_contact_decision_receipt(&request_hash)
            .unwrap()
            .expect("receipt must remain stored after the first failure");

        let second_error = remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                &request_hash,
                CollaborationContactDecision::Declined,
                current_timestamp(),
            )
            .await
            .unwrap_err();
        assert!(second_error
            .to_string()
            .contains("different terminal decision"));
        assert_eq!(
            remote_store
                .stored_contact_decision_receipt(&request_hash)
                .unwrap()
                .unwrap(),
            stored_receipt
        );
        assert_eq!(
            seed_provider.submission_log(),
            vec![LoggedRelayOperation::Request(request_hash)]
        );
    }

    #[tokio::test]
    async fn contact_request_reuses_stored_request_after_outage_then_retry() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_provider.reject_requests();
        seed_registry.register(seed_provider.clone()).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let _ =
            wait_for_transport_available(&remote_service, remote_store.as_ref(), &remote_profile)
                .await;
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let refreshed = wait_for_visible_peer(
            &local_service,
            local_store.as_ref(),
            &local_profile,
            "Remote",
        )
        .await;
        let advertisement_id = refreshed.visible_people()[0].advertisement_id().to_string();
        let remote_profile_did = remote_profile.document().profile_did.clone();
        let advertisement_hash = local_service
            .state
            .lock()
            .unwrap()
            .get(local_profile.document().profile_did.as_str())
            .unwrap()
            .visible_advertisements
            .values()
            .find(|cached| cached.verified.message().envelope_sha256() == advertisement_id)
            .unwrap()
            .verified
            .message()
            .envelope_sha256()
            .to_string();

        let (first, second) = tokio::join!(
            local_service.send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp()
            ),
            local_service.send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp()
            )
        );
        first.unwrap();
        second.unwrap();
        let stored_request = local_store
            .stored_outgoing_contact_request(
                &advertisement_hash,
                &remote_profile_did,
                current_timestamp(),
            )
            .unwrap()
            .expect("request must remain stored for retry");
        assert!(local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .is_err());
        assert!(remote_store.pending_incoming_requests().unwrap().is_empty());

        seed_provider.allow_requests();
        seed_provider
            .set_request_submission_response(serde_json::json!({"status":"error","data":{}}));
        assert!(local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .is_err());
        assert_eq!(
            local_store
                .stored_outgoing_contact_request(
                    &advertisement_hash,
                    &remote_profile_did,
                    current_timestamp()
                )
                .unwrap()
                .unwrap(),
            stored_request
        );
        seed_provider.clear_request_submission_response();
        let (first, second) = tokio::join!(
            local_service.send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp()
            ),
            local_service.send_contact_request(
                local_store.as_ref(),
                &advertisement_id,
                &local_profile,
                current_timestamp()
            )
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            local_store
                .stored_outgoing_contact_request(
                    &advertisement_hash,
                    &remote_profile_did,
                    current_timestamp()
                )
                .unwrap()
                .unwrap(),
            stored_request
        );
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let remote_requests = wait_for_incoming_request(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        assert_eq!(remote_requests.len(), 1);
        remote_service
            .submit_contact_decision(
                remote_store.as_ref(),
                &remote_profile,
                remote_requests[0].request_hash(),
                CollaborationContactDecision::Accepted,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        let local_contacts =
            wait_for_contact_count(&local_service, local_store.as_ref(), &local_profile, 1).await;
        let remote_contacts =
            wait_for_contact_count(&remote_service, remote_store.as_ref(), &remote_profile, 1)
                .await;
        assert_eq!(
            local_contacts.contacts()[0].conversation_id(),
            remote_contacts.contacts()[0].conversation_id()
        );
    }

    #[tokio::test]
    async fn status_resends_outbox_deterministically_with_budget_4_and_terminal_requests_excluded()
    {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider.clone()).await;

        let (local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let base_now = current_timestamp();
        let local_advertisement_now = base_now.saturating_sub(200);
        let local_advertisement_bytes = local_service
            .authority
            .prepare_advertisement(&local_profile, local_advertisement_now)
            .unwrap();
        let local_advertisement = verify_collaboration_discovery_advertisement(
            &local_advertisement_bytes,
            &local_service.authority.profile,
            local_advertisement_now,
        )
        .unwrap();
        local_store
            .store_local_advertisement(&local_advertisement_bytes, local_advertisement_now)
            .unwrap();

        let remote_device_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let remote_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&remote_device_key.to_bytes()),
            local_service.network_profile(),
        );
        let remote_profile =
            verified_discovery_profile(&remote_device_key, "Remote", Some("remote"));
        let mut expected_requests = Vec::new();
        let mut expected_decisions = Vec::new();
        let mut terminal_request_hashes = Vec::new();

        for index in (0..TEST_MAX_REQUESTS_PER_SENDER).rev() {
            let request_now = base_now.saturating_sub(100) + index as u64 + 1;
            let remote_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
            let remote_profile = verified_discovery_profile_with(
                &discovery_profile_signing_key(&remote_key),
                &remote_key,
                &format!("Remote Outgoing {index:02}"),
                Some(&format!("remote-outgoing-{index:02}")),
                1,
                None,
                request_now,
            );
            let remote_authority = CollaborationDiscoveryAuthority::new(
                SigningKey::from_bytes(&remote_key.to_bytes()),
                local_service.network_profile(),
            );
            let advertisement =
                cached_advertisement(&remote_authority, &remote_profile, request_now);
            merge_profile_scoped_advertisement(
                &mut seed_provider.inner.state.lock().unwrap().advertisements,
                advertisement.clone(),
            )
            .unwrap();
            let request_bytes = local_service
                .authority
                .prepare_contact_request(&advertisement.verified, &local_profile, request_now)
                .unwrap();
            local_store
                .record_outgoing_contact_request(
                    &request_bytes,
                    &advertisement.envelope_bytes,
                    request_now,
                )
                .unwrap();
            expected_requests.push((
                request_now,
                collaboration_message_envelope_sha256(&request_bytes),
            ));
        }

        for index in (0..TEST_MAX_DECISIONS_PER_SENDER).rev() {
            let request_now = base_now.saturating_sub(50) + index as u64 + 1;
            let request_bytes = remote_authority
                .prepare_contact_request(&local_advertisement, &remote_profile, request_now)
                .unwrap();
            let verified_request = verify_collaboration_contact_request(
                &request_bytes,
                &local_service.authority.profile,
                &local_advertisement,
                request_now,
            )
            .unwrap();
            let decided_at = request_now + 1;
            let decision_bytes = local_service
                .authority
                .prepare_contact_decision_receipt(
                    &verified_request,
                    &local_profile,
                    CollaborationContactDecision::Declined,
                    decided_at,
                )
                .unwrap();
            local_store
                .record_incoming_contact_request(&request_bytes, request_now)
                .unwrap();
            local_store
                .record_contact_decision_receipt(&decision_bytes, decided_at)
                .unwrap();
            seed_provider.inner.state.lock().unwrap().requests.insert(
                verified_request.message().envelope_sha256().to_string(),
                RelayContactRequest {
                    envelope_bytes: request_bytes.clone(),
                    verified: verified_request.clone(),
                    advertisement: CachedAdvertisement {
                        envelope_bytes: local_advertisement_bytes.clone(),
                        verified: local_advertisement.clone(),
                    },
                },
            );
            expected_decisions.push((
                decided_at,
                verified_request.message().envelope_sha256().to_string(),
            ));
        }

        for (decision, offset) in [
            (CollaborationContactDecision::Accepted, 1_u64),
            (CollaborationContactDecision::Declined, 2_u64),
        ] {
            let request_now = base_now.saturating_sub(25) + offset;
            let remote_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
            let remote_profile = verified_discovery_profile_with(
                &discovery_profile_signing_key(&remote_key),
                &remote_key,
                &format!("Terminal {offset}"),
                Some(&format!("terminal-{offset}")),
                1,
                None,
                request_now,
            );
            let remote_authority = CollaborationDiscoveryAuthority::new(
                SigningKey::from_bytes(&remote_key.to_bytes()),
                local_service.network_profile(),
            );
            let advertisement =
                cached_advertisement(&remote_authority, &remote_profile, request_now);
            merge_profile_scoped_advertisement(
                &mut seed_provider.inner.state.lock().unwrap().advertisements,
                advertisement.clone(),
            )
            .unwrap();
            let request_bytes = local_service
                .authority
                .prepare_contact_request(&advertisement.verified, &local_profile, request_now)
                .unwrap();
            let request_hash = collaboration_message_envelope_sha256(&request_bytes);
            local_store
                .record_outgoing_contact_request(
                    &request_bytes,
                    &advertisement.envelope_bytes,
                    request_now,
                )
                .unwrap();
            let decision_bytes = remote_authority
                .prepare_contact_decision_receipt(
                    &verify_collaboration_contact_request(
                        &request_bytes,
                        &local_service.authority.profile,
                        &advertisement.verified,
                        request_now,
                    )
                    .unwrap(),
                    &remote_profile,
                    decision,
                    request_now + 1,
                )
                .unwrap();
            local_store
                .record_contact_decision_receipt(&decision_bytes, request_now + 1)
                .unwrap();
            terminal_request_hashes.push(request_hash);
        }

        let local_device_did = local_service.authority.local_device_did();
        let principal_id = format!("person:local:{}", &local_device_did[8..16]);
        let localhost_root = crate::auth::principal_localhost_root(&principal_id);
        let restarted_store = Arc::new(
            CollaborationContactStore::new(
                &temp.path().join("local"),
                &principal_id,
                &localhost_root,
                local_service.network_profile(),
                &local_profile,
                &local_device_did,
            )
            .unwrap(),
        );
        let restarted_service = CollaborationDiscoveryService::new(
            SigningKey::from_bytes(&local_service.authority.signing_key.to_bytes()),
            local_service.network_profile(),
            local_registry.clone(),
        )
        .await
        .unwrap();

        let status = restarted_service
            .local_status(
                restarted_store.as_ref(),
                &local_profile,
                current_timestamp(),
            )
            .unwrap();
        assert!(!status.enabled());
        assert!(status.visible_people().is_empty());
        assert_eq!(
            restarted_store
                .resendable_outgoing_contact_requests(
                    current_timestamp(),
                    MAX_DISCOVERY_QUERY_RESULTS
                )
                .unwrap()
                .len(),
            TEST_MAX_REQUESTS_PER_SENDER
        );
        assert_eq!(
            restarted_store
                .resendable_contact_decisions(current_timestamp(), MAX_DISCOVERY_QUERY_RESULTS)
                .unwrap()
                .len(),
            TEST_MAX_DECISIONS_PER_SENDER
        );
        restarted_service
            .resubmit_pending_outbox(restarted_store.as_ref(), current_timestamp())
            .await
            .unwrap();

        expected_requests.sort_by(|(left_time, left_hash), (right_time, right_hash)| {
            left_time
                .cmp(right_time)
                .then_with(|| left_hash.cmp(right_hash))
        });
        expected_decisions.sort_by(|(left_time, left_hash), (right_time, right_hash)| {
            left_time
                .cmp(right_time)
                .then_with(|| left_hash.cmp(right_hash))
        });
        let mut request_ops = VecDeque::from(
            expected_requests
                .into_iter()
                .map(|(_, hash)| LoggedRelayOperation::Request(hash))
                .collect::<Vec<_>>(),
        );
        let mut decision_ops = VecDeque::from(
            expected_decisions
                .into_iter()
                .map(|(_, hash)| LoggedRelayOperation::Decision(hash))
                .collect::<Vec<_>>(),
        );
        let mut expected_operations = Vec::new();
        let mut prefer_requests = true;
        while expected_operations.len() < MAX_DISCOVERY_OUTBOX_SENDS_PER_SYNC
            && (!request_ops.is_empty() || !decision_ops.is_empty())
        {
            let take_request = match (request_ops.is_empty(), decision_ops.is_empty()) {
                (true, true) => break,
                (false, true) => true,
                (true, false) => false,
                (false, false) => prefer_requests,
            };
            if take_request {
                expected_operations.push(request_ops.pop_front().unwrap());
            } else {
                expected_operations.push(decision_ops.pop_front().unwrap());
            }
            if !request_ops.is_empty() && !decision_ops.is_empty() {
                prefer_requests = !prefer_requests;
            } else {
                prefer_requests = !request_ops.is_empty();
            }
        }
        assert_eq!(
            expected_operations.len(),
            MAX_DISCOVERY_OUTBOX_SENDS_PER_SYNC
        );
        let actual_operations = seed_provider.submission_log();
        assert_eq!(actual_operations, expected_operations);
        assert!(actual_operations.len() <= MAX_DISCOVERY_OUTBOX_SENDS_PER_SYNC);
        assert!(terminal_request_hashes.iter().all(|hash| {
            !actual_operations.iter().any(|operation| match operation {
                LoggedRelayOperation::Request(candidate)
                | LoggedRelayOperation::Decision(candidate) => candidate == hash,
            })
        }));
    }

    #[tokio::test]
    async fn refresh_reports_unavailable_when_mailboxes_fail_even_if_query_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider.clone()).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        seed_provider.reject_mailboxes();
        assert!(local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .is_err());
        let status = local_service
            .local_status(local_store.as_ref(), &local_profile, current_timestamp())
            .unwrap();
        assert!(!status.available());
        assert_eq!(status.visible_people().len(), 1);
        assert_eq!(status.visible_people()[0].display_name(), "Remote");
    }

    #[tokio::test]
    async fn disabling_discovery_retains_active_advertisement_when_withdrawal_fails() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider.clone()).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let visible = wait_for_visible_peer(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        assert_eq!(visible.visible_people().len(), 1);

        seed_provider.reject_withdrawals();
        let status = local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                false,
                current_timestamp(),
            )
            .await
            .unwrap();
        assert!(!status.available());
        assert!(!status.enabled());
        assert!(status.remote_visibility_may_remain());
        assert!(status.visible_people().is_empty());

        let local_status = local_service
            .local_status(local_store.as_ref(), &local_profile, current_timestamp())
            .unwrap();
        assert!(!local_status.enabled());
        assert!(local_status.remote_visibility_may_remain());
        assert!(local_status.visible_people().is_empty());
        assert!(local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .is_err());
        seed_provider.allow_withdrawals();
        for response in [
            serde_json::json!({"status":"error","data":{}}),
            serde_json::json!({"status":"ok"}),
        ] {
            seed_provider.set_withdrawal_response(response);
            assert!(local_service
                .refresh(local_store.as_ref(), &local_profile, current_timestamp())
                .await
                .is_err());
            assert!(local_store
                .published_local_advertisement(current_timestamp())
                .unwrap()
                .is_some());
            let status = local_service
                .local_status(local_store.as_ref(), &local_profile, current_timestamp())
                .unwrap();
            assert!(status.remote_visibility_may_remain());
            assert!(!status.available());
        }
        let refreshed = remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        assert_eq!(refreshed.visible_people().len(), 1);
        assert_eq!(refreshed.visible_people()[0].display_name(), "Local");
        let after_remote_ttl = local_service
            .local_status(
                local_store.as_ref(),
                &local_profile,
                current_timestamp()
                    .saturating_add(COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS + 1),
            )
            .unwrap();
        assert!(!after_remote_ttl.remote_visibility_may_remain());
    }

    #[tokio::test]
    async fn disabling_discovery_removes_visible_advertisement_after_successful_withdrawal() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket.clone()],
        )
        .await;
        let (_remote_registry, remote_service, remote_store, _remote_node) = discovery_service(
            &temp.path().join("remote"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let remote_profile = service_profile(&remote_service, "Remote", Some("remote"));

        local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        remote_service
            .set_enabled(
                remote_store.as_ref(),
                &remote_profile,
                true,
                current_timestamp(),
            )
            .await
            .unwrap();
        let visible = wait_for_visible_peer(
            &remote_service,
            remote_store.as_ref(),
            &remote_profile,
            "Local",
        )
        .await;
        assert_eq!(visible.visible_people().len(), 1);

        let status = local_service
            .set_enabled(
                local_store.as_ref(),
                &local_profile,
                false,
                current_timestamp(),
            )
            .await
            .unwrap();
        assert!(!status.enabled());
        local_service
            .refresh(local_store.as_ref(), &local_profile, current_timestamp())
            .await
            .unwrap();
        let refreshed = remote_service
            .refresh(remote_store.as_ref(), &remote_profile, current_timestamp())
            .await
            .unwrap();
        assert!(refreshed.visible_people().is_empty());
    }

    #[tokio::test]
    async fn invoke_bootstrap_rejects_substituted_node_id_for_ticket() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket.clone()]);
        let seed_provider = Arc::new(ControllableRelayProvider::new(seed_profile.clone()));
        seed_registry.register(seed_provider).await;

        let (_local_registry, local_service, local_store, _local_node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![seed_ticket],
        )
        .await;
        let local_profile = service_profile(&local_service, "Local", Some("local"));
        let cached = local_service
            .prepare_and_store_local_advertisement(
                local_store.as_ref(),
                &local_profile,
                current_timestamp(),
            )
            .unwrap();
        let bad_service = CollaborationDiscoveryService {
            authority: local_service.authority.clone(),
            registry: local_service.registry.clone(),
            bootstrap_peers: Arc::new(vec![CollaborationBootstrapPeer {
                node_id: "wrong-node".to_string(),
                connect_ticket: local_service.bootstrap_peers[0].connect_ticket.clone(),
            }]),
            state: local_service.state.clone(),
            profile_updates: local_service.profile_updates.clone(),
            sync_contexts: local_service.sync_contexts.clone(),
            sync_pass_lock: local_service.sync_pass_lock.clone(),
            intent_mutex: local_service.intent_mutex.clone(),
            direct_messages: local_service.direct_messages.clone(),
        };
        let error = bad_service
            .invoke_bootstrap(
                "query",
                serde_json::to_value(DiscoveryProviderQueryRequest {
                    op: "query".to_string(),
                    advertisement: encode_bytes(&cached.envelope_bytes),
                })
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("peer_did does not match connect_ticket"));
    }

    #[tokio::test]
    async fn relay_query_excludes_caller_and_bounds_results() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();
        let caller_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let caller_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&caller_key.to_bytes()),
            profile.clone(),
        );
        let caller_advertisement = caller_authority
            .prepare_advertisement(
                &verified_discovery_profile(&caller_key, "Peer 0", None),
                now,
            )
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&caller_advertisement),
            }))
            .await
            .unwrap();

        for index in 1..34u8 {
            let key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
            let authority = CollaborationDiscoveryAuthority::new(
                SigningKey::from_bytes(&key.to_bytes()),
                profile.clone(),
            );
            let advertisement = authority
                .prepare_advertisement(
                    &verified_discovery_profile(&key, &format!("Peer {index}"), None),
                    now,
                )
                .unwrap();
            relay
                .send_raw(&serde_json::json!({
                    "op": "advertise",
                    "advertisement": encode_bytes(&advertisement),
                }))
                .await
                .unwrap();
        }

        let response = relay
            .send_raw(&serde_json::json!({
                "op": "query",
                "advertisement": encode_bytes(&caller_advertisement),
            }))
            .await
            .unwrap();
        let response: DiscoveryProviderAdvertisementResponse =
            serde_json::from_value(response).unwrap();
        let advertisements =
            decode_advertisement_response(&profile, &response, current_timestamp()).unwrap();
        assert_eq!(advertisements.len(), 32);
        assert!(advertisements
            .iter()
            .all(|peer| peer.verified.display_name() != "Peer 0"));
    }

    #[tokio::test]
    async fn relay_conflicting_same_revision_advertisement_preserves_existing_entry() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();
        let profile_signer = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device.to_bytes()),
            profile.clone(),
        );
        let first_profile = verified_discovery_profile_with(
            &profile_signer,
            &device,
            "Alpha",
            Some("alpha"),
            1,
            None,
            now,
        );
        let first_advertisement = authority
            .prepare_advertisement(&first_profile, now)
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&first_advertisement),
            }))
            .await
            .unwrap();

        let conflicting_profile = verified_discovery_profile_with(
            &profile_signer,
            &device,
            "Beta",
            Some("beta"),
            1,
            None,
            now + 1,
        );
        let conflicting_advertisement = authority
            .prepare_advertisement(&conflicting_profile, now + 1)
            .unwrap();
        let error = relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&conflicting_advertisement),
            }))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting discovery profile revision"));
        {
            let state = relay.state.lock().unwrap();
            let stored = state
                .advertisements
                .get(first_profile.document().profile_did.as_str())
                .unwrap();
            assert_eq!(stored.envelope_bytes, first_advertisement);
        }

        let caller_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let caller_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&caller_key.to_bytes()),
            profile.clone(),
        );
        let caller_advertisement = caller_authority
            .prepare_advertisement(
                &verified_discovery_profile(&caller_key, "Caller", Some("caller")),
                now + 2,
            )
            .unwrap();
        let response = relay
            .send_raw(&serde_json::json!({
                "op": "query",
                "advertisement": encode_bytes(&caller_advertisement),
            }))
            .await
            .unwrap();
        let response: DiscoveryProviderAdvertisementResponse =
            serde_json::from_value(response).unwrap();
        assert_eq!(response.data.advertisements.len(), 1);
        assert_eq!(
            decode_bytes(
                response.data.advertisements[0].as_str(),
                "query advertisement response"
            )
            .unwrap(),
            first_advertisement
        );
    }

    #[test]
    fn due_sync_selection_rotates_after_an_outage_backoff() {
        let mut due = BTreeMap::new();
        due.insert("first", 10_u64);
        due.insert("second", 10_u64);
        assert_eq!(select_due_sync_keys(due.into_iter(), 10), vec!["first"]);

        let mut after_first_failure = BTreeMap::new();
        after_first_failure.insert("first", 15_u64);
        after_first_failure.insert("second", 10_u64);
        assert_eq!(
            select_due_sync_keys(after_first_failure.into_iter(), 10),
            vec!["second"]
        );
    }

    #[test]
    fn independent_direct_schedule_runs_when_discovery_is_disabled_not_due_or_failing() {
        let now = 100_u64;
        let mut discovery_next = 200;
        let mut discovery_failures = 0;
        let mut direct_next = now;
        let mut direct_failures = 0;
        assert_eq!(discovery_next.min(direct_next), now);

        update_sync_schedule(
            &mut direct_next,
            &mut direct_failures,
            true,
            DIRECT_SYNC_CADENCE_SECS,
            now,
        );
        assert_eq!(direct_failures, 0);
        assert_eq!(direct_next, now + DIRECT_SYNC_CADENCE_SECS);
        assert_eq!(discovery_next, 200);

        discovery_next = now;
        direct_next = now;
        update_sync_schedule(
            &mut discovery_next,
            &mut discovery_failures,
            false,
            DISCOVERY_SYNC_IDLE_CADENCE_SECS,
            now,
        );
        update_sync_schedule(
            &mut direct_next,
            &mut direct_failures,
            true,
            DIRECT_SYNC_CADENCE_SECS,
            now,
        );
        assert_eq!(discovery_failures, 1);
        assert_eq!(discovery_next, now + DISCOVERY_SYNC_BASE_BACKOFF_SECS);
        assert_eq!(direct_failures, 0);
        assert_eq!(direct_next, now + DIRECT_SYNC_CADENCE_SECS);
    }

    #[test]
    fn independent_direct_schedule_failure_does_not_change_successful_discovery() {
        let now = 100_u64;
        let mut discovery_next = now;
        let mut discovery_failures = 2;
        let mut direct_next = now;
        let mut direct_failures = 0;
        update_sync_schedule(
            &mut discovery_next,
            &mut discovery_failures,
            true,
            DISCOVERY_SYNC_ENABLED_CADENCE_SECS,
            now,
        );
        update_sync_schedule(
            &mut direct_next,
            &mut direct_failures,
            false,
            DIRECT_SYNC_CADENCE_SECS,
            now,
        );
        assert_eq!(discovery_failures, 0);
        assert_eq!(discovery_next, now + DISCOVERY_SYNC_ENABLED_CADENCE_SECS);
        assert_eq!(direct_failures, 1);
        assert_eq!(direct_next, now + DISCOVERY_SYNC_BASE_BACKOFF_SECS);
        assert_eq!(
            select_due_sync_keys(
                [("discovery", discovery_next), ("direct", direct_next)].into_iter(),
                direct_next,
            ),
            vec!["direct"]
        );
    }

    fn first_registered_context<'a>(
        snapshot: &'a serde_json::Value,
        section: &str,
    ) -> &'a serde_json::Value {
        snapshot
            .get(section)
            .and_then(serde_json::Value::as_array)
            .and_then(|contexts| contexts.first())
            .unwrap_or_else(|| panic!("missing {section} context snapshot"))
    }

    #[tokio::test]
    async fn runtime_owned_contexts_install_when_no_session_context_exists() {
        let temp = tempfile::tempdir().unwrap();
        let pair = durable_profile_peer_pair(temp.path()).await;
        let root_a = temp.path().join("a");

        assert_eq!(
            pair.service_a
                .register_runtime_owned_contexts(&root_a)
                .unwrap(),
            1
        );

        let snapshot = pair.service_a.registered_context_snapshot_for_test();
        assert_eq!(
            snapshot["sync"].as_array().map(Vec::len),
            Some(0),
            "runtime-owned registration must not create a session sync context"
        );
        assert_eq!(
            first_registered_context(&snapshot, "direct")["authority"]["kind"],
            "runtime_owned"
        );
        assert_eq!(
            first_registered_context(&snapshot, "profile_updates")["authority"]["kind"],
            "runtime_owned"
        );
    }

    #[tokio::test]
    async fn runtime_owned_profile_refresh_keeps_live_session_authority() {
        let temp = tempfile::tempdir().unwrap();
        let pair = durable_profile_peer_pair(temp.path()).await;
        let root_a = temp.path().join("a");
        let now = crate::auth::now_ts();

        assert_eq!(
            pair.service_a
                .register_runtime_owned_contexts(&root_a)
                .unwrap(),
            1
        );
        let grant = store_home_session_grant_for_test(&root_a, &pair.identity_a, "alice-home", now);
        pair.service_a
            .register_sync_context(
                pair.store_a.clone(),
                pair.identity_a.profile.clone(),
                &grant.session_id,
                Some(&pair.identity_a.proof_binding_id),
                &grant.grant_id,
                now,
            )
            .unwrap();

        let before = pair.service_a.registered_context_snapshot_for_test();
        assert_eq!(
            first_registered_context(&before, "direct")["authority"]["kind"],
            "session"
        );
        assert_eq!(
            first_registered_context(&before, "profile_updates")["authority"]["kind"],
            "session"
        );

        let renamed = crate::collaboration_profile_authority::update_profile_authority(
            &root_a,
            &pair.identity_a.principal_id,
            &pair.identity_a.localhost_root,
            &pair.identity_a.proof_binding_id,
            "Alice Renamed",
            Some("alice"),
            now + 1,
        )
        .unwrap();
        assert_eq!(
            pair.service_a
                .register_runtime_owned_contexts(&root_a)
                .unwrap(),
            1
        );

        let after = pair.service_a.registered_context_snapshot_for_test();
        assert_eq!(
            first_registered_context(&after, "direct")["authority"]["kind"],
            "session"
        );
        assert_eq!(
            first_registered_context(&after, "profile_updates")["authority"]["kind"],
            "session"
        );
        assert_eq!(
            first_registered_context(&after, "sync")["authority"]["session_id"],
            grant.session_id
        );
        assert_eq!(
            first_registered_context(&after, "sync")["authority"]["grant_id"],
            grant.grant_id
        );
        assert_eq!(
            first_registered_context(&after, "direct")["authority"]["session_id"],
            grant.session_id
        );
        assert_eq!(
            first_registered_context(&after, "profile_updates")["authority"]["session_id"],
            grant.session_id
        );
        assert_eq!(
            first_registered_context(&after, "direct")["profile_revision"],
            renamed.document().revision
        );
        assert_eq!(
            first_registered_context(&after, "profile_updates")["profile_revision"],
            renamed.document().revision
        );
        assert_ne!(
            first_registered_context(&before, "direct")["profile_hash"],
            first_registered_context(&after, "direct")["profile_hash"]
        );
        assert_ne!(
            first_registered_context(&before, "profile_updates")["profile_hash"],
            first_registered_context(&after, "profile_updates")["profile_hash"]
        );
    }

    #[tokio::test]
    async fn independent_direct_schedule_actual_worker_updates_each_result_separately() {
        let temp = tempfile::tempdir().unwrap();
        let pair = direct_peer_pair(temp.path()).await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let now = current_timestamp();
        let key = DiscoverySyncContextKey {
            principal_id: pair.store_a.principal_id().to_string(),
            localhost_root: pair.store_a.localhost_root().to_string(),
            profile_did: pair.profile_a.document().profile_did.clone(),
        };
        pair.service_a.sync_contexts.lock().unwrap().insert(
            key.clone(),
            DiscoverySyncContext {
                store: pair.store_a.clone(),
                profile: pair.profile_a.clone(),
                session_id: "worker-test-session".to_string(),
                proof_binding_id: None,
                grant_id: "worker-test-grant".to_string(),
                discovery_next_wake_at: now + 100,
                discovery_failures: 2,
                direct_next_wake_at: now,
                direct_failures: 0,
                profile_next_wake_at: now,
                profile_failures: 0,
                verified_for_test: true,
            },
        );
        let reachable = pair
            .store_a
            .snapshot()
            .unwrap()
            .contacts()
            .iter()
            .find(|contact| contact.conversation_id() == pair.conversation_id)
            .unwrap()
            .remote_profile_did()
            .to_string();
        let direct = pair.service_a.direct_message_service();
        let reachable_envelope = direct
            .persist_outgoing_for_test(
                &key.profile_did,
                "worker-reachable",
                &pair.conversation_id,
                &reachable,
                "reachable",
                now,
            )
            .unwrap();
        pair.service_a.sync_registered_contexts_once(now).await;
        let after_reachable = pair.service_a.sync_contexts.lock().unwrap()[&key].clone();
        assert_eq!(after_reachable.discovery_next_wake_at, now + 100);
        assert_eq!(after_reachable.discovery_failures, 2);
        assert_eq!(
            after_reachable.direct_next_wake_at,
            now + DIRECT_SYNC_CADENCE_SECS
        );
        assert_eq!(after_reachable.direct_failures, 0);
        assert!(
            direct
                .records_for_test(&key.profile_did, now)
                .unwrap()
                .iter()
                .find(|record| record.envelope_bytes == reachable_envelope)
                .unwrap()
                .receipt_settled
        );

        let (offline_did, offline_conversation) = add_offline_accepted_contact(&pair);
        let later = now + DIRECT_SYNC_CADENCE_SECS;
        let offline_envelope = direct
            .persist_outgoing_for_test(
                &key.profile_did,
                "worker-offline",
                &offline_conversation,
                &offline_did,
                "offline",
                later,
            )
            .unwrap();
        {
            let mut contexts = pair.service_a.sync_contexts.lock().unwrap();
            let context = contexts.get_mut(&key).unwrap();
            context.discovery_next_wake_at = later;
            context.discovery_failures = 1;
            context.direct_next_wake_at = later;
            context.direct_failures = 0;
        }
        pair.service_a.sync_registered_contexts_once(later).await;
        let after_offline = pair.service_a.sync_contexts.lock().unwrap()[&key].clone();
        assert_eq!(
            after_offline.discovery_next_wake_at,
            later + DISCOVERY_SYNC_IDLE_CADENCE_SECS
        );
        assert_eq!(after_offline.discovery_failures, 0);
        assert_eq!(
            after_offline.direct_next_wake_at,
            later + DISCOVERY_SYNC_BASE_BACKOFF_SECS
        );
        assert_eq!(after_offline.direct_failures, 1);
        assert!(
            !direct
                .records_for_test(&key.profile_did, later)
                .unwrap()
                .iter()
                .find(|record| record.envelope_bytes == offline_envelope)
                .unwrap()
                .receipt_settled
        );
    }

    #[test]
    fn client_state_capacity_rejects_new_live_profile_without_mutation() {
        let mut states = BTreeMap::new();
        let existing_keys = (0..MAX_DISCOVERY_CLIENT_STATES)
            .map(|index| format!("did:key:zprofile{index:02}"))
            .collect::<Vec<_>>();
        for (index, profile_did) in existing_keys.iter().enumerate() {
            let mut state = DiscoveryClientState::default();
            state.observed_profile_heads.insert(
                format!("did:key:zhead{index:02}"),
                ObservedProfileHead {
                    revision: 1,
                    profile_envelope_sha256: format!("sha256:{:064x}", index + 1),
                    observed_at: 1,
                },
            );
            states.insert(profile_did.clone(), state);
        }

        let error = match client_state_mut(&mut states, "did:key:znew") {
            Ok(_) => panic!("expected discovery client state capacity error"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("capacity is full"));
        assert_eq!(states.len(), MAX_DISCOVERY_CLIENT_STATES);
        assert!(!states.contains_key("did:key:znew"));
        for profile_did in existing_keys {
            assert!(states.contains_key(&profile_did));
        }
    }

    #[test]
    fn client_state_capacity_discards_only_a_true_default_state() {
        let mut states = BTreeMap::new();
        for index in 0..MAX_DISCOVERY_CLIENT_STATES {
            let mut state = DiscoveryClientState::default();
            if index != 0 {
                state.transport_available = false;
            }
            states.insert(format!("did:key:zprofile{index:02}"), state);
        }
        client_state_mut(&mut states, "did:key:znew").unwrap();
        assert_eq!(states.len(), MAX_DISCOVERY_CLIENT_STATES);
        assert!(states.contains_key("did:key:znew"));
        assert!(!states.contains_key("did:key:zprofile00"));
        for index in 1..MAX_DISCOVERY_CLIENT_STATES {
            assert!(states.contains_key(&format!("did:key:zprofile{index:02}")));
        }

        let mut protected_states = BTreeMap::new();
        for index in 0..MAX_DISCOVERY_CLIENT_STATES {
            let state = DiscoveryClientState {
                remote_visibility_may_remain_until: Some(1_000 + index as u64),
                ..DiscoveryClientState::default()
            };
            protected_states.insert(format!("did:key:zprotected{index:02}"), state);
        }
        let error = match client_state_mut(&mut protected_states, "did:key:znew") {
            Ok(_) => panic!("expected discovery client state capacity error"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("capacity is full"));
        assert!(!protected_states.contains_key("did:key:znew"));
    }

    #[tokio::test]
    async fn relay_withdrawal_uses_exact_advertisement_hash_when_one_device_has_two_profiles() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();
        let device_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_key.to_bytes()),
            profile.clone(),
        );

        let first_profile = verified_discovery_profile_with(
            &discovery_profile_signing_key(&SigningKey::from_bytes(
                &generate_keypair().0.to_bytes(),
            )),
            &device_key,
            "First",
            Some("first"),
            1,
            None,
            now,
        );
        let second_profile = verified_discovery_profile_with(
            &discovery_profile_signing_key(&SigningKey::from_bytes(
                &generate_keypair().0.to_bytes(),
            )),
            &device_key,
            "Second",
            Some("second"),
            1,
            None,
            now + 1,
        );

        let first_advertisement = authority
            .prepare_advertisement(&first_profile, now)
            .unwrap();
        let second_advertisement = authority
            .prepare_advertisement(&second_profile, now + 1)
            .unwrap();
        let verified_first =
            verify_collaboration_discovery_advertisement(&first_advertisement, &profile, now)
                .unwrap();
        let verified_second =
            verify_collaboration_discovery_advertisement(&second_advertisement, &profile, now + 1)
                .unwrap();

        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&first_advertisement),
            }))
            .await
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&second_advertisement),
            }))
            .await
            .unwrap();

        let withdrawal = authority
            .prepare_withdrawal(&verified_second, now + 2)
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "withdraw",
                "withdrawal": encode_bytes(&withdrawal),
            }))
            .await
            .unwrap();

        let state = relay.state.lock().unwrap();
        assert!(state
            .advertisements
            .contains_key(verified_first.profile_did()));
        assert!(!state
            .advertisements
            .contains_key(verified_second.profile_did()));
        let remaining = state
            .advertisements
            .get(verified_first.profile_did())
            .unwrap();
        assert_eq!(remaining.envelope_bytes, first_advertisement);
    }

    #[tokio::test]
    async fn relay_mailboxes_are_profile_scoped_and_require_bound_profile_devices() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(network_profile.clone());
        let now = current_timestamp();

        let profile_a_device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile_a_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&profile_a_device.to_bytes()),
            network_profile.clone(),
        );
        let profile_a = verified_discovery_profile(&profile_a_device, "Profile A", Some("a"));

        let profile_b_signing_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let revoked_profile_b_device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile_b_device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile_b_v1 = verified_discovery_profile_with(
            &profile_b_signing_key,
            &revoked_profile_b_device,
            "Profile B",
            Some("b"),
            1,
            None,
            now,
        );
        let profile_b = verified_discovery_profile_with(
            &profile_b_signing_key,
            &profile_b_device,
            "Profile B",
            Some("b"),
            2,
            Some(&profile_hash(&profile_b_v1)),
            now + 1,
        );
        let profile_b_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&profile_b_device.to_bytes()),
            network_profile.clone(),
        );
        let revoked_profile_b_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&revoked_profile_b_device.to_bytes()),
            network_profile.clone(),
        );

        let requester_device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let requester_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&requester_device.to_bytes()),
            network_profile.clone(),
        );
        let requester_profile =
            verified_discovery_profile(&requester_device, "Requester", Some("requester"));

        let profile_b_advertisement = profile_b_authority
            .prepare_advertisement(&profile_b, now + 1)
            .unwrap();
        let verified_profile_b_advertisement = verify_collaboration_discovery_advertisement(
            &profile_b_advertisement,
            &network_profile,
            now + 1,
        )
        .unwrap();
        let requester_advertisement = requester_authority
            .prepare_advertisement(&requester_profile, now + 1)
            .unwrap();
        let verified_requester_advertisement = verify_collaboration_discovery_advertisement(
            &requester_advertisement,
            &network_profile,
            now + 1,
        )
        .unwrap();
        for advertisement in [&profile_b_advertisement, &requester_advertisement] {
            relay
                .send_raw(&serde_json::json!({
                    "op": "advertise",
                    "advertisement": encode_bytes(advertisement),
                }))
                .await
                .unwrap();
        }

        let inbound_request = requester_authority
            .prepare_contact_request(
                &verified_profile_b_advertisement,
                &requester_profile,
                now + 2,
            )
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "send_contact_request",
                "request": encode_bytes(&inbound_request),
            }))
            .await
            .unwrap();

        let profile_a_request_poll = profile_a_authority
            .prepare_mailbox_poll(
                &profile_a,
                CollaborationDiscoveryMailboxKind::Requests,
                now + 3,
            )
            .unwrap();
        let profile_a_response = relay
            .send_raw(&serde_json::json!({
                "op": "poll_requests",
                "poll": encode_bytes(&profile_a_request_poll),
            }))
            .await
            .unwrap();
        assert!(profile_a_response["data"]["requests"]
            .as_array()
            .unwrap()
            .is_empty());

        let claimed_profile_b_request_poll = profile_a_authority
            .sign_message(
                &profile_b.document().profile_did,
                COLLABORATION_DISCOVERY_DIRECTORY_ID,
                CollaborationRecipient {
                    kind: CollaborationRecipientKind::Profile,
                    id: profile_b.document().profile_did.clone(),
                },
                COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
                serde_json::to_value(CollaborationDiscoveryMailboxPollPayload {
                    mailbox_kind: CollaborationDiscoveryMailboxKind::Requests,
                    profile_did: profile_b.document().profile_did.clone(),
                })
                .unwrap(),
                CollaborationMessageTiming {
                    created_at: now + 3,
                    ttl_secs: COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
                },
            )
            .unwrap();
        let claimed_response = relay
            .send_raw(&serde_json::json!({
                "op": "poll_requests",
                "poll": encode_bytes(&claimed_profile_b_request_poll),
            }))
            .await
            .unwrap();
        assert!(claimed_response["data"]["requests"]
            .as_array()
            .unwrap()
            .is_empty());

        let revoked_profile_b_request_poll = revoked_profile_b_authority
            .sign_message(
                &profile_b.document().profile_did,
                COLLABORATION_DISCOVERY_DIRECTORY_ID,
                CollaborationRecipient {
                    kind: CollaborationRecipientKind::Profile,
                    id: profile_b.document().profile_did.clone(),
                },
                COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
                serde_json::to_value(CollaborationDiscoveryMailboxPollPayload {
                    mailbox_kind: CollaborationDiscoveryMailboxKind::Requests,
                    profile_did: profile_b.document().profile_did.clone(),
                })
                .unwrap(),
                CollaborationMessageTiming {
                    created_at: now + 3,
                    ttl_secs: COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
                },
            )
            .unwrap();
        let revoked_response = relay
            .send_raw(&serde_json::json!({
                "op": "poll_requests",
                "poll": encode_bytes(&revoked_profile_b_request_poll),
            }))
            .await
            .unwrap();
        assert!(revoked_response["data"]["requests"]
            .as_array()
            .unwrap()
            .is_empty());

        let profile_b_request_poll = profile_b_authority
            .prepare_mailbox_poll(
                &profile_b,
                CollaborationDiscoveryMailboxKind::Requests,
                now + 3,
            )
            .unwrap();
        for _ in 0..2 {
            let response = relay
                .send_raw(&serde_json::json!({
                    "op": "poll_requests",
                    "poll": encode_bytes(&profile_b_request_poll),
                }))
                .await
                .unwrap();
            assert_eq!(response["data"]["requests"].as_array().unwrap().len(), 1);
        }

        let outgoing_request = profile_b_authority
            .prepare_contact_request(&verified_requester_advertisement, &profile_b, now + 4)
            .unwrap();
        let verified_outgoing_request = verify_collaboration_contact_request(
            &outgoing_request,
            &network_profile,
            &verified_requester_advertisement,
            now + 4,
        )
        .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "send_contact_request",
                "request": encode_bytes(&outgoing_request),
            }))
            .await
            .unwrap();
        let decision = requester_authority
            .prepare_contact_decision_receipt(
                &verified_outgoing_request,
                &requester_profile,
                CollaborationContactDecision::Accepted,
                now + 5,
            )
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "submit_contact_decision_receipt",
                "receipt": encode_bytes(&decision),
            }))
            .await
            .unwrap();

        let claimed_profile_b_decision_poll = profile_a_authority
            .sign_message(
                &profile_b.document().profile_did,
                COLLABORATION_DISCOVERY_DIRECTORY_ID,
                CollaborationRecipient {
                    kind: CollaborationRecipientKind::Profile,
                    id: profile_b.document().profile_did.clone(),
                },
                COLLABORATION_DISCOVERY_MAILBOX_POLL_PAYLOAD_TYPE,
                serde_json::to_value(CollaborationDiscoveryMailboxPollPayload {
                    mailbox_kind: CollaborationDiscoveryMailboxKind::Decisions,
                    profile_did: profile_b.document().profile_did.clone(),
                })
                .unwrap(),
                CollaborationMessageTiming {
                    created_at: now + 6,
                    ttl_secs: COLLABORATION_DISCOVERY_CONTROL_TTL_SECS,
                },
            )
            .unwrap();
        let claimed_decision_response = relay
            .send_raw(&serde_json::json!({
                "op": "poll_decisions",
                "poll": encode_bytes(&claimed_profile_b_decision_poll),
            }))
            .await
            .unwrap();
        assert!(claimed_decision_response["data"]["decisions"]
            .as_array()
            .unwrap()
            .is_empty());

        let profile_b_decision_poll = profile_b_authority
            .prepare_mailbox_poll(
                &profile_b,
                CollaborationDiscoveryMailboxKind::Decisions,
                now + 6,
            )
            .unwrap();
        for _ in 0..2 {
            let response = relay
                .send_raw(&serde_json::json!({
                    "op": "poll_decisions",
                    "poll": encode_bytes(&profile_b_decision_poll),
                }))
                .await
                .unwrap();
            assert_eq!(response["data"]["decisions"].as_array().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn relay_advertisement_capacity_prunes_then_admits_and_replays_at_cap() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();

        {
            let mut state = relay.state.lock().unwrap();
            for index in 0..MAX_DISCOVERY_TOTAL_ADVERTISEMENTS {
                let device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
                let authority = CollaborationDiscoveryAuthority::new(
                    SigningKey::from_bytes(&device.to_bytes()),
                    profile.clone(),
                );
                let created_at = if index == 0 {
                    now.saturating_sub(COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS + 1)
                } else {
                    now.saturating_sub(1)
                };
                let advertisement = cached_advertisement(
                    &authority,
                    &verified_discovery_profile(&device, &format!("Peer {index:02}"), None),
                    created_at,
                );
                state.advertisements.insert(
                    advertisement.verified.profile_did().to_string(),
                    advertisement,
                );
            }
        }

        let admitted_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let admitted_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&admitted_key.to_bytes()),
            profile.clone(),
        );
        let admitted_advertisement = admitted_authority
            .prepare_advertisement(
                &verified_discovery_profile(&admitted_key, "Admitted", Some("admitted")),
                now,
            )
            .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&admitted_advertisement),
            }))
            .await
            .unwrap();
        {
            let state = relay.state.lock().unwrap();
            assert_eq!(
                state.advertisements.len(),
                MAX_DISCOVERY_TOTAL_ADVERTISEMENTS
            );
            assert!(state
                .advertisements
                .values()
                .all(|cached| { cached.verified.message().envelope().payload.expires_at > now }));
        }

        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&admitted_advertisement),
            }))
            .await
            .unwrap();

        let rejected_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let rejected_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&rejected_key.to_bytes()),
            profile.clone(),
        );
        let rejected_advertisement = rejected_authority
            .prepare_advertisement(
                &verified_discovery_profile(&rejected_key, "Rejected", Some("rejected")),
                now,
            )
            .unwrap();
        let error = relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&rejected_advertisement),
            }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("advertisement capacity is full"));
        let rejected_profile =
            verify_collaboration_discovery_advertisement(&rejected_advertisement, &profile, now)
                .unwrap();
        let state = relay.state.lock().unwrap();
        assert_eq!(
            state.advertisements.len(),
            MAX_DISCOVERY_TOTAL_ADVERTISEMENTS
        );
        assert!(!state
            .advertisements
            .contains_key(rejected_profile.profile_did()));
    }

    #[tokio::test]
    async fn relay_request_capacity_replay_and_rejection_are_atomic() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();
        let local_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let local_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&local_key.to_bytes()),
            profile.clone(),
        );
        let local_advertisement = local_authority
            .prepare_advertisement(
                &verified_discovery_profile(&local_key, "Local", Some("local")),
                now,
            )
            .unwrap();
        let verified_local_advertisement =
            verify_collaboration_discovery_advertisement(&local_advertisement, &profile, now)
                .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&local_advertisement),
            }))
            .await
            .unwrap();

        let secondary_local_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let secondary_local_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&secondary_local_key.to_bytes()),
            profile.clone(),
        );
        let secondary_local_advertisement = secondary_local_authority
            .prepare_advertisement(
                &verified_discovery_profile(&secondary_local_key, "Secondary", Some("secondary")),
                now,
            )
            .unwrap();
        let verified_secondary_local_advertisement = verify_collaboration_discovery_advertisement(
            &secondary_local_advertisement,
            &profile,
            now,
        )
        .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&secondary_local_advertisement),
            }))
            .await
            .unwrap();
        let primary_recipient_profile_did = verified_local_advertisement.profile_did().to_string();
        let secondary_recipient_profile_did = verified_secondary_local_advertisement
            .profile_did()
            .to_string();
        let tertiary_local_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let tertiary_local_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&tertiary_local_key.to_bytes()),
            profile.clone(),
        );
        let tertiary_local_advertisement = tertiary_local_authority
            .prepare_advertisement(
                &verified_discovery_profile(&tertiary_local_key, "Tertiary", Some("tertiary")),
                now,
            )
            .unwrap();
        let verified_tertiary_local_advertisement = verify_collaboration_discovery_advertisement(
            &tertiary_local_advertisement,
            &profile,
            now,
        )
        .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&tertiary_local_advertisement),
            }))
            .await
            .unwrap();
        let tertiary_recipient_profile_did = verified_tertiary_local_advertisement
            .profile_did()
            .to_string();
        let mut replay_request = None;
        let mut replay_hash = None;
        {
            let mut state = relay.state.lock().unwrap();
            for index in 0..MAX_DISCOVERY_TOTAL_REQUESTS {
                let remote_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
                let remote_authority = CollaborationDiscoveryAuthority::new(
                    SigningKey::from_bytes(&remote_key.to_bytes()),
                    profile.clone(),
                );
                let remote_profile =
                    verified_discovery_profile(&remote_key, &format!("Remote {index:02}"), None);
                let target_advertisement = if index < (MAX_DISCOVERY_TOTAL_REQUESTS / 2) {
                    &verified_local_advertisement
                } else {
                    &verified_secondary_local_advertisement
                };
                let (mailbox_profile_did, target_cached) =
                    if index < (MAX_DISCOVERY_TOTAL_REQUESTS / 2) {
                        (
                            primary_recipient_profile_did.clone(),
                            CachedAdvertisement {
                                envelope_bytes: local_advertisement.clone(),
                                verified: verified_local_advertisement.clone(),
                            },
                        )
                    } else {
                        (
                            secondary_recipient_profile_did.clone(),
                            CachedAdvertisement {
                                envelope_bytes: secondary_local_advertisement.clone(),
                                verified: verified_secondary_local_advertisement.clone(),
                            },
                        )
                    };
                let request_bytes = remote_authority
                    .prepare_contact_request(
                        target_advertisement,
                        &remote_profile,
                        now + u64::try_from(index).unwrap() + 1,
                    )
                    .unwrap();
                let verified_request = verify_collaboration_contact_request(
                    &request_bytes,
                    &profile,
                    target_advertisement,
                    now + u64::try_from(index).unwrap() + 1,
                )
                .unwrap();
                let request_hash = verified_request.message().envelope_sha256().to_string();
                if index == 0 {
                    replay_request = Some(request_bytes.clone());
                    replay_hash = Some(request_hash.clone());
                }
                state.requests.insert(
                    request_hash.clone(),
                    RelayContactRequest {
                        envelope_bytes: request_bytes,
                        verified: verified_request,
                        advertisement: target_cached,
                    },
                );
                state
                    .request_mailboxes
                    .entry(mailbox_profile_did)
                    .or_default()
                    .push(request_hash);
            }
            assert_eq!(state.requests.len(), MAX_DISCOVERY_TOTAL_REQUESTS);
            assert_eq!(
                total_request_mailbox_entries(&state),
                MAX_DISCOVERY_TOTAL_REQUEST_MAILBOX_ENTRIES
            );
        }
        let recipient_profile_did = primary_recipient_profile_did;

        relay
            .send_raw(&serde_json::json!({
                "op": "send_contact_request",
                "request": encode_bytes(replay_request.as_ref().unwrap()),
            }))
            .await
            .unwrap();

        let overflow_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let overflow_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&overflow_key.to_bytes()),
            profile.clone(),
        );
        let overflow_request = overflow_authority
            .prepare_contact_request(
                &verified_tertiary_local_advertisement,
                &verified_discovery_profile(&overflow_key, "Overflow", Some("overflow")),
                now + 1,
            )
            .unwrap();
        let overflow_hash = collaboration_message_envelope_sha256(&overflow_request);
        let error = relay
            .send_raw(&serde_json::json!({
                "op": "send_contact_request",
                "request": encode_bytes(&overflow_request),
            }))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("request capacity is full"),
            "unexpected request-capacity error: {error}"
        );

        let state = relay.state.lock().unwrap();
        assert_eq!(state.requests.len(), MAX_DISCOVERY_TOTAL_REQUESTS);
        assert_eq!(
            state
                .request_mailboxes
                .get(&recipient_profile_did)
                .map_or(0, Vec::len),
            MAX_DISCOVERY_TOTAL_REQUESTS / 2
        );
        assert!(!state
            .request_mailboxes
            .contains_key(&tertiary_recipient_profile_did));
        assert!(!state.requests.contains_key(&overflow_hash));
        assert!(state
            .request_mailboxes
            .get(&recipient_profile_did)
            .unwrap()
            .contains(replay_hash.as_ref().unwrap()));
    }

    #[tokio::test]
    async fn relay_decision_mailbox_capacity_replay_and_rejection_are_atomic() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let profile = signed_profile(NETWORK, &trusted, Vec::new());
        let relay = CollaborationDiscoveryRelayProvider::new(profile.clone());
        let now = current_timestamp();
        let local_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let local_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&local_key.to_bytes()),
            profile.clone(),
        );
        let local_profile = verified_discovery_profile(&local_key, "Local", Some("local"));
        let local_advertisement = local_authority
            .prepare_advertisement(&local_profile, now)
            .unwrap();
        let verified_local_advertisement =
            verify_collaboration_discovery_advertisement(&local_advertisement, &profile, now)
                .unwrap();
        relay
            .send_raw(&serde_json::json!({
                "op": "advertise",
                "advertisement": encode_bytes(&local_advertisement),
            }))
            .await
            .unwrap();

        let replay_requester_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let replay_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&replay_requester_key.to_bytes()),
            profile.clone(),
        );
        let replay_request = replay_authority
            .prepare_contact_request(
                &verified_local_advertisement,
                &verified_discovery_profile(&replay_requester_key, "Replay", Some("replay")),
                now + 1,
            )
            .unwrap();
        let replay_verified_request = verify_collaboration_contact_request(
            &replay_request,
            &profile,
            &verified_local_advertisement,
            now + 1,
        )
        .unwrap();
        let replay_request_hash = replay_verified_request
            .message()
            .envelope_sha256()
            .to_string();
        let replay_requester_profile_did =
            replay_verified_request.requester_profile_did().to_string();
        let replay_decision = local_authority
            .prepare_contact_decision_receipt(
                &replay_verified_request,
                &local_profile,
                CollaborationContactDecision::Accepted,
                now + 2,
            )
            .unwrap();

        let overflow_requester_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let overflow_authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&overflow_requester_key.to_bytes()),
            profile.clone(),
        );
        let overflow_request = overflow_authority
            .prepare_contact_request(
                &verified_local_advertisement,
                &verified_discovery_profile(&overflow_requester_key, "Overflow", Some("overflow")),
                now + 3,
            )
            .unwrap();
        let overflow_verified_request = verify_collaboration_contact_request(
            &overflow_request,
            &profile,
            &verified_local_advertisement,
            now + 3,
        )
        .unwrap();
        let overflow_request_hash = overflow_verified_request
            .message()
            .envelope_sha256()
            .to_string();
        let overflow_requester_profile_did = overflow_verified_request
            .requester_profile_did()
            .to_string();
        let overflow_decision = local_authority
            .prepare_contact_decision_receipt(
                &overflow_verified_request,
                &local_profile,
                CollaborationContactDecision::Declined,
                now + 4,
            )
            .unwrap();
        let recipient_profile_did = verified_local_advertisement.profile_did().to_string();

        {
            let mut state = relay.state.lock().unwrap();
            state.requests.insert(
                replay_request_hash.clone(),
                RelayContactRequest {
                    envelope_bytes: replay_request.clone(),
                    verified: replay_verified_request.clone(),
                    advertisement: CachedAdvertisement {
                        envelope_bytes: local_advertisement.clone(),
                        verified: verified_local_advertisement.clone(),
                    },
                },
            );
            state.requests.insert(
                overflow_request_hash.clone(),
                RelayContactRequest {
                    envelope_bytes: overflow_request.clone(),
                    verified: overflow_verified_request.clone(),
                    advertisement: CachedAdvertisement {
                        envelope_bytes: local_advertisement.clone(),
                        verified: verified_local_advertisement.clone(),
                    },
                },
            );
            state
                .request_mailboxes
                .entry(recipient_profile_did.clone())
                .or_default()
                .extend([replay_request_hash.clone(), overflow_request_hash.clone()]);
            state.decision_mailboxes.insert(
                replay_requester_profile_did.clone(),
                vec![RelayDecision {
                    envelope_bytes: replay_decision.clone(),
                    request_hash: replay_request_hash.clone(),
                }],
            );
            for index in 1..MAX_DISCOVERY_TOTAL_DECISION_MAILBOX_ENTRIES {
                let requester_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
                let requester_authority = CollaborationDiscoveryAuthority::new(
                    SigningKey::from_bytes(&requester_key.to_bytes()),
                    profile.clone(),
                );
                let requester_profile = verified_discovery_profile(
                    &requester_key,
                    &format!("Mailbox {index:02}"),
                    Some(&format!("mailbox-{index:02}")),
                );
                let request_now = now + 10 + u64::try_from(index).unwrap();
                let request_bytes = requester_authority
                    .prepare_contact_request(
                        &verified_local_advertisement,
                        &requester_profile,
                        request_now,
                    )
                    .unwrap();
                let verified_request = verify_collaboration_contact_request(
                    &request_bytes,
                    &profile,
                    &verified_local_advertisement,
                    request_now,
                )
                .unwrap();
                let request_hash = verified_request.message().envelope_sha256().to_string();
                let decision_bytes = local_authority
                    .prepare_contact_decision_receipt(
                        &verified_request,
                        &local_profile,
                        CollaborationContactDecision::Declined,
                        request_now + 1,
                    )
                    .unwrap();
                state.requests.insert(
                    request_hash.clone(),
                    RelayContactRequest {
                        envelope_bytes: request_bytes,
                        verified: verified_request.clone(),
                        advertisement: CachedAdvertisement {
                            envelope_bytes: local_advertisement.clone(),
                            verified: verified_local_advertisement.clone(),
                        },
                    },
                );
                state.decision_mailboxes.insert(
                    verified_request.requester_profile_did().to_string(),
                    vec![RelayDecision {
                        envelope_bytes: decision_bytes,
                        request_hash,
                    }],
                );
            }
            assert_eq!(
                total_decision_mailbox_entries(&state),
                MAX_DISCOVERY_TOTAL_DECISION_MAILBOX_ENTRIES
            );
        }

        relay
            .send_raw(&serde_json::json!({
                "op": "submit_contact_decision_receipt",
                "receipt": encode_bytes(&replay_decision),
            }))
            .await
            .unwrap();

        let error = relay
            .send_raw(&serde_json::json!({
                "op": "submit_contact_decision_receipt",
                "receipt": encode_bytes(&overflow_decision),
            }))
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("decision mailbox capacity is full"));

        let state = relay.state.lock().unwrap();
        assert_eq!(
            total_decision_mailbox_entries(&state),
            MAX_DISCOVERY_TOTAL_DECISION_MAILBOX_ENTRIES
        );
        assert!(state
            .decision_mailboxes
            .get(&replay_requester_profile_did)
            .unwrap()
            .iter()
            .any(|decision| decision.envelope_bytes == replay_decision));
        assert!(!state
            .decision_mailboxes
            .contains_key(&overflow_requester_profile_did));
        let request_mailbox = state.request_mailboxes.get(&recipient_profile_did).unwrap();
        assert!(!request_mailbox.contains(&replay_request_hash));
        assert!(request_mailbox.contains(&overflow_request_hash));
    }

    #[test]
    fn decode_response_is_stably_ordered_across_input_permutations() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let device_a = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device_b = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device_c = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority_a = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_a.to_bytes()),
            network_profile.clone(),
        );
        let authority_b = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_b.to_bytes()),
            network_profile.clone(),
        );
        let authority_c = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_c.to_bytes()),
            network_profile.clone(),
        );
        let ad_a = cached_advertisement(
            &authority_a,
            &verified_discovery_profile(&device_a, "Alpha", Some("alpha")),
            100,
        );
        let ad_b = cached_advertisement(
            &authority_b,
            &verified_discovery_profile(&device_b, "Beta", Some("beta")),
            101,
        );
        let ad_c = cached_advertisement(
            &authority_c,
            &verified_discovery_profile(&device_c, "Gamma", Some("gamma")),
            99,
        );
        let ordered_ids = |ads: &[CachedAdvertisement]| -> Vec<String> {
            decode_advertisement_response(
                &network_profile,
                &DiscoveryProviderAdvertisementResponse {
                    status: "ok".to_string(),
                    data: DiscoveryProviderAdvertisementData {
                        advertisements: ads
                            .iter()
                            .map(|ad| encode_bytes(&ad.envelope_bytes))
                            .collect(),
                    },
                },
                101,
            )
            .unwrap()
            .into_iter()
            .map(|ad| ad.verified.message().envelope_sha256().to_string())
            .collect()
        };
        let expected = ordered_ids(&[ad_a.clone(), ad_b.clone(), ad_c.clone()]);
        assert_eq!(
            ordered_ids(&[ad_c.clone(), ad_a.clone(), ad_b.clone()]),
            expected
        );
        assert_eq!(ordered_ids(&[ad_b, ad_c, ad_a]), expected);
    }

    #[test]
    fn decode_response_deduplicates_same_profile_revision_from_multiple_devices() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let shared_profile_signer = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device_a = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device_b = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let shared_profile = signed_profile_document_for_test(
            &shared_profile_signer,
            "Shared Person",
            Some("shared"),
            1,
            None,
            10,
            vec![
                encode_did_key(&device_a.verifying_key()),
                encode_did_key(&device_b.verifying_key()),
            ],
        )
        .unwrap();
        let authority_a = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_a.to_bytes()),
            network_profile.clone(),
        );
        let authority_b = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_b.to_bytes()),
            network_profile.clone(),
        );
        let ad_a = cached_advertisement(&authority_a, &shared_profile, 100);
        let ad_b = cached_advertisement(&authority_b, &shared_profile, 100);
        let expected = if compare_cached_advertisements(&ad_a, &ad_b).is_gt() {
            ad_a.verified.message().envelope_sha256().to_string()
        } else {
            ad_b.verified.message().envelope_sha256().to_string()
        };
        let response = DiscoveryProviderAdvertisementResponse {
            status: "ok".to_string(),
            data: DiscoveryProviderAdvertisementData {
                advertisements: vec![
                    encode_bytes(&ad_a.envelope_bytes),
                    encode_bytes(&ad_b.envelope_bytes),
                ],
            },
        };

        let advertisements =
            decode_advertisement_response(&network_profile, &response, 100).unwrap();
        assert_eq!(advertisements.len(), 1);
        assert_eq!(
            advertisements[0].verified.profile_did(),
            shared_profile.document().profile_did
        );
        assert_eq!(
            advertisements[0].verified.message().envelope_sha256(),
            expected
        );
    }

    #[test]
    fn decode_response_accepts_exact_duplicate_advertisements_idempotently() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device.to_bytes()),
            network_profile.clone(),
        );
        let advertisement = cached_advertisement(
            &authority,
            &verified_discovery_profile(&device, "Shared Person", Some("shared")),
            100,
        );
        let response = DiscoveryProviderAdvertisementResponse {
            status: "ok".to_string(),
            data: DiscoveryProviderAdvertisementData {
                advertisements: vec![
                    encode_bytes(&advertisement.envelope_bytes),
                    encode_bytes(&advertisement.envelope_bytes),
                ],
            },
        };

        let advertisements =
            decode_advertisement_response(&network_profile, &response, 100).unwrap();
        assert_eq!(advertisements.len(), 1);
        assert_eq!(
            advertisements[0].verified.message().envelope_sha256(),
            advertisement.verified.message().envelope_sha256()
        );
    }

    #[test]
    fn decode_response_keeps_same_display_name_for_distinct_profiles_separate() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let device_a = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device_b = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority_a = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_a.to_bytes()),
            network_profile.clone(),
        );
        let authority_b = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device_b.to_bytes()),
            network_profile.clone(),
        );
        let ad_a = cached_advertisement(
            &authority_a,
            &verified_discovery_profile(&device_a, "Same Name", Some("one")),
            100,
        );
        let ad_b = cached_advertisement(
            &authority_b,
            &verified_discovery_profile(&device_b, "Same Name", Some("two")),
            101,
        );
        let response = DiscoveryProviderAdvertisementResponse {
            status: "ok".to_string(),
            data: DiscoveryProviderAdvertisementData {
                advertisements: vec![
                    encode_bytes(&ad_a.envelope_bytes),
                    encode_bytes(&ad_b.envelope_bytes),
                ],
            },
        };

        let advertisements =
            decode_advertisement_response(&network_profile, &response, 101).unwrap();
        assert_eq!(advertisements.len(), 2);
        assert_eq!(
            advertisements
                .iter()
                .filter(|ad| ad.verified.display_name() == "Same Name")
                .count(),
            2
        );
        assert_ne!(
            advertisements[0].verified.profile_did(),
            advertisements[1].verified.profile_did()
        );
    }

    #[test]
    fn decode_response_rejects_raw_cardinality_over_limit_before_decoding() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let response = DiscoveryProviderAdvertisementResponse {
            status: "ok".to_string(),
            data: DiscoveryProviderAdvertisementData {
                advertisements: vec![
                    "not-even-base64".to_string();
                    MAX_DISCOVERY_QUERY_RESULTS + 1
                ],
            },
        };

        let error = match decode_advertisement_response(&network_profile, &response, 100) {
            Ok(_) => panic!("expected raw advertisement cardinality rejection"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("discovery query returned too many advertisements"));
    }

    #[test]
    fn observed_profile_heads_prevent_downgrade_after_visibility_gap() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let profile_signer = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device.to_bytes()),
            network_profile,
        );
        let revision_one = cached_advertisement(
            &authority,
            &verified_discovery_profile_with(
                &profile_signer,
                &device,
                "Downgrade Test",
                Some("v1"),
                1,
                None,
                10,
            ),
            10,
        );
        let revision_two = cached_advertisement(
            &authority,
            &verified_discovery_profile_with(
                &profile_signer,
                &device,
                "Downgrade Test",
                Some("v2"),
                2,
                Some(&revision_one.verified.profile_envelope_sha256().unwrap()),
                20,
            ),
            20,
        );
        let mut state = DiscoveryClientState {
            current_advertisement: None,
            visible_advertisements: BTreeMap::new(),
            observed_profile_heads: BTreeMap::new(),
            remote_visibility_may_remain_until: None,
            transport_available: false,
        };

        apply_decoded_advertisements(&mut state, vec![revision_two.clone()], true).unwrap();
        assert_eq!(state.visible_advertisements.len(), 1);
        apply_decoded_advertisements(&mut state, Vec::new(), true).unwrap();
        assert!(state.visible_advertisements.is_empty());
        apply_decoded_advertisements(&mut state, vec![revision_one], true).unwrap();
        assert!(state.visible_advertisements.is_empty());
        let head = state
            .observed_profile_heads
            .get(revision_two.verified.profile_did())
            .unwrap();
        assert_eq!(head.revision, 2);
        assert_eq!(
            head.profile_envelope_sha256,
            revision_two.verified.profile_envelope_sha256().unwrap()
        );
    }

    #[test]
    fn observed_profile_head_pruning_evicts_deterministically() {
        let mut heads = BTreeMap::new();
        for index in 0..=MAX_DISCOVERY_QUERY_RESULTS {
            heads.insert(
                format!("profile-{index:02}"),
                ObservedProfileHead {
                    revision: 1,
                    profile_envelope_sha256: format!("sha256:{index:064x}"),
                    observed_at: 10,
                },
            );
        }

        prune_observed_profile_heads(&mut heads);
        assert_eq!(heads.len(), MAX_DISCOVERY_QUERY_RESULTS);
        assert!(!heads.contains_key("profile-00"));
        assert!(heads.contains_key("profile-01"));
        assert!(heads.contains_key(&format!("profile-{:02}", MAX_DISCOVERY_QUERY_RESULTS)));
    }

    #[test]
    fn conflicting_equal_revision_response_is_atomic() {
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let network_profile = signed_profile(NETWORK, &trusted, Vec::new());
        let profile_signer = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let device = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let authority = CollaborationDiscoveryAuthority::new(
            SigningKey::from_bytes(&device.to_bytes()),
            network_profile,
        );
        let first = cached_advertisement(
            &authority,
            &verified_discovery_profile_with(
                &profile_signer,
                &device,
                "Conflict Test",
                Some("one"),
                1,
                None,
                10,
            ),
            10,
        );
        let conflicting = cached_advertisement(
            &authority,
            &verified_discovery_profile_with(
                &profile_signer,
                &device,
                "Conflict Test",
                Some("two"),
                1,
                None,
                11,
            ),
            11,
        );
        let mut state = DiscoveryClientState {
            current_advertisement: None,
            visible_advertisements: BTreeMap::new(),
            observed_profile_heads: BTreeMap::new(),
            remote_visibility_may_remain_until: None,
            transport_available: false,
        };

        let error =
            apply_decoded_advertisements(&mut state, vec![first, conflicting], true).unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting discovery profile revision for the same profile DID"));
        assert!(state.visible_advertisements.is_empty());
        assert!(state.observed_profile_heads.is_empty());
        assert!(!state.transport_available);
    }

    #[tokio::test]
    async fn discovery_status_is_disabled_by_default() {
        let temp = tempfile::tempdir().unwrap();
        let trusted = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_key = SigningKey::from_bytes(&generate_keypair().0.to_bytes());
        let seed_registry = Arc::new(ProviderRegistry::new());
        let seed_did = encode_did_key(&seed_key.verifying_key());
        let seed_node = start_carrier_node_with_registry(
            &seed_key,
            &seed_did,
            temp.path().join("seed"),
            Some(Arc::downgrade(&seed_registry)),
        )
        .await
        .unwrap();
        seed_registry
            .set_carrier_invoker(Arc::new(
                crate::carrier::CarrierProviderInvoker::with_carrier_endpoint(
                    seed_node.endpoint.clone(),
                ),
            ))
            .await;
        seed_registry
            .register_sub_provider(
                "peer",
                Arc::new(CarrierGossipProvider::new(seed_node.gossip_state.clone())),
            )
            .await
            .unwrap();
        let seed_ticket = ticket_from_peer_provider(&seed_registry).await;
        let seed_profile = signed_profile(NETWORK, &trusted, vec![seed_ticket]);
        let seed_provider: Arc<dyn Provider> = Arc::new(CollaborationDiscoveryRelayProvider::new(
            seed_profile.clone(),
        ));
        seed_registry.register(seed_provider).await;

        let (_registry, service, store, _node) = discovery_service(
            &temp.path().join("local"),
            &trusted,
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            vec![],
        )
        .await;

        let profile = service_profile(&service, "Local", Some("local"));
        let status = service
            .status(store.as_ref(), &profile, current_timestamp())
            .await
            .unwrap();
        assert!(!status.enabled());
        assert!(status.visible_people().is_empty());
        assert!(status.incoming_requests().is_empty());
    }
}
