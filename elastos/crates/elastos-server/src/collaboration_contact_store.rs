//! Durable local contact authority derived from signed discovery chains.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Context;
use elastos_common::collaboration_protocol::{
    canonical_signed_collaboration_message_bytes, collaboration_message_envelope_sha256,
};
use elastos_common::localhost::rooted_localhost_fs_path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::{
    load_principal_root_protection, read_principal_root_object,
    write_protected_principal_root_object,
};
use crate::collaboration_core::{
    ensure_owner_only_directory, validate_owner_only_regular_file, ExclusiveFileLock,
};
use crate::collaboration_discovery::{
    bind_stored_collaboration_contact_request,
    canonical_signed_collaboration_contact_decision_receipt_bytes,
    verify_collaboration_contact_decision_receipt, verify_collaboration_contact_request,
    verify_collaboration_discovery_advertisement,
    verify_stored_collaboration_contact_decision_receipt,
    verify_stored_collaboration_contact_revocation,
    verify_stored_collaboration_discovery_advertisement,
    verify_stored_unbound_collaboration_contact_request, CollaborationContactDecision,
    SignedCollaborationContactDecisionReceipt, VerifiedCollaborationContactDecisionReceipt,
    VerifiedCollaborationContactRequest, VerifiedCollaborationContactRevocation,
    VerifiedCollaborationDiscoveryAdvertisement, COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE,
    COLLABORATION_DISCOVERY_SERVICE, MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES,
};
use crate::collaboration_network::{validate_network_id, VerifiedCollaborationNetworkProfile};
use crate::collaboration_profile_authority::{
    VerifiedCollaborationProfileDocument, MAX_RETAINED_PROFILE_REVISIONS,
};
use crate::collaboration_protocol::validate_id;
use crate::crypto::{decode_did_key, encode_did_key};

const CONTACT_STORE_SCHEMA: &str = "elastos.people.contact-store/v1";
/// Removed relationships stay visible and keep their retained head, so they
/// share the head budget: accepted plus removed together stay within
/// `MAX_ACCEPTED_PROFILE_HEADS`.
const MAX_REMOVED_CONTACTS: usize = MAX_ACCEPTED_PROFILE_HEADS;
const MAX_REVOCATION_ENVELOPE_BYTES: usize = 8 * 1024;
const CONTACT_STORE_LOCK_FILE: &str = ".contact-state.lock";
const DIRECT_CONVERSATION_DOMAIN: &[u8] = b"elastos.people.direct-conversation.v1";
const PEOPLE_CONTACT_STATE_OBJECT_PATH: &str = ".AppData/ElastOS/People/contact-state.json";

const MAX_CONTACT_STORE_STATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_ADVERTISEMENT_RECORDS: usize = 128;
const MAX_ADVERTISEMENT_BYTES: usize = 512 * 1024;
const MAX_ADVERTISEMENTS_PER_SENDER: usize = 8;
const MAX_REQUEST_RECORDS: usize = 128;
const MAX_REQUEST_BYTES: usize = 512 * 1024;
const MAX_REQUESTS_PER_SENDER: usize = 16;
const MAX_RECEIPT_RECORDS: usize = 128;
const MAX_RECEIPT_BYTES: usize = 512 * 1024;
const MAX_RECEIPTS_PER_SENDER: usize = 16;
const MAX_REVOKED_REQUEST_HASHES: usize = 128;
const MAX_ACCEPTED_PROFILE_HEADS: usize = 64;
const MAX_ACCEPTED_PROFILE_HEAD_BYTES: usize = 128 * 1024;

pub(crate) struct CollaborationContactStore {
    data_root: PathBuf,
    principal_id: String,
    localhost_root: String,
    profile: VerifiedCollaborationNetworkProfile,
    local_profile_did: String,
    local_device_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationAcceptedContact {
    remote_profile_did: String,
    remote_presence_device_did: String,
    remote_display_name: String,
    remote_handle: Option<String>,
    added_at: u64,
    conversation_id: String,
}

impl CollaborationAcceptedContact {
    pub(crate) fn remote_profile_did(&self) -> &str {
        &self.remote_profile_did
    }

    pub(crate) fn remote_display_name(&self) -> &str {
        &self.remote_display_name
    }

    pub(crate) fn remote_presence_device_did(&self) -> &str {
        &self.remote_presence_device_did
    }

    pub(crate) fn remote_handle(&self) -> Option<&str> {
        self.remote_handle.as_deref()
    }

    pub(crate) fn added_at(&self) -> u64 {
        self.added_at
    }

    pub(crate) fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
}

impl CollaborationContactStore {
    pub(crate) fn local_profile_did(&self) -> &str {
        &self.local_profile_did
    }

    pub(crate) fn local_device_did(&self) -> &str {
        &self.local_device_did
    }

    pub(crate) fn principal_id(&self) -> &str {
        &self.principal_id
    }

