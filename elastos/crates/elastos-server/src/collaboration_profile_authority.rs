use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Context};
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::Digest;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use crate::crypto::decode_did_key;

pub(crate) const COLLABORATION_PROFILE_AUTHORITY_BUNDLE_SCHEMA_V1: &str =
    "elastos.profile-authority-bundle/v1";
pub(crate) const COLLABORATION_PROFILE_DOCUMENT_SCHEMA_V1: &str = "elastos.profile-document/v1";
pub(crate) const COLLABORATION_PROFILE_DOCUMENT_SIGNATURE_DOMAIN_V1: &str =
    "elastos.profile-document.v1";
pub(crate) const COLLABORATION_ENDPOINT_PURPOSE: &str = "collaboration";
const RESERVED_PROFILE_DISPLAY_NAMES: &[&str] = &["ElastOS user", "ElastOS Home", "Person"];
const MAX_PROFILE_DOCUMENT_BYTES: usize = 16 * 1024;
const MAX_PROFILE_AUTHORITY_BUNDLE_BYTES: usize = 128 * 1024;
const MAX_PROFILE_HANDLE_BYTES: usize = 128;
const MAX_PROFILE_ENDPOINT_BINDINGS: usize = 8;
const MAX_PROFILE_SIGNING_AUTHORITIES: usize = 8;
const MAX_PROFILE_SIGNING_DELEGATIONS: usize = 8;
const MAX_PROFILE_SIGNING_OBJECT_TYPES: usize = 16;
const DEFAULT_PROFILE_SIGNING_SCOPES: &[(&str, &[&str])] = &[
    (
        "chat",
        &[
            "elastos.chat.direct-message/v1",
            "elastos.chat.message/v1",
            "elastos.chat.presence/v1",
            "elastos.collaboration.acceptance-receipt/v1",
            "elastos.room.accept.v1",
            "elastos.room.invite.v1",
            "elastos.room.join-invite.v1",
        ],
    ),
    (
        "people",
        &[
            "elastos.collaboration.acceptance-receipt/v1",
            "elastos.people.contact-decision-receipt/v1",
            "elastos.people.contact-request/v1",
            "elastos.people.contact-revocation/v1",
            "elastos.people.discovery.advertisement/v1",
            "elastos.people.discovery.mailbox-poll/v1",
            "elastos.people.discovery.withdrawal/v1",
            "elastos.people.profile-update/v1",
        ],
    ),
];
/// Upper bound on the signed revisions retained for announcement, including the
/// head. A contact further behind than this cannot be caught up by an exact
/// chain segment and needs a new contact approval.
pub(crate) const MAX_RETAINED_PROFILE_REVISIONS: usize = 8;

