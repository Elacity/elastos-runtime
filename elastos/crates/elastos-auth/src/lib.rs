//! Runtime authority primitives for proof-bound sessions.
//!
//! Blockchain integrations are adapters behind this model. The runtime owns
//! principals, proof bindings, session grants, capability scope, and audit.

use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::Digest as Sha2Digest;
use sha3::Keccak256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalId(pub String);

impl PrincipalId {
    pub fn local_person(id: &str) -> Self {
        Self(format!("person:local:{id}"))
    }

    pub fn device_did(did: &str) -> Self {
        Self(format!("device:{did}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofBindingKind {
    PasskeyWebAuthn,
    EvmAccount,
    BtcAddress,
    DidKey,
    DidElastos,
    Essentials,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofBinding {
    pub kind: ProofBindingKind,
    pub subject: String,
    pub chain_id: Option<u64>,
    pub verified_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey: Option<PasskeyWebAuthnBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasskeyWebAuthnBinding {
    pub credential_id: String,
    pub public_key: String,
    pub sign_count: u32,
    pub user_verified: bool,
    pub origin: String,
    pub rp_id: String,
    pub created_at: u64,
    pub last_used_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<u64>,
}

impl ProofBinding {
    pub fn evm_account(chain_id: u64, address: &str, verified_at: u64) -> Self {
        Self {
            kind: ProofBindingKind::EvmAccount,
            subject: normalize_evm_address(address),
            chain_id: Some(chain_id),
            verified_at,
            passkey: None,
        }
    }

    pub fn passkey_webauthn(binding: PasskeyWebAuthnBinding) -> Self {
        Self {
            kind: ProofBindingKind::PasskeyWebAuthn,
            subject: binding.credential_id.clone(),
            chain_id: None,
            verified_at: binding.last_used_at,
            passkey: Some(binding),
        }
    }

    pub fn id(&self) -> String {
        match self.kind {
            ProofBindingKind::PasskeyWebAuthn => {
                if let Some(passkey) = &self.passkey {
                    let digest = sha2::Sha256::digest(format!(
                        "{}:{}",
                        passkey.rp_id, passkey.credential_id
                    ));
                    format!(
                        "proof:passkey:{}:{}",
                        passkey.rp_id,
                        hex::encode(&digest[..16])
                    )
                } else {
                    let digest = sha2::Sha256::digest(self.subject.as_bytes());
                    format!("proof:passkey:unknown:{}", hex::encode(&digest[..16]))
                }
            }
            ProofBindingKind::EvmAccount => {
                format!(
                    "proof:wallet:eip155:{}:{}",
                    self.chain_id.unwrap_or_default(),
                    self.subject
                )
            }
            ProofBindingKind::BtcAddress => format!("proof:wallet:bip122:{}", self.subject),
            ProofBindingKind::DidKey => format!("proof:{}", self.subject),
            ProofBindingKind::DidElastos => format!("proof:{}", self.subject),
            ProofBindingKind::Essentials => format!("proof:essentials:{}", self.subject),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallengeV1 {
    pub schema: String,
    pub challenge_id: String,
    pub domain: String,
    pub uri: String,
    pub statement: String,
    pub address: String,
    pub chain_id: u64,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthChallengeInput {
    pub challenge_id: String,
    pub domain: String,
    pub uri: String,
    pub address: String,
    pub chain_id: u64,
    pub nonce: String,
    pub issued_at: u64,
    pub ttl_secs: u64,
    pub resources: Vec<String>,
}

impl AuthChallengeV1 {
    pub const SCHEMA: &'static str = "elastos.auth.challenge/v1";

    pub fn new(input: AuthChallengeInput) -> Self {
        let address = checksum_evm_address(&input.address)
            .expect("AuthChallengeInput address must be a valid EVM address");
        Self {
            schema: Self::SCHEMA.to_string(),
            challenge_id: input.challenge_id,
            domain: input.domain,
            uri: input.uri,
            statement: "Sign in to ElastOS Runtime.".to_string(),
            address,
            chain_id: input.chain_id,
            nonce: input.nonce,
            issued_at: input.issued_at,
            expires_at: input.issued_at.saturating_add(input.ttl_secs),
            resources: input.resources,
        }
    }

    pub fn challenge_resource(&self) -> String {
        format!("elastos://auth/challenge/{}", self.challenge_id)
    }

    pub fn siwe_message(&self) -> String {
        let mut message = format!(
            "{domain} wants you to sign in with your Ethereum account:\n\
             {address}\n\n\
             {statement}\n\n\
             URI: {uri}\n\
             Version: 1\n\
             Chain ID: {chain_id}\n\
             Nonce: {nonce}\n\
             Issued At: {issued_at}\n\
             Expiration Time: {expires_at}",
            domain = self.domain,
            address = self.address.trim(),
            statement = self.statement,
            uri = self.uri,
            chain_id = self.chain_id,
            nonce = self.nonce,
            issued_at = rfc3339(self.issued_at),
            expires_at = rfc3339(self.expires_at),
        );
        if !self.resources.is_empty() {
            message.push_str("\nResources:");
            for resource in &self.resources {
                message.push_str("\n- ");
                message.push_str(resource);
            }
        }
        message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSessionGrantV1 {
    pub schema: String,
    pub grant_id: String,
    pub session_id: String,
    pub principal_id: String,
    pub proof_binding_id: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub apps: Vec<String>,
}

impl AuthSessionGrantV1 {
    pub const SCHEMA: &'static str = "elastos.auth.session-grant/v1";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAuditEventV1 {
    pub schema: String,
    pub event_id: String,
    pub event_type: String,
    pub principal_id: Option<String>,
    pub proof_binding_id: Option<String>,
    pub session_id: Option<String>,
    pub challenge_id: Option<String>,
    pub capsule_id: Option<String>,
    pub result: String,
    pub reason: String,
    pub occurred_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl RuntimeAuditEventV1 {
    pub const SCHEMA: &'static str = "elastos.audit.event/v1";
}

pub const PRINCIPAL_ROOT_PROTECTION_SCHEMA: &str = "elastos.principal.root-protection/v1";
pub const PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA: &str = "elastos.principal.root-recovery.status/v1";
pub const RECOVERY_KIT_SCHEMA: &str = "elastos.recovery-kit/v1";
pub const DEFAULT_PRINCIPAL_ROOT_CIPHER: &str = "aes-256-gcm";
pub const DEFAULT_PRINCIPAL_ROOT_SIGNATURES: &[&str] = &["ed25519", "ml-dsa-65"];
pub const DEFAULT_PRINCIPAL_ROOT_KEMS: &[&str] = &["x25519", "ml-kem-768"];
pub const DEFAULT_PRINCIPAL_ROOT_RECOVERY_KDF: &str = "argon2id";
pub const ALLOWED_PRINCIPAL_ROOT_CIPHERS: &[&str] = &["aes-256-gcm", "chacha20-poly1305"];
pub const ALLOWED_PRINCIPAL_ROOT_SIGNATURES: &[&str] =
    &["ed25519", "ml-dsa-65", "slh-dsa-sha2-256s"];
pub const REQUIRED_PRINCIPAL_ROOT_PQ_SIGNATURES: &[&str] = &["ml-dsa-65", "slh-dsa-sha2-256s"];
pub const ALLOWED_PRINCIPAL_ROOT_KEMS: &[&str] = &["x25519", "ml-kem-768", "hqc"];
pub const REQUIRED_PRINCIPAL_ROOT_HYBRID_KEMS: &[&str] = &["x25519", "ml-kem-768"];
pub const ALLOWED_PRINCIPAL_ROOT_RECOVERY_KDFS: &[&str] =
    &["argon2id", "webauthn-prf", "hkdf-sha256"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootCryptoProfileV1 {
    pub cipher: String,
    pub signatures: Vec<String>,
    pub kems: Vec<String>,
    pub recovery_kdf: String,
}

impl Default for PrincipalRootCryptoProfileV1 {
    fn default() -> Self {
        Self {
            cipher: DEFAULT_PRINCIPAL_ROOT_CIPHER.to_string(),
            signatures: DEFAULT_PRINCIPAL_ROOT_SIGNATURES
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            kems: DEFAULT_PRINCIPAL_ROOT_KEMS
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            recovery_kdf: DEFAULT_PRINCIPAL_ROOT_RECOVERY_KDF.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRootProtectorKind {
    RecoveryPhrase,
    RecoveryKit,
    WebAuthnPrf,
    DidRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootProtectorV1 {
    pub protector_id: String,
    pub kind: PrincipalRootProtectorKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<PrincipalRootProtectorEnvelopeV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<PrincipalRootRecoveryArchiveV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootProtectorEnvelopeV1 {
    pub cipher: String,
    pub kdf: String,
    pub salt: String,
    pub nonce: String,
    pub wrapped_data_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootRecoveryArchiveV1 {
    pub cipher: String,
    pub nonce: String,
    pub encrypted_recovery_kit: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootProtectionV1 {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub data_key_id: String,
    pub crypto: PrincipalRootCryptoProfileV1,
    pub protectors: Vec<PrincipalRootProtectorV1>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRootRecoveryStatusV1 {
    pub schema: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub root_encrypted: bool,
    pub recovery_configured: bool,
    pub recovery_download_available: bool,
    pub protection_configured: bool,
    pub required_actions: Vec<String>,
    pub crypto: PrincipalRootCryptoProfileV1,
}

impl PrincipalRootRecoveryStatusV1 {
    pub fn unprotected(principal_id: String, localhost_root: String) -> Self {
        Self {
            schema: PRINCIPAL_ROOT_RECOVERY_STATUS_SCHEMA.to_string(),
            principal_id,
            localhost_root,
            root_encrypted: false,
            recovery_configured: false,
            recovery_download_available: false,
            protection_configured: false,
            required_actions: vec![
                "generate_principal_data_key".to_string(),
                "encrypt_principal_root".to_string(),
                "create_recovery_kit".to_string(),
                "verify_recovery_before_public_guest_hosting".to_string(),
            ],
            crypto: PrincipalRootCryptoProfileV1::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryKitV1 {
    pub schema: String,
    pub kit_id: String,
    pub protector_id: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub data_key_id: String,
    pub recovery_phrase: String,
    pub salt: String,
    pub nonce: String,
    pub wrapped_data_key: String,
    pub encrypted_root_descriptor: String,
    pub crypto: PrincipalRootCryptoProfileV1,
    pub created_at: u64,
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DidRecoveryProofV1 {
    pub schema: String,
    pub did: String,
    pub principal_id: String,
    pub localhost_root: String,
    pub protector_id: String,
    pub data_key_id: String,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub signature: String,
}

pub fn validate_principal_root_crypto_profile(
    crypto: &PrincipalRootCryptoProfileV1,
) -> Result<(), String> {
    require_allowed_auth_algorithm(
        &crypto.cipher,
        ALLOWED_PRINCIPAL_ROOT_CIPHERS,
        "principal_root.crypto.cipher",
    )?;
    require_allowed_auth_algorithm(
        &crypto.recovery_kdf,
        ALLOWED_PRINCIPAL_ROOT_RECOVERY_KDFS,
        "principal_root.crypto.recovery_kdf",
    )?;
    require_non_empty_auth_algorithm_list(&crypto.signatures, "principal_root.crypto.signatures")?;
    require_non_empty_auth_algorithm_list(&crypto.kems, "principal_root.crypto.kems")?;
    require_all_allowed_auth_algorithms(
        &crypto.signatures,
        ALLOWED_PRINCIPAL_ROOT_SIGNATURES,
        "principal_root.crypto.signatures",
    )?;
    require_any_auth_algorithm(
        &crypto.signatures,
        REQUIRED_PRINCIPAL_ROOT_PQ_SIGNATURES,
        "principal_root.crypto.signatures",
    )?;
    require_all_allowed_auth_algorithms(
        &crypto.kems,
        ALLOWED_PRINCIPAL_ROOT_KEMS,
        "principal_root.crypto.kems",
    )?;
    require_all_required_auth_algorithms(
        &crypto.kems,
        REQUIRED_PRINCIPAL_ROOT_HYBRID_KEMS,
        "principal_root.crypto.kems",
    )
}

pub fn validate_principal_root_protection(
    protection: &PrincipalRootProtectionV1,
) -> Result<(), String> {
    if protection.schema != PRINCIPAL_ROOT_PROTECTION_SCHEMA {
        return Err("unsupported principal root protection schema".to_string());
    }
    validate_principal_root_common(
        &protection.principal_id,
        &protection.localhost_root,
        &protection.data_key_id,
    )?;
    validate_principal_root_crypto_profile(&protection.crypto)?;
    if protection.protectors.is_empty() {
        return Err("principal root protection requires at least one protector".to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    for protector in &protection.protectors {
        validate_auth_token_like_id(&protector.protector_id, "protector_id")?;
        if protector.label.trim().is_empty() {
            return Err("protector label is required".to_string());
        }
        validate_principal_root_protector_subject(protector)?;
        if !seen.insert(protector.protector_id.as_str()) {
            return Err("duplicate protector_id".to_string());
        }
        if let Some(envelope) = &protector.envelope {
            validate_principal_root_protector_envelope(envelope)?;
            validate_principal_root_protector_kind_envelope(protector, envelope)?;
        } else if protector.verified_at.is_some() {
            return Err("verified protector requires an encrypted data-key envelope".to_string());
        }
        if let Some(archive) = &protector.archive {
            validate_principal_root_protector_archive_kind(protector)?;
            validate_principal_root_recovery_archive(archive)?;
        }
    }
    Ok(())
}

pub fn validate_recovery_kit(kit: &RecoveryKitV1) -> Result<(), String> {
    if kit.schema != RECOVERY_KIT_SCHEMA {
        return Err("unsupported recovery kit schema".to_string());
    }
    validate_principal_root_common(&kit.principal_id, &kit.localhost_root, &kit.data_key_id)?;
    validate_auth_token_like_id(&kit.kit_id, "kit_id")?;
    validate_auth_token_like_id(&kit.protector_id, "protector_id")?;
    validate_principal_root_crypto_profile(&kit.crypto)?;
    validate_recovery_phrase(&kit.recovery_phrase)?;
    validate_base64url_field(&kit.salt, "recovery kit salt")?;
    validate_base64url_field(&kit.nonce, "recovery kit nonce")?;
    validate_base64url_field(&kit.wrapped_data_key, "wrapped data key")?;
    if kit.encrypted_root_descriptor.trim().is_empty() {
        return Err("recovery kit encrypted_root_descriptor is required".to_string());
    }
    if kit.instructions.is_empty()
        || kit
            .instructions
            .iter()
            .any(|instruction| instruction.trim().is_empty())
    {
        return Err("recovery kit instructions are required".to_string());
    }
    Ok(())
}

pub fn validate_did_recovery_proof(proof: &DidRecoveryProofV1) -> Result<(), String> {
    if proof.schema != "elastos.did.recovery-proof/v1" {
        return Err("unsupported DID recovery proof schema".to_string());
    }
    if !(proof.did.starts_with("did:key:") || proof.did.starts_with("did:elastos:")) {
        return Err("DID recovery proof subject must be did:key or did:elastos".to_string());
    }
    validate_principal_root_common(
        &proof.principal_id,
        &proof.localhost_root,
        &proof.data_key_id,
    )?;
    validate_auth_token_like_id(&proof.protector_id, "did_recovery_proof.protector_id")?;
    validate_auth_token_like_id(&proof.nonce, "did_recovery_proof.nonce")?;
    if proof.signature.is_empty()
        || proof.signature.len() > 4096
        || proof.signature.len() % 2 != 0
        || !proof.signature.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err("did_recovery_proof.signature must be hex encoded".to_string());
    }
    if proof.expires_at <= proof.issued_at {
        return Err("DID recovery proof expiry is invalid".to_string());
    }
    Ok(())
}

fn validate_principal_root_protector_envelope(
    envelope: &PrincipalRootProtectorEnvelopeV1,
) -> Result<(), String> {
    require_allowed_auth_algorithm(
        &envelope.cipher,
        ALLOWED_PRINCIPAL_ROOT_CIPHERS,
        "protector.envelope.cipher",
    )?;
    require_allowed_auth_algorithm(
        &envelope.kdf,
        ALLOWED_PRINCIPAL_ROOT_RECOVERY_KDFS,
        "protector.envelope.kdf",
    )?;
    validate_base64url_field(&envelope.salt, "protector envelope salt")?;
    validate_base64url_field(&envelope.nonce, "protector envelope nonce")?;
    validate_base64url_field(&envelope.wrapped_data_key, "protector wrapped data key")
}

fn validate_principal_root_protector_kind_envelope(
    protector: &PrincipalRootProtectorV1,
    envelope: &PrincipalRootProtectorEnvelopeV1,
) -> Result<(), String> {
    if protector.kind == PrincipalRootProtectorKind::WebAuthnPrf && envelope.kdf != "webauthn-prf" {
        return Err("WebAuthn PRF protector envelope must use webauthn-prf".to_string());
    }
    if protector.kind == PrincipalRootProtectorKind::DidRecovery && envelope.kdf != "hkdf-sha256" {
        return Err("DID recovery protector envelope must use hkdf-sha256".to_string());
    }
    Ok(())
}

fn validate_principal_root_protector_subject(
    protector: &PrincipalRootProtectorV1,
) -> Result<(), String> {
    if let Some(subject) = &protector.subject {
        validate_auth_token_like_id(subject, "protector.subject")?;
    }
    if protector.kind == PrincipalRootProtectorKind::DidRecovery {
        let Some(subject) = protector.subject.as_deref() else {
            return Err(
                "DID recovery protector subject must be did:key or did:elastos".to_string(),
            );
        };
        if !(subject.starts_with("did:key:") || subject.starts_with("did:elastos:")) {
            return Err(
                "DID recovery protector subject must be did:key or did:elastos".to_string(),
            );
        }
        if protector.envelope.is_none() {
            return Err(
                "DID recovery protector requires an encrypted data-key envelope".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_principal_root_protector_archive_kind(
    protector: &PrincipalRootProtectorV1,
) -> Result<(), String> {
    if protector.kind != PrincipalRootProtectorKind::RecoveryKit {
        return Err("only Recovery Kit protectors can carry recovery archives".to_string());
    }
    Ok(())
}

fn validate_principal_root_recovery_archive(
    archive: &PrincipalRootRecoveryArchiveV1,
) -> Result<(), String> {
    require_allowed_auth_algorithm(
        &archive.cipher,
        ALLOWED_PRINCIPAL_ROOT_CIPHERS,
        "protector.archive.cipher",
    )?;
    validate_base64url_field(&archive.nonce, "protector archive nonce")?;
    validate_base64url_field(
        &archive.encrypted_recovery_kit,
        "protector encrypted recovery kit",
    )
}

fn validate_recovery_phrase(value: &str) -> Result<(), String> {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() < 8
        || parts
            .iter()
            .any(|part| part.len() != 4 || !part.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return Err("recovery phrase must be generated by ElastOS".to_string());
    }
    Ok(())
}

fn validate_base64url_field(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 4096
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '=')
    {
        return Err(format!("{field} must be base64url encoded"));
    }
    Ok(())
}

fn validate_principal_root_common(
    principal_id: &str,
    localhost_root: &str,
    data_key_id: &str,
) -> Result<(), String> {
    validate_auth_token_like_id(principal_id, "principal_id")?;
    validate_auth_token_like_id(data_key_id, "data_key_id")?;
    if !data_key_id.starts_with("pdek:") {
        return Err("data_key_id must start with pdek:".to_string());
    }
    validate_principal_localhost_root(localhost_root)
}

fn validate_principal_localhost_root(localhost_root: &str) -> Result<(), String> {
    let Some(root_segment) = localhost_root.strip_prefix("localhost://Users/") else {
        return Err("localhost_root must be a principal-owned localhost user root".to_string());
    };
    if root_segment.is_empty()
        || root_segment == "self"
        || root_segment == "."
        || root_segment == ".."
        || root_segment.len() > 256
        || root_segment
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err("localhost_root must be a principal-owned localhost user root".to_string());
    }
    Ok(())
}

fn validate_auth_token_like_id(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > 512
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        Err(format!("{field} is invalid"))
    } else {
        Ok(())
    }
}

fn require_non_empty_auth_algorithm_list(values: &[String], field: &str) -> Result<(), String> {
    if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
        Err(format!("{field} must be a non-empty list"))
    } else {
        Ok(())
    }
}

fn require_allowed_auth_algorithm(
    value: &str,
    allowed: &[&str],
    field: &str,
) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} is required"));
    }
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} uses unsupported algorithm: {value}"))
    }
}

fn require_all_allowed_auth_algorithms(
    values: &[String],
    allowed: &[&str],
    field: &str,
) -> Result<(), String> {
    for value in values {
        require_allowed_auth_algorithm(value, allowed, field)?;
    }
    Ok(())
}

fn require_any_auth_algorithm(
    values: &[String],
    required: &[&str],
    field: &str,
) -> Result<(), String> {
    if values
        .iter()
        .any(|value| required.contains(&value.as_str()))
    {
        Ok(())
    } else {
        Err(format!("{field} must include a post-quantum algorithm"))
    }
}

fn require_all_required_auth_algorithms(
    values: &[String],
    required: &[&str],
    field: &str,
) -> Result<(), String> {
    for required_value in required {
        if !values.iter().any(|value| value == required_value) {
            return Err(format!("{field} must include {required_value}"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiweMessage {
    pub domain: String,
    pub address: String,
    pub uri: String,
    pub version: String,
    pub chain_id: u64,
    pub nonce: String,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSiweProof {
    pub binding: ProofBinding,
    pub recovered_address: String,
    pub message_hash: [u8; 32],
}

pub fn verify_siwe_challenge(
    challenge: &AuthChallengeV1,
    message: &str,
    signature_hex: &str,
    now: u64,
) -> Result<VerifiedSiweProof, String> {
    if challenge.schema != AuthChallengeV1::SCHEMA {
        return Err("unsupported auth challenge schema".to_string());
    }
    if challenge.expires_at <= now {
        return Err("auth challenge expired".to_string());
    }
    validate_siwe_origin_binding(&challenge.domain, &challenge.uri)?;
    let parsed = parse_siwe_message(message)?;
    if parsed.domain != challenge.domain {
        return Err("SIWE domain does not match challenge".to_string());
    }
    if parsed.uri != challenge.uri {
        return Err("SIWE URI does not match challenge".to_string());
    }
    if parsed.version != "1" {
        return Err("unsupported SIWE version".to_string());
    }
    if parsed.chain_id != challenge.chain_id {
        return Err("SIWE chain ID does not match challenge".to_string());
    }
    if parsed.nonce != challenge.nonce {
        return Err("SIWE nonce does not match challenge".to_string());
    }
    if normalize_evm_address(&parsed.address) != normalize_evm_address(&challenge.address) {
        return Err("SIWE address does not match challenge".to_string());
    }
    if parsed.issued_at > now.saturating_add(300) {
        return Err("SIWE issued-at is in the future".to_string());
    }
    let Some(expires_at) = parsed.expires_at else {
        return Err("SIWE expiration-time is required".to_string());
    };
    if expires_at <= now {
        return Err("SIWE proof expired".to_string());
    }
    if expires_at > challenge.expires_at {
        return Err("SIWE proof outlives challenge".to_string());
    }
    if parsed.resources != challenge.resources {
        return Err("SIWE resources do not match challenge".to_string());
    }
    if !parsed
        .resources
        .iter()
        .any(|resource| resource == &challenge.challenge_resource())
    {
        return Err("SIWE resources missing challenge binding".to_string());
    }
    if message != challenge.siwe_message() {
        return Err("SIWE message does not match challenge".to_string());
    }

    let (recovered_address, message_hash) = recover_evm_address(message, signature_hex)?;
    if normalize_evm_address(&recovered_address) != normalize_evm_address(&challenge.address) {
        return Err("SIWE signature does not recover expected address".to_string());
    }

    Ok(VerifiedSiweProof {
        binding: ProofBinding::evm_account(challenge.chain_id, &recovered_address, now),
        recovered_address: normalize_evm_address(&recovered_address),
        message_hash,
    })
}

pub fn parse_siwe_message(message: &str) -> Result<SiweMessage, String> {
    let mut lines = message.lines();
    let first = lines
        .next()
        .ok_or_else(|| "SIWE message missing domain line".to_string())?;
    let suffix = " wants you to sign in with your Ethereum account:";
    let domain = first
        .strip_suffix(suffix)
        .ok_or_else(|| "SIWE message has invalid domain line".to_string())?
        .to_string();
    let address = lines
        .next()
        .ok_or_else(|| "SIWE message missing address".to_string())?
        .trim()
        .to_string();
    validate_evm_address(&address)?;

    let mut uri = None;
    let mut version = None;
    let mut chain_id = None;
    let mut nonce = None;
    let mut issued_at = None;
    let mut expires_at = None;
    let mut resources = Vec::new();
    let mut in_resources = false;

    for line in lines {
        if line == "Resources:" {
            in_resources = true;
            continue;
        }
        if in_resources {
            let Some(resource) = line.strip_prefix("- ") else {
                return Err("SIWE resources must use '- ' lines".to_string());
            };
            resources.push(resource.to_string());
            continue;
        }

        if let Some(value) = line.strip_prefix("URI: ") {
            uri = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Version: ") {
            version = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Chain ID: ") {
            chain_id = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| "SIWE chain ID must be numeric".to_string())?,
            );
        } else if let Some(value) = line.strip_prefix("Nonce: ") {
            nonce = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("Issued At: ") {
            issued_at = Some(parse_rfc3339(value)?);
        } else if let Some(value) = line.strip_prefix("Expiration Time: ") {
            expires_at = Some(parse_rfc3339(value)?);
        }
    }

    let uri = uri.ok_or_else(|| "SIWE URI is required".to_string())?;
    validate_siwe_origin_binding(&domain, &uri)?;

    Ok(SiweMessage {
        domain,
        address: normalize_evm_address(&address),
        uri,
        version: version.ok_or_else(|| "SIWE version is required".to_string())?,
        chain_id: chain_id.ok_or_else(|| "SIWE chain ID is required".to_string())?,
        nonce: nonce.ok_or_else(|| "SIWE nonce is required".to_string())?,
        issued_at: issued_at.ok_or_else(|| "SIWE issued-at is required".to_string())?,
        expires_at,
        resources,
    })
}

pub fn validate_siwe_origin_binding(origin: &str, uri: &str) -> Result<(), String> {
    validate_explicit_siwe_origin(origin)?;
    let uri_origin = raw_web_origin(uri, "SIWE URI")?;
    if uri_origin != origin {
        return Err("SIWE URI origin does not match SIWE domain".to_string());
    }
    Ok(())
}

fn validate_explicit_siwe_origin(origin: &str) -> Result<(), String> {
    let raw_origin = raw_web_origin(origin, "SIWE domain")?;
    if raw_origin != origin {
        return Err("SIWE domain must contain only the explicit origin".to_string());
    }
    Ok(())
}

fn raw_web_origin<'a>(value: &'a str, field: &str) -> Result<&'a str, String> {
    if value.is_empty()
        || value.len() > 512
        || !value.is_ascii()
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err(format!("{field} is invalid"));
    }
    let (scheme, remainder) = value
        .split_once("://")
        .ok_or_else(|| format!("{field} must include an explicit scheme"))?;
    if scheme != "http" && scheme != "https" {
        return Err(format!("{field} must use http or https"));
    }
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    validate_web_authority(authority, field)?;
    let suffix = &remainder[authority_end..];
    if suffix.contains('\\') || !valid_percent_encoding(suffix) {
        return Err(format!("{field} is not a valid absolute URL"));
    }
    Ok(&value[..scheme.len() + 3 + authority.len()])
}

fn validate_web_authority(authority: &str, field: &str) -> Result<(), String> {
    if authority.is_empty() || authority.contains('@') || authority.contains('\\') {
        return Err(format!("{field} has an invalid authority"));
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| format!("{field} has an invalid authority"))?;
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| format!("{field} has an invalid authority"))?;
        if !suffix.is_empty() {
            let port = suffix
                .strip_prefix(':')
                .ok_or_else(|| format!("{field} has an invalid authority"))?;
            validate_web_port(port, field)?;
        }
        return Ok(());
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err(format!("{field} has an invalid authority"));
            }
            (host, Some(port))
        }
        None => (authority, None),
    };
    if let Some(port) = port {
        validate_web_port(port, field)?;
    }
    if host.is_empty()
        || host.len() > 253
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(format!("{field} has an invalid authority"));
    }
    Ok(())
}

fn validate_web_port(port: &str, field: &str) -> Result<(), String> {
    if port.is_empty()
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().is_err()
    {
        return Err(format!("{field} has an invalid authority"));
    }
    Ok(())
}

fn valid_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return false;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    true
}

pub fn recover_evm_address(
    message: &str,
    signature_hex: &str,
) -> Result<(String, [u8; 32]), String> {
    let signature_hex = signature_hex.strip_prefix("0x").unwrap_or(signature_hex);
    let bytes =
        hex::decode(signature_hex).map_err(|err| format!("invalid signature hex: {err}"))?;
    if bytes.len() != 65 {
        return Err("EVM signature must be 65 bytes".to_string());
    }
    let signature =
        Signature::try_from(&bytes[..64]).map_err(|err| format!("invalid EVM signature: {err}"))?;
    let recovery_id = normalize_recovery_id(bytes[64])?;
    let message_hash = ethereum_signed_message_hash(message.as_bytes());
    let verifying_key = VerifyingKey::recover_from_prehash(&message_hash, &signature, recovery_id)
        .map_err(|err| format!("failed to recover EVM signer: {err}"))?;
    let encoded = verifying_key.to_encoded_point(false);
    let public_key = encoded.as_bytes();
    if public_key.len() != 65 {
        return Err("unexpected recovered public key length".to_string());
    }
    let digest = Keccak256::digest(&public_key[1..]);
    let address = format!("0x{}", hex::encode(&digest[12..]));
    Ok((address, message_hash))
}

pub fn ethereum_signed_message_hash(message: &[u8]) -> [u8; 32] {
    let prefix = format!("\x19Ethereum Signed Message:\n{}", message.len());
    let mut hasher = Keccak256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(message);
    hasher.finalize().into()
}

pub fn normalize_evm_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

pub fn validate_evm_address(address: &str) -> Result<(), String> {
    let address = address.trim();
    let raw = address
        .strip_prefix("0x")
        .ok_or_else(|| "EVM address must start with 0x".to_string())?;
    if raw.len() != 40 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("EVM address must be 20 bytes of hex".to_string());
    }
    Ok(())
}

pub fn checksum_evm_address(address: &str) -> Result<String, String> {
    validate_evm_address(address)?;
    let raw = address
        .trim()
        .strip_prefix("0x")
        .ok_or_else(|| "EVM address must start with 0x".to_string())?
        .to_ascii_lowercase();
    let hash = hex::encode(Keccak256::digest(raw.as_bytes()));
    let mut checksum = String::with_capacity(42);
    checksum.push_str("0x");
    for (index, ch) in raw.chars().enumerate() {
        let hash_nibble = hash
            .as_bytes()
            .get(index)
            .and_then(|byte| (*byte as char).to_digit(16))
            .ok_or_else(|| "failed to checksum EVM address".to_string())?;
        if ch.is_ascii_alphabetic() && hash_nibble >= 8 {
            checksum.push(ch.to_ascii_uppercase());
        } else {
            checksum.push(ch);
        }
    }
    Ok(checksum)
}

fn normalize_recovery_id(v: u8) -> Result<RecoveryId, String> {
    let id = match v {
        0 | 1 => v,
        27 | 28 => v - 27,
        _ => return Err("unsupported EVM recovery id".to_string()),
    };
    RecoveryId::try_from(id).map_err(|err| format!("invalid recovery id: {err}"))
}

pub fn rfc3339(unix_secs: u64) -> String {
    Utc.timestamp_opt(unix_secs as i64, 0)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_opt(0, 0)
                .single()
                .expect("unix epoch is valid")
        })
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse_rfc3339(value: &str) -> Result<u64, String> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|err| format!("invalid RFC3339 timestamp: {err}"))?;
    let timestamp = parsed.timestamp();
    if timestamp < 0 {
        return Err("timestamp before unix epoch".to_string());
    }
    Ok(timestamp as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;
    use sha3::{Digest, Keccak256};

    fn test_address(signing_key: &SigningKey) -> String {
        let verifying_key = signing_key.verifying_key();
        let encoded = verifying_key.to_encoded_point(false);
        let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
        format!("0x{}", hex::encode(&digest[12..]))
    }

    fn sign_message(signing_key: &SigningKey, message: &str) -> String {
        let hash = ethereum_signed_message_hash(message.as_bytes());
        let (signature, recovery_id) = signing_key
            .sign_prehash_recoverable(&hash)
            .expect("test signature should be recoverable");
        let mut bytes = signature.to_bytes().to_vec();
        bytes.push(recovery_id.to_byte());
        format!("0x{}", hex::encode(bytes))
    }

    fn challenge() -> (SigningKey, AuthChallengeV1) {
        let signing_key = SigningKey::from_bytes((&[7u8; 32]).into()).unwrap();
        let address = test_address(&signing_key);
        let challenge = AuthChallengeV1::new(AuthChallengeInput {
            challenge_id: "challenge-1".to_string(),
            domain: "https://elastos.local".to_string(),
            uri: "https://elastos.local/apps/home/".to_string(),
            address,
            chain_id: 8453,
            nonce: "ABC123xyz".to_string(),
            issued_at: 1_800_000_000,
            ttl_secs: 300,
            resources: vec![
                "elastos://auth/challenge/challenge-1".to_string(),
                "elastos://apps/home".to_string(),
                "elastos://apps/system".to_string(),
            ],
        });
        (signing_key, challenge)
    }

    #[test]
    fn verifies_matching_siwe_challenge() {
        let (signing_key, challenge) = challenge();
        let message = challenge.siwe_message();
        let signature = sign_message(&signing_key, &message);
        let proof = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect("valid proof should verify");
        assert_eq!(
            proof.binding.id(),
            format!(
                "proof:wallet:eip155:8453:{}",
                normalize_evm_address(&challenge.address)
            )
        );
    }

    #[test]
    fn siwe_message_preserves_wallet_address_display_case() {
        let signing_key = SigningKey::from_bytes((&[7u8; 32]).into()).unwrap();
        let address = test_address(&signing_key);
        let display_address = checksum_evm_address(&address).unwrap();
        let lower_address = address.to_ascii_lowercase();
        let challenge = AuthChallengeV1::new(AuthChallengeInput {
            challenge_id: "challenge-1".to_string(),
            domain: "https://elastos.local".to_string(),
            uri: "https://elastos.local/apps/home/".to_string(),
            address: lower_address,
            chain_id: 8453,
            nonce: "ABC123xyz".to_string(),
            issued_at: 1_800_000_000,
            ttl_secs: 300,
            resources: vec!["elastos://auth/challenge/challenge-1".to_string()],
        });
        let message = challenge.siwe_message();

        assert_eq!(challenge.address, display_address);
        assert!(message.contains(&format!("Ethereum account:\n{display_address}\n\n")));
        let signature = sign_message(&signing_key, &message);
        let proof = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect("case-preserved SIWE proof should verify");
        assert_eq!(proof.recovered_address, normalize_evm_address(&address));
    }

    #[test]
    fn passkey_binding_id_is_stable_and_origin_scoped() {
        let binding = ProofBinding::passkey_webauthn(PasskeyWebAuthnBinding {
            credential_id: "credential-1".to_string(),
            public_key: "public-key-cose".to_string(),
            sign_count: 7,
            user_verified: true,
            origin: "https://elastos.elacitylabs.com".to_string(),
            rp_id: "elastos.elacitylabs.com".to_string(),
            created_at: 1_800_000_000,
            last_used_at: 1_800_000_010,
            revoked_at: None,
        });

        assert_eq!(binding.kind, ProofBindingKind::PasskeyWebAuthn);
        assert_eq!(binding.chain_id, None);
        assert_eq!(binding.verified_at, 1_800_000_010);
        assert!(binding
            .id()
            .starts_with("proof:passkey:elastos.elacitylabs.com:"));
        assert_eq!(binding.id(), binding.id());
    }

    #[test]
    fn rejects_replay_nonce_mismatch() {
        let (signing_key, challenge) = challenge();
        let mut message = challenge.siwe_message();
        message = message.replace("Nonce: ABC123xyz", "Nonce: XYZ123abc");
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect_err("nonce mismatch should fail");
        assert!(err.contains("nonce"));
    }

    #[test]
    fn rejects_wrong_chain() {
        let (signing_key, challenge) = challenge();
        let mut message = challenge.siwe_message();
        message = message.replace("Chain ID: 8453", "Chain ID: 20");
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect_err("chain mismatch should fail");
        assert!(err.contains("chain ID"));
    }

    #[test]
    fn rejects_wrong_origin() {
        let (signing_key, challenge) = challenge();
        let mut message = challenge.siwe_message();
        message = message.replace("https://elastos.local wants", "https://evil.example wants");
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect_err("domain mismatch should fail");
        assert!(err.contains("domain"));
    }

    #[test]
    fn accepts_scheme_explicit_localhost_http_origin() {
        let (signing_key, mut challenge) = challenge();
        challenge.domain = "http://localhost:61180".to_string();
        challenge.uri = "http://localhost:61180/apps/home/".to_string();
        let message = challenge.siwe_message();
        assert!(message.starts_with(
            "http://localhost:61180 wants you to sign in with your Ethereum account:"
        ));
        let signature = sign_message(&signing_key, &message);
        verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect("scheme-explicit localhost SIWE proof should verify");
    }

    #[test]
    fn rejects_siwe_origin_component_substitutions_and_legacy_domain() {
        let (signing_key, challenge) = challenge();
        let expected = challenge.siwe_message();
        for (from, to) in [
            ("https://elastos.local wants", "http://elastos.local wants"),
            ("https://elastos.local wants", "https://evil.example wants"),
            (
                "https://elastos.local wants",
                "https://elastos.local:8443 wants",
            ),
            (
                "URI: https://elastos.local/apps/home/",
                "URI: https://elastos.local:8443/apps/home/",
            ),
            ("https://elastos.local wants", "elastos.local wants"),
        ] {
            let message = expected.replace(from, to);
            assert_ne!(message, expected);
            let signature = sign_message(&signing_key, &message);
            verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
                .expect_err("SIWE origin substitutions and legacy domains must fail closed");
        }
    }

    #[test]
    fn rejects_modified_statement() {
        let (signing_key, challenge) = challenge();
        let mut message = challenge.siwe_message();
        message = message.replace(
            "Sign in to ElastOS Runtime.",
            "Sign this harmless demo message.",
        );
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect_err("statement tampering should fail");
        assert!(err.contains("does not match challenge"));
    }

    #[test]
    fn rejects_wrong_resources() {
        let (signing_key, challenge) = challenge();
        let mut message = challenge.siwe_message();
        message = message.replace("elastos://apps/system", "elastos://apps/wallet");
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_010)
            .expect_err("resource mismatch should fail");
        assert!(err.contains("resources"));
    }

    #[test]
    fn rejects_expired_challenge() {
        let (signing_key, challenge) = challenge();
        let message = challenge.siwe_message();
        let signature = sign_message(&signing_key, &message);
        let err = verify_siwe_challenge(&challenge, &message, &signature, 1_800_000_400)
            .expect_err("expired challenge should fail");
        assert!(err.contains("expired"));
    }

    fn principal_root_protection() -> PrincipalRootProtectionV1 {
        PrincipalRootProtectionV1 {
            schema: PRINCIPAL_ROOT_PROTECTION_SCHEMA.to_string(),
            principal_id: "person:local:abc123".to_string(),
            localhost_root: "localhost://Users/person:local:abc123".to_string(),
            data_key_id: "pdek:abc123".to_string(),
            crypto: PrincipalRootCryptoProfileV1::default(),
            protectors: vec![PrincipalRootProtectorV1 {
                protector_id: "protector:recovery:abc123".to_string(),
                kind: PrincipalRootProtectorKind::RecoveryKit,
                label: "Recovery Kit".to_string(),
                subject: None,
                created_at: 1_800_000_000,
                verified_at: Some(1_800_000_010),
                envelope: Some(PrincipalRootProtectorEnvelopeV1 {
                    cipher: "aes-256-gcm".to_string(),
                    kdf: "hkdf-sha256".to_string(),
                    salt: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                    nonce: "AAAAAAAAAAAAAAAA".to_string(),
                    wrapped_data_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
                }),
                archive: None,
            }],
            created_at: 1_800_000_000,
            updated_at: 1_800_000_010,
        }
    }

    #[test]
    fn principal_root_protection_requires_pq_hybrid_metadata() {
        let protection = principal_root_protection();
        validate_principal_root_protection(&protection)
            .expect("default protection contract should be valid");

        let mut classical_only = protection.clone();
        classical_only.crypto.signatures = vec!["ed25519".to_string()];
        let err = validate_principal_root_protection(&classical_only)
            .expect_err("classical-only signatures must fail");
        assert!(err.contains("post-quantum"));

        let mut missing_ml_kem = protection;
        missing_ml_kem.crypto.kems = vec!["x25519".to_string()];
        let err = validate_principal_root_protection(&missing_ml_kem)
            .expect_err("hybrid KEM metadata must include ML-KEM");
        assert!(err.contains("ml-kem-768"));
    }

    #[test]
    fn principal_root_protection_rejects_shared_users_self_root() {
        let mut protection = principal_root_protection();
        protection.localhost_root = "localhost://Users/self/Documents".to_string();
        let err = validate_principal_root_protection(&protection)
            .expect_err("shared Users/self roots must not validate");
        assert!(err.contains("principal-owned localhost user root"));
    }

    #[test]
    fn principal_root_protection_rejects_user_root_subpaths() {
        let mut protection = principal_root_protection();
        protection.localhost_root = "localhost://Users/abc123/Documents".to_string();
        let err = validate_principal_root_protection(&protection)
            .expect_err("root protection must bind to one root, not a subpath");
        assert!(err.contains("principal-owned localhost user root"));
    }

    #[test]
    fn principal_root_protection_rejects_unknown_contract_fields_at_decode() {
        let mut root = serde_json::to_value(principal_root_protection()).unwrap();
        root.as_object_mut()
            .unwrap()
            .insert("hidden_authority".to_string(), serde_json::json!(true));
        let err = serde_json::from_value::<PrincipalRootProtectionV1>(root)
            .unwrap_err()
            .to_string();
        assert!(err.contains("hidden_authority"));

        let mut nested = serde_json::to_value(principal_root_protection()).unwrap();
        nested["protectors"][0]
            .as_object_mut()
            .unwrap()
            .insert("raw_prf_output".to_string(), serde_json::json!("secret"));
        let err = serde_json::from_value::<PrincipalRootProtectionV1>(nested)
            .unwrap_err()
            .to_string();
        assert!(err.contains("raw_prf_output"));
    }

    #[test]
    fn principal_root_protection_accepts_webauthn_prf_protector() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.protector_id = "protector:webauthn-prf:abc123".to_string();
        protector.kind = PrincipalRootProtectorKind::WebAuthnPrf;
        protector.label = "Passkey PRF".to_string();
        protector.archive = None;
        protector.envelope.as_mut().unwrap().kdf = "webauthn-prf".to_string();

        validate_principal_root_protection(&protection)
            .expect("WebAuthn PRF protector with PRF envelope should validate");
    }

    #[test]
    fn principal_root_protection_rejects_webauthn_prf_with_wrong_kdf() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.kind = PrincipalRootProtectorKind::WebAuthnPrf;
        protector.archive = None;
        protector.envelope.as_mut().unwrap().kdf = "hkdf-sha256".to_string();

        let err = validate_principal_root_protection(&protection)
            .expect_err("WebAuthn PRF protector must not validate with a phrase KDF");
        assert!(err.contains("webauthn-prf"));
    }

    #[test]
    fn principal_root_protection_rejects_archive_on_non_recovery_kit_protector() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.kind = PrincipalRootProtectorKind::WebAuthnPrf;
        protector.envelope.as_mut().unwrap().kdf = "webauthn-prf".to_string();
        protector.archive = Some(PrincipalRootRecoveryArchiveV1 {
            cipher: "aes-256-gcm".to_string(),
            nonce: "AAAAAAAAAAAAAAAA".to_string(),
            encrypted_recovery_kit: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            created_at: 1_800_000_010,
        });

        let err = validate_principal_root_protection(&protection)
            .expect_err("only Recovery Kit protectors should carry downloadable archives");
        assert!(err.contains("only Recovery Kit protectors"));
    }

    #[test]
    fn principal_root_protection_accepts_did_recovery_protector() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.protector_id = "protector:did:abc123".to_string();
        protector.kind = PrincipalRootProtectorKind::DidRecovery;
        protector.label = "Recovery DID".to_string();
        protector.subject =
            Some("did:key:z6Mkh11111111111111111111111111111111111111111".to_string());
        protector.archive = None;
        protector.envelope.as_mut().unwrap().kdf = "hkdf-sha256".to_string();

        validate_principal_root_protection(&protection)
            .expect("DID recovery protector with DID subject should validate");
    }

    #[test]
    fn principal_root_protection_rejects_did_recovery_without_did_subject() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.kind = PrincipalRootProtectorKind::DidRecovery;
        protector.subject = None;
        protector.archive = None;
        protector.envelope.as_mut().unwrap().kdf = "hkdf-sha256".to_string();

        let err = validate_principal_root_protection(&protection)
            .expect_err("DID recovery protector must identify a DID subject");
        assert!(err.contains("DID recovery protector subject"));
    }

    #[test]
    fn principal_root_protection_rejects_did_recovery_with_wrong_kdf() {
        let mut protection = principal_root_protection();
        let protector = &mut protection.protectors[0];
        protector.kind = PrincipalRootProtectorKind::DidRecovery;
        protector.subject = Some("did:elastos:abc123".to_string());
        protector.archive = None;
        protector.envelope.as_mut().unwrap().kdf = "argon2id".to_string();

        let err = validate_principal_root_protection(&protection)
            .expect_err("DID recovery protector must use DID-bound wrapping");
        assert!(err.contains("DID recovery protector envelope"));
    }

    #[test]
    fn recovery_kit_contract_validates_without_raw_key_material() {
        let kit = RecoveryKitV1 {
            schema: RECOVERY_KIT_SCHEMA.to_string(),
            kit_id: "kit:abc123".to_string(),
            protector_id: "protector:recovery:abc123".to_string(),
            principal_id: "person:local:abc123".to_string(),
            localhost_root: "localhost://Users/person:local:abc123".to_string(),
            data_key_id: "pdek:abc123".to_string(),
            recovery_phrase: "aaaa-bbbb-cccc-dddd-eeee-ffff-1111-2222-3333-4444".to_string(),
            salt: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            nonce: "AAAAAAAAAAAAAAAA".to_string(),
            wrapped_data_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
            encrypted_root_descriptor: "enc:v1:metadata-ciphertext".to_string(),
            crypto: PrincipalRootCryptoProfileV1 {
                recovery_kdf: "hkdf-sha256".to_string(),
                ..PrincipalRootCryptoProfileV1::default()
            },
            created_at: 1_800_000_000,
            instructions: vec![
                "Keep the recovery phrase offline.".to_string(),
                "Import this kit only through ElastOS Runtime recovery.".to_string(),
            ],
        };

        validate_recovery_kit(&kit).expect("recovery kit contract should be valid");
        let json = serde_json::to_string(&kit).unwrap();
        assert!(!json.contains("data_key_hex"));
        assert!(!json.contains("data_key_bytes"));
        assert!(!json.contains("private_key"));
        assert!(!json.contains("mnemonic"));
    }

    #[test]
    fn recovery_kit_rejects_unknown_fields_at_decode() {
        let value = serde_json::json!({
            "schema": RECOVERY_KIT_SCHEMA,
            "kit_id": "kit:abc123",
            "protector_id": "protector:recovery:abc123",
            "principal_id": "person:local:abc123",
            "localhost_root": "localhost://Users/abc123",
            "data_key_id": "pdek:abc123",
            "recovery_phrase": "aaaa-bbbb-cccc-dddd-eeee-ffff-1111-2222-3333-4444",
            "salt": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "nonce": "AAAAAAAAAAAAAAAA",
            "wrapped_data_key": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "encrypted_root_descriptor": "enc:v1:metadata-ciphertext",
            "crypto": PrincipalRootCryptoProfileV1::default(),
            "created_at": 1_800_000_000_u64,
            "instructions": ["Keep it offline."],
            "hidden_key": "must not deserialize",
        });

        assert!(serde_json::from_value::<RecoveryKitV1>(value).is_err());
    }

    #[test]
    fn did_recovery_proof_rejects_malformed_signature() {
        let proof = DidRecoveryProofV1 {
            schema: "elastos.did.recovery-proof/v1".to_string(),
            did: "did:key:z6Mktest".to_string(),
            principal_id: "person:local:abc123".to_string(),
            localhost_root: "localhost://Users/abc123".to_string(),
            protector_id: "protector:did:abc123".to_string(),
            data_key_id: "pdek:abc123".to_string(),
            nonce: "nonce:abc123".to_string(),
            issued_at: 1_800_000_000,
            expires_at: 1_800_000_300,
            signature: "not-hex".to_string(),
        };

        assert!(validate_did_recovery_proof(&proof)
            .unwrap_err()
            .contains("hex encoded"));
    }
}