    pub(crate) fn localhost_root(&self) -> &str {
        &self.localhost_root
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationContactStoreSnapshot {
    contacts: Vec<CollaborationAcceptedContact>,
    removed: Vec<CollaborationRemovedContact>,
}

/// An ended relationship, kept visible on both sides. The retained Profile
/// head remains the signed source of its display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationRemovedContact {
    remote_profile_did: String,
    remote_presence_device_did: String,
    display_name: String,
    handle: Option<String>,
    conversation_id: String,
    removed_at: u64,
    removed_by_local: bool,
}

impl CollaborationRemovedContact {
    pub(crate) fn remote_profile_did(&self) -> &str {
        &self.remote_profile_did
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    pub(crate) fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    pub(crate) fn removed_at(&self) -> u64 {
        self.removed_at
    }

    pub(crate) fn removed_by_local(&self) -> bool {
        self.removed_by_local
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingIncomingContactRequest {
    request_hash: String,
    requester_profile_did: String,
    display_name: String,
    handle: Option<String>,
    created_at: u64,
    expires_at: u64,
}

/// A pre-contact relationship state projected from signed truth: an
/// unexpired outgoing request with no decision (`requested`), or a chain
/// whose terminal decision was Declined (`declined`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollaborationRelationshipEvent {
    pub(crate) remote_profile_did: String,
    pub(crate) display_name: String,
    pub(crate) handle: Option<String>,
    pub(crate) occurred_at: u64,
}

impl CollaborationContactStoreSnapshot {
    pub(crate) fn contacts(&self) -> &[CollaborationAcceptedContact] {
        &self.contacts
    }

    pub(crate) fn removed(&self) -> &[CollaborationRemovedContact] {
        &self.removed
    }
}

impl PendingIncomingContactRequest {
    pub(crate) fn request_hash(&self) -> &str {
        &self.request_hash
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn created_at(&self) -> u64 {
        self.created_at
    }

    pub(crate) fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub(crate) fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn test_new(
        request_hash: impl Into<String>,
        requester_profile_did: impl Into<String>,
        display_name: impl Into<String>,
        handle: Option<String>,
        created_at: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            request_hash: request_hash.into(),
            requester_profile_did: requester_profile_did.into(),
            display_name: display_name.into(),
            handle,
            created_at,
            expires_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContactStoreWrite {
    Recorded,
    Replayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactStoreState {
    schema: String,
    binding: ContactStoreBinding,
    discovery_enabled: bool,
    published_local_advertisement_envelope_sha256: Option<String>,
    advertisements: Vec<StoredEnvelope>,
    requests: Vec<StoredEnvelope>,
    decisions: Vec<StoredEnvelope>,
    revoked_request_hashes: Vec<String>,
    /// Latest verified signed Profile document per accepted contact, so a
    /// rename or endpoint change is retained without a second approval.
    accepted_profile_heads: Vec<StoredAcceptedProfileHead>,
    /// Ended relationships. Removal is symmetric and visible: both sides keep
    /// the pair in a removed state rather than letting it vanish, and the
    /// signed revocation envelope is the durable artifact for it.
    removed_contacts: Vec<StoredRemovedContact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRemovedContact {
    /// Canonical signed revocation envelope — self-describing: pair, verb,
    /// and removal time all live inside it.
    revocation_envelope: String,
    /// Whether this side initiated the removal. Drives honest product copy
    /// ("you removed" vs "removed you") and delivery direction.
    removed_by_local: bool,
    /// For a local removal: the peer's device acknowledged the revocation.
    /// Remote removals are settled by construction.
    revocation_settled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAcceptedProfileHead {
    /// Canonical signed profile document JSON.
    signed_profile: String,
    /// Device that delivered this head. It must be authorized by the head
    /// itself, and it becomes the contact's delivery endpoint.
    delivered_by_device_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContactStoreBinding {
    network_id: String,
    local_profile_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEnvelope {
    envelope: String,
}

struct LoadedState {
    state: ContactStoreState,
    advertisements: HashMap<String, VerifiedCollaborationDiscoveryAdvertisement>,
    requests: HashMap<String, VerifiedCollaborationContactRequest>,
    decisions: HashMap<String, VerifiedCollaborationContactDecisionReceipt>,
    revoked_request_hashes: HashSet<String>,
    /// Verified head per accepted Profile DID, with the device that delivered
    /// it and is therefore the contact's current delivery endpoint.
    accepted_profile_heads: HashMap<String, (VerifiedCollaborationProfileDocument, String)>,
    /// Verified removed relationship per remote Profile DID.
    removed_contacts: HashMap<String, LoadedRemovedContact>,
}

#[derive(Clone)]
struct LoadedRemovedContact {
    revocation: VerifiedCollaborationContactRevocation,
    removed_by_local: bool,
    revocation_settled: bool,
}

#[derive(Clone)]
struct AcceptedChain {
    request_hash: String,
    request: VerifiedCollaborationContactRequest,
    advertisement: VerifiedCollaborationDiscoveryAdvertisement,
    receipt: VerifiedCollaborationContactDecisionReceipt,
}

#[derive(Clone)]
struct RemotePresentation {
    remote_profile_did: String,
    remote_presence_device_did: String,
    display_name: String,
    handle: Option<String>,
    decided_at: u64,
    profile_revision: u64,
    profile_envelope_hash: String,
    event_created_at: u64,
    event_envelope_hash: String,
}

impl CollaborationContactStore {
    pub(crate) fn new(
        data_root: &Path,
        principal_id: &str,
        localhost_root: &str,
        profile: VerifiedCollaborationNetworkProfile,
        local_profile: &VerifiedCollaborationProfileDocument,
        local_device_did: &str,
    ) -> anyhow::Result<Self> {
        if principal_id.trim().is_empty() {
            anyhow::bail!("principal contact store principal_id is required");
        }
        if localhost_root.trim().is_empty() {
            anyhow::bail!("principal contact store localhost_root is required");
        }
        validate_canonical_did(local_device_did, "local contact store device DID")?;
        let local_profile_did = local_profile.document().profile_did.as_str();
        validate_canonical_did(local_profile_did, "local contact store profile DID")?;
        if !local_profile.authorizes_endpoint(local_device_did) {
            anyhow::bail!("local contact store profile does not authorize the current device DID");
        }
        Ok(Self {
            data_root: data_root.to_path_buf(),
            principal_id: principal_id.to_string(),
            localhost_root: localhost_root.to_string(),
            profile,
            local_profile_did: local_profile_did.to_string(),
            local_device_did: local_device_did.to_string(),
        })
    }

    pub(crate) fn snapshot(&self) -> anyhow::Result<CollaborationContactStoreSnapshot> {
        let Some(loaded) = self.load_state()? else {
            return Ok(CollaborationContactStoreSnapshot {
                contacts: Vec::new(),
                removed: Vec::new(),
            });
        };
        Ok(CollaborationContactStoreSnapshot {
            contacts: derive_contacts(&loaded, &self.local_profile_did)?,
            removed: derive_removed_contacts(&loaded, &self.local_profile_did)?,
        })
    }

    pub(crate) fn accepted_profile(
        &self,
        profile_did: &str,
    ) -> anyhow::Result<Option<VerifiedCollaborationProfileDocument>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(None);
        };
        if !derive_contacts(&loaded, &self.local_profile_did)?
            .iter()
            .any(|contact| contact.remote_profile_did() == profile_did)
        {
            return Ok(None);
        }
        let Some((_, canonical)) =
            current_profile_presentation(&loaded, &self.local_profile_did)?.remove(profile_did)
        else {
            anyhow::bail!("accepted contact has no signed Profile presentation");
        };
        let signed = serde_json::from_str(&canonical)
            .context("accepted contact Profile presentation is invalid")?;
        Ok(Some(
            crate::collaboration_profile_authority::verify_signed_profile_document(&signed)?,
        ))
    }

    /// Missing state is intentionally Discovery-off and remains read-only.
    pub(crate) fn discovery_enabled(&self) -> anyhow::Result<bool> {
        Ok(self
            .load_state()?
            .is_some_and(|loaded| loaded.state.discovery_enabled))
    }

    /// Only an explicit People mutation may persist Discovery intent.
    pub(crate) fn set_discovery_enabled(&self, enabled: bool, now: u64) -> anyhow::Result<()> {
        self.with_mutation(now, |loaded| {
            let changed = loaded.state.discovery_enabled != enabled;
            loaded.state.discovery_enabled = enabled;
            Ok(((), changed))
        })
    }

    /// Returns the exact locally published advertisement pending either normal
    /// visibility or bounded withdrawal settlement. It never creates or renews.
    pub(crate) fn published_local_advertisement(
        &self,
        now: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(None);
        };
        let Some(published_hash) = loaded.state.published_local_advertisement_envelope_sha256
        else {
            return Ok(None);
        };
        let advertisement = loaded
            .advertisements
            .get(&published_hash)
            .ok_or_else(|| anyhow::anyhow!("published local discovery advertisement is missing"))?;
        if advertisement.profile_did() != self.local_profile_did {
            anyhow::bail!("published local discovery advertisement belongs to another profile");
        }
        if advertisement.message().envelope().payload.expires_at <= now {
            return Ok(None);
        }
        Ok(Some(canonical_signed_collaboration_message_bytes(
            advertisement.message().envelope(),
        )?))
    }

    /// Clears an expired published/pending advertisement pointer without
    /// deleting immutable request or decision evidence which still references
    /// that advertisement. Missing state remains read-only.
    pub(crate) fn clear_expired_published_local_advertisement(
        &self,
        now: u64,
    ) -> anyhow::Result<bool> {
        let Some(loaded) = self.load_state()? else {
            return Ok(false);
        };
        let Some(published_hash) = loaded
            .state
            .published_local_advertisement_envelope_sha256
            .as_deref()
        else {
            return Ok(false);
        };
        let is_expired = loaded
            .advertisements
            .get(published_hash)
            .is_some_and(|advertisement| {
                advertisement.message().envelope().payload.expires_at <= now
            });
        if !is_expired {
            return Ok(false);
        }
        let published_hash = published_hash.to_string();
        self.with_mutation(now, |loaded| {
            if loaded
                .state
                .published_local_advertisement_envelope_sha256
                .as_deref()
                != Some(published_hash.as_str())
            {
                return Ok((false, false));
            }
            let still_expired =
                loaded
                    .advertisements
                    .get(&published_hash)
                    .is_some_and(|advertisement| {
                        advertisement.message().envelope().payload.expires_at <= now
                    });
            if !still_expired {
                return Ok((false, false));
            }
            loaded.state.published_local_advertisement_envelope_sha256 = None;
            Ok((true, true))
        })
    }

    /// Settles one locally-created discovery advertisement after its exact
    /// signed withdrawal has reached the relay. This clears only the published
    /// pointer; request/decision evidence remains immutable.
    pub(crate) fn settle_published_local_advertisement_withdrawal(
        &self,
        advertisement_envelope_sha256: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        let advertisement_envelope_sha256 = advertisement_envelope_sha256.trim();
        if advertisement_envelope_sha256.is_empty() {
            anyhow::bail!("discovery advertisement hash is required");
        }
        self.with_mutation(now, |loaded| {
            match loaded
                .state
                .published_local_advertisement_envelope_sha256
                .as_deref()
            {
                None => return Ok(((), false)),
                Some(published) if published == advertisement_envelope_sha256 => {}
                Some(_) => {
                    anyhow::bail!("discovery withdrawal does not match the published advertisement")
                }
            }
            let advertisement = loaded
                .advertisements
                .get(advertisement_envelope_sha256)
                .ok_or_else(|| {
                    anyhow::anyhow!("published local discovery advertisement is missing")
                })?;
            if advertisement.profile_did() != self.local_profile_did {
                anyhow::bail!("published local discovery advertisement belongs to another profile");
            }
            loaded.state.published_local_advertisement_envelope_sha256 = None;
            Ok(((), true))
        })
    }

    /// Outgoing requests still waiting for the other side: signed by this
    /// profile, unexpired, undecided. These are the People "requested" rows.
    pub(crate) fn outgoing_pending_requests(
        &self,
        now: u64,
    ) -> anyhow::Result<Vec<CollaborationRelationshipEvent>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        let mut requested = Vec::new();
        for request in loaded.requests.values() {
            let envelope = &request.message().envelope().payload;
            if request.requester_profile_did() != self.local_profile_did
                || envelope.expires_at <= now
                || loaded
                    .decisions
                    .contains_key(request.message().envelope_sha256())
                || loaded
                    .revoked_request_hashes
                    .contains(request.message().envelope_sha256())
            {
                continue;
            }
            let Some(advertisement) = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
            else {
                continue;
            };
            requested.push(CollaborationRelationshipEvent {
                remote_profile_did: advertisement.profile_did().to_string(),
                display_name: advertisement.display_name().to_string(),
                handle: advertisement.handle().map(str::to_string),
                occurred_at: envelope.created_at,
            });
        }
        requested.sort_by(|left, right| {
            left.occurred_at
                .cmp(&right.occurred_at)
                .then_with(|| left.remote_profile_did.cmp(&right.remote_profile_did))
        });
        Ok(requested)
    }

    /// Chains whose terminal decision was Declined, in either direction.
    /// These are the People "declined" rows; a pair later accepted or removed
    /// is not declined.
    pub(crate) fn declined_relationships(
        &self,
    ) -> anyhow::Result<Vec<CollaborationRelationshipEvent>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        let mut current_state: HashMap<String, CollaborationRelationshipEvent> = HashMap::new();
        let mut settled: HashSet<String> = HashSet::new();
        for chain in accepted_chains(&loaded)? {
            settled
                .insert(remote_presentation(&chain, &self.local_profile_did)?.remote_profile_did);
        }
        settled.extend(loaded.removed_contacts.keys().cloned());
        for (request_hash, decision) in &loaded.decisions {
            if decision.envelope().payload.decision != CollaborationContactDecision::Declined {
                continue;
            }
            let Some(request) = loaded.requests.get(request_hash) else {
                continue;
            };
            let Some(advertisement) = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
            else {
                continue;
            };
            let (remote_profile_did, display_name, handle) =
                if request.requester_profile_did() == self.local_profile_did {
                    (
                        advertisement.profile_did().to_string(),
                        advertisement.display_name().to_string(),
                        advertisement.handle().map(str::to_string),
                    )
                } else {
                    (
                        request.requester_profile_did().to_string(),
                        request.display_name().to_string(),
                        request.handle().map(str::to_string),
                    )
                };
            if settled.contains(&remote_profile_did) {
                continue;
            }
            let event = CollaborationRelationshipEvent {
                remote_profile_did: remote_profile_did.clone(),
                display_name,
                handle,
                occurred_at: decision.envelope().payload.decided_at,
            };
            match current_state.get(&remote_profile_did) {
                Some(existing) if existing.occurred_at >= event.occurred_at => {}
                _ => {
                    current_state.insert(remote_profile_did, event);
                }
            }
        }
        let mut declined: Vec<_> = current_state.into_values().collect();
        declined.sort_by(|left, right| {
            left.occurred_at
                .cmp(&right.occurred_at)
                .then_with(|| left.remote_profile_did.cmp(&right.remote_profile_did))
        });
        Ok(declined)
    }

    pub(crate) fn pending_incoming_requests(
        &self,
    ) -> anyhow::Result<Vec<PendingIncomingContactRequest>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        derive_pending_incoming_requests(&loaded, &self.local_profile_did)
    }

    pub(crate) fn pending_incoming_request(
        &self,
        request_hash: &str,
        now: u64,
    ) -> anyhow::Result<Option<VerifiedCollaborationContactRequest>> {
        validate_sha256_label(request_hash, "pending discovery request hash")?;
        let Some(loaded) = self.load_state()? else {
            return Ok(None);
        };
        if loaded.revoked_request_hashes.contains(request_hash)
            || loaded.decisions.contains_key(request_hash)
        {
            return Ok(None);
        }
        let Some(request) = loaded.requests.get(request_hash).cloned() else {
            return Ok(None);
        };
        let message = &request.message().envelope().payload;
        let advertisement = loaded
            .advertisements
            .get(request.advertisement_envelope_sha256())
            .ok_or_else(|| anyhow::anyhow!("bound discovery advertisement is missing"))?;
        if advertisement.profile_did() != self.local_profile_did {
            return Ok(None);
        }
        if message.created_at
            > now.saturating_add(
                elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS,
            )
            || message.expires_at <= now
        {
            return Ok(None);
        }
        Ok(Some(request))
    }

    pub(crate) fn stored_contact_decision_receipt(
        &self,
        request_hash: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        validate_sha256_label(request_hash, "stored discovery request hash")?;
        let Some(loaded) = self.load_state()? else {
            return Ok(None);
        };
        if loaded.revoked_request_hashes.contains(request_hash) {
            return Ok(None);
        }
        let Some(request) = loaded.requests.get(request_hash) else {
            return Ok(None);
        };
        let advertisement = loaded
            .advertisements
            .get(request.advertisement_envelope_sha256())
            .ok_or_else(|| anyhow::anyhow!("bound discovery advertisement is missing"))?;
        if advertisement.profile_did() != self.local_profile_did {
            anyhow::bail!("stored discovery request recipient does not match this profile");
        }
        let Some(receipt) = loaded.decisions.get(request_hash) else {
            return Ok(None);
        };
        Ok(Some(
            canonical_signed_collaboration_contact_decision_receipt_bytes(receipt.envelope())?,
        ))
    }

    pub(crate) fn stored_outgoing_contact_request(
        &self,
        advertisement_hash: &str,
        recipient_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        validate_sha256_label(advertisement_hash, "discovery advertisement hash")?;
        validate_canonical_did(
            recipient_profile_did,
            "outgoing discovery recipient profile DID",
        )?;
        let Some(loaded) = self.load_state()? else {
            return Ok(None);
        };
        let mut matching =
            loaded.requests.values().filter(|request| {
                let Some(advertisement) = loaded
                    .advertisements
                    .get(request.advertisement_envelope_sha256())
                else {
                    return false;
                };
                let envelope = &request.message().envelope().payload;
                request.requester_profile_did() == self.local_profile_did
                && advertisement.profile_did() == recipient_profile_did
                && request.advertisement_envelope_sha256() == advertisement_hash
                && envelope.created_at
                    <= now.saturating_add(
                        elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS,
                    )
                && envelope.expires_at > now
                && !loaded
                    .revoked_request_hashes
                    .contains(request.message().envelope_sha256())
                && !loaded.decisions.contains_key(request.message().envelope_sha256())
            });
        let Some(request) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            anyhow::bail!("stored discovery outgoing request state is ambiguous");
        }
        Ok(Some(canonical_signed_collaboration_message_bytes(
            request.message().envelope(),
        )?))
    }

    pub(crate) fn resendable_outgoing_contact_requests(
        &self,
        now: u64,
        limit: usize,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        let mut resendable = loaded
            .requests
            .values()
            .filter(|request| {
                let envelope = &request.message().envelope().payload;
                request.requester_profile_did() == self.local_profile_did
                    && envelope.created_at
                        <= now.saturating_add(
                            elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS,
                        )
                    && envelope.expires_at > now
                    && !loaded
                        .revoked_request_hashes
                        .contains(request.message().envelope_sha256())
                    && !loaded.decisions.contains_key(request.message().envelope_sha256())
                    && loaded
                        .advertisements
                        .contains_key(request.advertisement_envelope_sha256())
            })
            .collect::<Vec<_>>();
        resendable.sort_by(|left, right| {
            let left_payload = &left.message().envelope().payload;
            let right_payload = &right.message().envelope().payload;
            left_payload
                .created_at
                .cmp(&right_payload.created_at)
                .then_with(|| {
                    left.message()
                        .envelope_sha256()
                        .cmp(right.message().envelope_sha256())
                })
        });
        resendable
            .into_iter()
            .take(limit)
            .map(|request| {
                canonical_signed_collaboration_message_bytes(request.message().envelope())
                    .map_err(anyhow::Error::from)
            })
            .collect()
    }

    pub(crate) fn resendable_contact_decisions(
        &self,
        now: u64,
        limit: usize,
    ) -> anyhow::Result<Vec<Vec<u8>>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        let mut resendable = loaded
            .decisions
            .iter()
            .filter_map(|(request_hash, decision)| {
                let request = loaded.requests.get(request_hash)?;
                let request_payload = &request.message().envelope().payload;
                let decision_payload = &decision.envelope().payload;
                if loaded.revoked_request_hashes.contains(request_hash) {
                    return None;
                }
                if decision_payload.recipient_profile_did != self.local_profile_did {
                    return None;
                }
                if request_payload.created_at
                    > now.saturating_add(
                        elastos_common::collaboration_protocol::MAX_COLLABORATION_CLOCK_SKEW_SECS,
                    )
                    || request_payload.expires_at <= now
                {
                    return None;
                }
                Some((request_hash.as_str(), decision))
            })
            .collect::<Vec<_>>();
        resendable.sort_by(|(left_hash, left), (right_hash, right)| {
            left.envelope()
                .payload
                .decided_at
                .cmp(&right.envelope().payload.decided_at)
                .then_with(|| left_hash.cmp(right_hash))
        });
        resendable
            .into_iter()
            .take(limit)
            .map(|(_, decision)| {
                canonical_signed_collaboration_contact_decision_receipt_bytes(decision.envelope())
                    .map_err(anyhow::Error::from)
            })
            .collect()
    }

    pub(crate) fn store_local_advertisement(
        &self,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        let advertisement =
            verify_collaboration_discovery_advertisement(envelope_bytes, &self.profile, now)?;
        if advertisement
            .message()
            .envelope()
            .payload
            .sender_profile_did
            != self.local_profile_did
            || advertisement.message().envelope().signer_did != self.local_device_did
        {
            anyhow::bail!("local discovery advertisement authority does not match this Runtime");
        }
        if advertisement.profile_did() != self.local_profile_did {
            anyhow::bail!("local discovery advertisement profile does not match this profile");
        }
        self.with_mutation(now, |loaded| {
            let inserted =
                persist_advertisement_if_missing(loaded, envelope_bytes, advertisement.clone())?;
            let published_hash = advertisement.message().envelope_sha256().to_string();
            let pointer_changed = loaded
                .state
                .published_local_advertisement_envelope_sha256
                .as_deref()
                != Some(published_hash.as_str());
            loaded.state.published_local_advertisement_envelope_sha256 = Some(published_hash);
            Ok((
                if inserted {
                    ContactStoreWrite::Recorded
                } else {
                    ContactStoreWrite::Replayed
                },
                inserted || pointer_changed,
            ))
        })
    }

    pub(crate) fn record_outgoing_contact_request(
        &self,
        request_bytes: &[u8],
        remote_advertisement_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        let advertisement = verify_collaboration_discovery_advertisement(
            remote_advertisement_bytes,
            &self.profile,
            now,
        )?;
        if advertisement
            .message()
            .envelope()
            .payload
            .sender_profile_did
            == self.local_profile_did
        {
            anyhow::bail!(
                "outgoing contact request cannot target the local discovery advertisement"
            );
        }
        let request = verify_collaboration_contact_request(
            request_bytes,
            &self.profile,
            &advertisement,
            now,
        )?;
        if request.message().envelope().signer_did != self.local_device_did {
            anyhow::bail!("outgoing contact request signer is not this Runtime endpoint");
        }
        if request.requester_profile_did() != self.local_profile_did {
            anyhow::bail!("outgoing contact request sender profile does not match this profile");
        }
        self.with_mutation(now, |loaded| {
            let _inserted_advertisement = persist_advertisement_if_missing(
                loaded,
                remote_advertisement_bytes,
                advertisement.clone(),
            )?;
            if loaded
                .revoked_request_hashes
                .contains(request.message().envelope_sha256())
            {
                return Ok((ContactStoreWrite::Replayed, false));
            }
            let inserted_request = persist_request_if_missing(loaded, request_bytes, &request)?;
            Ok((
                if inserted_request {
                    ContactStoreWrite::Recorded
                } else {
                    ContactStoreWrite::Replayed
                },
                inserted_request,
            ))
        })
    }

    pub(crate) fn record_incoming_contact_request(
        &self,
        request_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        let unbound_request =
            verify_stored_unbound_collaboration_contact_request(request_bytes, &self.profile)?;
        self.with_mutation(now, |loaded| {
            let advertisement = loaded
                .advertisements
                .get(unbound_request.advertisement_envelope_sha256())
                .ok_or_else(|| anyhow::anyhow!("bound local discovery advertisement is missing"))?
                .clone();
            let request = verify_collaboration_contact_request(
                request_bytes,
                &self.profile,
                &advertisement,
                now,
            )?;
            if advertisement.profile_did() != self.local_profile_did {
                anyhow::bail!(
                    "incoming contact request advertisement profile does not match this profile"
                );
            }
            if loaded
                .revoked_request_hashes
                .contains(request.message().envelope_sha256())
            {
                return Ok((ContactStoreWrite::Replayed, false));
            }
            if request.message().envelope().payload.recipient.id != self.local_profile_did {
                anyhow::bail!("incoming contact request recipient does not match this Profile");
            }
            let inserted = persist_request_if_missing(loaded, request_bytes, &request)?;
            Ok((
                if inserted {
                    ContactStoreWrite::Recorded
                } else {
                    ContactStoreWrite::Replayed
                },
                inserted,
            ))
        })
    }

    pub(crate) fn record_contact_decision_receipt(
        &self,
        receipt_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        if receipt_bytes.len() > MAX_COLLABORATION_CONTACT_DECISION_RECEIPT_BYTES {
            anyhow::bail!("discovery contact decision receipt exceeds the protocol byte limit");
        }
        self.with_mutation(now, |loaded| {
            let raw_receipt: SignedCollaborationContactDecisionReceipt =
                serde_json::from_slice(receipt_bytes)
                    .context("invalid discovery contact decision receipt envelope")?;
            if loaded
                .revoked_request_hashes
                .contains(&raw_receipt.payload.request_envelope_sha256)
            {
                return Ok((ContactStoreWrite::Replayed, false));
            }
            let request = loaded
                .requests
                .get(&raw_receipt.payload.request_envelope_sha256)
                .ok_or_else(|| anyhow::anyhow!("decided discovery request is missing"))?
                .clone();
            let advertisement = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
                .ok_or_else(|| anyhow::anyhow!("bound discovery advertisement is missing"))?
                .clone();
            let verified = verify_collaboration_contact_decision_receipt(
                receipt_bytes,
                &request,
                &advertisement,
                now,
            )?;
            let accepted =
                verified.envelope().payload.decision == CollaborationContactDecision::Accepted;
            let counterpart =
                if verified.envelope().payload.requester_profile_did == self.local_profile_did {
                    verified.envelope().payload.recipient_profile_did.clone()
                } else {
                    verified.envelope().payload.requester_profile_did.clone()
                };
            let inserted = persist_receipt_if_missing(
                loaded,
                receipt_bytes,
                request.message().envelope_sha256(),
                verified,
            )?;
            // A fresh acceptance reopens a removed relationship: the removed
            // record retires and the pair is a contact again. Re-adding needs
            // no special verb — a new request through Inbox is the path, and
            // this is where its acceptance lands.
            let mut reopened = false;
            if inserted && accepted && loaded.removed_contacts.remove(&counterpart).is_some() {
                let profile = self.profile.clone();
                loaded.state.removed_contacts.retain(|stored| {
                    verify_stored_collaboration_contact_revocation(
                        stored.revocation_envelope.as_bytes(),
                        &profile,
                    )
                    .map(|revocation| {
                        revocation.revoked_profile_did() != counterpart
                            && revocation.revoking_profile_did() != counterpart
                    })
                    .unwrap_or(true)
                });
                reopened = true;
            }
            Ok((
                if inserted {
                    ContactStoreWrite::Recorded
                } else {
                    ContactStoreWrite::Replayed
                },
                inserted || reopened,
            ))
        })
    }

    /// Applies a signed Profile chain segment for an already accepted contact.
    ///
    /// The Profile signature over an exact revision chain is the authority; the
    /// delivering endpoint only proves the route, so it must be authorized by
    /// the signed head being applied. This lets a Profile's signed revision
    /// chain move delivery to a replacement device even when the old device is
    /// gone. A byte-identical replay is idempotent. A same
    /// revision with different bytes, a rollback, a gap the segment cannot
    /// bridge, a profile that is not an accepted contact, an unauthorized
    /// device, or malformed data all fail closed without changing stored state.
    pub(crate) fn apply_accepted_profile_chain(
        &self,
        signed_profiles: &[Vec<u8>],
        delivered_by_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        if signed_profiles.is_empty()
            || signed_profiles.len() > MAX_RETAINED_PROFILE_REVISIONS
            || signed_profiles.iter().map(Vec::len).sum::<usize>() > MAX_ACCEPTED_PROFILE_HEAD_BYTES
        {
            anyhow::bail!("accepted profile chain has an invalid size");
        }
        validate_canonical_did(
            delivered_by_endpoint_did,
            "accepted profile source endpoint",
        )?;

        // Verify every step, then require the segment itself to be one exact
        // contiguous chain for a single profile.
        let mut steps: Vec<(u64, String, VerifiedCollaborationProfileDocument)> = Vec::new();
        for bytes in signed_profiles {
            let signed: crate::collaboration_profile_authority::SignedCollaborationProfileDocument =
                serde_json::from_slice(bytes).context("invalid accepted profile envelope")?;
            let canonical = serde_json::to_string(&signed)?;
            let verified =
                crate::collaboration_profile_authority::verify_signed_profile_document(&signed)?;
            steps.push((verified.document().revision, canonical, verified));
        }
        let profile_did = steps[0].2.document().profile_did.clone();
        for window in steps.windows(2) {
            if window[1].2.document().profile_did != profile_did {
                anyhow::bail!("accepted profile chain mixes profiles");
            }
            if window[1].0 != window[0].0.saturating_add(1) {
                anyhow::bail!("accepted profile chain is not contiguous");
            }
            if window[1].2.document().previous_profile_sha256.as_deref()
                != Some(sha256_label_bytes(window[0].1.as_bytes()).as_str())
            {
                anyhow::bail!("accepted profile chain breaks the chain hash");
            }
            crate::collaboration_profile_authority::validate_profile_authority_transition(
                window[0].2.document(),
                window[1].2.document(),
            )?;
        }
        let head = steps.last().expect("verified chain is not empty");
        if head.2.sole_endpoint_did()? != delivered_by_endpoint_did {
            anyhow::bail!("profile update did not arrive from the signed Profile endpoint");
        }
        self.with_mutation(now, |loaded| {
            if profile_did == self.local_profile_did {
                anyhow::bail!("local profile is not an accepted contact head");
            }
            let (known_revision, known_canonical) =
                current_profile_presentation(loaded, &self.local_profile_did)?
                    .remove(&profile_did)
                    .ok_or_else(|| {
                        anyhow::anyhow!("profile chain does not belong to an accepted contact")
                    })?;

            let head = steps.last().expect("verified chain is not empty");
            match head.0.cmp(&known_revision) {
                std::cmp::Ordering::Less => {
                    anyhow::bail!("accepted profile chain rolls the revision back")
                }
                std::cmp::Ordering::Equal => {
                    if head.1 != known_canonical {
                        anyhow::bail!("accepted profile chain conflicts with the known revision");
                    }
                    return Ok((ContactStoreWrite::Replayed, false));
                }
                std::cmp::Ordering::Greater => {}
            }

            // The segment must bridge from what is already known: it has to
            // contain the exact next revision, and that step must name the
            // known revision's signed bytes.
            let bridge = steps
                .iter()
                .find(|(revision, _, _)| *revision == known_revision.saturating_add(1))
                .ok_or_else(|| anyhow::anyhow!("accepted profile chain skips a revision"))?;
            if bridge.2.document().previous_profile_sha256.as_deref()
                != Some(sha256_label_bytes(known_canonical.as_bytes()).as_str())
            {
                anyhow::bail!("accepted profile chain breaks the chain hash");
            }

            loaded
                .state
                .accepted_profile_heads
                .retain(|stored| !stored.signed_profile.contains(&profile_did));
            loaded
                .state
                .accepted_profile_heads
                .push(StoredAcceptedProfileHead {
                    signed_profile: head.1.clone(),
                    delivered_by_device_did: delivered_by_endpoint_did.to_string(),
                });
            if loaded.state.accepted_profile_heads.len() > MAX_ACCEPTED_PROFILE_HEADS {
                anyhow::bail!("accepted profile head limit exceeded");
            }
            Ok((ContactStoreWrite::Recorded, true))
        })
    }

    /// Records the signed revocation this Runtime minted for a live accepted
    /// pair. Local removal is immediate: the pair leaves the contact set in
    /// this same write, every messaging path keyed on contacts() goes dark,
    /// and the exact envelope becomes the durable artifact the sync loop
    /// retries until the peer's device acknowledges it. The signed request and
    /// decision chains stay — removal ends the relationship without deleting
    /// its history, and the retained head keeps naming it.
    pub(crate) fn record_local_contact_revocation(
        &self,
        envelope_bytes: &[u8],
        local_profile: &VerifiedCollaborationProfileDocument,
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        if envelope_bytes.len() > MAX_REVOCATION_ENVELOPE_BYTES {
            anyhow::bail!("contact revocation exceeds the protocol byte limit");
        }
        let revocation =
            verify_stored_collaboration_contact_revocation(envelope_bytes, &self.profile)?;
        if revocation.revoking_profile_did() != self.local_profile_did {
            anyhow::bail!("local contact revocation must be signed by this profile");
        }
        if local_profile.document().profile_did != self.local_profile_did
            || revocation.message().envelope().payload.sender_profile_did != self.local_profile_did
            || !local_profile.authorizes_signer(
                &revocation.message().envelope().signer_did,
                COLLABORATION_DISCOVERY_SERVICE,
                COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE,
            )
        {
            anyhow::bail!("local contact revocation authority does not match this Runtime");
        }
        let remote_profile_did = revocation.revoked_profile_did().to_string();
        if revocation.message().envelope().payload.recipient.id != remote_profile_did {
            anyhow::bail!("contact revocation does not target the remote Profile");
        }
        self.with_mutation(now, |loaded| {
            if loaded.removed_contacts.contains_key(&remote_profile_did) {
                return Ok((ContactStoreWrite::Replayed, false));
            }
            let is_live_contact = derive_contacts(loaded, &self.local_profile_did)?
                .iter()
                .any(|contact| contact.remote_profile_did() == remote_profile_did);
            if !is_live_contact {
                anyhow::bail!("contact revocation target is not an accepted contact");
            }
            require_pair_conversation(&revocation, &self.local_profile_did, &remote_profile_did)?;
            if loaded.removed_contacts.len() >= MAX_REMOVED_CONTACTS {
                anyhow::bail!("removed contact records exceed their limit");
            }
            loaded.state.removed_contacts.push(StoredRemovedContact {
                revocation_envelope: String::from_utf8(envelope_bytes.to_vec())
                    .context("contact revocation envelope must be UTF-8 JSON")?,
                removed_by_local: true,
                revocation_settled: false,
            });
            loaded.removed_contacts.insert(
                remote_profile_did.clone(),
                LoadedRemovedContact {
                    revocation,
                    removed_by_local: true,
                    revocation_settled: false,
                },
            );
            Ok((ContactStoreWrite::Recorded, true))
        })
    }

    /// Applies a peer's signed revocation. Verified against the pair's current
    /// delivery endpoint — the same authority a direct message needs — and
    /// idempotent: an already-removed pair settles the peer's retry without a
    /// second state change.
    pub(crate) fn apply_remote_contact_revocation(
        &self,
        envelope_bytes: &[u8],
        delivered_by_endpoint_did: &str,
        now: u64,
    ) -> anyhow::Result<ContactStoreWrite> {
        if envelope_bytes.len() > MAX_REVOCATION_ENVELOPE_BYTES {
            anyhow::bail!("contact revocation exceeds the protocol byte limit");
        }
        let revocation =
            verify_stored_collaboration_contact_revocation(envelope_bytes, &self.profile)?;
        if revocation.revoked_profile_did() != self.local_profile_did {
            anyhow::bail!("contact revocation does not target this profile");
        }
        let remote_profile_did = revocation.revoking_profile_did().to_string();
        let envelope = revocation.message().envelope();
        if envelope.payload.recipient.id != self.local_profile_did {
            anyhow::bail!("contact revocation does not target this Profile");
        }
        let signer_did = envelope.signer_did.clone();
        validate_canonical_did(
            delivered_by_endpoint_did,
            "contact revocation source endpoint",
        )?;
        self.with_mutation(now, |loaded| {
            let (_, canonical_profile) =
                current_profile_presentation(loaded, &self.local_profile_did)?
                    .remove(&remote_profile_did)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "contact revocation sender has no accepted Profile presentation"
                        )
                    })?;
            let signed_profile = serde_json::from_str(&canonical_profile)
                .context("contact revocation sender Profile is invalid")?;
            let remote_profile =
                crate::collaboration_profile_authority::verify_signed_profile_document(
                    &signed_profile,
                )?;
            if remote_profile.sole_endpoint_did()? != delivered_by_endpoint_did {
                anyhow::bail!(
                    "contact revocation did not arrive from the accepted Profile endpoint"
                );
            }
            if !remote_profile.authorizes_signer(
                &signer_did,
                COLLABORATION_DISCOVERY_SERVICE,
                COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE,
            ) {
                anyhow::bail!("contact revocation signer is not authorized by the sender Profile");
            }
            if loaded.removed_contacts.contains_key(&remote_profile_did) {
                return Ok((ContactStoreWrite::Replayed, false));
            }
            let contacts = derive_contacts(loaded, &self.local_profile_did)?;
            let contact = contacts
                .iter()
                .find(|contact| contact.remote_profile_did() == remote_profile_did)
                .ok_or_else(|| {
                    anyhow::anyhow!("contact revocation sender is not an accepted contact")
                })?;
            if revocation.message().envelope().payload.sender_profile_did
                != contact.remote_profile_did()
            {
                anyhow::bail!("contact revocation sender is not the accepted Profile");
            }
            require_pair_conversation(&revocation, &self.local_profile_did, &remote_profile_did)?;
            if loaded.removed_contacts.len() >= MAX_REMOVED_CONTACTS {
                anyhow::bail!("removed contact records exceed their limit");
            }
            loaded.state.removed_contacts.push(StoredRemovedContact {
                revocation_envelope: String::from_utf8(envelope_bytes.to_vec())
                    .context("contact revocation envelope must be UTF-8 JSON")?,
                removed_by_local: false,
                revocation_settled: true,
            });
            loaded.removed_contacts.insert(
                remote_profile_did.clone(),
                LoadedRemovedContact {
                    revocation,
                    removed_by_local: false,
                    revocation_settled: true,
                },
            );
            Ok((ContactStoreWrite::Recorded, true))
        })
    }

