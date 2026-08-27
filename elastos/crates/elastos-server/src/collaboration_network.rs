//! Pure validation for signed collaboration-network profiles.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{decode_did_key, verify_signed_json_envelope_against_dids};

pub const COLLABORATION_NETWORK_PROFILE_SCHEMA: &str = "elastos.collaboration-network.profile/v1";
pub const COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN: &str =
    "elastos.collaboration-network.profile.v1";

pub(crate) const MAX_PROFILE_BYTES: usize = 64 * 1024;
const MAX_NETWORK_ID_BYTES: usize = 128;
pub(crate) const MAX_TRUSTED_SIGNERS: usize = 32;
const MAX_BOOTSTRAP_PEERS: usize = 16;
const MAX_ENDPOINTS_PER_TICKET: usize = 8;
const MAX_NODE_ID_BYTES: usize = 128;
const MAX_CONNECT_TICKET_BYTES: usize = 8 * 1024;
const MAX_GRANT_CID_BYTES: usize = 256;
const RAW_CID_CODEC: u64 = 0x55;
const SHA2_256_MULTIHASH_CODE: u64 = 0x12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCollaborationNetworkProfile {
    pub payload: CollaborationNetworkProfile,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationNetworkProfile {
    pub schema: String,
    pub network_id: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_profile_sha256: Option<String>,
    pub signer_did: String,
    pub bootstrap_peers: Vec<CollaborationBootstrapPeer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_conversation: Option<DefaultConversationGrantDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationBootstrapPeer {
    pub node_id: String,
    pub connect_ticket: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultConversationGrantDescriptor {
    pub grant_cid: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollaborationConnectTicket {
    endpoints: Vec<CollaborationConnectTicketEndpoint>,
    topic: (),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollaborationConnectTicketEndpoint {
    addrs: BTreeSet<iroh::TransportAddr>,
    id: iroh::EndpointId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCollaborationNetworkProfile {
    profile: CollaborationNetworkProfile,
    profile_sha256: String,
}

impl VerifiedCollaborationNetworkProfile {
    pub fn profile(&self) -> &CollaborationNetworkProfile {
        &self.profile
    }

    pub fn profile_sha256(&self) -> &str {
        &self.profile_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollaborationNetworkProfileMode {
    Isolated,
    Configured(VerifiedCollaborationNetworkProfile),
}

/// Return the exact canonical payload bytes covered by the profile signature.
pub fn canonical_collaboration_network_profile_payload_bytes(
    profile: &CollaborationNetworkProfile,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(profile)?)?)
}

/// Validate one optional signed profile against caller-owned trust and chain state.
///
/// This function performs no I/O. In particular, it does not fetch bootstrap
/// peers or grants and does not create identities, sessions, or application state.
pub fn validate_collaboration_network_profile(
    profile_bytes: Option<&[u8]>,
    expected_network_id: &str,
    trusted_signer_dids: &[String],
    previous: Option<&VerifiedCollaborationNetworkProfile>,
) -> anyhow::Result<CollaborationNetworkProfileMode> {
    let Some(profile_bytes) = profile_bytes else {
        return Ok(CollaborationNetworkProfileMode::Isolated);
    };

    if profile_bytes.is_empty() || profile_bytes.len() > MAX_PROFILE_BYTES {
        anyhow::bail!("collaboration-network profile has an invalid byte length");
    }
    validate_network_id(expected_network_id).context("expected collaboration network_id")?;
    validate_trusted_signers(trusted_signer_dids)?;

    let envelope: SignedCollaborationNetworkProfile = serde_json::from_slice(profile_bytes)
        .context("invalid collaboration-network profile envelope")?;
    let canonical_envelope = serde_json::to_vec(&serde_json::to_value(&envelope)?)?;
    if canonical_envelope != profile_bytes {
        anyhow::bail!("collaboration-network profile envelope is not canonical JSON");
    }

    validate_signature_shape(&envelope.signature)?;
    if envelope.payload.signer_did != envelope.signer_did {
        anyhow::bail!("collaboration-network profile signer binding does not match envelope");
    }
    let (_, verified_signer) = verify_signed_json_envelope_against_dids(
        profile_bytes,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
        trusted_signer_dids,
    )?;
    if verified_signer != envelope.payload.signer_did {
        anyhow::bail!("collaboration-network profile signer verification mismatch");
    }

    validate_profile_payload(&envelope.payload, expected_network_id)?;
    validate_profile_chain(&envelope.payload, previous, expected_network_id)?;

    Ok(CollaborationNetworkProfileMode::Configured(
        VerifiedCollaborationNetworkProfile {
            profile: envelope.payload,
            profile_sha256: sha256_label(profile_bytes),
        },
    ))
}

fn validate_profile_payload(
    profile: &CollaborationNetworkProfile,
    expected_network_id: &str,
) -> anyhow::Result<()> {
    if profile.schema != COLLABORATION_NETWORK_PROFILE_SCHEMA {
        anyhow::bail!("unsupported collaboration-network profile schema");
    }
    validate_network_id(&profile.network_id)?;
    if profile.network_id != expected_network_id {
        anyhow::bail!("collaboration-network profile network_id does not match expected network");
    }
    if profile.revision == 0 {
        anyhow::bail!("collaboration-network profile revision must start at 1");
    }
    if let Some(previous_hash) = &profile.previous_profile_sha256 {
        validate_sha256_label(previous_hash, "previous collaboration-network profile hash")?;
    }
    decode_did_key(&profile.signer_did).context("invalid collaboration-network profile signer")?;
    if profile.bootstrap_peers.len() > MAX_BOOTSTRAP_PEERS {
        anyhow::bail!("collaboration-network profile has too many bootstrap peers");
    }

    let mut peers_by_node = BTreeMap::new();
    let mut tickets = BTreeSet::new();
    for peer in &profile.bootstrap_peers {
        validate_collaboration_bootstrap_peer(peer)?;
        if let Some(existing_ticket) = peers_by_node.insert(&peer.node_id, &peer.connect_ticket) {
            if existing_ticket == &peer.connect_ticket {
                anyhow::bail!("collaboration-network profile has a duplicate bootstrap peer");
            }
            anyhow::bail!("collaboration-network profile has conflicting bootstrap peer routes");
        }
        if !tickets.insert(&peer.connect_ticket) {
            anyhow::bail!("collaboration-network profile reuses a bootstrap connect ticket");
        }
    }

    if let Some(descriptor) = &profile.default_conversation {
        validate_default_conversation_descriptor(descriptor)?;
    }
    Ok(())
}

fn validate_profile_chain(
    profile: &CollaborationNetworkProfile,
    previous: Option<&VerifiedCollaborationNetworkProfile>,
    expected_network_id: &str,
) -> anyhow::Result<()> {
    match previous {
        None => {
            if profile.revision != 1 {
                anyhow::bail!("initial collaboration-network profile revision must be 1");
            }
            if profile.previous_profile_sha256.is_some() {
                anyhow::bail!(
                    "initial collaboration-network profile must not name a previous hash"
                );
            }
        }
        Some(previous) => {
            if previous.profile.network_id != expected_network_id {
                anyhow::bail!("previous collaboration-network profile belongs to another network");
            }
            let expected_revision = previous.profile.revision.checked_add(1).ok_or_else(|| {
                anyhow::anyhow!("collaboration-network profile revision overflow")
            })?;
            if profile.revision < expected_revision {
                anyhow::bail!("collaboration-network profile revision rollback");
            }
            if profile.revision > expected_revision {
                anyhow::bail!("collaboration-network profile revision gap");
            }
            if profile.previous_profile_sha256.as_deref() != Some(previous.profile_sha256.as_str())
            {
                anyhow::bail!("collaboration-network profile previous hash mismatch");
            }
        }
    }
    Ok(())
}

fn validate_trusted_signers(trusted_signer_dids: &[String]) -> anyhow::Result<()> {
    if trusted_signer_dids.is_empty() || trusted_signer_dids.len() > MAX_TRUSTED_SIGNERS {
        anyhow::bail!("collaboration-network trusted signer set has an invalid size");
    }
    let mut unique = BTreeSet::new();
    for signer_did in trusted_signer_dids {
        if !unique.insert(signer_did) {
            anyhow::bail!("collaboration-network trusted signer set contains a duplicate DID");
        }
        decode_did_key(signer_did).context("invalid trusted collaboration-network signer DID")?;
    }
    Ok(())
}

pub(crate) fn validate_network_id(network_id: &str) -> anyhow::Result<()> {
    let bytes = network_id.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NETWORK_ID_BYTES {
        anyhow::bail!("network_id has an invalid byte length");
    }
    if !bytes[0].is_ascii_lowercase() && !bytes[0].is_ascii_digit() {
        anyhow::bail!("network_id must start with a lowercase ASCII letter or digit");
    }
    if !bytes.iter().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'.' | b'_' | b'-' | b':')
    }) {
        anyhow::bail!("network_id contains unsupported or noncanonical characters");
    }
    Ok(())
}

fn validate_signature_shape(signature: &str) -> anyhow::Result<()> {
    if signature.len() != 128
        || !signature
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("collaboration-network profile signature must be lowercase Ed25519 hex");
    }
    Ok(())
}

pub(crate) fn validate_collaboration_bootstrap_peer(
    peer: &CollaborationBootstrapPeer,
) -> anyhow::Result<()> {
    if peer.node_id.is_empty() || peer.node_id.len() > MAX_NODE_ID_BYTES {
        anyhow::bail!("collaboration-network bootstrap node_id has an invalid length");
    }
    let expected_node = peer
        .node_id
        .parse::<iroh::EndpointId>()
        .context("invalid collaboration-network bootstrap node_id")?;
    if expected_node.to_string() != peer.node_id {
        anyhow::bail!("collaboration-network bootstrap node_id is not canonical");
    }
    if peer.connect_ticket.is_empty()
        || peer.connect_ticket.len() > MAX_CONNECT_TICKET_BYTES
        || !peer
            .connect_ticket
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
    {
        anyhow::bail!("collaboration-network bootstrap connect ticket is not canonical base32");
    }
    let ticket_bytes = data_encoding::BASE32_NOPAD
        .decode(peer.connect_ticket.to_ascii_uppercase().as_bytes())
        .context("malformed collaboration-network bootstrap connect ticket")?;
    let mut canonical_ticket = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
    canonical_ticket.make_ascii_lowercase();
    if canonical_ticket != peer.connect_ticket {
        anyhow::bail!("collaboration-network bootstrap connect ticket is not canonical base32");
    }

    let ticket: CollaborationConnectTicket = serde_json::from_slice(&ticket_bytes)
        .context("malformed collaboration-network bootstrap connect ticket")?;
    if serde_json::to_vec(&ticket)? != ticket_bytes {
        anyhow::bail!("collaboration-network bootstrap connect ticket JSON is not canonical");
    }
    if ticket.endpoints.is_empty() || ticket.endpoints.len() > MAX_ENDPOINTS_PER_TICKET {
        anyhow::bail!("collaboration-network bootstrap connect ticket has invalid endpoints");
    }
    if ticket
        .endpoints
        .iter()
        .any(|endpoint| endpoint.id != expected_node)
    {
        anyhow::bail!("collaboration-network bootstrap ticket node identity mismatch");
    }
    Ok(())
}

fn validate_default_conversation_descriptor(
    descriptor: &DefaultConversationGrantDescriptor,
) -> anyhow::Result<()> {
    if descriptor.grant_cid.is_empty() || descriptor.grant_cid.len() > MAX_GRANT_CID_BYTES {
        anyhow::bail!("default-conversation grant CID has an invalid length");
    }
    let grant_cid = cid::Cid::try_from(descriptor.grant_cid.as_str())
        .context("invalid default-conversation grant CID")?;
    if grant_cid.to_string() != descriptor.grant_cid {
        anyhow::bail!("default-conversation grant CID is not canonical");
    }
    if grant_cid.codec() != RAW_CID_CODEC
        || grant_cid.hash().code() != SHA2_256_MULTIHASH_CODE
        || grant_cid.hash().digest().len() != 32
    {
        anyhow::bail!("default-conversation grant must use a raw SHA-256 CID");
    }
    Ok(())
}

fn validate_sha256_label(value: &str, field: &str) -> anyhow::Result<()> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if !valid {
        anyhow::bail!("{field} must be sha256:<64 lowercase hex>");
    }
    Ok(())
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::signature::{generate_keypair, SigningKey};

    fn ticket_for(secret_byte: u8) -> CollaborationBootstrapPeer {
        let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
        let endpoint = iroh::EndpointAddr::from(secret.public());
        let ticket_json = serde_json::json!({
            "topic": null,
            "endpoints": [endpoint],
        });
        let ticket_bytes = serde_json::to_vec(&ticket_json).unwrap();
        let mut connect_ticket = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
        connect_ticket.make_ascii_lowercase();
        CollaborationBootstrapPeer {
            node_id: secret.public().to_string(),
            connect_ticket,
        }
    }

    fn encode_ticket_value(ticket: &serde_json::Value) -> String {
        let mut encoded = data_encoding::BASE32_NOPAD.encode(&serde_json::to_vec(ticket).unwrap());
        encoded.make_ascii_lowercase();
        encoded
    }

    fn raw_cid(hash_code: u64, codec: u64, digest: &[u8]) -> String {
        let hash = cid::multihash::Multihash::<64>::wrap(hash_code, digest).unwrap();
        cid::Cid::new_v1(codec, hash).to_string()
    }

    fn profile(signer_did: &str, revision: u64) -> CollaborationNetworkProfile {
        CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: "elastos-collaboration-test".to_string(),
            revision,
            previous_profile_sha256: None,
            signer_did: signer_did.to_string(),
            bootstrap_peers: vec![ticket_for(7)],
            default_conversation: None,
        }
    }

    fn sign_profile(signing_key: &SigningKey, payload: CollaborationNetworkProfile) -> Vec<u8> {
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, signer_did) = crate::crypto::domain_separated_sign(
            signing_key,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        assert_eq!(payload.signer_did, signer_did);
        let envelope = SignedCollaborationNetworkProfile {
            payload,
            signature,
            signer_did,
        };
        serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap()
    }

    fn configured(mode: CollaborationNetworkProfileMode) -> VerifiedCollaborationNetworkProfile {
        match mode {
            CollaborationNetworkProfileMode::Configured(profile) => profile,
            CollaborationNetworkProfileMode::Isolated => panic!("expected configured profile"),
        }
    }

    #[test]
    fn valid_initial_profile_and_chained_update() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let initial_bytes = sign_profile(&signing_key, profile(&signer_did, 1));
        let initial = configured(
            validate_collaboration_network_profile(
                Some(&initial_bytes),
                "elastos-collaboration-test",
                &trusted,
                None,
            )
            .unwrap(),
        );

        let mut update_payload = profile(&signer_did, 2);
        update_payload.previous_profile_sha256 = Some(initial.profile_sha256.clone());
        update_payload.bootstrap_peers.push(ticket_for(8));
        let update_bytes = sign_profile(&signing_key, update_payload);
        let update = configured(
            validate_collaboration_network_profile(
                Some(&update_bytes),
                "elastos-collaboration-test",
                &trusted,
                Some(&initial),
            )
            .unwrap(),
        );

        assert_eq!(update.profile.revision, 2);
        assert_eq!(
            update.profile.previous_profile_sha256,
            Some(initial.profile_sha256)
        );
    }

    #[test]
    fn absent_profile_is_explicit_isolated_mode() {
        assert_eq!(
            validate_collaboration_network_profile(None, "ignored-while-isolated", &[], None)
                .unwrap(),
            CollaborationNetworkProfileMode::Isolated
        );
    }

    #[test]
    fn rejects_untrusted_signer_and_invalid_signature() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let bytes = sign_profile(&signing_key, profile(&signer_did, 1));
        let (other_key, _) = generate_keypair();
        let other_did = crate::crypto::encode_did_key(&other_key.verifying_key());
        let err = validate_collaboration_network_profile(
            Some(&bytes),
            "elastos-collaboration-test",
            std::slice::from_ref(&other_did),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Signer DID mismatch"));

        let mut wrong_binding: SignedCollaborationNetworkProfile =
            serde_json::from_slice(&bytes).unwrap();
        wrong_binding.signer_did = other_did;
        let wrong_binding_bytes =
            serde_json::to_vec(&serde_json::to_value(wrong_binding).unwrap()).unwrap();
        assert!(validate_collaboration_network_profile(
            Some(&wrong_binding_bytes),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("signer binding"));

        let mut envelope: SignedCollaborationNetworkProfile =
            serde_json::from_slice(&bytes).unwrap();
        let replacement = if envelope.signature.starts_with("00") {
            "01"
        } else {
            "00"
        };
        envelope.signature.replace_range(0..2, replacement);
        let invalid_bytes = serde_json::to_vec(&serde_json::to_value(envelope).unwrap()).unwrap();
        assert!(validate_collaboration_network_profile(
            Some(&invalid_bytes),
            "elastos-collaboration-test",
            &[signer_did],
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("signature verification failed"));
    }

    #[test]
    fn rejects_revision_rollback_gap_and_previous_hash_errors() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let initial_bytes = sign_profile(&signing_key, profile(&signer_did, 1));
        let initial = configured(
            validate_collaboration_network_profile(
                Some(&initial_bytes),
                "elastos-collaboration-test",
                &trusted,
                None,
            )
            .unwrap(),
        );

        for (revision, previous_hash, expected) in [
            (1, Some(initial.profile_sha256.clone()), "revision rollback"),
            (3, Some(initial.profile_sha256.clone()), "revision gap"),
            (2, None, "previous hash mismatch"),
            (
                2,
                Some(format!("sha256:{}", "0".repeat(64))),
                "previous hash mismatch",
            ),
        ] {
            let mut candidate = profile(&signer_did, revision);
            candidate.previous_profile_sha256 = previous_hash;
            let bytes = sign_profile(&signing_key, candidate);
            let err = validate_collaboration_network_profile(
                Some(&bytes),
                "elastos-collaboration-test",
                &trusted,
                Some(&initial),
            )
            .unwrap_err();
            assert!(err.to_string().contains(expected), "{err:#}");
        }

        let mut wrong_initial = profile(&signer_did, 1);
        wrong_initial.previous_profile_sha256 = Some(format!("sha256:{}", "0".repeat(64)));
        let bytes = sign_profile(&signing_key, wrong_initial);
        assert!(validate_collaboration_network_profile(
            Some(&bytes),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("must not name a previous hash"));
    }

    #[test]
    fn rejects_duplicate_conflicting_and_mismatched_bootstrap_peers() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let peer = ticket_for(7);

        let mut duplicate = profile(&signer_did, 1);
        duplicate.bootstrap_peers = vec![peer.clone(), peer.clone()];
        let err = validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, duplicate)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate bootstrap peer"));

        let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let alternate_endpoint = iroh::EndpointAddr::from(secret.public())
            .with_ip_addr("127.0.0.1:12345".parse().unwrap());
        let alternate_ticket = encode_ticket_value(&serde_json::json!({
            "topic": null,
            "endpoints": [alternate_endpoint],
        }));
        let conflicting_peer = CollaborationBootstrapPeer {
            node_id: peer.node_id.clone(),
            connect_ticket: alternate_ticket,
        };
        let mut conflicting = profile(&signer_did, 1);
        conflicting.bootstrap_peers = vec![peer.clone(), conflicting_peer];
        let err = validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, conflicting)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("conflicting bootstrap peer routes"));

        let mut mismatch = profile(&signer_did, 1);
        mismatch.bootstrap_peers[0].node_id = ticket_for(8).node_id;
        assert!(validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, mismatch)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("node identity mismatch"));

        let mut malformed = profile(&signer_did, 1);
        malformed.bootstrap_peers[0].connect_ticket = "not-a-ticket".to_string();
        assert!(validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, malformed)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("canonical base32"));
    }

    #[test]
    fn rejects_non_v1_or_partially_malformed_connect_tickets() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let valid_peer = ticket_for(7);
        let ticket_bytes = data_encoding::BASE32_NOPAD
            .decode(valid_peer.connect_ticket.to_ascii_uppercase().as_bytes())
            .unwrap();
        let ticket: serde_json::Value = serde_json::from_slice(&ticket_bytes).unwrap();

        let mut malformed_endpoint = ticket.clone();
        malformed_endpoint["endpoints"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({ "id": 7, "addrs": [] }));

        let mut unknown_field = ticket.clone();
        unknown_field["unexpected"] = serde_json::json!(true);

        let mut non_null_topic = ticket.clone();
        non_null_topic["topic"] = serde_json::json!("general");

        let endpoint = ticket["endpoints"][0].clone();
        let mut endpoint_overflow = ticket;
        endpoint_overflow["endpoints"] = serde_json::Value::Array(vec![endpoint; 9]);

        for (candidate, expected) in [
            (malformed_endpoint, "malformed"),
            (unknown_field, "unknown field"),
            (non_null_topic, "invalid type"),
            (endpoint_overflow, "invalid endpoints"),
        ] {
            let mut payload = profile(&signer_did, 1);
            payload.bootstrap_peers[0].connect_ticket = encode_ticket_value(&candidate);
            let err = validate_collaboration_network_profile(
                Some(&sign_profile(&signing_key, payload)),
                "elastos-collaboration-test",
                &trusted,
                None,
            )
            .unwrap_err();
            assert!(format!("{err:#}").contains(expected), "{err:#}");
        }
    }

    #[test]
    fn rejects_bounds_unknown_fields_wrong_schema_network_and_noncanonical_bytes() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];

        let mut oversized_list = profile(&signer_did, 1);
        oversized_list.bootstrap_peers = (1..=17).map(ticket_for).collect();
        assert!(validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, oversized_list)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("too many bootstrap peers"));

        let mut oversized_field = profile(&signer_did, 1);
        oversized_field.network_id = "a".repeat(MAX_NETWORK_ID_BYTES + 1);
        assert!(validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, oversized_field)),
            "a",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("invalid byte length"));

        let valid_bytes = sign_profile(&signing_key, profile(&signer_did, 1));
        let mut unknown: serde_json::Value = serde_json::from_slice(&valid_bytes).unwrap();
        unknown["unknown"] = serde_json::json!(true);
        let err = validate_collaboration_network_profile(
            Some(&serde_json::to_vec(&unknown).unwrap()),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("unknown field"));

        let mut wrong_schema = profile(&signer_did, 1);
        wrong_schema.schema = "elastos.collaboration-network.profile/v2".to_string();
        assert!(validate_collaboration_network_profile(
            Some(&sign_profile(&signing_key, wrong_schema)),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("unsupported"));

        let other_network_bytes = sign_profile(&signing_key, profile(&signer_did, 1));
        assert!(validate_collaboration_network_profile(
            Some(&other_network_bytes),
            "another-network",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("does not match"));
        assert!(matches!(
            validate_collaboration_network_profile(
                Some(&other_network_bytes),
                "elastos-collaboration-test",
                &trusted,
                None,
            )
            .unwrap(),
            CollaborationNetworkProfileMode::Configured(_)
        ));

        let mut noncanonical = valid_bytes.clone();
        noncanonical.push(b'\n');
        assert!(validate_collaboration_network_profile(
            Some(&noncanonical),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("not canonical JSON"));

        let oversized_bytes = vec![b' '; MAX_PROFILE_BYTES + 1];
        assert!(validate_collaboration_network_profile(
            Some(&oversized_bytes),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap_err()
        .to_string()
        .contains("invalid byte length"));
    }

    #[test]
    fn default_conversation_is_hash_bound_and_validation_is_deterministic_and_pure() {
        let (signing_key, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signing_key.verifying_key());
        let trusted = vec![signer_did.clone()];
        let mut payload = profile(&signer_did, 1);
        payload.default_conversation = Some(DefaultConversationGrantDescriptor {
            grant_cid: raw_cid(
                SHA2_256_MULTIHASH_CODE,
                RAW_CID_CODEC,
                &Sha256::digest(b"signed default-conversation grant"),
            ),
        });
        let bytes = sign_profile(&signing_key, payload);
        let state_dir = tempfile::tempdir().unwrap();
        let marker = state_dir.path().join("unchanged");
        std::fs::write(&marker, b"original").unwrap();

        let first = validate_collaboration_network_profile(
            Some(&bytes),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap();
        let second = validate_collaboration_network_profile(
            Some(&bytes),
            "elastos-collaboration-test",
            &trusted,
            None,
        )
        .unwrap();

        assert_eq!(first, second);
        let verified = configured(first);
        assert!(verified.profile.default_conversation.is_some());
        assert_eq!(std::fs::read(&marker).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(state_dir.path()).unwrap().count(), 1);

        for invalid_cid in [
            raw_cid(
                SHA2_256_MULTIHASH_CODE,
                0x70,
                &Sha256::digest(b"wrong codec"),
            ),
            raw_cid(0x13, RAW_CID_CODEC, &[7u8; 64]),
        ] {
            let mut invalid_payload = profile(&signer_did, 1);
            invalid_payload.default_conversation = Some(DefaultConversationGrantDescriptor {
                grant_cid: invalid_cid,
            });
            let err = validate_collaboration_network_profile(
                Some(&sign_profile(&signing_key, invalid_payload)),
                "elastos-collaboration-test",
                &trusted,
                None,
            )
            .unwrap_err();
            assert!(err.to_string().contains("raw SHA-256 CID"));
        }
    }
}