fn profile_authority_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationProfileDocument {
    pub collaboration_endpoint: CollaborationProfileEndpointAuthority,
    pub collaboration_signers: Vec<CollaborationProfileSigningAuthority>,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_profile_sha256: Option<String>,
    pub profile_did: String,
    pub revision: u64,
    pub schema: String,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationProfileEndpointAuthority {
    pub bindings: Vec<CollaborationProfileEndpointBinding>,
    pub generation: u64,
    pub purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationProfileEndpointBinding {
    pub endpoint_did: String,
    pub profile_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationProfileSigningAuthority {
    pub delegations: Vec<CollaborationProfileSigningDelegation>,
    pub generation: u64,
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationProfileSigningDelegation {
    pub object_types: Vec<String>,
    pub profile_revision: u64,
    pub signer_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedCollaborationProfileDocument {
    signed_envelope: SignedCollaborationProfileDocument,
    document: CollaborationProfileDocument,
}

impl VerifiedCollaborationProfileDocument {
    pub(crate) fn document(&self) -> &CollaborationProfileDocument {
        &self.document
    }

    pub(crate) fn signed_envelope(&self) -> &SignedCollaborationProfileDocument {
        &self.signed_envelope
    }

    pub(crate) fn authorizes_endpoint(&self, endpoint_did: &str) -> bool {
        self.document.collaboration_endpoint.purpose == COLLABORATION_ENDPOINT_PURPOSE
            && self
                .document
                .collaboration_endpoint
                .bindings
                .iter()
                .any(|binding| binding.endpoint_did == endpoint_did)
    }

    /// Return the only routable endpoint declared by this Profile.
    ///
    /// The initial collaboration protocol has no endpoint-selection policy.
    /// A Profile with zero or multiple bindings is therefore valid authority
    /// data, but cannot be used for direct delivery until selection is defined.
    pub(crate) fn sole_endpoint_did(&self) -> anyhow::Result<&str> {
        match self.document.collaboration_endpoint.bindings.as_slice() {
            [binding] => Ok(binding.endpoint_did.as_str()),
            _ => anyhow::bail!("collaboration Profile does not declare exactly one endpoint"),
        }
    }

    pub(crate) fn authorizes_signer(
        &self,
        signer_did: &str,
        service: &str,
        object_type: &str,
    ) -> bool {
        self.document
            .collaboration_signers
            .iter()
            .find(|authority| authority.service == service)
            .is_some_and(|authority| {
                authority.delegations.iter().any(|delegation| {
                    delegation.signer_did == signer_did
                        && delegation
                            .object_types
                            .iter()
                            .any(|candidate| candidate == object_type)
                })
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedCollaborationProfileDocument {
    payload: CollaborationProfileDocument,
    signature: String,
    signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CollaborationProfileAuthorityBundle {
    schema: String,
    profile_signing_seed_hex: String,
    /// Revisions preceding the head, oldest first. Retained so an update can be
    /// announced as an exact chain segment to a contact that missed one.
    previous_profiles: Vec<SignedCollaborationProfileDocument>,
    signed_profile: SignedCollaborationProfileDocument,
}

pub(crate) fn load_existing_device_did(data_dir: &Path) -> anyhow::Result<Option<String>> {
    Ok(load_existing_device_signing_key(data_dir)?.map(|(_, did)| did))
}

pub(crate) fn load_existing_device_signing_key(
    data_dir: &Path,
) -> anyhow::Result<Option<(SigningKey, String)>> {
    let device_key = data_dir.join("identity").join("device.key");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = match options.open(&device_key) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to open {}", device_key.display()))
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect {}", device_key.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("device.key must be a regular file");
    }
    let mut bytes = Vec::with_capacity(32);
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", device_key.display()))?;
    let bytes_len = bytes.len();
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("device.key has invalid length {} (expected 32)", bytes_len))?;
    let (signing_key, did) = elastos_identity::derive_did(&seed);
    if did.trim().is_empty() {
        anyhow::bail!("derived local device DID is empty");
    }
    decode_did_key(&did).context("invalid local device DID")?;
    Ok(Some((signing_key, did)))
}

pub(crate) fn profile_authority_object_uri(localhost_root: &str) -> String {
    format!("{localhost_root}/.AppData/ElastOS/Profile/profile-authority.json")
}

pub(crate) fn profile_authority_path(
    data_dir: &Path,
    localhost_root: &str,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &profile_authority_object_uri(localhost_root))
        .ok_or_else(|| anyhow!("invalid profile authority root"))
}

pub(crate) fn load_profile_authority(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<VerifiedCollaborationProfileDocument>> {
    load_bundle_state(data_dir, principal_id, localhost_root)
        .map(|state| state.map(|state| state.verified))
}

pub(crate) fn update_profile_authority(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    proof_binding_id: &str,
    display_name: &str,
    handle: Option<&str>,
    updated_at: u64,
) -> anyhow::Result<VerifiedCollaborationProfileDocument> {
    let _guard = profile_authority_mutation_lock()
        .lock()
        .map_err(|_| anyhow!("profile authority mutation lock poisoned"))?;
    require_profile_authority_passkey_binding(data_dir, principal_id, Some(proof_binding_id))?;
    let (local_device_did, display_name, handle) =
        prepare_profile_authority_update(data_dir, display_name, handle)?;
    let path = profile_authority_path(data_dir, localhost_root)?;
    let protection =
        crate::auth::load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    if protection.is_none() {
        anyhow::bail!("protected principal root is required for profile authority");
    }

    let existing = load_bundle_state(data_dir, principal_id, localhost_root)?;
    let signing_key = match existing.as_ref() {
        Some(state) => state.signing_key.clone(),
        None => random_profile_signing_key()?,
    };
    let profile_did = existing
        .as_ref()
        .map(|state| state.verified.document().profile_did.clone())
        .unwrap_or_else(|| crate::crypto::encode_signing_key_did(&signing_key));
    let revision = existing
        .as_ref()
        .map(|state| {
            state
                .verified
                .document()
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("profile revision overflow"))
        })
        .transpose()?
        .unwrap_or(1);
    let previous_updated_at = existing
        .as_ref()
        .map(|state| state.verified.document().updated_at)
        .unwrap_or(0);
    let min_updated_at = if previous_updated_at == 0 {
        1
    } else {
        previous_updated_at
            .checked_add(1)
            .ok_or_else(|| anyhow!("profile updated_at overflow"))?
    };
    let effective_updated_at = updated_at.max(min_updated_at);
    let previous_profile_sha256 = existing
        .as_ref()
        .map(|state| sha256_label_from_envelope(state.verified.signed_envelope()))
        .transpose()?;
    let (collaboration_endpoint, collaboration_signers) = build_collaboration_authority(
        existing.as_ref().map(|state| state.verified.document()),
        vec![local_device_did.clone()],
        vec![local_device_did],
        revision,
    )?;
    let document = CollaborationProfileDocument {
        schema: COLLABORATION_PROFILE_DOCUMENT_SCHEMA_V1.to_string(),
        profile_did,
        collaboration_endpoint,
        collaboration_signers,
        display_name,
        handle,
        revision,
        previous_profile_sha256,
        updated_at: effective_updated_at,
    };
    let signed_profile = sign_profile_document(&signing_key, document)?;
    // Roll the retained ring: the outgoing head becomes the newest retained
    // revision, and the oldest are dropped once the ring is full.
    let previous_profiles = match existing.as_ref() {
        None => Vec::new(),
        Some(state) => {
            let mut retained = state.previous_profiles.clone();
            retained.push(state.verified.signed_envelope().clone());
            while retained.len() >= MAX_RETAINED_PROFILE_REVISIONS {
                retained.remove(0);
            }
            retained
        }
    };
    let bundle = CollaborationProfileAuthorityBundle {
        schema: COLLABORATION_PROFILE_AUTHORITY_BUNDLE_SCHEMA_V1.to_string(),
        profile_signing_seed_hex: hex::encode(signing_key.to_bytes()),
        previous_profiles,
        signed_profile,
    };
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    if bytes.len() > MAX_PROFILE_AUTHORITY_BUNDLE_BYTES {
        anyhow::bail!("profile authority bundle is too large");
    }
    prepare_profile_authority_parent_for_write(data_dir, &path)?;
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &profile_authority_object_uri(localhost_root),
        &path,
        &bytes,
    )?;
    load_profile_authority(data_dir, principal_id, localhost_root)?
        .ok_or_else(|| anyhow!("profile authority bundle missing after write"))
}

pub(crate) fn validate_profile_authority_update(
    data_dir: &Path,
    display_name: &str,
    handle: Option<&str>,
) -> anyhow::Result<()> {
    prepare_profile_authority_update(data_dir, display_name, handle).map(|_| ())
}

fn prepare_profile_authority_update(
    data_dir: &Path,
    display_name: &str,
    handle: Option<&str>,
) -> anyhow::Result<(String, String, Option<String>)> {
    let local_device_did = load_existing_device_did(data_dir)?.ok_or_else(|| {
        anyhow!("existing local device DID required before profile authority setup")
    })?;
    let display_name = clean_profile_display_name(display_name)?;
    let handle = clean_profile_handle(handle)?;
    Ok((local_device_did, display_name, handle))
}

pub(crate) fn require_profile_authority_passkey_binding(
    data_dir: &Path,
    principal_id: &str,
    proof_binding_id: Option<&str>,
) -> anyhow::Result<()> {
    let proof_binding_id =
        proof_binding_id.ok_or_else(|| anyhow!("proof-bound passkey session required"))?;
    if !proof_binding_id.starts_with("proof:passkey:") {
        anyhow::bail!("proof-bound passkey session required");
    }
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if principal.principal_id != principal_id {
        anyhow::bail!("proof binding does not match the active principal");
    }
    Ok(())
}

fn decode_profile_authority_bundle(
    bytes: &[u8],
) -> anyhow::Result<CollaborationProfileAuthorityBundle> {
    if bytes.len() > MAX_PROFILE_AUTHORITY_BUNDLE_BYTES {
        anyhow::bail!("profile authority bundle is too large");
    }
    let bundle: CollaborationProfileAuthorityBundle =
        serde_json::from_slice(bytes).context("invalid profile authority bundle")?;
    if bundle.schema != COLLABORATION_PROFILE_AUTHORITY_BUNDLE_SCHEMA_V1 {
        anyhow::bail!("unsupported profile authority bundle schema");
    }
    let seed = decode_profile_signing_seed(&bundle.profile_signing_seed_hex)?;
    let signing_key = SigningKey::from_bytes(&seed);
    let expected_signer_did = crate::crypto::encode_signing_key_did(&signing_key);
    if bundle.signed_profile.signer_did != expected_signer_did {
        anyhow::bail!("profile authority signer DID mismatch");
    }
    validate_retained_profile_chain(&bundle)?;
    Ok(bundle)
}

/// The retained revisions must form an exact, contiguous chain ending at the
/// head. Each step advances the revision by one and names the previous signed
/// envelope hash, so an announced segment cannot skip or reorder a revision.
fn validate_retained_profile_chain(
    bundle: &CollaborationProfileAuthorityBundle,
) -> anyhow::Result<()> {
    if bundle.previous_profiles.len() >= MAX_RETAINED_PROFILE_REVISIONS {
        anyhow::bail!("profile authority retains too many revisions");
    }
    let mut expected_previous: Option<(u64, String, CollaborationProfileDocument)> = None;
    for signed in bundle
        .previous_profiles
        .iter()
        .chain(std::iter::once(&bundle.signed_profile))
    {
        let verified = verify_signed_profile_document(signed)?;
        let document = verified.document();
        if document.profile_did != bundle.signed_profile.payload.profile_did {
            anyhow::bail!("retained profile revision belongs to another profile");
        }
        if let Some((previous_revision, previous_hash, previous_document)) = expected_previous {
            if document.revision != previous_revision.saturating_add(1) {
                anyhow::bail!("retained profile revisions are not contiguous");
            }
            if document.previous_profile_sha256.as_deref() != Some(previous_hash.as_str()) {
                anyhow::bail!("retained profile revision breaks the chain hash");
            }
            validate_profile_authority_transition(&previous_document, document)?;
        }
        expected_previous = Some((
            document.revision,
            sha256_label_from_envelope(signed)?,
            document.clone(),
        ));
    }
    Ok(())
}

/// The decrypted profile authority bundle — signing seed, retained revision
/// ring, and signed head — for the Full Recovery Bundle. `None` when this
/// principal has no saved Profile. The caller owns transport protection; this
/// is the same trust class as the Wallet recovery keys riding beside it.
pub(crate) fn export_profile_authority_bundle_for_recovery(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let path = profile_authority_path(data_dir, localhost_root)?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &profile_authority_object_uri(localhost_root),
        &path,
    )?;
    // Never export material this Runtime cannot itself verify.
    let bundle = decode_profile_authority_bundle(&bytes)?;
    verify_signed_profile_document(&bundle.signed_profile)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Writes a recovered profile authority bundle back under the restored
/// protected root, verifying it first. Returns the verified head so the
/// caller can rebind the current device against it.
pub(crate) fn restore_profile_authority_bundle_for_recovery(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
    bundle_value: &serde_json::Value,
) -> anyhow::Result<VerifiedCollaborationProfileDocument> {
    let bytes = serde_json::to_vec_pretty(bundle_value)?;
    let bundle = decode_profile_authority_bundle(&bytes)?;
    let verified = verify_signed_profile_document(&bundle.signed_profile)?;
    let seed_key = SigningKey::from_bytes(&decode_profile_signing_seed(
        &bundle.profile_signing_seed_hex,
    )?);
    if crate::crypto::encode_signing_key_did(&seed_key) != verified.document().profile_did {
        anyhow::bail!("recovered profile signing seed does not control the recovered Profile DID");
    }
    let path = profile_authority_path(data_dir, localhost_root)?;
    if crate::auth::load_principal_root_protection(data_dir, principal_id, localhost_root)?
        .is_none()
    {
        anyhow::bail!("protected principal root is required for profile authority");
    }
    prepare_profile_authority_parent_for_write(data_dir, &path)?;
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &profile_authority_object_uri(localhost_root),
        &path,
        &bytes,
    )?;
    Ok(verified)
}

fn prepare_profile_authority_parent_for_write(data_dir: &Path, path: &Path) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("profile authority path has no parent"))?;
    crate::auth::create_owner_only_dir_all(data_dir, parent)
}

/// The exact signed chain segment to announce, oldest first, ending at the head.
/// A receiver applies each step under the strict next-revision and
/// previous-hash rules.
pub(crate) fn profile_chain_segment(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<Vec<SignedCollaborationProfileDocument>>> {
    let Some(state) = load_bundle_state(data_dir, principal_id, localhost_root)? else {
        return Ok(None);
    };
    let mut segment = state.previous_profiles;
    segment.push(state.verified.signed_envelope().clone());
    Ok(Some(segment))
}

pub(crate) fn verify_signed_profile_document(
    signed_profile: &SignedCollaborationProfileDocument,
) -> anyhow::Result<VerifiedCollaborationProfileDocument> {
    if serde_json::to_vec(signed_profile)?.len() > MAX_PROFILE_DOCUMENT_BYTES {
        anyhow::bail!("profile document is too large");
    }
    let payload_bytes = serde_json::to_vec(&signed_profile.payload)?;
    crate::crypto::verify_domain_separated_signature(
        &signed_profile.signer_did,
        COLLABORATION_PROFILE_DOCUMENT_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
        &signed_profile.signature,
    )?;
    validate_profile_document(&signed_profile.payload, &signed_profile.signer_did)?;
    Ok(VerifiedCollaborationProfileDocument {
        signed_envelope: signed_profile.clone(),
        document: signed_profile.payload.clone(),
    })
}

fn validate_profile_document(
    document: &CollaborationProfileDocument,
    signer_did: &str,
) -> anyhow::Result<()> {
    if document.schema != COLLABORATION_PROFILE_DOCUMENT_SCHEMA_V1 {
        anyhow::bail!("unsupported profile document schema");
    }
    decode_did_key(&document.profile_did).context("invalid profile DID")?;
    if document.profile_did != signer_did {
        anyhow::bail!("profile DID signer mismatch");
    }
    clean_profile_display_name(&document.display_name)?;
    let _ = clean_profile_handle(document.handle.as_deref())?;
    if document.revision == 0 {
        anyhow::bail!("profile revision must be positive");
    }
    if document.revision == 1 {
        if document.previous_profile_sha256.is_some() {
            anyhow::bail!("initial profile revision must not declare a previous profile hash");
        }
    } else if !document
        .previous_profile_sha256
        .as_deref()
        .is_some_and(is_sha256_label)
    {
        anyhow::bail!("profile revision chain is invalid");
    }
    if document.updated_at == 0 {
        anyhow::bail!("profile updated_at must be positive");
    }
    validate_profile_collaboration_authority(document)?;
    Ok(())
}

fn build_collaboration_authority(
    previous: Option<&CollaborationProfileDocument>,
    mut endpoint_dids: Vec<String>,
    mut signer_dids: Vec<String>,
    profile_revision: u64,
) -> anyhow::Result<(
    CollaborationProfileEndpointAuthority,
    Vec<CollaborationProfileSigningAuthority>,
)> {
    endpoint_dids.sort();
    if endpoint_dids.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("duplicate collaboration endpoint DID");
    }
    signer_dids.sort();
    if signer_dids.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("duplicate collaboration signer DID");
    }
    let endpoint_bindings = endpoint_dids
        .iter()
        .map(|endpoint_did| CollaborationProfileEndpointBinding {
            endpoint_did: endpoint_did.clone(),
            profile_revision,
        })
        .collect::<Vec<_>>();
    let endpoint_generation = match previous {
        None => 1,
        Some(previous) => next_authority_generation(
            previous.collaboration_endpoint.generation,
            !same_endpoint_bindings(
                &previous.collaboration_endpoint.bindings,
                &endpoint_bindings,
            ),
        )?,
    };
    let endpoint = CollaborationProfileEndpointAuthority {
        bindings: endpoint_bindings,
        generation: endpoint_generation,
        purpose: COLLABORATION_ENDPOINT_PURPOSE.to_string(),
    };

    let mut signers = Vec::new();
    for (service, object_types) in DEFAULT_PROFILE_SIGNING_SCOPES {
        let delegations = signer_dids
            .iter()
            .map(|signer_did| CollaborationProfileSigningDelegation {
                object_types: object_types
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect(),
                profile_revision,
                signer_did: signer_did.clone(),
            })
            .collect::<Vec<_>>();
        let previous_authority = previous.and_then(|document| {
            document
                .collaboration_signers
                .iter()
                .find(|authority| authority.service == *service)
        });
        let generation = match previous_authority {
            None => 1,
            Some(previous) => next_authority_generation(
                previous.generation,
                !same_signing_delegations(&previous.delegations, &delegations),
            )?,
        };
        signers.push(CollaborationProfileSigningAuthority {
            delegations,
            generation,
            service: (*service).to_string(),
        });
    }
    if let Some(previous) = previous {
        for authority in &previous.collaboration_signers {
            if DEFAULT_PROFILE_SIGNING_SCOPES
                .iter()
                .any(|(service, _)| *service == authority.service)
            {
                continue;
            }
            let mut authority = authority.clone();
            for delegation in &mut authority.delegations {
                delegation.profile_revision = profile_revision;
            }
            signers.push(authority);
        }
    }
    signers.sort_by(|left, right| left.service.cmp(&right.service));
    Ok((endpoint, signers))
}

fn next_authority_generation(current: u64, changed: bool) -> anyhow::Result<u64> {
    if !changed {
        return Ok(current);
    }
    current
        .checked_add(1)
        .ok_or_else(|| anyhow!("profile authority generation overflow"))
}

fn same_endpoint_bindings(
    left: &[CollaborationProfileEndpointBinding],
    right: &[CollaborationProfileEndpointBinding],
) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.endpoint_did == right.endpoint_did)
}