    /// Unsettled local revocations the sync loop should deliver, oldest first.
    /// An expired envelope is returned too — the caller re-mints the same
    /// removal fact into a fresh envelope and records it with
    /// `refresh_local_contact_revocation`, because a peer offline longer than
    /// one envelope lifetime must still learn the relationship ended.
    pub(crate) fn resendable_contact_revocations(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<ResendableContactRevocation>> {
        let Some(loaded) = self.load_state()? else {
            return Ok(Vec::new());
        };
        let removed_contacts = derive_removed_contacts(&loaded, &self.local_profile_did)?;
        let mut resendable = Vec::new();
        for (remote_profile_did, entry) in &loaded.removed_contacts {
            if !entry.removed_by_local || entry.revocation_settled {
                continue;
            }
            resendable.push(ResendableContactRevocation {
                remote_profile_did: remote_profile_did.clone(),
                recipient_endpoint_did: removed_contacts
                    .iter()
                    .find(|contact| contact.remote_profile_did() == remote_profile_did)
                    .ok_or_else(|| anyhow::anyhow!("removed relationship route is missing"))?
                    .remote_presence_device_did
                    .clone(),
                envelope: canonical_signed_collaboration_message_bytes(
                    entry.revocation.message().envelope(),
                )?,
                expires_at: entry.revocation.message().envelope().payload.expires_at,
                removed_at: entry.revocation.removed_at(),
            });
            if resendable.len() >= limit {
                break;
            }
        }
        resendable.sort_by(|left, right| left.removed_at.cmp(&right.removed_at));
        Ok(resendable)
    }

    /// Replaces an expired unsettled revocation envelope with a freshly minted
    /// one for the same pair and verb. The removal fact and its timestamp stay;
    /// only the delivery envelope is renewed.
    pub(crate) fn refresh_local_contact_revocation(
        &self,
        remote_profile_did: &str,
        envelope_bytes: &[u8],
        now: u64,
    ) -> anyhow::Result<()> {
        if envelope_bytes.len() > MAX_REVOCATION_ENVELOPE_BYTES {
            anyhow::bail!("contact revocation exceeds the protocol byte limit");
        }
        let revocation =
            verify_stored_collaboration_contact_revocation(envelope_bytes, &self.profile)?;
        if revocation.revoking_profile_did() != self.local_profile_did
            || revocation.revoked_profile_did() != remote_profile_did
        {
            anyhow::bail!("contact revocation refresh does not match the removed pair");
        }
        self.with_mutation(now, |loaded| {
            let entry = loaded
                .removed_contacts
                .get_mut(remote_profile_did)
                .ok_or_else(|| anyhow::anyhow!("removed relationship is unknown"))?;
            if !entry.removed_by_local || entry.revocation_settled {
                anyhow::bail!(
                    "contact revocation refresh applies only to unsettled local removals"
                );
            }
            if revocation.removed_at() != entry.revocation.removed_at() {
                anyhow::bail!("contact revocation refresh must keep the original removal time");
            }
            let stored = loaded
                .state
                .removed_contacts
                .iter_mut()
                .find(|stored| {
                    verify_stored_collaboration_contact_revocation(
                        stored.revocation_envelope.as_bytes(),
                        &self.profile,
                    )
                    .map(|existing| existing.revoked_profile_did() == remote_profile_did)
                    .unwrap_or(false)
                })
                .ok_or_else(|| anyhow::anyhow!("removed relationship record is missing"))?;
            stored.revocation_envelope = String::from_utf8(envelope_bytes.to_vec())
                .context("contact revocation envelope must be UTF-8 JSON")?;
            entry.revocation = revocation;
            Ok(((), true))
        })
    }

    /// Marks a local removal's revocation as acknowledged by the peer device.
    pub(crate) fn settle_local_contact_revocation(
        &self,
        remote_profile_did: &str,
        now: u64,
    ) -> anyhow::Result<()> {
        self.with_mutation(now, |loaded| {
            let entry = loaded
                .removed_contacts
                .get_mut(remote_profile_did)
                .ok_or_else(|| anyhow::anyhow!("removed relationship is unknown"))?;
            if !entry.removed_by_local {
                return Ok(((), false));
            }
            if entry.revocation_settled {
                return Ok(((), false));
            }
            entry.revocation_settled = true;
            let stored = loaded
                .state
                .removed_contacts
                .iter_mut()
                .find(|stored| {
                    verify_stored_collaboration_contact_revocation(
                        stored.revocation_envelope.as_bytes(),
                        &self.profile,
                    )
                    .map(|existing| existing.revoked_profile_did() == remote_profile_did)
                    .unwrap_or(false)
                })
                .ok_or_else(|| anyhow::anyhow!("removed relationship record is missing"))?;
            stored.revocation_settled = true;
            Ok(((), true))
        })
    }

    fn with_mutation<T>(
        &self,
        now: u64,
        mutate: impl FnOnce(&mut LoadedState) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _guard = contact_state_mutation_lock()
            .lock()
            .map_err(|_| anyhow::anyhow!("discovery mutation lock is poisoned"))?;
        self.ensure_writable_state_parent()?;
        let _file_guard = ExclusiveFileLock::acquire(&self.lock_path()?)?;
        let mut loaded = self.load_state()?.unwrap_or_else(|| LoadedState {
            state: ContactStoreState {
                schema: CONTACT_STORE_SCHEMA.to_string(),
                binding: self.state_binding(),
                discovery_enabled: false,
                published_local_advertisement_envelope_sha256: None,
                advertisements: Vec::new(),
                requests: Vec::new(),
                decisions: Vec::new(),
                revoked_request_hashes: Vec::new(),
                accepted_profile_heads: Vec::new(),
                removed_contacts: Vec::new(),
            },
            advertisements: HashMap::new(),
            requests: HashMap::new(),
            decisions: HashMap::new(),
            revoked_request_hashes: HashSet::new(),
            accepted_profile_heads: HashMap::new(),
            removed_contacts: HashMap::new(),
        });
        let pruned_before = self.prune_unaccepted_expired(&mut loaded, now)?;
        let (value, changed) = mutate(&mut loaded)?;
        let pruned_after = if changed {
            self.prune_unaccepted_expired(&mut loaded, now)?
        } else {
            false
        };
        if pruned_before || changed || pruned_after {
            loaded = self.verify_state(loaded.state.clone())?;
            self.write_state(&loaded.state)?;
        }
        Ok(value)
    }

    fn load_state(&self) -> anyhow::Result<Option<LoadedState>> {
        let path = self.state_path()?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                anyhow::bail!("contact store state must be a regular file")
            }
            Ok(metadata) => validate_owner_only_regular_file(&path, &metadata)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        }
        if !self.has_principal_root_protection()? {
            anyhow::bail!("contact store state requires principal-root protection");
        }
        let bytes = match read_principal_root_object(
            &self.data_root,
            &self.principal_id,
            &self.localhost_root,
            &self.state_object_uri(),
            &path,
        ) {
            Ok(bytes) => bytes,
            Err(err) if is_missing_state_file(&err) => {
                anyhow::bail!("contact store state disappeared during protected read: {err}")
            }
            Err(err) => return Err(err),
        };
        if bytes.len() > MAX_CONTACT_STORE_STATE_BYTES {
            anyhow::bail!("contact store exceeds its byte limit");
        }
        let state: ContactStoreState =
            serde_json::from_slice(&bytes).context("invalid contact store")?;
        if canonical_state_bytes(&state)? != bytes {
            anyhow::bail!("contact store is not canonical JSON");
        }
        Ok(Some(self.verify_state(state)?))
    }

    fn has_principal_root_protection(&self) -> anyhow::Result<bool> {
        Ok(load_principal_root_protection(
            &self.data_root,
            &self.principal_id,
            &self.localhost_root,
        )?
        .is_some())
    }

    fn ensure_writable_state_parent(&self) -> anyhow::Result<()> {
        if !self.has_principal_root_protection()? {
            anyhow::bail!("contact store state requires principal-root protection");
        }
        ensure_owner_only_directory(&self.data_root)?;
        let localhost_root = self.state_path_for_uri(&self.localhost_root)?;
        let relative_root = localhost_root
            .strip_prefix(&self.data_root)
            .map_err(|_| anyhow::anyhow!("principal localhost root is outside the data root"))?;
        let mut current = self.data_root.clone();
        for component in relative_root.components() {
            current.push(component.as_os_str());
            ensure_owner_only_directory(&current)?;
        }
        let app_data = self.state_path_for_uri(&format!("{}/.AppData", self.localhost_root))?;
        ensure_owner_only_directory(&app_data)?;
        let elastos_dir =
            self.state_path_for_uri(&format!("{}/.AppData/ElastOS", self.localhost_root))?;
        ensure_owner_only_directory(&elastos_dir)?;
        let people_dir =
            self.state_path_for_uri(&format!("{}/.AppData/ElastOS/People", self.localhost_root))?;
        ensure_owner_only_directory(&people_dir)?;
        Ok(())
    }

    fn verify_state(&self, state: ContactStoreState) -> anyhow::Result<LoadedState> {
        if state.schema != CONTACT_STORE_SCHEMA || state.binding != self.state_binding() {
            anyhow::bail!("contact store binding or schema mismatch");
        }
        if state.advertisements.len() > MAX_ADVERTISEMENT_RECORDS
            || state.requests.len() > MAX_REQUEST_RECORDS
            || state.decisions.len() > MAX_RECEIPT_RECORDS
        {
            anyhow::bail!("contact store exceeds its entry limits");
        }
        if state
            .advertisements
            .iter()
            .map(|entry| entry.envelope.len())
            .sum::<usize>()
            > MAX_ADVERTISEMENT_BYTES
            || state
                .requests
                .iter()
                .map(|entry| entry.envelope.len())
                .sum::<usize>()
                > MAX_REQUEST_BYTES
            || state
                .decisions
                .iter()
                .map(|entry| entry.envelope.len())
                .sum::<usize>()
                > MAX_RECEIPT_BYTES
        {
            anyhow::bail!("contact store exceeds its byte limits");
        }

        let loaded = self.loaded_state_from_state(state.clone())?;
        if let Some(published_hash) = state
            .published_local_advertisement_envelope_sha256
            .as_deref()
        {
            validate_sha256_label(published_hash, "published local discovery advertisement")?;
            let advertisement = loaded.advertisements.get(published_hash).ok_or_else(|| {
                anyhow::anyhow!("published local discovery advertisement is missing")
            })?;
            if advertisement.profile_did() != self.local_profile_did {
                anyhow::bail!("published local discovery advertisement belongs to another profile");
            }
        }
        let mut ad_senders = HashMap::<String, usize>::new();
        for ad in loaded.advertisements.values() {
            *ad_senders
                .entry(ad.message().envelope().payload.sender_profile_did.clone())
                .or_default() += 1;
        }
        if ad_senders
            .values()
            .any(|count| *count > MAX_ADVERTISEMENTS_PER_SENDER)
        {
            anyhow::bail!("discovery advertisements exceed the per-sender limit");
        }

        let mut req_senders = HashMap::<String, usize>::new();
        for request in loaded.requests.values() {
            *req_senders
                .entry(request.requester_profile_did().to_string())
                .or_default() += 1;
            let advertisement = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
                .ok_or_else(|| {
                    anyhow::anyhow!("stored discovery request advertisement is missing")
                })?;
            if request.requester_profile_did() != self.local_profile_did
                && advertisement.profile_did() != self.local_profile_did
            {
                anyhow::bail!("discovery request does not involve this profile");
            }
        }
        if req_senders
            .values()
            .any(|count| *count > MAX_REQUESTS_PER_SENDER)
        {
            anyhow::bail!("discovery requests exceed the per-sender limit");
        }

        let accepted = accepted_chains(&loaded)?;
        let accepted_request_hashes: HashSet<_> = accepted
            .iter()
            .map(|chain| chain.request_hash.clone())
            .collect();
        let referenced_advertisements: HashSet<_> = loaded
            .requests
            .values()
            .map(|request| request.advertisement_envelope_sha256().to_string())
            .collect();

        for advertisement in loaded.advertisements.values() {
            let hash = advertisement.message().envelope_sha256();
            if advertisement.profile_did() != self.local_profile_did
                && !referenced_advertisements.contains(hash)
            {
                anyhow::bail!("discovery store contains an unbound remote advertisement");
            }
        }

        let mut decision_senders = HashMap::<String, usize>::new();
        for (request_hash, decision) in &loaded.decisions {
            let sender = decision.envelope().payload.recipient_profile_did.clone();
            *decision_senders.entry(sender).or_default() += 1;
            let request = loaded.requests.get(request_hash).ok_or_else(|| {
                anyhow::anyhow!("discovery store decision request binding is invalid")
            })?;
            let advertisement = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
                .ok_or_else(|| {
                    anyhow::anyhow!("discovery store decision advertisement is missing")
                })?;
            if decision.envelope().payload.recipient_profile_did != advertisement.profile_did() {
                anyhow::bail!(
                    "discovery contact decision recipient profile does not match the advertisement"
                );
            }
            if decision.envelope().payload.decision == CollaborationContactDecision::Accepted
                && !accepted_request_hashes.contains(request_hash)
            {
                anyhow::bail!("discovery store accepted request binding is invalid");
            }
        }
        if decision_senders
            .values()
            .any(|count| *count > MAX_RECEIPTS_PER_SENDER)
        {
            anyhow::bail!("discovery contact decisions exceed the per-sender limit");
        }

        if loaded.revoked_request_hashes.len() != state.revoked_request_hashes.len() {
            anyhow::bail!("discovery store has duplicate revoked request hashes");
        }
        if loaded.revoked_request_hashes.len() > MAX_REVOKED_REQUEST_HASHES {
            anyhow::bail!("discovery revoked request hashes exceed the record limit");
        }

        if loaded.removed_contacts.len() > MAX_REMOVED_CONTACTS
            || state.removed_contacts.len() != loaded.removed_contacts.len()
        {
            anyhow::bail!("removed contact records exceed their limit");
        }
        // Every removed relationship must trace to signed acceptance truth,
        // and its revocation must be scoped to exactly that pair's stable
        // conversation. A removed pair with no chain would be an orphan a
        // hostile write could invent.
        let accepted_remotes: HashSet<String> = accepted
            .iter()
            .map(|chain| {
                remote_presentation(chain, &self.local_profile_did)
                    .map(|remote| remote.remote_profile_did)
            })
            .collect::<anyhow::Result<_>>()?;
        for (remote_profile_did, entry) in &loaded.removed_contacts {
            if !accepted_remotes.contains(remote_profile_did) {
                anyhow::bail!("removed relationship has no signed acceptance chain");
            }
            require_pair_conversation(
                &entry.revocation,
                &self.local_profile_did,
                remote_profile_did,
            )?;
        }
        // Heads bind to known relationships — accepted or removed — never to
        // profiles this store has no signed relationship with.
        for profile_did in loaded.accepted_profile_heads.keys() {
            if !accepted_remotes.contains(profile_did) {
                anyhow::bail!("accepted profile head is not bound to a known relationship");
            }
        }

        let bytes = canonical_state_bytes(&state)?;
        if bytes.len() > MAX_CONTACT_STORE_STATE_BYTES {
            anyhow::bail!("contact store exceeds its byte limit");
        }
        Ok(loaded)
    }

    fn state_binding(&self) -> ContactStoreBinding {
        ContactStoreBinding {
            network_id: self.profile.profile().network_id.clone(),
            local_profile_did: self.local_profile_did.clone(),
        }
    }

    fn state_object_uri(&self) -> String {
        format!(
            "{}/{}",
            self.localhost_root, PEOPLE_CONTACT_STATE_OBJECT_PATH
        )
    }

    fn state_path_for_uri(&self, uri: &str) -> anyhow::Result<PathBuf> {
        rooted_localhost_fs_path(&self.data_root, uri)
            .ok_or_else(|| anyhow::anyhow!("invalid contact store state root"))
    }

    fn state_path(&self) -> anyhow::Result<PathBuf> {
        self.state_path_for_uri(&self.state_object_uri())
    }