fn same_signing_delegations(
    left: &[CollaborationProfileSigningDelegation],
    right: &[CollaborationProfileSigningDelegation],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.signer_did == right.signer_did && left.object_types == right.object_types
        })
}

fn validate_profile_collaboration_authority(
    document: &CollaborationProfileDocument,
) -> anyhow::Result<()> {
    let endpoint = &document.collaboration_endpoint;
    if endpoint.purpose != COLLABORATION_ENDPOINT_PURPOSE
        || endpoint.generation == 0
        || endpoint.bindings.len() > MAX_PROFILE_ENDPOINT_BINDINGS
    {
        anyhow::bail!("profile collaboration endpoint authority is invalid");
    }
    if document.revision == 1 && endpoint.generation != 1 {
        anyhow::bail!("initial profile endpoint generation must be one");
    }
    if endpoint
        .bindings
        .windows(2)
        .any(|pair| pair[0].endpoint_did >= pair[1].endpoint_did)
    {
        anyhow::bail!("profile collaboration endpoint bindings are not canonical");
    }
    for binding in &endpoint.bindings {
        decode_did_key(&binding.endpoint_did).context("invalid collaboration endpoint DID")?;
        if binding.profile_revision != document.revision {
            anyhow::bail!("collaboration endpoint binding has the wrong Profile revision");
        }
    }

    if document.collaboration_signers.len() > MAX_PROFILE_SIGNING_AUTHORITIES
        || document
            .collaboration_signers
            .windows(2)
            .any(|pair| pair[0].service >= pair[1].service)
    {
        anyhow::bail!("profile signing authorities are not canonical");
    }
    for authority in &document.collaboration_signers {
        crate::collaboration_protocol::validate_service(&authority.service)?;
        if authority.generation == 0
            || authority.delegations.len() > MAX_PROFILE_SIGNING_DELEGATIONS
        {
            anyhow::bail!("profile signing authority is invalid");
        }
        if document.revision == 1 && authority.generation != 1 {
            anyhow::bail!("initial profile signing generation must be one");
        }
        if authority
            .delegations
            .windows(2)
            .any(|pair| pair[0].signer_did >= pair[1].signer_did)
        {
            anyhow::bail!("profile signing delegations are not canonical");
        }
        for delegation in &authority.delegations {
            decode_did_key(&delegation.signer_did)
                .context("invalid Profile-scoped collaboration signer DID")?;
            if delegation.profile_revision != document.revision
                || delegation.object_types.is_empty()
                || delegation.object_types.len() > MAX_PROFILE_SIGNING_OBJECT_TYPES
                || delegation
                    .object_types
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
            {
                anyhow::bail!("profile signing delegation is invalid");
            }
            for object_type in &delegation.object_types {
                crate::collaboration_protocol::validate_payload_type(object_type)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_profile_authority_transition(
    previous: &CollaborationProfileDocument,
    next: &CollaborationProfileDocument,
) -> anyhow::Result<()> {
    if previous.profile_did != next.profile_did
        || next.revision != previous.revision.saturating_add(1)
    {
        anyhow::bail!("profile authority transition changes identity or skips a revision");
    }
    let endpoint_changed = !same_endpoint_bindings(
        &previous.collaboration_endpoint.bindings,
        &next.collaboration_endpoint.bindings,
    );
    let expected_endpoint_generation =
        next_authority_generation(previous.collaboration_endpoint.generation, endpoint_changed)?;
    if next.collaboration_endpoint.purpose != previous.collaboration_endpoint.purpose
        || next.collaboration_endpoint.generation != expected_endpoint_generation
    {
        anyhow::bail!("profile endpoint rotation has an invalid generation");
    }

    for previous_authority in &previous.collaboration_signers {
        if !next
            .collaboration_signers
            .iter()
            .any(|authority| authority.service == previous_authority.service)
        {
            anyhow::bail!("profile signing authority must retain its generation tombstone");
        }
    }
    for next_authority in &next.collaboration_signers {
        let expected_generation = match previous
            .collaboration_signers
            .iter()
            .find(|authority| authority.service == next_authority.service)
        {
            None => 1,
            Some(previous_authority) => next_authority_generation(
                previous_authority.generation,
                !same_signing_delegations(
                    &previous_authority.delegations,
                    &next_authority.delegations,
                ),
            )?,
        };
        if next_authority.generation != expected_generation {
            anyhow::bail!("profile signer rotation has an invalid generation");
        }
    }
    Ok(())
}

fn sign_profile_document(
    signing_key: &SigningKey,
    document: CollaborationProfileDocument,
) -> anyhow::Result<SignedCollaborationProfileDocument> {
    validate_profile_document(&document, &document.profile_did)?;
    let payload_bytes = serde_json::to_vec(&document)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        signing_key,
        COLLABORATION_PROFILE_DOCUMENT_SIGNATURE_DOMAIN_V1,
        &payload_bytes,
    );
    Ok(SignedCollaborationProfileDocument {
        payload: document,
        signature,
        signer_did,
    })
}

#[cfg(test)]
pub(crate) fn signed_profile_document_for_test(
    signing_key: &SigningKey,
    display_name: &str,
    handle: Option<&str>,
    revision: u64,
    previous_profile_sha256: Option<&str>,
    updated_at: u64,
    endpoint_dids: Vec<String>,
) -> anyhow::Result<VerifiedCollaborationProfileDocument> {
    signed_profile_document_with_authority_for_test(
        signing_key,
        display_name,
        handle,
        revision,
        previous_profile_sha256,
        updated_at,
        ProfileAuthorityForTest {
            endpoint_dids: endpoint_dids.clone(),
            signer_dids: endpoint_dids,
        },
    )
}

#[cfg(test)]
pub(crate) struct ProfileAuthorityForTest {
    pub(crate) endpoint_dids: Vec<String>,
    pub(crate) signer_dids: Vec<String>,
}

#[cfg(test)]
pub(crate) fn signed_profile_document_with_authority_for_test(
    signing_key: &SigningKey,
    display_name: &str,
    handle: Option<&str>,
    revision: u64,
    previous_profile_sha256: Option<&str>,
    updated_at: u64,
    authority: ProfileAuthorityForTest,
) -> anyhow::Result<VerifiedCollaborationProfileDocument> {
    let (collaboration_endpoint, collaboration_signers) = build_collaboration_authority(
        None,
        authority.endpoint_dids,
        authority.signer_dids,
        revision,
    )?;
    let document = CollaborationProfileDocument {
        schema: COLLABORATION_PROFILE_DOCUMENT_SCHEMA_V1.to_string(),
        profile_did: crate::crypto::encode_signing_key_did(signing_key),
        collaboration_endpoint,
        collaboration_signers,
        display_name: clean_profile_display_name(display_name)?,
        handle: clean_profile_handle(handle)?,
        revision,
        previous_profile_sha256: previous_profile_sha256.map(str::to_string),
        updated_at,
    };
    let signed = sign_profile_document(signing_key, document)?;
    verify_signed_profile_document(&signed)
}

struct LoadedBundleState {
    signing_key: SigningKey,
    previous_profiles: Vec<SignedCollaborationProfileDocument>,
    verified: VerifiedCollaborationProfileDocument,
}

fn load_bundle_state(
    data_dir: &Path,
    principal_id: &str,
    localhost_root: &str,
) -> anyhow::Result<Option<LoadedBundleState>> {
    let path = profile_authority_path(data_dir, localhost_root)?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", path.display()))
        }
    };
    if !metadata.is_file() {
        anyhow::bail!("profile authority bundle must be a regular file");
    }
    let protection =
        crate::auth::load_principal_root_protection(data_dir, principal_id, localhost_root)?;
    if protection.is_none() {
        anyhow::bail!("protected principal root is required for profile authority");
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        localhost_root,
        &profile_authority_object_uri(localhost_root),
        &path,
    )?;
    let bundle = decode_profile_authority_bundle(&bytes)?;
    let verified = verify_signed_profile_document(&bundle.signed_profile)?;
    let signing_key = SigningKey::from_bytes(&decode_profile_signing_seed(
        &bundle.profile_signing_seed_hex,
    )?);
    Ok(Some(LoadedBundleState {
        signing_key,
        previous_profiles: bundle.previous_profiles,
        verified,
    }))
}

fn random_profile_signing_key() -> anyhow::Result<SigningKey> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).context("OS randomness unavailable for profile authority")?;
    Ok(SigningKey::from_bytes(&seed))
}