    fn lock_path(&self) -> anyhow::Result<PathBuf> {
        let state_path = self.state_path()?;
        let parent = state_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("contact store state parent is missing"))?;
        Ok(parent.join(CONTACT_STORE_LOCK_FILE))
    }

    fn write_state(&self, state: &ContactStoreState) -> anyhow::Result<()> {
        let bytes = canonical_state_bytes(state)?;
        if bytes.len() > MAX_CONTACT_STORE_STATE_BYTES {
            anyhow::bail!("contact store exceeds its byte limit");
        }
        let path = self.state_path()?;
        write_protected_principal_root_object(
            &self.data_root,
            &self.principal_id,
            &self.localhost_root,
            &self.state_object_uri(),
            &path,
            &bytes,
        )
    }

    fn loaded_state_from_state(&self, state: ContactStoreState) -> anyhow::Result<LoadedState> {
        let advertisements = state
            .advertisements
            .iter()
            .map(|entry| {
                let verified = verify_stored_collaboration_discovery_advertisement(
                    entry.envelope.as_bytes(),
                    &self.profile,
                )?;
                Ok((verified.message().envelope_sha256().to_string(), verified))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        if advertisements.len() != state.advertisements.len() {
            anyhow::bail!("discovery store has duplicate advertisements");
        }
        let requests = state
            .requests
            .iter()
            .map(|entry| {
                let request = verify_stored_unbound_collaboration_contact_request(
                    entry.envelope.as_bytes(),
                    &self.profile,
                )?;
                let advertisement = advertisements
                    .get(request.advertisement_envelope_sha256())
                    .ok_or_else(|| {
                        anyhow::anyhow!("stored discovery request advertisement is missing")
                    })?;
                let verified = bind_stored_collaboration_contact_request(request, advertisement)?;
                Ok((verified.message().envelope_sha256().to_string(), verified))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        if requests.len() != state.requests.len() {
            anyhow::bail!("discovery store has duplicate requests");
        }
        let decisions = state
            .decisions
            .iter()
            .map(|entry| {
                let raw: SignedCollaborationContactDecisionReceipt =
                    serde_json::from_str(&entry.envelope)?;
                let request_hash = raw.payload.request_envelope_sha256.clone();
                let request = requests.get(&request_hash).ok_or_else(|| {
                    anyhow::anyhow!("stored discovery decision request is missing")
                })?;
                let advertisement = advertisements
                    .get(request.advertisement_envelope_sha256())
                    .ok_or_else(|| {
                        anyhow::anyhow!("stored discovery decision advertisement is missing")
                    })?;
                let verified = verify_stored_collaboration_contact_decision_receipt(
                    entry.envelope.as_bytes(),
                    request,
                    advertisement,
                )?;
                Ok((request_hash, verified))
            })
            .collect::<anyhow::Result<HashMap<_, _>>>()?;
        if decisions.len() != state.decisions.len() {
            anyhow::bail!("discovery store has conflicting contact decisions");
        }
        let revoked_request_hashes = state
            .revoked_request_hashes
            .iter()
            .map(|request_hash| {
                validate_sha256_label(request_hash, "discovery revoked request hash")?;
                Ok(request_hash.clone())
            })
            .collect::<anyhow::Result<HashSet<_>>>()?;
        let mut accepted_profile_heads = HashMap::new();
        for head in &state.accepted_profile_heads {
            let signed: crate::collaboration_profile_authority::SignedCollaborationProfileDocument =
                serde_json::from_str(&head.signed_profile)
                    .context("invalid stored accepted profile head")?;
            let verified =
                crate::collaboration_profile_authority::verify_signed_profile_document(&signed)?;
            validate_canonical_did(
                &head.delivered_by_device_did,
                "accepted profile head device",
            )?;
            if verified.sole_endpoint_did()? != head.delivered_by_device_did {
                anyhow::bail!("accepted profile head endpoint does not match its signed Profile");
            }
            if accepted_profile_heads
                .insert(
                    verified.document().profile_did.clone(),
                    (verified, head.delivered_by_device_did.clone()),
                )
                .is_some()
            {
                anyhow::bail!("duplicate accepted profile head for one profile DID");
            }
        }
        let mut removed_contacts = HashMap::new();
        for stored in &state.removed_contacts {
            if stored.revocation_envelope.len() > MAX_REVOCATION_ENVELOPE_BYTES {
                anyhow::bail!("stored contact revocation exceeds the protocol byte limit");
            }
            let revocation = verify_stored_collaboration_contact_revocation(
                stored.revocation_envelope.as_bytes(),
                &self.profile,
            )?;
            let (local_did, remote_did) = if stored.removed_by_local {
                (
                    revocation.revoking_profile_did(),
                    revocation.revoked_profile_did(),
                )
            } else {
                (
                    revocation.revoked_profile_did(),
                    revocation.revoking_profile_did(),
                )
            };
            if local_did != self.local_profile_did {
                anyhow::bail!("stored contact revocation does not involve this profile");
            }
            if !stored.removed_by_local && !stored.revocation_settled {
                anyhow::bail!("a remote contact revocation is settled by construction");
            }
            if removed_contacts
                .insert(
                    remote_did.to_string(),
                    LoadedRemovedContact {
                        revocation,
                        removed_by_local: stored.removed_by_local,
                        revocation_settled: stored.revocation_settled,
                    },
                )
                .is_some()
            {
                anyhow::bail!("duplicate removed relationship for one profile DID");
            }
        }
        Ok(LoadedState {
            state,
            advertisements,
            requests,
            decisions,
            revoked_request_hashes,
            accepted_profile_heads,
            removed_contacts,
        })
    }

    fn prune_unaccepted_expired(&self, loaded: &mut LoadedState, now: u64) -> anyhow::Result<bool> {
        let terminal_request_hashes: HashSet<_> = loaded.decisions.keys().cloned().collect();
        let original = loaded.state.clone();
        loaded.state.requests.retain(|entry| {
            let hash = collaboration_message_envelope_sha256(entry.envelope.as_bytes());
            terminal_request_hashes.contains(&hash)
                || loaded
                    .requests
                    .get(&hash)
                    .map(|request| request.message().envelope().payload.expires_at > now)
                    .unwrap_or(true)
        });

        let referenced_ad_hashes: HashSet<_> = loaded
            .state
            .requests
            .iter()
            .map(|entry| collaboration_message_envelope_sha256(entry.envelope.as_bytes()))
            .filter_map(|hash| loaded.requests.get(&hash))
            .map(|request| request.advertisement_envelope_sha256().to_string())
            .collect();

        loaded.state.advertisements.retain(|entry| {
            let hash = collaboration_message_envelope_sha256(entry.envelope.as_bytes());
            referenced_ad_hashes.contains(&hash)
                || loaded
                    .advertisements
                    .get(&hash)
                    .map(|advertisement| {
                        advertisement.profile_did() == self.local_profile_did
                            && advertisement.message().envelope().payload.expires_at > now
                    })
                    .unwrap_or(true)
        });
        if let Some(published_hash) = loaded
            .state
            .published_local_advertisement_envelope_sha256
            .as_deref()
        {
            let active_remains = loaded.state.advertisements.iter().any(|entry| {
                collaboration_message_envelope_sha256(entry.envelope.as_bytes()) == published_hash
            });
            if !active_remains {
                loaded.state.published_local_advertisement_envelope_sha256 = None;
            }
        }

        if loaded.state == original {
            return Ok(false);
        }
        *loaded = self.loaded_state_from_state(loaded.state.clone())?;
        Ok(true)
    }
}

fn persist_advertisement_if_missing(
    loaded: &mut LoadedState,
    envelope_bytes: &[u8],
    advertisement: VerifiedCollaborationDiscoveryAdvertisement,
) -> anyhow::Result<bool> {
    let envelope_hash = advertisement.message().envelope_sha256().to_string();
    if loaded.advertisements.contains_key(&envelope_hash) {
        return Ok(false);
    }
    let sender_did = advertisement
        .message()
        .envelope()
        .payload
        .sender_profile_did
        .clone();
    prune_unreferenced_advertisements_for_sender(loaded, &sender_did);
    let count_for_sender = loaded
        .advertisements
        .values()
        .filter(|existing| existing.message().envelope().payload.sender_profile_did == sender_did)
        .count();
    if count_for_sender >= MAX_ADVERTISEMENTS_PER_SENDER {
        anyhow::bail!("discovery advertisement per-sender limit exceeded");
    }
    if loaded.advertisements.values().any(|existing| {
        let existing_message = &existing.message().envelope().payload;
        existing_message.sender_profile_did == sender_did
            && existing_message.created_at == advertisement.message().envelope().payload.created_at
            && existing.message().envelope_sha256() != advertisement.message().envelope_sha256()
    }) {
        anyhow::bail!("discovery advertisement conflicts with an existing sender event");
    }
    loaded.state.advertisements.push(StoredEnvelope {
        envelope: std::str::from_utf8(envelope_bytes)
            .context("discovery advertisement is not UTF-8 JSON")?
            .to_string(),
    });
    loaded.advertisements.insert(envelope_hash, advertisement);
    Ok(true)
}

fn persist_request_if_missing(
    loaded: &mut LoadedState,
    request_bytes: &[u8],
    request: &VerifiedCollaborationContactRequest,
) -> anyhow::Result<bool> {
    let request_hash = request.message().envelope_sha256().to_string();
    if loaded.requests.contains_key(&request_hash) {
        return Ok(false);
    }
    let sender = request.requester_profile_did().to_string();
    if loaded.requests.values().any(|existing| {
        existing.requester_profile_did() == sender
            && existing.advertisement_envelope_sha256() == request.advertisement_envelope_sha256()
            && existing.message().envelope_sha256() != request_hash
            && !loaded
                .revoked_request_hashes
                .contains(existing.message().envelope_sha256())
            && !loaded
                .decisions
                .contains_key(existing.message().envelope_sha256())
    }) {
        anyhow::bail!("discovery contact request is already pending for this person");
    }
    let count_for_sender = loaded
        .requests
        .values()
        .filter(|existing| existing.requester_profile_did() == sender)
        .count();
    if count_for_sender >= MAX_REQUESTS_PER_SENDER {
        anyhow::bail!("discovery contact request per-sender limit exceeded");
    }
    let message = &request.message().envelope().payload;
    for existing in loaded.requests.values() {
        let existing_message = &existing.message().envelope().payload;
        if existing_message.sender_profile_did == message.sender_profile_did
            && existing_message.message_id == message.message_id
            && existing.message().envelope_sha256() != request_hash
        {
            anyhow::bail!("discovery contact request message_id conflict");
        }
        if existing_message.sender_profile_did == message.sender_profile_did
            && existing_message.nonce == message.nonce
            && existing.message().envelope_sha256() != request_hash
        {
            anyhow::bail!("discovery contact request nonce conflict");
        }
    }
    loaded.state.requests.push(StoredEnvelope {
        envelope: std::str::from_utf8(request_bytes)
            .context("discovery contact request is not UTF-8 JSON")?
            .to_string(),
    });
    loaded.requests.insert(request_hash, request.clone());
    Ok(true)
}

fn persist_receipt_if_missing(
    loaded: &mut LoadedState,
    receipt_bytes: &[u8],
    request_hash: &str,
    receipt: VerifiedCollaborationContactDecisionReceipt,
) -> anyhow::Result<bool> {
    match loaded.decisions.get(request_hash) {
        Some(existing) if existing.envelope() == receipt.envelope() => return Ok(false),
        Some(_) => anyhow::bail!("discovery contact decision conflicts with the stored request"),
        None => {}
    }
    let signer_did = receipt.envelope().signer_did.clone();
    let count_for_signer = loaded
        .decisions
        .values()
        .filter(|existing| existing.envelope().signer_did == signer_did)
        .count();
    if count_for_signer >= MAX_RECEIPTS_PER_SENDER {
        anyhow::bail!("discovery contact decision per-sender limit exceeded");
    }
    loaded.state.decisions.push(StoredEnvelope {
        envelope: std::str::from_utf8(receipt_bytes)
            .context("discovery contact decision is not UTF-8 JSON")?
            .to_string(),
    });
    loaded.decisions.insert(request_hash.to_string(), receipt);
    Ok(true)
}

fn prune_unreferenced_advertisements_for_sender(
    loaded: &mut LoadedState,
    sender_profile_did: &str,
) -> bool {
    let referenced_advertisement_hashes: HashSet<_> = loaded
        .requests
        .values()
        .map(|request| request.advertisement_envelope_sha256().to_string())
        .collect();
    let remove_hashes: HashSet<_> = loaded
        .advertisements
        .iter()
        .filter(|(hash, advertisement)| {
            advertisement
                .message()
                .envelope()
                .payload
                .sender_profile_did
                == sender_profile_did
                && loaded
                    .state
                    .published_local_advertisement_envelope_sha256
                    .as_deref()
                    != Some(hash.as_str())
                && !referenced_advertisement_hashes.contains(*hash)
        })
        .map(|(hash, _)| hash.clone())
        .collect();
    if remove_hashes.is_empty() {
        return false;
    }
    loaded
        .advertisements
        .retain(|hash, _| !remove_hashes.contains(hash));
    loaded.state.advertisements.retain(|entry| {
        !remove_hashes.contains(&collaboration_message_envelope_sha256(
            entry.envelope.as_bytes(),
        ))
    });
    true
}

fn contact_state_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn is_missing_state_file(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// A local revocation awaiting the peer's acknowledgement.
#[derive(Debug, Clone)]
pub(crate) struct ResendableContactRevocation {
    pub(crate) remote_profile_did: String,
    pub(crate) recipient_endpoint_did: String,
    pub(crate) envelope: Vec<u8>,
    pub(crate) expires_at: u64,
    pub(crate) removed_at: u64,
}

/// The revocation's conversation must be the pair's stable direct id — the
/// same selector the pair's messages use, derived, never asserted.
fn require_pair_conversation(
    revocation: &VerifiedCollaborationContactRevocation,
    local_profile_did: &str,
    remote_profile_did: &str,
) -> anyhow::Result<()> {
    let expected = stable_direct_conversation_id(
        &revocation.message().envelope().payload.network_id,
        local_profile_did,
        remote_profile_did,
    )?;
    if revocation.message().envelope().payload.conversation_id != expected {
        anyhow::bail!("contact revocation conversation does not match the pair");
    }
    Ok(())
}

fn accepted_chains(loaded: &LoadedState) -> anyhow::Result<Vec<AcceptedChain>> {
    loaded
        .decisions
        .iter()
        .filter(|(_, decision)| {
            decision.envelope().payload.decision == CollaborationContactDecision::Accepted
        })
        .map(|(request_hash, _)| request_hash)
        .map(|request_hash| {
            let request = loaded
                .requests
                .get(request_hash)
                .ok_or_else(|| anyhow::anyhow!("accepted discovery request is missing"))?
                .clone();
            let receipt = loaded
                .decisions
                .get(request_hash)
                .ok_or_else(|| anyhow::anyhow!("accepted discovery receipt is missing"))?
                .clone();
            let advertisement = loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
                .ok_or_else(|| anyhow::anyhow!("accepted discovery advertisement is missing"))?
                .clone();
            Ok(AcceptedChain {
                request_hash: request_hash.clone(),
                request,
                advertisement,
                receipt,
            })
        })
        .collect()
}

/// Revision and canonical signed bytes currently known for each accepted
/// contact, taking the retained head when it is ahead of the discovery chain.
fn current_profile_presentation(
    loaded: &LoadedState,
    local_profile_did: &str,
) -> anyhow::Result<HashMap<String, (u64, String)>> {
    let mut current: HashMap<String, (u64, String)> = HashMap::new();
    for chain in accepted_chains(loaded)? {
        let remote = remote_presentation(&chain, local_profile_did)?;
        let signed = match chain.advertisement.profile_did() == local_profile_did {
            true => serde_json::to_string(chain.request.signed_profile())?,
            false => serde_json::to_string(chain.advertisement.signed_profile())?,
        };
        let entry = current
            .entry(remote.remote_profile_did.clone())
            .or_insert_with(|| (remote.profile_revision, signed.clone()));
        if remote.profile_revision > entry.0 {
            *entry = (remote.profile_revision, signed);
        }
    }
    for (profile_did, (verified, _)) in &loaded.accepted_profile_heads {
        let Some(entry) = current.get_mut(profile_did) else {
            continue;
        };
        let revision = verified.document().revision;
        if revision >= entry.0 {
            *entry = (revision, serde_json::to_string(verified.signed_envelope())?);
        }
    }
    Ok(current)
}

/// The removed set: every ended pair, named from the same signed presentation
/// path the live contacts use — the retained head first, else the newest
/// chain presentation. Removal must not orphan the name.
fn derive_removed_contacts(
    loaded: &LoadedState,
    local_profile_did: &str,
) -> anyhow::Result<Vec<CollaborationRemovedContact>> {
    let mut grouped: HashMap<String, Vec<RemotePresentation>> = HashMap::new();
    for chain in accepted_chains(loaded)? {
        let remote = remote_presentation(&chain, local_profile_did)?;
        if loaded
            .removed_contacts
            .contains_key(&remote.remote_profile_did)
        {
            grouped
                .entry(remote.remote_profile_did.clone())
                .or_default()
                .push(remote);
        }
    }
    let mut removed = Vec::new();
    for (remote_profile_did, entry) in &loaded.removed_contacts {
        let Some(mut candidates) = grouped.remove(remote_profile_did) else {
            anyhow::bail!("removed relationship has no signed acceptance chain");
        };
        let winner = choose_remote_presentation(&mut candidates)?;
        let (display_name, handle, endpoint) =
            match loaded.accepted_profile_heads.get(remote_profile_did) {
                Some((verified, device_did))
                    if verified.document().revision > winner.profile_revision =>
                {
                    (
                        verified.document().display_name.clone(),
                        verified.document().handle.clone(),
                        device_did.clone(),
                    )
                }
                _ => (
                    winner.display_name.clone(),
                    winner.handle.clone(),
                    winner.remote_presence_device_did.clone(),
                ),
            };
        removed.push(CollaborationRemovedContact {
            remote_profile_did: remote_profile_did.clone(),
            remote_presence_device_did: endpoint,
            display_name,
            handle,
            conversation_id: stable_direct_conversation_id(
                &entry.revocation.message().envelope().payload.network_id,
                local_profile_did,
                remote_profile_did,
            )?,
            removed_at: entry.revocation.removed_at(),
            removed_by_local: entry.removed_by_local,
        });
    }
    removed.sort_by(|left, right| left.remote_profile_did.cmp(&right.remote_profile_did));
    Ok(removed)
}

fn derive_contacts(
    loaded: &LoadedState,
    local_profile_did: &str,
) -> anyhow::Result<Vec<CollaborationAcceptedContact>> {
    let mut grouped: HashMap<String, Vec<RemotePresentation>> = HashMap::new();
    for chain in accepted_chains(loaded)? {
        let remote = remote_presentation(&chain, local_profile_did)?;
        // A removed pair keeps its signed chain as history but is no longer a
        // contact: it surfaces through the removed set instead, and every
        // messaging path keyed on contacts() goes dark with it.
        if loaded
            .removed_contacts
            .contains_key(&remote.remote_profile_did)
        {
            continue;
        }
        grouped
            .entry(stable_direct_conversation_id(
                &chain.request.message().envelope().payload.network_id,
                chain.request.requester_profile_did(),
                chain.advertisement.profile_did(),
            )?)
            .or_default()
            .push(remote);
    }

    let mut contacts = Vec::new();
    for (conversation_id, mut candidates) in grouped {
        let added_at = candidates
            .iter()
            .map(|candidate| candidate.decided_at)
            .max()
            .expect("contact candidates exist");
        let winner = choose_remote_presentation(&mut candidates)?;
        // A retained head that is ahead of the discovery chain supersedes the
        // name, handle, and delivery endpoint without changing the contact
        // identity or the stable conversation ID.
        let (display_name, handle, endpoint) = match loaded
            .accepted_profile_heads
            .get(&winner.remote_profile_did)
        {
            Some((verified, device_did))
                if verified.document().revision > winner.profile_revision =>
            {
                (
                    verified.document().display_name.clone(),
                    verified.document().handle.clone(),
                    device_did.clone(),
                )
            }
            _ => (
                winner.display_name,
                winner.handle,
                winner.remote_presence_device_did,
            ),
        };
        contacts.push(CollaborationAcceptedContact {
            remote_profile_did: winner.remote_profile_did,
            remote_presence_device_did: endpoint,
            remote_display_name: display_name,
            remote_handle: handle,
            added_at,
            conversation_id,
        });
    }
    contacts.sort_by(|left, right| left.remote_profile_did.cmp(&right.remote_profile_did));
    Ok(contacts)
}

fn choose_remote_presentation(
    candidates: &mut Vec<RemotePresentation>,
) -> anyhow::Result<RemotePresentation> {
    candidates.sort_by(|left, right| {
        left.profile_revision
            .cmp(&right.profile_revision)
            .then_with(|| left.profile_envelope_hash.cmp(&right.profile_envelope_hash))
            .then_with(|| left.event_created_at.cmp(&right.event_created_at))
            .then_with(|| left.event_envelope_hash.cmp(&right.event_envelope_hash))
    });
    let winner = candidates
        .pop()
        .ok_or_else(|| anyhow::anyhow!("contact candidates are missing"))?;
    for candidate in candidates.iter() {
        if candidate.profile_revision == winner.profile_revision
            && candidate.profile_envelope_hash != winner.profile_envelope_hash
        {
            anyhow::bail!(
                "accepted contact profile revision {} has conflicting signed profile bytes",
                winner.profile_revision
            );
        }
    }
    Ok(winner)
}

fn contact_store_state_object_uri(localhost_root: &str) -> String {
    format!("{localhost_root}/{PEOPLE_CONTACT_STATE_OBJECT_PATH}")
}

/// The signed contact-store state for the Full Recovery Bundle. `None` when
/// this principal has never had a contact store. Everything inside is signed
/// wire material plus local decision flags; without it a recovered Profile
/// has no contacts to announce its rebound device to, which is why identity
/// recovery carries it.
pub(crate) fn export_contact_store_state_for_recovery(
    data_dir: &std::path::Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let uri = contact_store_state_object_uri(localhost_root);
    let Some(path) = rooted_localhost_fs_path(data_dir, &uri) else {
        anyhow::bail!("invalid contact store state root");
    };
    if !path.exists() {
        return Ok(None);
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &uri,
        &path,
    )?;
    // Never export state this Runtime cannot itself decode as the current
    // schema; a corrupt store must fail recovery export loudly, not travel.
    let state: ContactStoreState = serde_json::from_slice(&bytes)
        .context("contact store state is not a current-schema store")?;
    if state.schema != CONTACT_STORE_SCHEMA {
        anyhow::bail!("contact store state schema is not current");
    }
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Writes recovered contact-store state back under the restored protected
/// root after checking it is a current-schema store bound to the recovered
/// Profile. Full signed-chain verification happens on first use, exactly as
/// it does for a store that never left the machine.
pub(crate) fn restore_contact_store_state_for_recovery(
    data_dir: &std::path::Path,
    principal_id: &str,
    localhost_root: &str,
    state_value: &serde_json::Value,
    expected_profile_did: &str,
) -> anyhow::Result<()> {
    let state: ContactStoreState = serde_json::from_value(state_value.clone())
        .context("recovered contact state is not a current-schema store")?;
    if state.schema != CONTACT_STORE_SCHEMA {
        anyhow::bail!("recovered contact state schema is not current");
    }
    if state.binding.local_profile_did != expected_profile_did {
        anyhow::bail!("recovered contact state is bound to another Profile");
    }
    // The store loads only its own canonical byte form.
    let bytes = canonical_state_bytes(&state)?;
    let uri = contact_store_state_object_uri(localhost_root);
    let Some(path) = rooted_localhost_fs_path(data_dir, &uri) else {
        anyhow::bail!("invalid contact store state root");
    };
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &uri,
        &path,
        &bytes,
    )
}

fn derive_pending_incoming_requests(
    loaded: &LoadedState,
    local_profile_did: &str,
) -> anyhow::Result<Vec<PendingIncomingContactRequest>> {
    let mut pending = loaded
        .requests
        .iter()
        .filter(|(request_hash, request)| {
            loaded
                .advertisements
                .get(request.advertisement_envelope_sha256())
                .is_some_and(|advertisement| advertisement.profile_did() == local_profile_did)
                && !loaded.decisions.contains_key(*request_hash)
                && !loaded.revoked_request_hashes.contains(*request_hash)
        })
        .map(|(request_hash, request)| PendingIncomingContactRequest {
            request_hash: request_hash.clone(),
            requester_profile_did: request.requester_profile_did().to_string(),
            display_name: request.display_name().to_string(),
            handle: request.handle().map(str::to_string),
            created_at: request.message().envelope().payload.created_at,
            expires_at: request.message().envelope().payload.expires_at,
        })
        .collect::<Vec<_>>();
    pending.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.request_hash.cmp(&right.request_hash))
    });
    Ok(pending)
}

fn remote_presentation(
    chain: &AcceptedChain,
    local_profile_did: &str,
) -> anyhow::Result<RemotePresentation> {
    if chain.advertisement.profile_did() == local_profile_did {
        Ok(RemotePresentation {
            remote_profile_did: chain.request.requester_profile_did().to_string(),
            remote_presence_device_did: chain.request.route_endpoint_did()?.to_string(),
            display_name: chain.request.display_name().to_string(),
            handle: chain.request.handle().map(str::to_string),
            decided_at: chain.receipt.envelope().payload.decided_at,
            profile_revision: chain.request.profile_revision(),
            profile_envelope_hash: chain.request.profile_envelope_sha256()?,
            event_created_at: chain.request.message().envelope().payload.created_at,
            event_envelope_hash: chain.request_hash.clone(),
        })
    } else if chain.request.requester_profile_did() == local_profile_did {
        Ok(RemotePresentation {
            remote_profile_did: chain.advertisement.profile_did().to_string(),
            remote_presence_device_did: chain.advertisement.route_endpoint_did()?.to_string(),
            display_name: chain.advertisement.display_name().to_string(),
            handle: chain.advertisement.handle().map(str::to_string),
            decided_at: chain.receipt.envelope().payload.decided_at,
            profile_revision: chain.advertisement.profile_revision(),
            profile_envelope_hash: chain.advertisement.profile_envelope_sha256()?,
            event_created_at: chain.advertisement.message().envelope().payload.created_at,
            event_envelope_hash: chain.advertisement.message().envelope_sha256().to_string(),
        })
    } else {
        anyhow::bail!("accepted discovery chain does not involve the local profile")
    }
}

pub(crate) fn stable_direct_conversation_id(
    network_id: &str,
    left: &str,
    right: &str,
) -> anyhow::Result<String> {
    validate_network_id(network_id)?;
    validate_canonical_did(left, "left direct conversation profile DID")?;
    validate_canonical_did(right, "right direct conversation profile DID")?;
    if left == right {
        anyhow::bail!("direct conversation pair cannot contain the same profile DID");
    }
    let (left, right) = ordered_pair(left, right);
    let mut hasher = Sha256::new();
    for field in [
        DIRECT_CONVERSATION_DOMAIN,
        network_id.as_bytes(),
        left.as_bytes(),
        right.as_bytes(),
    ] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    let conversation_id = format!("direct:sha256:{}", hex::encode(hasher.finalize()));
    validate_id(&conversation_id, "direct conversation_id")?;
    Ok(conversation_id)
}

fn ordered_pair<'a>(left: &'a str, right: &'a str) -> (&'a str, &'a str) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

fn validate_canonical_did(value: &str, field: &str) -> anyhow::Result<()> {
    let key = decode_did_key(value).with_context(|| format!("invalid {field}"))?;
    if encode_did_key(&key) != value {
        anyhow::bail!("{field} is not canonical");
    }
    Ok(())
}

fn sha256_label_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn validate_sha256_label(value: &str, field: &str) -> anyhow::Result<()> {
    if value.len() != "sha256:".len() + 64
        || !value.starts_with("sha256:")
        || !value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("{field} is invalid");
    }
    Ok(())
}