fn decode_profile_signing_seed(value: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(value).context("invalid profile signing seed")?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("profile signing seed must be 32 bytes"))?;
    Ok(seed)
}

fn clean_profile_display_name(input: &str) -> anyhow::Result<String> {
    let display_name = crate::auth::clean_principal_display_name(Some(input))?
        .ok_or_else(|| anyhow!("display name must not be empty"))?;
    let normalized = display_name.to_ascii_lowercase();
    if RESERVED_PROFILE_DISPLAY_NAMES
        .iter()
        .map(|reserved| reserved.to_ascii_lowercase())
        .any(|reserved| reserved == normalized)
        || is_device_label_placeholder(&display_name)
    {
        anyhow::bail!("choose a real display name first");
    }
    Ok(display_name)
}

fn clean_profile_handle(input: Option<&str>) -> anyhow::Result<Option<String>> {
    let Some(input) = input else {
        return Ok(None);
    };
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_PROFILE_HANDLE_BYTES || trimmed.chars().any(|ch| ch.is_ascii_control()) {
        anyhow::bail!("invalid profile handle");
    }
    Ok(Some(trimmed.to_string()))
}

fn is_sha256_label(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes)))
}

fn sha256_label_from_envelope(
    envelope: &SignedCollaborationProfileDocument,
) -> anyhow::Result<String> {
    Ok(sha256_label(&serde_json::to_vec(envelope)?))
}