fn canonical_state_bytes(state: &ContactStoreState) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(state)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::{symlink, PermissionsExt};

    use sha2::Digest;

    use elastos_common::collaboration_protocol::{
        canonical_collaboration_message_bytes, canonical_signed_collaboration_message_bytes,
        CollaborationMessage, CollaborationRecipient, CollaborationRecipientKind,
        SignedCollaborationMessage, COLLABORATION_MESSAGE_SCHEMA_V1,
        COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
    };
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    use crate::collaboration_core::random_hex_128;
    use crate::collaboration_discovery::{
        canonical_signed_collaboration_contact_decision_receipt_bytes,
        CollaborationContactDecision, CollaborationContactDecisionReceipt,
        CollaborationContactRequestPayload, CollaborationDiscoveryAdvertisementPayload,
        SignedCollaborationContactDecisionReceipt,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1,
        COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
        COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
        COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS, COLLABORATION_DISCOVERY_CONTACT_ID,
        COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
        COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS, COLLABORATION_DISCOVERY_DIRECTORY_ID,
        COLLABORATION_DISCOVERY_SERVICE,
    };
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes,
        validate_collaboration_network_profile, CollaborationNetworkProfile,
        CollaborationNetworkProfileMode, SignedCollaborationNetworkProfile,
        COLLABORATION_NETWORK_PROFILE_SCHEMA, COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_profile_authority::signed_profile_document_for_test;

    const NETWORK: &str = "collaboration-store-test";
    const NOW: u64 = 1_800_000_000;

    struct Fixture {
        _temp: tempfile::TempDir,
        data_root: PathBuf,
        principal_id: String,
        localhost_root: String,
        profile: VerifiedCollaborationNetworkProfile,
        local_profile: VerifiedCollaborationProfileDocument,
        local_key: SigningKey,
        local_did: String,
        store: CollaborationContactStore,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data_root = temp.path().join("data");
            fs::create_dir_all(&data_root).unwrap();
            fs::set_permissions(&data_root, fs::Permissions::from_mode(0o700)).unwrap();
            let principal_id = "person:local:contact-store-test".to_string();
            let protection =
                crate::auth::store_test_principal_root_protection(&data_root, &principal_id);
            let localhost_root = protection.localhost_root.clone();
            let (profile_signer, _) = generate_keypair();
            let profile = verified_profile(&profile_signer, NETWORK);
            let (local_key, _) = generate_keypair();
            let local_did = device_did(&local_key);
            let (local_profile_signer, _) = generate_keypair();
            let local_profile = signed_profile_document_for_test(
                &local_profile_signer,
                "Local",
                Some("local"),
                1,
                None,
                NOW,
                vec![local_did.clone()],
            )
            .unwrap();
            let store = CollaborationContactStore::new(
                &data_root,
                &principal_id,
                &localhost_root,
                profile.clone(),
                &local_profile,
                &local_did,
            )
            .unwrap();
            Self {
                _temp: temp,
                data_root,
                principal_id,
                localhost_root,
                profile,
                local_profile,
                local_key,
                local_did,
                store,
            }
        }
    }

    fn device_did(signing_key: &SigningKey) -> String {
        crate::crypto::encode_did_key(&signing_key.verifying_key())
    }

    fn profile_hash(profile: &VerifiedCollaborationProfileDocument) -> String {
        let bytes = serde_json::to_vec(profile.signed_envelope()).unwrap();
        format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)))
    }

    fn verified_profile(
        signing_key: &SigningKey,
        network_id: &str,
    ) -> VerifiedCollaborationNetworkProfile {
        let signer_did = device_did(signing_key);
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: network_id.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: None,
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did,
        };
        let bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        match validate_collaboration_network_profile(
            Some(&bytes),
            network_id,
            &[device_did(signing_key)],
            None,
        )
        .unwrap()
        {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    fn canonical_test_payload_value<T: serde::Serialize>(
        payload: &T,
    ) -> anyhow::Result<serde_json::Value> {
        // Same round-trip the production signer uses, so field order matches
        // what the canonical-payload verifier demands.
        Ok(serde_json::from_slice(&serde_json::to_vec(payload)?)?)
    }

    fn revocation_bytes(
        signing_key: &SigningKey,
        revoking_profile_did: &str,
        revoked_profile_did: &str,
        removed_at: u64,
    ) -> Vec<u8> {
        signed_message(
            signing_key,
            revoking_profile_did,
            &stable_direct_conversation_id(NETWORK, revoking_profile_did, revoked_profile_did)
                .unwrap(),
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: revoked_profile_did.to_string(),
            },
            crate::collaboration_discovery::COLLABORATION_CONTACT_REVOCATION_PAYLOAD_TYPE,
            canonical_test_payload_value(
                &crate::collaboration_discovery::CollaborationContactRevocationPayload {
                    revoking_profile_did: revoking_profile_did.to_string(),
                    revoked_profile_did: revoked_profile_did.to_string(),
                    end_verb: crate::collaboration_discovery::COLLABORATION_CONTACT_END_VERB_REMOVE
                        .to_string(),
                    removed_at,
                },
            )
            .unwrap(),
            removed_at
                ..removed_at
                    + crate::collaboration_discovery::COLLABORATION_CONTACT_REVOCATION_TTL_SECS,
        )
    }

    fn signed_message(
        signing_key: &SigningKey,
        sender_profile_did: &str,
        conversation_id: &str,
        recipient: CollaborationRecipient,
        payload_type: &str,
        payload: serde_json::Value,
        validity: std::ops::Range<u64>,
    ) -> Vec<u8> {
        let message = CollaborationMessage {
            schema: COLLABORATION_MESSAGE_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: random_hex_128().unwrap(),
            nonce: random_hex_128().unwrap(),
            created_at: validity.start,
            expires_at: validity.end,
            sender_profile_did: sender_profile_did.to_string(),
            sender_service: COLLABORATION_DISCOVERY_SERVICE.to_string(),
            recipient,
            payload_type: payload_type.to_string(),
            payload,
        };
        let payload_bytes = canonical_collaboration_message_bytes(&message).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_MESSAGE_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_message_bytes(&SignedCollaborationMessage {
            payload: message,
            signature,
            signer_did,
        })
        .unwrap()
    }

    fn signed_profile_for_device(
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
            vec![device_did(device_signing_key)],
        )
        .unwrap()
    }

    fn advertisement_with_profile(
        signing_key: &SigningKey,
        profile: &VerifiedCollaborationProfileDocument,
        created_at: u64,
    ) -> Vec<u8> {
        signed_message(
            signing_key,
            &profile.document().profile_did,
            COLLABORATION_DISCOVERY_DIRECTORY_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Conversation,
                id: COLLABORATION_DISCOVERY_DIRECTORY_ID.to_string(),
            },
            COLLABORATION_DISCOVERY_ADVERTISEMENT_PAYLOAD_TYPE,
            serde_json::to_value(CollaborationDiscoveryAdvertisementPayload {
                signed_profile: profile.signed_envelope().clone(),
            })
            .unwrap(),
            created_at..created_at + COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS,
        )
    }

    fn advertisement(signing_key: &SigningKey, display_name: &str, created_at: u64) -> Vec<u8> {
        let (profile_signing_key, _) = generate_keypair();
        let handle = display_name.to_lowercase().replace(' ', "-");
        let profile = signed_profile_for_device(
            &profile_signing_key,
            signing_key,
            display_name,
            Some(&handle),
            1,
            None,
            created_at,
        );
        advertisement_with_profile(signing_key, &profile, created_at)
    }

    fn advertisement_profile_did(envelope: &[u8]) -> String {
        let signed: SignedCollaborationMessage = serde_json::from_slice(envelope).unwrap();
        let payload: CollaborationDiscoveryAdvertisementPayload =
            serde_json::from_value(signed.payload.payload).unwrap();
        crate::collaboration_profile_authority::verify_signed_profile_document(
            &payload.signed_profile,
        )
        .unwrap()
        .document()
        .profile_did
        .clone()
    }

    fn outgoing_request_with_profile(
        requester_key: &SigningKey,
        requester_profile: &VerifiedCollaborationProfileDocument,
        recipient_profile_did: &str,
        advertisement_hash: &str,
        created_at: u64,
    ) -> Vec<u8> {
        signed_message(
            requester_key,
            &requester_profile.document().profile_did,
            COLLABORATION_DISCOVERY_CONTACT_ID,
            CollaborationRecipient {
                kind: CollaborationRecipientKind::Profile,
                id: recipient_profile_did.to_string(),
            },
            COLLABORATION_DISCOVERY_CONTACT_REQUEST_PAYLOAD_TYPE,
            serde_json::to_value(CollaborationContactRequestPayload {
                advertisement_envelope_sha256: advertisement_hash.to_string(),
                signed_profile: requester_profile.signed_envelope().clone(),
            })
            .unwrap(),
            created_at..created_at + COLLABORATION_DISCOVERY_CONTACT_REQUEST_TTL_SECS,
        )
    }

    fn outgoing_request(
        requester_key: &SigningKey,
        recipient_profile_did: &str,
        advertisement_hash: &str,
        display_name: &str,
        created_at: u64,
    ) -> Vec<u8> {
        let (profile_signing_key, _) = generate_keypair();
        let handle = display_name.to_lowercase().replace(' ', "-");
        let profile = signed_profile_for_device(
            &profile_signing_key,
            requester_key,
            display_name,
            Some(&handle),
            1,
            None,
            created_at,
        );
        outgoing_request_with_profile(
            requester_key,
            &profile,
            recipient_profile_did,
            advertisement_hash,
            created_at,
        )
    }

    fn decision_receipt(
        recipient_key: &SigningKey,
        request_bytes: &[u8],
        decision: CollaborationContactDecision,
        decided_at: u64,
        recipient_profile_did: &str,
    ) -> Vec<u8> {
        let request: SignedCollaborationMessage = serde_json::from_slice(request_bytes).unwrap();
        let request_payload: CollaborationContactRequestPayload =
            serde_json::from_value(request.payload.payload.clone()).unwrap();
        let requester_profile =
            crate::collaboration_profile_authority::verify_signed_profile_document(
                &request_payload.signed_profile,
            )
            .unwrap();
        let payload = CollaborationContactDecisionReceipt {
            schema: COLLABORATION_CONTACT_DECISION_RECEIPT_SCHEMA_V1.to_string(),
            network_id: NETWORK.to_string(),
            request_envelope_sha256: collaboration_message_envelope_sha256(request_bytes),
            conversation_id: COLLABORATION_DISCOVERY_CONTACT_ID.to_string(),
            requester_profile_did: requester_profile.document().profile_did.clone(),
            requester_endpoint_did: request.signer_did,
            request_message_id: request.payload.message_id,
            request_message_nonce: request.payload.nonce,
            recipient_profile_did: recipient_profile_did.to_string(),
            recipient_endpoint_did: device_did(recipient_key),
            decision,
            decided_at,
        };
        let payload_bytes = serde_json::to_vec(&serde_json::to_value(&payload).unwrap()).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            recipient_key,
            COLLABORATION_CONTACT_DECISION_RECEIPT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        canonical_signed_collaboration_contact_decision_receipt_bytes(
            &SignedCollaborationContactDecisionReceipt {
                payload,
                signature,
                signer_did,
            },
        )
        .unwrap()
    }

    fn store_state_path(fixture: &Fixture) -> PathBuf {
        rooted_localhost_fs_path(
            &fixture.data_root,
            &format!(
                "{}/{}",
                fixture.localhost_root, PEOPLE_CONTACT_STATE_OBJECT_PATH
            ),
        )
        .unwrap()
    }

    fn write_state(fixture: &Fixture, bytes: &[u8]) {
        crate::auth::write_protected_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &store_state_path(fixture),
            bytes,
        )
        .unwrap();
    }

    fn write_state_struct(fixture: &Fixture, state: &ContactStoreState) {
        write_state(fixture, &canonical_state_bytes(state).unwrap());
    }

    fn encode_bytes(bytes: &[u8]) -> String {
        use base64::Engine as _;

        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn first_read_is_empty_and_creates_no_files() {
        let fixture = Fixture::new();
        assert!(fixture.store.snapshot().unwrap().contacts().is_empty());
        assert!(!fixture.data_root.join("collaboration").exists());
    }

    #[test]
    fn unprotected_principal_root_rejects_mutation_and_creates_zero_people_paths() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        fs::create_dir_all(&data_root).unwrap();
        fs::set_permissions(&data_root, fs::Permissions::from_mode(0o700)).unwrap();
        let principal_id = "person:local:unprotected-contact-store-test";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer, NETWORK);
        let (local_key, _) = generate_keypair();
        let local_did = device_did(&local_key);
        let (local_profile_signer, _) = generate_keypair();
        let local_profile = signed_profile_for_device(
            &local_profile_signer,
            &local_key,
            "Local",
            Some("local"),
            1,
            None,
            NOW,
        );
        let store = CollaborationContactStore::new(
            &data_root,
            principal_id,
            &localhost_root,
            profile,
            &local_profile,
            &local_did,
        )
        .unwrap();
        let state_path = rooted_localhost_fs_path(
            &data_root,
            &format!("{}/{}", localhost_root, PEOPLE_CONTACT_STATE_OBJECT_PATH),
        )
        .unwrap();
        let lock_path = state_path.parent().unwrap().join(CONTACT_STORE_LOCK_FILE);
        let people_dir = state_path.parent().unwrap().to_path_buf();
        let localhost_path = rooted_localhost_fs_path(&data_root, &localhost_root).unwrap();
        let local_ad = advertisement_with_profile(&local_key, &local_profile, NOW);

        assert!(store.store_local_advertisement(&local_ad, NOW).is_err());
        assert!(!localhost_path.exists());
        assert!(!people_dir.exists());
        assert!(!state_path.exists());
        assert!(!lock_path.exists());
    }

    #[test]
    fn round_trip_restart_and_expired_chain_still_derives_contact() {
        let fixture = Fixture::new();
        let (remote_key, _) = generate_keypair();
        let remote_ad = advertisement(&remote_key, "Remote", NOW);
        fixture
            .store
            .store_local_advertisement(
                &advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW),
                NOW,
            )
            .unwrap();
        let request_bytes = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &advertisement_profile_did(&remote_ad),
            &collaboration_message_envelope_sha256(&remote_ad),
            NOW + 1,
        );
        fixture
            .store
            .record_outgoing_contact_request(&request_bytes, &remote_ad, NOW + 1)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &remote_key,
                    &request_bytes,
                    CollaborationContactDecision::Accepted,
                    NOW + 2,
                    verify_collaboration_discovery_advertisement(&remote_ad, &fixture.profile, NOW)
                        .unwrap()
                        .profile_did(),
                ),
                NOW + 2,
            )
            .unwrap();
        let restarted = CollaborationContactStore::new(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            fixture.profile.clone(),
            &fixture.local_profile,
            &fixture.local_did,
        )
        .unwrap();
        let snapshot = restarted.snapshot().unwrap();
        assert_eq!(snapshot.contacts().len(), 1);
        assert_eq!(snapshot.contacts()[0].remote_display_name(), "Remote");
        assert_eq!(snapshot.contacts()[0].added_at(), NOW + 2);
    }

    #[test]
    fn owner_mode_symlink_truncation_and_unknown_fields_fail_closed() {
        let fixture = Fixture::new();
        let empty = fixture.store.snapshot().unwrap();
        assert!(empty.contacts().is_empty());

        fixture
            .store
            .store_local_advertisement(
                &advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW),
                NOW,
            )
            .unwrap();
        let state_path = store_state_path(&fixture);
        let original = fs::read(&state_path).unwrap();
        let original_plaintext = crate::auth::read_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
        )
        .unwrap();
        let symlink_target = fixture._temp.path().join("contact-state-target.json");
        fs::write(&symlink_target, &original).unwrap();
        fs::remove_file(&state_path).unwrap();
        symlink(&symlink_target, &state_path).unwrap();
        assert!(fixture.store.snapshot().is_err());
        fs::remove_file(&state_path).unwrap();
        crate::auth::write_protected_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
            &original_plaintext,
        )
        .unwrap();
        let lock_path = fixture.store.lock_path().unwrap();
        assert_eq!(
            fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(state_path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(fixture.store.snapshot().is_err());
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::write(&state_path, b"{}").unwrap();
        assert!(fixture.store.snapshot().is_err());
        fs::write(&state_path, &original).unwrap();

        let mut unknown: serde_json::Value = serde_json::from_slice(&original).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("extra".to_string(), serde_json::json!(true));
        fs::write(&state_path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(fixture.store.snapshot().is_err());
    }

    #[test]
    fn principal_scoped_stores_are_isolated_and_copied_ciphertext_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        fs::create_dir_all(&data_root).unwrap();
        fs::set_permissions(&data_root, fs::Permissions::from_mode(0o700)).unwrap();
        let (profile_signer, _) = generate_keypair();
        let profile = verified_profile(&profile_signer, NETWORK);

        let principal_a = "person:local:contact-store-a";
        let protection_a =
            crate::auth::store_test_principal_root_protection(&data_root, principal_a);
        let (local_key_a, _) = generate_keypair();
        let local_did_a = device_did(&local_key_a);
        let local_profile_a = signed_profile_for_device(
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            &local_key_a,
            "Local A",
            Some("local-a"),
            1,
            None,
            NOW,
        );
        let store_a = CollaborationContactStore::new(
            &data_root,
            principal_a,
            &protection_a.localhost_root,
            profile.clone(),
            &local_profile_a,
            &local_did_a,
        )
        .unwrap();

        let principal_b = "person:local:contact-store-b";
        let protection_b =
            crate::auth::store_test_principal_root_protection(&data_root, principal_b);
        let (local_key_b, _) = generate_keypair();
        let local_did_b = device_did(&local_key_b);
        let local_profile_b = signed_profile_for_device(
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            &local_key_b,
            "Local B",
            Some("local-b"),
            1,
            None,
            NOW,
        );
        let store_b = CollaborationContactStore::new(
            &data_root,
            principal_b,
            &protection_b.localhost_root,
            profile,
            &local_profile_b,
            &local_did_b,
        )
        .unwrap();

        let (remote_key, _) = generate_keypair();
        let remote_ad = advertisement(&remote_key, "Remote", NOW);
        store_a
            .store_local_advertisement(
                &advertisement_with_profile(&local_key_a, &local_profile_a, NOW),
                NOW,
            )
            .unwrap();
        let request = outgoing_request_with_profile(
            &local_key_a,
            &local_profile_a,
            &advertisement_profile_did(&remote_ad),
            &collaboration_message_envelope_sha256(&remote_ad),
            NOW + 1,
        );
        store_a
            .record_outgoing_contact_request(&request, &remote_ad, NOW + 1)
            .unwrap();

        assert!(store_b.snapshot().unwrap().contacts().is_empty());

        let a_path = rooted_localhost_fs_path(
            &data_root,
            &format!(
                "{}/{}",
                protection_a.localhost_root, PEOPLE_CONTACT_STATE_OBJECT_PATH
            ),
        )
        .unwrap();
        let b_path = rooted_localhost_fs_path(
            &data_root,
            &format!(
                "{}/{}",
                protection_b.localhost_root, PEOPLE_CONTACT_STATE_OBJECT_PATH
            ),
        )
        .unwrap();
        fs::create_dir_all(b_path.parent().unwrap()).unwrap();
        fs::set_permissions(b_path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::copy(&a_path, &b_path).unwrap();
        fs::set_permissions(&b_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store_b.snapshot().is_err());
    }

    #[test]
    fn duplicate_advertisements_and_requests_fail_closed_on_load() {
        let fixture = Fixture::new();
        let (remote_key, _) = generate_keypair();
        let remote_ad = advertisement(&remote_key, "Remote", NOW);
        fixture
            .store
            .store_local_advertisement(
                &advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW),
                NOW,
            )
            .unwrap();
        let request = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &advertisement_profile_did(&remote_ad),
            &collaboration_message_envelope_sha256(&remote_ad),
            NOW + 1,
        );
        fixture
            .store
            .record_outgoing_contact_request(&request, &remote_ad, NOW + 1)
            .unwrap();

        let state_path = store_state_path(&fixture);
        let original_bytes = crate::auth::read_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
        )
        .unwrap();
        let original: ContactStoreState = serde_json::from_slice(&original_bytes).unwrap();

        let mut duplicated_advertisements = original.clone();
        duplicated_advertisements
            .advertisements
            .push(duplicated_advertisements.advertisements[0].clone());
        crate::auth::write_protected_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
            &canonical_state_bytes(&duplicated_advertisements).unwrap(),
        )
        .unwrap();
        assert!(fixture.store.snapshot().is_err());

        let mut duplicated_requests = original.clone();
        duplicated_requests
            .requests
            .push(duplicated_requests.requests[0].clone());
        crate::auth::write_protected_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
            &canonical_state_bytes(&duplicated_requests).unwrap(),
        )
        .unwrap();
        assert!(fixture.store.snapshot().is_err());
    }

    #[test]
    fn foreign_profile_for_same_local_device_is_rejected_at_all_local_admission_points() {
        let fixture = Fixture::new();
        let foreign_profile = signed_profile_for_device(
            &SigningKey::from_bytes(&generate_keypair().0.to_bytes()),
            &fixture.local_key,
            "Foreign Local",
            Some("foreign-local"),
            1,
            None,
            NOW,
        );
        let foreign_local_ad =
            advertisement_with_profile(&fixture.local_key, &foreign_profile, NOW);
        assert!(fixture
            .store
            .store_local_advertisement(&foreign_local_ad, NOW)
            .is_err());

        let (remote_key, _) = generate_keypair();
        let remote_ad = advertisement(&remote_key, "Remote", NOW);
        let outgoing = outgoing_request_with_profile(
            &fixture.local_key,
            &foreign_profile,
            &advertisement_profile_did(&remote_ad),
            &collaboration_message_envelope_sha256(&remote_ad),
            NOW + 1,
        );
        assert!(fixture
            .store
            .record_outgoing_contact_request(&outgoing, &remote_ad, NOW + 1)
            .is_err());

        let foreign_state = ContactStoreState {
            schema: CONTACT_STORE_SCHEMA.to_string(),
            binding: fixture.store.state_binding(),
            discovery_enabled: false,
            published_local_advertisement_envelope_sha256: None,
            advertisements: vec![StoredEnvelope {
                envelope: encode_bytes(&foreign_local_ad),
            }],
            requests: Vec::new(),
            decisions: Vec::new(),
            revoked_request_hashes: Vec::new(),
            accepted_profile_heads: Vec::new(),
            removed_contacts: Vec::new(),
        };
        write_state_struct(&fixture, &foreign_state);
        let incoming = outgoing_request(
            &remote_key,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&foreign_local_ad),
            "Remote",
            NOW + 2,
        );
        assert!(fixture
            .store
            .record_incoming_contact_request(&incoming, NOW + 2)
            .is_err());
    }

    #[test]
    fn strict_v1_contact_store_rejects_schema_unknown_fields_noncanonical_ciphertext_and_path_violations(
    ) {
        let fixture = Fixture::new();
        fixture
            .store
            .store_local_advertisement(
                &advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW),
                NOW,
            )
            .unwrap();
        let state_path = store_state_path(&fixture);
        let original_plaintext = crate::auth::read_principal_root_object(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            &fixture.store.state_object_uri(),
            &state_path,
        )
        .unwrap();
        let original_state: ContactStoreState =
            serde_json::from_slice(&original_plaintext).unwrap();
        assert_eq!(original_state.schema, CONTACT_STORE_SCHEMA);
        fixture.store.snapshot().unwrap();

        let mut wrong_schema = original_state.clone();
        wrong_schema.schema = "elastos.people.contact-store/v2".to_string();
        write_state_struct(&fixture, &wrong_schema);
        assert!(fixture.store.snapshot().is_err());

        let mut unknown_schema = original_state.clone();
        unknown_schema.schema = "elastos.people.contact-store/v999".to_string();
        write_state_struct(&fixture, &unknown_schema);
        assert!(fixture.store.snapshot().is_err());

        let mut old_draft_schema = original_state.clone();
        old_draft_schema.schema = "elastos.people.discovery-contact-store/v5".to_string();
        write_state_struct(&fixture, &old_draft_schema);
        assert!(fixture.store.snapshot().is_err());

        let mut unknown = serde_json::to_value(&original_state).unwrap();
        unknown
            .as_object_mut()
            .unwrap()
            .insert("unexpected".to_string(), serde_json::json!(true));
        write_state(&fixture, &serde_json::to_vec(&unknown).unwrap());
        assert!(fixture.store.snapshot().is_err());

        let noncanonical = serde_json::to_string_pretty(&original_state).unwrap();
        write_state(&fixture, noncanonical.as_bytes());
        assert!(fixture.store.snapshot().is_err());

        fs::write(&state_path, b"not-a-principal-root-object").unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(fixture.store.snapshot().is_err());
    }

    #[test]
    fn caps_expiry_pruning_cross_request_convergence_remove_readd_and_group_bytes_are_stable() {
        let fixture = Fixture::new();
        let group_dir = fixture.data_root.join("collaboration/default-conversation");
        fs::create_dir_all(&group_dir).unwrap();
        fs::set_permissions(
            fixture.data_root.join("collaboration"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::set_permissions(&group_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let group_path = group_dir.join("sentinel.bin");
        fs::write(&group_path, b"group-stable").unwrap();
        let group_before = fs::read(&group_path).unwrap();

        let local_ad = advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW);
        fixture
            .store
            .store_local_advertisement(&local_ad, NOW)
            .unwrap();

        let (remote_key, _) = generate_keypair();
        let (remote_profile_signer, _) = generate_keypair();
        let remote_profile_old = signed_profile_for_device(
            &remote_profile_signer,
            &remote_key,
            "Remote Old",
            Some("remote-old"),
            1,
            None,
            NOW,
        );
        let remote_profile_new = signed_profile_for_device(
            &remote_profile_signer,
            &remote_key,
            "Remote New",
            Some("remote-new"),
            2,
            Some(&profile_hash(&remote_profile_old)),
            NOW + 5,
        );
        let remote_ad_old = advertisement_with_profile(&remote_key, &remote_profile_old, NOW);
        let remote_ad_new = advertisement_with_profile(&remote_key, &remote_profile_new, NOW + 5);

        let outgoing = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &remote_profile_old.document().profile_did,
            &collaboration_message_envelope_sha256(&remote_ad_old),
            NOW + 1,
        );
        fixture
            .store
            .record_outgoing_contact_request(&outgoing, &remote_ad_old, NOW + 1)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &remote_key,
                    &outgoing,
                    CollaborationContactDecision::Accepted,
                    NOW + 2,
                    verify_collaboration_discovery_advertisement(
                        &remote_ad_old,
                        &fixture.profile,
                        NOW,
                    )
                    .unwrap()
                    .profile_did(),
                ),
                NOW + 2,
            )
            .unwrap();

        let incoming = outgoing_request_with_profile(
            &remote_key,
            &remote_profile_new,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_ad),
            NOW + 6,
        );
        fixture
            .store
            .record_incoming_contact_request(&incoming, NOW + 6)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &fixture.local_key,
                    &incoming,
                    CollaborationContactDecision::Accepted,
                    NOW + 7,
                    fixture.local_profile.document().profile_did.as_str(),
                ),
                NOW + 7,
            )
            .unwrap();

        let snapshot = fixture.store.snapshot().unwrap();
        assert_eq!(snapshot.contacts().len(), 1);
        let contact = &snapshot.contacts()[0];
        assert_eq!(
            contact.remote_profile_did(),
            remote_profile_new.document().profile_did
        );
        assert_eq!(contact.remote_display_name(), "Remote New");
        assert_eq!(contact.remote_handle(), Some("remote-new"));
        assert_eq!(contact.added_at(), NOW + 7);

        fixture
            .store
            .record_local_contact_revocation(
                &revocation_bytes(
                    &fixture.local_key,
                    fixture.local_profile.document().profile_did.as_str(),
                    &remote_profile_new.document().profile_did,
                    NOW + 200,
                ),
                &fixture.local_profile,
                NOW + 200,
            )
            .unwrap();
        let after = fixture.store.snapshot().unwrap();
        assert!(after.contacts().is_empty());
        // Removal is visible, not a disappearance: the pair stays as a
        // removed relationship, still named by its signed presentation.
        assert_eq!(after.removed().len(), 1);
        assert_eq!(after.removed()[0].display_name(), "Remote New");
        assert!(after.removed()[0].removed_by_local());
        // The retained chain absorbs an exact replay of its request without
        // resurrecting the contact — the pair stays removed.
        assert_eq!(
            fixture
                .store
                .record_outgoing_contact_request(&outgoing, &remote_ad_old, NOW + 200)
                .unwrap(),
            ContactStoreWrite::Replayed
        );
        assert!(fixture.store.snapshot().unwrap().contacts().is_empty());
        assert_eq!(fixture.store.snapshot().unwrap().removed().len(), 1);

        let outgoing_readd = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &remote_profile_new.document().profile_did,
            &collaboration_message_envelope_sha256(&remote_ad_new),
            NOW + 201,
        );
        fixture
            .store
            .record_outgoing_contact_request(&outgoing_readd, &remote_ad_new, NOW + 201)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &remote_key,
                    &outgoing_readd,
                    CollaborationContactDecision::Accepted,
                    NOW + 202,
                    verify_collaboration_discovery_advertisement(
                        &remote_ad_new,
                        &fixture.profile,
                        NOW + 5,
                    )
                    .unwrap()
                    .profile_did(),
                ),
                NOW + 202,
            )
            .unwrap();

        let readded = fixture.store.snapshot().unwrap();
        // A fresh accepted chain reopens the pair: contact again, removed
        // record retired, same stable conversation.
        assert_eq!(readded.contacts().len(), 1);
        assert!(readded.removed().is_empty());
        assert_eq!(
            readded.contacts()[0].conversation_id(),
            contact.conversation_id()
        );

        assert_eq!(fs::read(&group_path).unwrap(), group_before);
    }

    /// Builds an accepted contact, then returns its signed head so a chain can
    /// be extended from it.
    fn accepted_contact_with_profile(
        fixture: &Fixture,
        profile_signer: &SigningKey,
        device_key: &SigningKey,
    ) -> VerifiedCollaborationProfileDocument {
        let local_ad = advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW);
        fixture
            .store
            .store_local_advertisement(&local_ad, NOW)
            .unwrap();
        let remote_profile = signed_profile_for_device(
            profile_signer,
            device_key,
            "Remote",
            Some("remote"),
            1,
            None,
            NOW,
        );
        let incoming = outgoing_request_with_profile(
            device_key,
            &remote_profile,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_ad),
            NOW + 1,
        );
        fixture
            .store
            .record_incoming_contact_request(&incoming, NOW + 1)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &fixture.local_key,
                    &incoming,
                    CollaborationContactDecision::Accepted,
                    NOW + 2,
                    fixture.local_profile.document().profile_did.as_str(),
                ),
                NOW + 2,
            )
            .unwrap();
        remote_profile
    }

    fn signed_bytes(profile: &VerifiedCollaborationProfileDocument) -> Vec<u8> {
        serde_json::to_vec(profile.signed_envelope()).unwrap()
    }

    #[test]
    fn accepted_profile_chain_updates_name_and_endpoint_without_changing_identity() {
        let fixture = Fixture::new();
        let (profile_signer, _) = generate_keypair();
        let (device_key, _) = generate_keypair();
        let head1 = accepted_contact_with_profile(&fixture, &profile_signer, &device_key);

        let before = fixture.store.snapshot().unwrap();
        let contact = &before.contacts()[0];
        let conversation_id = contact.conversation_id().to_string();
        let remote_profile_did = contact.remote_profile_did().to_string();
        assert_eq!(contact.remote_display_name(), "Remote");

        // Rename on a newly authorized device, one exact revision forward.
        let (device_key2, _) = generate_keypair();
        let head2 = signed_profile_for_device(
            &profile_signer,
            &device_key2,
            "Remote Renamed",
            Some("renamed"),
            2,
            Some(&profile_hash(&head1)),
            NOW + 3,
        );
        let before_substitution = fixture.store.snapshot().unwrap();
        assert!(fixture
            .store
            .apply_accepted_profile_chain(
                &[signed_bytes(&head2)],
                &device_did(&device_key),
                NOW + 4,
            )
            .is_err());
        assert_eq!(fixture.store.snapshot().unwrap(), before_substitution);
        assert_eq!(
            fixture
                .store
                .apply_accepted_profile_chain(
                    &[signed_bytes(&head2)],
                    &device_did(&device_key2),
                    NOW + 4,
                )
                .unwrap(),
            ContactStoreWrite::Recorded
        );

        let after = fixture.store.snapshot().unwrap();
        assert_eq!(after.contacts().len(), 1);
        let updated = &after.contacts()[0];
        assert_eq!(updated.remote_display_name(), "Remote Renamed");
        assert_eq!(updated.remote_handle(), Some("renamed"));
        assert_eq!(
            updated.remote_presence_device_did(),
            device_did(&device_key2)
        );
        // Identity and conversation are untouched: no new contact, no approval.
        assert_eq!(updated.remote_profile_did(), remote_profile_did);
        assert_eq!(updated.conversation_id(), conversation_id);

        // A byte-identical replay changes nothing.
        assert_eq!(
            fixture
                .store
                .apply_accepted_profile_chain(
                    &[signed_bytes(&head2)],
                    &device_did(&device_key2),
                    NOW + 5,
                )
                .unwrap(),
            ContactStoreWrite::Replayed
        );

        // Survives a reopen.
        let reopened = CollaborationContactStore::new(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            fixture.profile.clone(),
            &fixture.local_profile,
            &fixture.local_did,
        )
        .unwrap();
        let restored = reopened.snapshot().unwrap();
        assert_eq!(
            restored.contacts()[0].remote_display_name(),
            "Remote Renamed"
        );
        assert_eq!(restored.contacts()[0].conversation_id(), conversation_id);
    }

    #[test]
    fn accepted_profile_chain_fails_closed_on_gap_rollback_conflict_and_foreign_profile() {
        let fixture = Fixture::new();
        let (profile_signer, _) = generate_keypair();
        let (device_key, _) = generate_keypair();
        let head1 = accepted_contact_with_profile(&fixture, &profile_signer, &device_key);

        let head2 = signed_profile_for_device(
            &profile_signer,
            &device_key,
            "Remote Two",
            None,
            2,
            Some(&profile_hash(&head1)),
            NOW + 3,
        );
        let head3 = signed_profile_for_device(
            &profile_signer,
            &device_key,
            "Remote Three",
            None,
            3,
            Some(&profile_hash(&head2)),
            NOW + 4,
        );

        // A revision gap cannot be bridged.
        assert!(fixture
            .store
            .apply_accepted_profile_chain(
                &[signed_bytes(&head3)],
                &device_did(&device_key),
                NOW + 5
            )
            .is_err());

        // A conflicting same revision fails closed.
        let conflicting = signed_profile_for_device(
            &profile_signer,
            &device_key,
            "Remote Conflict",
            None,
            1,
            None,
            NOW + 6,
        );
        assert!(fixture
            .store
            .apply_accepted_profile_chain(
                &[signed_bytes(&conflicting)],
                &device_did(&device_key),
                NOW + 6,
            )
            .is_err());

        // A device the head does not authorize fails closed.
        let (stranger, _) = generate_keypair();
        assert!(fixture
            .store
            .apply_accepted_profile_chain(&[signed_bytes(&head2)], &device_did(&stranger), NOW + 7)
            .is_err());

        // A profile that is not an accepted contact fails closed.
        let (other_signer, _) = generate_keypair();
        let foreign =
            signed_profile_for_device(&other_signer, &device_key, "Stranger", None, 1, None, NOW);
        assert!(fixture
            .store
            .apply_accepted_profile_chain(
                &[signed_bytes(&foreign)],
                &device_did(&device_key),
                NOW + 8
            )
            .is_err());

        // The whole segment applies, and a rollback afterwards fails closed.
        assert_eq!(
            fixture
                .store
                .apply_accepted_profile_chain(
                    &[signed_bytes(&head2), signed_bytes(&head3)],
                    &device_did(&device_key),
                    NOW + 9,
                )
                .unwrap(),
            ContactStoreWrite::Recorded
        );
        assert_eq!(
            fixture.store.snapshot().unwrap().contacts()[0].remote_display_name(),
            "Remote Three"
        );
        assert!(fixture
            .store
            .apply_accepted_profile_chain(
                &[signed_bytes(&head2)],
                &device_did(&device_key),
                NOW + 10
            )
            .is_err());
    }

    #[test]
    fn device_rotation_keeps_profile_contact_identity_and_updates_presence_endpoint() {
        let fixture = Fixture::new();
        let local_ad = advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW);
        fixture
            .store
            .store_local_advertisement(&local_ad, NOW)
            .unwrap();

        let (remote_key_v1, _) = generate_keypair();
        let remote_did_v1 = device_did(&remote_key_v1);
        let (remote_profile_signer, _) = generate_keypair();
        let remote_profile_v1 = signed_profile_for_device(
            &remote_profile_signer,
            &remote_key_v1,
            "Remote",
            Some("remote"),
            1,
            None,
            NOW,
        );
        let remote_ad_v1 = advertisement_with_profile(&remote_key_v1, &remote_profile_v1, NOW);

        let outgoing_v1 = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &remote_profile_v1.document().profile_did,
            &collaboration_message_envelope_sha256(&remote_ad_v1),
            NOW + 1,
        );
        fixture
            .store
            .record_outgoing_contact_request(&outgoing_v1, &remote_ad_v1, NOW + 1)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &remote_key_v1,
                    &outgoing_v1,
                    CollaborationContactDecision::Accepted,
                    NOW + 2,
                    remote_profile_v1.document().profile_did.as_str(),
                ),
                NOW + 2,
            )
            .unwrap();

        let initial = fixture.store.snapshot().unwrap();
        assert_eq!(initial.contacts().len(), 1);
        let initial_contact = &initial.contacts()[0];
        let initial_conversation = initial_contact.conversation_id().to_string();
        assert_eq!(
            initial_contact.remote_profile_did(),
            remote_profile_v1.document().profile_did
        );
        assert_eq!(
            initial_contact.remote_presence_device_did(),
            remote_did_v1.as_str()
        );

        let (remote_key_v2, _) = generate_keypair();
        let remote_did_v2 = device_did(&remote_key_v2);
        let remote_profile_v2 = signed_profile_for_device(
            &remote_profile_signer,
            &remote_key_v2,
            "Remote Updated",
            Some("remote-updated"),
            2,
            Some(&profile_hash(&remote_profile_v1)),
            NOW + 3,
        );
        let incoming_v2 = outgoing_request_with_profile(
            &remote_key_v2,
            &remote_profile_v2,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_ad),
            NOW + 4,
        );
        fixture
            .store
            .record_incoming_contact_request(&incoming_v2, NOW + 4)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &fixture.local_key,
                    &incoming_v2,
                    CollaborationContactDecision::Accepted,
                    NOW + 5,
                    fixture.local_profile.document().profile_did.as_str(),
                ),
                NOW + 5,
            )
            .unwrap();

        let rotated = fixture.store.snapshot().unwrap();
        assert_eq!(rotated.contacts().len(), 1);
        let rotated_contact = &rotated.contacts()[0];
        assert_eq!(
            rotated_contact.remote_profile_did(),
            remote_profile_v2.document().profile_did
        );
        assert_eq!(rotated_contact.conversation_id(), initial_conversation);
        assert_eq!(
            rotated_contact.remote_presence_device_did(),
            remote_did_v2.as_str()
        );

        let incoming_v1_late = outgoing_request_with_profile(
            &remote_key_v1,
            &remote_profile_v1,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_ad),
            NOW + 6,
        );
        fixture
            .store
            .record_incoming_contact_request(&incoming_v1_late, NOW + 6)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &fixture.local_key,
                    &incoming_v1_late,
                    CollaborationContactDecision::Accepted,
                    NOW + 7,
                    fixture.local_profile.document().profile_did.as_str(),
                ),
                NOW + 7,
            )
            .unwrap();

        let reopened = CollaborationContactStore::new(
            &fixture.data_root,
            &fixture.principal_id,
            &fixture.localhost_root,
            fixture.profile.clone(),
            &fixture.local_profile,
            &fixture.local_did,
        )
        .unwrap();
        let after_restart = reopened.snapshot().unwrap();
        assert_eq!(after_restart.contacts().len(), 1);
        let retained_contact = &after_restart.contacts()[0];
        assert_eq!(
            retained_contact.remote_profile_did(),
            remote_profile_v2.document().profile_did
        );
        assert_eq!(retained_contact.conversation_id(), initial_conversation);
        assert_eq!(retained_contact.remote_display_name(), "Remote Updated");
        assert_eq!(retained_contact.remote_handle(), Some("remote-updated"));
        assert_eq!(
            retained_contact.remote_presence_device_did(),
            remote_did_v2.as_str()
        );
    }

    #[test]
    fn accepted_and_declined_decisions_stop_request_resend_and_only_accept_creates_contact() {
        let fixture = Fixture::new();
        fixture
            .store
            .store_local_advertisement(
                &advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW),
                NOW,
            )
            .unwrap();

        let (accepted_remote_key, _) = generate_keypair();
        let accepted_ad = advertisement(&accepted_remote_key, "Accepted Remote", NOW);
        let accepted_request = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &advertisement_profile_did(&accepted_ad),
            &collaboration_message_envelope_sha256(&accepted_ad),
            NOW + 1,
        );
        fixture
            .store
            .record_outgoing_contact_request(&accepted_request, &accepted_ad, NOW + 1)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &accepted_remote_key,
                    &accepted_request,
                    CollaborationContactDecision::Accepted,
                    NOW + 2,
                    verify_collaboration_discovery_advertisement(
                        &accepted_ad,
                        &fixture.profile,
                        NOW,
                    )
                    .unwrap()
                    .profile_did(),
                ),
                NOW + 2,
            )
            .unwrap();

        let (declined_remote_key, _) = generate_keypair();
        let declined_ad = advertisement(&declined_remote_key, "Declined Remote", NOW + 3);
        let declined_request = outgoing_request_with_profile(
            &fixture.local_key,
            &fixture.local_profile,
            &advertisement_profile_did(&declined_ad),
            &collaboration_message_envelope_sha256(&declined_ad),
            NOW + 4,
        );
        fixture
            .store
            .record_outgoing_contact_request(&declined_request, &declined_ad, NOW + 4)
            .unwrap();
        fixture
            .store
            .record_contact_decision_receipt(
                &decision_receipt(
                    &declined_remote_key,
                    &declined_request,
                    CollaborationContactDecision::Declined,
                    NOW + 5,
                    verify_collaboration_discovery_advertisement(
                        &declined_ad,
                        &fixture.profile,
                        NOW + 3,
                    )
                    .unwrap()
                    .profile_did(),
                ),
                NOW + 5,
            )
            .unwrap();

        assert!(fixture
            .store
            .stored_outgoing_contact_request(
                &collaboration_message_envelope_sha256(&accepted_ad),
                verify_collaboration_discovery_advertisement(&accepted_ad, &fixture.profile, NOW)
                    .unwrap()
                    .profile_did(),
                NOW + 6,
            )
            .unwrap()
            .is_none());
        assert!(fixture
            .store
            .stored_outgoing_contact_request(
                &collaboration_message_envelope_sha256(&declined_ad),
                verify_collaboration_discovery_advertisement(
                    &declined_ad,
                    &fixture.profile,
                    NOW + 3,
                )
                .unwrap()
                .profile_did(),
                NOW + 6,
            )
            .unwrap()
            .is_none());
        let snapshot = fixture.store.snapshot().unwrap();
        assert_eq!(snapshot.contacts().len(), 1);
        assert_eq!(
            snapshot.contacts()[0].remote_display_name(),
            "Accepted Remote"
        );
    }

    #[test]
    fn mutation_path_rejects_total_request_cap() {
        let fixture = Fixture::new();
        let local_ad = advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW);
        fixture
            .store
            .store_local_advertisement(&local_ad, NOW)
            .unwrap();
        for offset in 0..MAX_REQUEST_RECORDS {
            let (remote_key, _) = generate_keypair();
            let created_at = NOW + u64::try_from(offset).unwrap();
            let request = outgoing_request(
                &remote_key,
                &fixture.local_profile.document().profile_did,
                &collaboration_message_envelope_sha256(&local_ad),
                &format!("Remote {offset}"),
                created_at + 1,
            );
            fixture
                .store
                .record_incoming_contact_request(&request, created_at + 1)
                .unwrap();
        }

        let (overflow_key, _) = generate_keypair();
        let overflow_request = outgoing_request(
            &overflow_key,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_ad),
            "Remote Overflow",
            NOW + u64::try_from(MAX_REQUEST_RECORDS).unwrap() + 1,
        );
        assert!(fixture
            .store
            .record_incoming_contact_request(
                &overflow_request,
                NOW + u64::try_from(MAX_REQUEST_RECORDS).unwrap() + 1
            )
            .is_err());
    }

    #[test]
    fn expiry_clears_published_pointer_without_pruning_referenced_evidence() {
        let fixture = Fixture::new();
        let local_advertisement =
            advertisement_with_profile(&fixture.local_key, &fixture.local_profile, NOW);
        fixture
            .store
            .store_local_advertisement(&local_advertisement, NOW)
            .unwrap();
        let (remote_key, _) = generate_keypair();
        let request = outgoing_request(
            &remote_key,
            &fixture.local_profile.document().profile_did,
            &collaboration_message_envelope_sha256(&local_advertisement),
            "Remote",
            NOW + 1,
        );
        fixture
            .store
            .record_incoming_contact_request(&request, NOW + 1)
            .unwrap();

        let expired_at = NOW + COLLABORATION_DISCOVERY_ADVERTISEMENT_TTL_SECS + 1;
        assert!(fixture
            .store
            .clear_expired_published_local_advertisement(expired_at)
            .unwrap());
        assert!(fixture
            .store
            .published_local_advertisement(expired_at)
            .unwrap()
            .is_none());
        assert_eq!(fixture.store.pending_incoming_requests().unwrap().len(), 1);
        assert!(fixture
            .store
            .load_state()
            .unwrap()
            .unwrap()
            .state
            .published_local_advertisement_envelope_sha256
            .is_none());
    }

    #[test]
    fn stable_conversation_id_is_order_independent_and_network_separated() {
        let (left_key, _) = generate_keypair();
        let (right_key, _) = generate_keypair();
        let a = device_did(&left_key);
        let b = device_did(&right_key);
        assert_eq!(
            stable_direct_conversation_id(NETWORK, &a, &b).unwrap(),
            stable_direct_conversation_id(NETWORK, &b, &a).unwrap()
        );
        assert_ne!(
            stable_direct_conversation_id(NETWORK, &a, &b).unwrap(),
            stable_direct_conversation_id("other.network", &a, &b).unwrap()
        );
        assert!(stable_direct_conversation_id(NETWORK, &a, &a).is_err());
    }
}