fn is_device_label_placeholder(value: &str) -> bool {
    value
        .to_ascii_lowercase()
        .strip_prefix("device ")
        .is_some_and(|suffix| {
            suffix.len() == 8
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::auth::{PasskeyWebAuthnBinding, ProofBinding};

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    /// Protected principal-root writes require an owner-only parent chain, and
    /// `tempfile` creates its root using the process umask. A fixture that
    /// writes a profile authority bundle must own its root explicitly.
    fn owner_only_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    fn write_device_key(data_dir: &Path, seed_byte: u8) -> String {
        let seed = [seed_byte; 32];
        std::fs::create_dir_all(data_dir.join("identity")).unwrap();
        std::fs::write(data_dir.join("identity").join("device.key"), seed).unwrap();
        let (_, did) = elastos_identity::derive_did(&seed);
        did
    }

    fn store_passkey_principal(
        data_dir: &Path,
        principal_id: &str,
        credential_id: &str,
        display_name: Option<&str>,
    ) -> String {
        let now = crate::auth::now_ts();
        let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
            credential_id: credential_id.to_string(),
            public_key: "profile-authority-test-public-key".to_string(),
            sign_count: 1,
            user_verified: true,
            origin: "https://elastos.elacitylabs.com".to_string(),
            rp_id: "elastos.elacitylabs.com".to_string(),
            created_at: now,
            last_used_at: now,
            revoked_at: None,
        });
        crate::auth::upsert_principal_for_binding_as_role_named(
            data_dir,
            binding,
            principal_id.to_string(),
            crate::auth::RuntimePrincipalRole::Admin,
            display_name,
            now,
        )
        .unwrap()
        .proof_binding_id
    }

    fn read_bundle_bytes(data_dir: &Path, principal_id: &str, localhost_root: &str) -> Vec<u8> {
        let path = profile_authority_path(data_dir, localhost_root).unwrap();
        crate::auth::read_principal_root_object(
            data_dir,
            principal_id,
            localhost_root,
            &profile_authority_object_uri(localhost_root),
            &path,
        )
        .unwrap()
    }

    fn write_bundle_bytes(data_dir: &Path, principal_id: &str, localhost_root: &str, bytes: &[u8]) {
        let path = profile_authority_path(data_dir, localhost_root).unwrap();
        crate::auth::write_protected_principal_root_object(
            data_dir,
            principal_id,
            localhost_root,
            &profile_authority_object_uri(localhost_root),
            &path,
            bytes,
        )
        .unwrap();
    }

    fn raw_signed_profile_document(
        signing_key: &SigningKey,
        payload: CollaborationProfileDocument,
    ) -> SignedCollaborationProfileDocument {
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_PROFILE_DOCUMENT_SIGNATURE_DOMAIN_V1,
            &payload_bytes,
        );
        SignedCollaborationProfileDocument {
            payload,
            signature,
            signer_did,
        }
    }

    #[test]
    fn profile_authority_separates_endpoint_and_signer_grants() {
        let profile_key = SigningKey::from_bytes(&[41u8; 32]);
        let endpoint_key = SigningKey::from_bytes(&[42u8; 32]);
        let signer_key = SigningKey::from_bytes(&[43u8; 32]);
        let other_key = SigningKey::from_bytes(&[44u8; 32]);
        let endpoint_did = crate::crypto::encode_signing_key_did(&endpoint_key);
        let signer_did = crate::crypto::encode_signing_key_did(&signer_key);
        let other_did = crate::crypto::encode_signing_key_did(&other_key);
        let profile = signed_profile_document_with_authority_for_test(
            &profile_key,
            "Alice",
            None,
            1,
            None,
            1_785_900_000,
            ProfileAuthorityForTest {
                endpoint_dids: vec![endpoint_did.clone()],
                signer_dids: vec![signer_did.clone()],
            },
        )
        .unwrap();

        assert!(profile.authorizes_endpoint(&endpoint_did));
        assert!(!profile.authorizes_endpoint(&signer_did));
        assert!(!profile.authorizes_endpoint(&other_did));
        assert!(!profile.authorizes_signer(
            &endpoint_did,
            "chat",
            "elastos.chat.direct-message/v1"
        ));
        let chat_authority = profile
            .document()
            .collaboration_signers
            .iter()
            .find(|authority| authority.service == "chat")
            .unwrap();
        assert_eq!(
            chat_authority.delegations[0].object_types,
            vec![
                "elastos.chat.direct-message/v1".to_string(),
                "elastos.chat.message/v1".to_string(),
                "elastos.chat.presence/v1".to_string(),
                "elastos.collaboration.acceptance-receipt/v1".to_string(),
                "elastos.room.accept.v1".to_string(),
                "elastos.room.invite.v1".to_string(),
                "elastos.room.join-invite.v1".to_string(),
            ]
        );
        assert!(profile.authorizes_signer(&signer_did, "chat", "elastos.chat.direct-message/v1"));
        assert!(profile.authorizes_signer(&signer_did, "chat", "elastos.room.accept.v1"));
        assert!(profile.authorizes_signer(&signer_did, "chat", "elastos.room.invite.v1"));
        assert!(profile.authorizes_signer(&signer_did, "chat", "elastos.room.join-invite.v1"));
        assert!(!profile.authorizes_signer(
            &signer_did,
            "people",
            "elastos.chat.direct-message/v1"
        ));
        assert!(!profile.authorizes_signer(
            &signer_did,
            "chat",
            "elastos.people.contact-request/v1"
        ));
        assert!(!profile.authorizes_signer(&other_did, "chat", "elastos.chat.direct-message/v1"));
    }

    #[test]
    fn profile_authority_rotation_and_revocation_require_new_generations() {
        let profile_key = SigningKey::from_bytes(&[51u8; 32]);
        let first_endpoint =
            crate::crypto::encode_signing_key_did(&SigningKey::from_bytes(&[52u8; 32]));
        let second_endpoint =
            crate::crypto::encode_signing_key_did(&SigningKey::from_bytes(&[53u8; 32]));
        let first = signed_profile_document_for_test(
            &profile_key,
            "Alice",
            None,
            1,
            None,
            1_785_900_000,
            vec![first_endpoint.clone()],
        )
        .unwrap();
        let mut rotated = first.document().clone();
        rotated.revision = 2;
        rotated.previous_profile_sha256 =
            Some(sha256_label_from_envelope(first.signed_envelope()).unwrap());
        rotated.updated_at += 1;
        let (endpoint, signers) = build_collaboration_authority(
            Some(first.document()),
            vec![second_endpoint.clone()],
            vec![second_endpoint.clone()],
            rotated.revision,
        )
        .unwrap();
        rotated.collaboration_endpoint = endpoint;
        rotated.collaboration_signers = signers;
        let rotated =
            verify_signed_profile_document(&sign_profile_document(&profile_key, rotated).unwrap())
                .unwrap();
        validate_profile_authority_transition(first.document(), rotated.document()).unwrap();
        assert_eq!(rotated.document().collaboration_endpoint.generation, 2);
        assert!(!rotated.authorizes_endpoint(&first_endpoint));
        assert!(rotated.authorizes_endpoint(&second_endpoint));
        assert!(!rotated.authorizes_signer(&first_endpoint, "chat", "elastos.chat.message/v1"));

        let mut stale_generation = rotated.document().clone();
        stale_generation.collaboration_endpoint.generation = 1;
        assert!(
            validate_profile_authority_transition(first.document(), &stale_generation).is_err()
        );

        let mut revoked = rotated.document().clone();
        revoked.revision = 3;
        revoked.previous_profile_sha256 =
            Some(sha256_label_from_envelope(rotated.signed_envelope()).unwrap());
        revoked.updated_at += 1;
        revoked.collaboration_endpoint.bindings.clear();
        revoked.collaboration_endpoint.generation += 1;
        for authority in &mut revoked.collaboration_signers {
            for delegation in &mut authority.delegations {
                delegation.profile_revision = revoked.revision;
            }
        }
        let revoked =
            verify_signed_profile_document(&sign_profile_document(&profile_key, revoked).unwrap())
                .unwrap();
        validate_profile_authority_transition(rotated.document(), revoked.document()).unwrap();
        assert!(!revoked.authorizes_endpoint(&second_endpoint));
    }

    #[test]
    fn load_profile_authority_empty_root_is_side_effect_free() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:profile-empty";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
        assert!(load_existing_device_did(dir.path()).unwrap().is_none());
        assert!(
            load_profile_authority(dir.path(), principal_id, &localhost_root)
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
        assert!(!dir.path().join("identity").exists());
    }

    #[cfg(unix)]
    #[test]
    fn profile_mutation_bootstraps_owner_only_parent_chain_before_first_write() {
        use std::os::unix::fs::PermissionsExt;

        let dir = owner_only_tempdir();
        let principal_id = "person:local:profile-parent-bootstrap";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(dir.path(), 31);
        let proof_binding_id = store_passkey_principal(
            dir.path(),
            principal_id,
            "credential-parent-bootstrap",
            None,
        );
        crate::auth::store_test_principal_root_protection(dir.path(), principal_id);
        let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
        assert!(!path.parent().unwrap().exists());

        update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Profile Bootstrap",
            None,
            1_787_000_000,
        )
        .unwrap();

        let relative_parent = path.parent().unwrap().strip_prefix(dir.path()).unwrap();
        let mut current = dir.path().to_path_buf();
        for component in relative_parent.components() {
            current.push(component.as_os_str());
            assert_eq!(
                std::fs::symlink_metadata(&current)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700,
                "{} is not owner-only",
                current.display()
            );
        }
        assert!(path.is_file());
        assert_eq!(
            load_profile_authority(dir.path(), principal_id, &localhost_root)
                .unwrap()
                .unwrap()
                .document()
                .display_name,
            "Profile Bootstrap"
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_mutation_narrows_legacy_public_card_parent_without_changing_card() {
        use std::os::unix::fs::PermissionsExt;

        let dir = owner_only_tempdir();
        let principal_id = "person:local:legacy-profile-card";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(dir.path(), 32);
        let proof_binding_id = store_passkey_principal(
            dir.path(),
            principal_id,
            "credential-legacy-profile-card",
            None,
        );
        crate::auth::store_test_principal_root_protection(dir.path(), principal_id);
        let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
        let profile_dir = path.parent().unwrap();
        crate::auth::create_owner_only_dir_all(dir.path(), profile_dir.parent().unwrap()).unwrap();
        std::fs::create_dir(profile_dir).unwrap();
        std::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let public_card = profile_dir.join("profile-card.json");
        let public_card_bytes = br#"{"schema":"elastos.profile-card/v1","display_name":"Legacy"}"#;
        std::fs::write(&public_card, public_card_bytes).unwrap();
        std::fs::set_permissions(&public_card, std::fs::Permissions::from_mode(0o600)).unwrap();

        update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Migrated Profile",
            None,
            1_787_000_001,
        )
        .unwrap();

        assert_eq!(
            std::fs::symlink_metadata(profile_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(std::fs::read(&public_card).unwrap(), public_card_bytes);
        assert_eq!(
            std::fs::symlink_metadata(&public_card)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn profile_load_does_not_create_or_narrow_legacy_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = owner_only_tempdir();
        let principal_id = "person:local:read-only-legacy-profile";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
        let profile_dir = path.parent().unwrap();
        crate::auth::create_owner_only_dir_all(dir.path(), profile_dir.parent().unwrap()).unwrap();
        std::fs::create_dir(profile_dir).unwrap();
        std::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let public_card = profile_dir.join("profile-card.json");
        let public_card_bytes = b"public profile card";
        std::fs::write(&public_card, public_card_bytes).unwrap();
        std::fs::set_permissions(&public_card, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert!(
            load_profile_authority(dir.path(), principal_id, &localhost_root)
                .unwrap()
                .is_none()
        );

        assert!(!path.exists());
        assert_eq!(
            std::fs::symlink_metadata(profile_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(std::fs::read(&public_card).unwrap(), public_card_bytes);
        assert_eq!(
            std::fs::symlink_metadata(&public_card)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn profile_mutation_rejects_symlink_and_non_directory_parents_without_writing() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        for parent_kind in ["symlink", "non-directory"] {
            let dir = owner_only_tempdir();
            let principal_id = format!("person:local:profile-parent-{parent_kind}");
            let localhost_root = crate::auth::principal_localhost_root(&principal_id);
            write_device_key(dir.path(), 33);
            let proof_binding_id = store_passkey_principal(
                dir.path(),
                &principal_id,
                &format!("credential-profile-parent-{parent_kind}"),
                None,
            );
            crate::auth::store_test_principal_root_protection(dir.path(), &principal_id);
            let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
            let profile_dir = path.parent().unwrap();
            crate::auth::create_owner_only_dir_all(dir.path(), profile_dir.parent().unwrap())
                .unwrap();
            let outside = tempfile::tempdir().unwrap();
            let sentinel = b"must remain unchanged";
            if parent_kind == "symlink" {
                symlink(outside.path(), profile_dir).unwrap();
            } else {
                std::fs::write(profile_dir, sentinel).unwrap();
                std::fs::set_permissions(profile_dir, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
            }

            let error = update_profile_authority(
                dir.path(),
                &principal_id,
                &localhost_root,
                &proof_binding_id,
                "Rejected Profile",
                None,
                1_787_000_002,
            )
            .unwrap_err();

            assert!(
                error.to_string().contains("owner-only directory")
                    || error
                        .to_string()
                        .contains("protected principal-root parent")
                    || error.to_string().contains("failed to inspect"),
                "unexpected {parent_kind} rejection: {error:#}"
            );
            assert!(!path.is_file());
            assert!(!outside.path().join("profile-authority.json").exists());
            if parent_kind == "non-directory" {
                assert_eq!(std::fs::read(profile_dir).unwrap(), sentinel);
            }
        }
    }

    #[test]
    fn load_existing_device_did_rejects_symlink_and_non_regular_paths() {
        let dir = tempfile::tempdir().unwrap();
        let identity_dir = dir.path().join("identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        let device_key = identity_dir.join("device.key");

        #[cfg(unix)]
        {
            symlink(dir.path().join("missing-device-key"), &device_key).unwrap();
            let err = load_existing_device_did(dir.path()).unwrap_err();
            assert!(
                err.to_string().contains("device.key") || err.to_string().contains("regular file")
            );
            std::fs::remove_file(&device_key).unwrap();
        }

        std::fs::create_dir_all(&device_key).unwrap();
        let err = load_existing_device_did(dir.path()).unwrap_err();
        assert!(err.to_string().contains("regular file"));
    }

    #[test]
    fn load_existing_device_signing_key_matches_load_or_create_did_without_creating() {
        let missing = tempfile::tempdir().unwrap();
        assert!(load_existing_device_signing_key(missing.path())
            .unwrap()
            .is_none());
        assert!(!missing.path().join("identity").exists());

        let dir = tempfile::tempdir().unwrap();
        let (expected_key, expected_did) =
            elastos_identity::load_or_create_did(dir.path()).unwrap();
        let loaded = load_existing_device_signing_key(dir.path())
            .unwrap()
            .expect("existing device key should load");
        assert_eq!(loaded.1, expected_did);
        assert_eq!(
            crate::crypto::encode_signing_key_did(&loaded.0),
            expected_did
        );
        assert_eq!(loaded.0.to_bytes(), expected_key.to_bytes());
    }

    #[test]
    fn load_profile_authority_rejects_symlink_and_non_regular_bundle_paths() {
        let dir = tempfile::tempdir().unwrap();
        let principal_id = "person:local:profile-path-check";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        let path = profile_authority_path(dir.path(), &localhost_root).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        #[cfg(unix)]
        {
            symlink(dir.path().join("missing-profile-authority"), &path).unwrap();
            let err =
                load_profile_authority(dir.path(), principal_id, &localhost_root).unwrap_err();
            assert!(err.to_string().contains("regular file"));
            std::fs::remove_file(&path).unwrap();
        }

        std::fs::create_dir_all(&path).unwrap();
        let err = load_profile_authority(dir.path(), principal_id, &localhost_root).unwrap_err();
        assert!(err.to_string().contains("regular file"));
    }

    #[test]
    fn update_profile_authority_rejects_invalid_state_without_writing_a_bundle() {
        let now = 1_785_900_000;

        let unprotected = tempfile::tempdir().unwrap();
        let principal_id = "person:local:unprotected-profile";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(unprotected.path(), 1);
        let proof_binding_id = store_passkey_principal(
            unprotected.path(),
            principal_id,
            "credential-unprotected",
            None,
        );
        let path = profile_authority_path(unprotected.path(), &localhost_root).unwrap();
        let err = update_profile_authority(
            unprotected.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Alice",
            None,
            now,
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("protected principal root is required for profile authority"));
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());

        let placeholder = tempfile::tempdir().unwrap();
        let principal_id = "person:local:placeholder";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(placeholder.path(), 2);
        let proof_binding_id = store_passkey_principal(
            placeholder.path(),
            principal_id,
            "credential-placeholder",
            None,
        );
        crate::auth::store_test_principal_root_protection(placeholder.path(), principal_id);
        let path = profile_authority_path(placeholder.path(), &localhost_root).unwrap();
        for display_name in ["ElastOS user", "person", "DEVICE 86CBFB59"] {
            let err = update_profile_authority(
                placeholder.path(),
                principal_id,
                &localhost_root,
                &proof_binding_id,
                display_name,
                None,
                now,
            )
            .unwrap_err();
            assert_eq!(err.to_string(), "choose a real display name first");
            assert!(!path.exists());
        }

        let wrong_principal = tempfile::tempdir().unwrap();
        let principal_id = "person:local:correct-principal";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(wrong_principal.path(), 3);
        crate::auth::store_test_principal_root_protection(wrong_principal.path(), principal_id);
        store_passkey_principal(
            wrong_principal.path(),
            principal_id,
            "credential-correct",
            None,
        );
        let other_binding = store_passkey_principal(
            wrong_principal.path(),
            "person:local:other-principal",
            "credential-other",
            None,
        );
        let path = profile_authority_path(wrong_principal.path(), &localhost_root).unwrap();
        let err = update_profile_authority(
            wrong_principal.path(),
            principal_id,
            &localhost_root,
            &other_binding,
            "Alice",
            None,
            now,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "proof binding does not match the active principal"
        );
        assert!(!path.exists());

        let revoked = tempfile::tempdir().unwrap();
        let principal_id = "person:local:revoked-principal";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(revoked.path(), 4);
        let revoked_binding =
            store_passkey_principal(revoked.path(), principal_id, "credential-revoked", None);
        crate::auth::store_test_principal_root_protection(revoked.path(), principal_id);
        crate::auth::revoke_passkey_binding(revoked.path(), &revoked_binding, now + 1).unwrap();
        let path = profile_authority_path(revoked.path(), &localhost_root).unwrap();
        let err = update_profile_authority(
            revoked.path(),
            principal_id,
            &localhost_root,
            &revoked_binding,
            "Alice",
            None,
            now,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("passkey")
                || err.to_string().contains("proof")
                || err.to_string().contains("revoked")
        );
        assert!(!path.exists());

        let wallet = tempfile::tempdir().unwrap();
        let principal_id = "person:local:wallet-proof";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(wallet.path(), 5);
        store_passkey_principal(wallet.path(), principal_id, "credential-wallet", None);
        crate::auth::store_test_principal_root_protection(wallet.path(), principal_id);
        let path = profile_authority_path(wallet.path(), &localhost_root).unwrap();
        let err = update_profile_authority(
            wallet.path(),
            principal_id,
            &localhost_root,
            "proof:wallet:test",
            "Alice",
            None,
            now,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "proof-bound passkey session required");
        assert!(!path.exists());
    }

    #[test]
    fn profile_chain_segment_is_contiguous_and_bounded() {
        let dir = owner_only_tempdir();
        let principal_id = "person:local:chain-segment";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(dir.path(), 11);
        let proof_binding_id =
            store_passkey_principal(dir.path(), principal_id, "credential-segment", None);
        crate::auth::store_test_principal_root_protection(dir.path(), principal_id);

        assert!(
            profile_chain_segment(dir.path(), principal_id, &localhost_root)
                .unwrap()
                .is_none()
        );

        for revision in 1..=12u64 {
            update_profile_authority(
                dir.path(),
                principal_id,
                &localhost_root,
                &proof_binding_id,
                &format!("Alice {revision}"),
                None,
                1_785_900_000 + revision,
            )
            .unwrap();

            let segment = profile_chain_segment(dir.path(), principal_id, &localhost_root)
                .unwrap()
                .expect("segment after a write");
            assert!(segment.len() <= MAX_RETAINED_PROFILE_REVISIONS);
            assert_eq!(
                segment.len(),
                (revision as usize).min(MAX_RETAINED_PROFILE_REVISIONS)
            );

            // Oldest first, ending at the head, with every step advancing by one
            // revision and naming the previous signed envelope hash.
            let head = segment.last().expect("segment head");
            assert_eq!(head.payload.revision, revision);
            for pair in segment.windows(2) {
                assert_eq!(pair[1].payload.revision, pair[0].payload.revision + 1);
                assert_eq!(
                    pair[1].payload.previous_profile_sha256.as_deref(),
                    Some(sha256_label_from_envelope(&pair[0]).unwrap().as_str())
                );
                assert_eq!(pair[1].payload.profile_did, pair[0].payload.profile_did);
            }
        }
    }

    #[test]
    fn update_profile_authority_preserves_profile_did_and_revision_chain() {
        let dir = owner_only_tempdir();
        let principal_id = "person:local:revision-chain";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        let device_did = write_device_key(dir.path(), 9);
        let proof_binding_id =
            store_passkey_principal(dir.path(), principal_id, "credential-revision", None);
        crate::auth::store_test_principal_root_protection(dir.path(), principal_id);

        let first = update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Alice",
            None,
            1_785_900_100,
        )
        .unwrap();
        let first_hash = sha256_label_from_envelope(first.signed_envelope()).unwrap();

        let second = update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Alice Updated",
            None,
            1_785_900_100,
        )
        .unwrap();

        assert_eq!(first.document().profile_did, second.document().profile_did);
        assert_eq!(first.document().revision, 1);
        assert_eq!(second.document().revision, 2);
        assert_eq!(first.document().updated_at, 1_785_900_100);
        assert_eq!(second.document().updated_at, 1_785_900_101);
        assert_eq!(
            second.document().previous_profile_sha256.as_deref(),
            Some(first_hash.as_str())
        );
        assert!(second.authorizes_endpoint(&device_did));
        let loaded = load_profile_authority(dir.path(), principal_id, &localhost_root)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.document().display_name, "Alice Updated");
        assert_eq!(loaded.document().profile_did, second.document().profile_did);
        assert!(loaded.authorizes_endpoint(&device_did));
    }

    #[test]
    fn load_profile_authority_rejects_tampered_or_noncanonical_bundles() {
        let dir = owner_only_tempdir();
        let principal_id = "person:local:tampered-profile";
        let localhost_root = crate::auth::principal_localhost_root(principal_id);
        write_device_key(dir.path(), 7);
        let proof_binding_id =
            store_passkey_principal(dir.path(), principal_id, "credential-tampered", None);
        crate::auth::store_test_principal_root_protection(dir.path(), principal_id);

        update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Alice",
            None,
            1_785_900_200,
        )
        .unwrap();
        update_profile_authority(
            dir.path(),
            principal_id,
            &localhost_root,
            &proof_binding_id,
            "Alice Updated",
            None,
            1_785_900_200,
        )
        .unwrap();

        let valid_bytes = read_bundle_bytes(dir.path(), principal_id, &localhost_root);
        let valid_bundle = decode_profile_authority_bundle(&valid_bytes).unwrap();

        let mut tampered_payload = valid_bundle.clone();
        tampered_payload.signed_profile.payload.display_name = "Mallory".to_string();
        write_bundle_bytes(
            dir.path(),
            principal_id,
            &localhost_root,
            &serde_json::to_vec_pretty(&tampered_payload).unwrap(),
        );
        assert!(load_profile_authority(dir.path(), principal_id, &localhost_root).is_err());

        write_bundle_bytes(dir.path(), principal_id, &localhost_root, &valid_bytes);
        let mut wrong_seed = valid_bundle.clone();
        wrong_seed.profile_signing_seed_hex = hex::encode([8u8; 32]);
        write_bundle_bytes(
            dir.path(),
            principal_id,
            &localhost_root,
            &serde_json::to_vec_pretty(&wrong_seed).unwrap(),
        );
        let err = load_profile_authority(dir.path(), principal_id, &localhost_root).unwrap_err();
        assert!(err
            .to_string()
            .contains("profile authority signer DID mismatch"));

        write_bundle_bytes(dir.path(), principal_id, &localhost_root, &valid_bytes);
        let mut uppercase_previous = valid_bundle.clone();
        let mut payload = uppercase_previous.signed_profile.payload.clone();
        payload.previous_profile_sha256 = payload
            .previous_profile_sha256
            .as_ref()
            .map(|value| value.to_uppercase());
        let signing_key = SigningKey::from_bytes(
            &decode_profile_signing_seed(&uppercase_previous.profile_signing_seed_hex).unwrap(),
        );
        uppercase_previous.signed_profile = raw_signed_profile_document(&signing_key, payload);
        write_bundle_bytes(
            dir.path(),
            principal_id,
            &localhost_root,
            &serde_json::to_vec_pretty(&uppercase_previous).unwrap(),
        );
        let err = load_profile_authority(dir.path(), principal_id, &localhost_root).unwrap_err();
        assert!(err
            .to_string()
            .contains("profile revision chain is invalid"));
    }
}
