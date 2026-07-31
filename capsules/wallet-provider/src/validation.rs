use super::*;
use elastos_auth::{normalize_evm_address, validate_evm_address, validate_siwe_origin_binding};
use sha2::Digest;

pub(super) struct LinkAccountInput {
    pub(super) principal_id: String,
    pub(super) proof_binding_id: String,
    pub(super) chain_namespace: String,
    pub(super) address: String,
    pub(super) proof_type: String,
    pub(super) connector_id: Option<String>,
    pub(super) label: Option<String>,
}

pub(super) struct CreateManagedAccountInput {
    pub(super) principal_id: String,
    pub(super) chain_namespace: String,
    pub(super) label: Option<String>,
    pub(super) create_new: bool,
}

pub(super) struct SetDefaultAccountInput {
    pub(super) principal_id: String,
    pub(super) chain_namespace: String,
    pub(super) intent: String,
    pub(super) account_id: String,
}

pub(super) struct ImportManagedRecoveryKeyInput {
    pub(super) principal_id: String,
    pub(super) recovery_key: Value,
    pub(super) label: Option<String>,
}

pub(super) struct ChallengeInput {
    pub(super) domain: String,
    pub(super) uri: String,
    pub(super) address: String,
    pub(super) chain_id: u64,
    pub(super) resources: Vec<String>,
}

pub(super) struct BitcoinChallengeInput {
    pub(super) domain: String,
    pub(super) uri: String,
    pub(super) address: String,
    pub(super) network: String,
    pub(super) resources: Vec<String>,
}

pub(super) struct SignatureRequestInput {
    pub(super) request_id: String,
    pub(super) wallet_request_sha256: String,
    pub(super) authority_binding: String,
    pub(super) principal_id: String,
    pub(super) session_id: String,
    pub(super) launch_id: String,
    pub(super) account_id: String,
    pub(super) chain_namespace: String,
    pub(super) intent: String,
    pub(super) requested_by_actor: String,
    pub(super) resource: String,
    pub(super) reason: String,
    pub(super) payload: Value,
    pub(super) expires_at: u64,
}

impl ChallengeInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_siwe_origin_binding(&self.domain, &self.uri)?;
        validate_evm_address(&self.address)?;
        if self.chain_id == 0 {
            return Err("chain_id must be non-zero".to_string());
        }
        for resource in &self.resources {
            validate_resource(resource)?;
        }
        Ok(())
    }
}

impl BitcoinChallengeInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_domain(&self.domain)?;
        validate_uri(&self.uri)?;
        validate_bitcoin_network(&self.network)?;
        validate_bitcoin_address(&self.address, &self.network)?;
        for resource in &self.resources {
            validate_resource(resource)?;
        }
        Ok(())
    }
}

impl SignatureRequestInput {
    pub(super) fn validate(&self, now: u64) -> Result<(), String> {
        validate_opaque_id(&self.request_id, "request_id")?;
        validate_opaque_id(&self.wallet_request_sha256, "wallet_request_sha256")?;
        validate_hash(&self.authority_binding, "authority_binding")?;
        validate_opaque_id(&self.principal_id, "principal_id")?;
        validate_opaque_id(&self.session_id, "session_id")?;
        validate_opaque_id(&self.launch_id, "launch_id")?;
        validate_opaque_id(&self.account_id, "account_id")?;
        validate_opaque_id(&self.chain_namespace, "chain_namespace")?;
        validate_signing_intent(&self.intent)?;
        validate_opaque_id(&self.requested_by_actor, "requested_by_actor")?;
        validate_resource(&self.resource)?;
        validate_reason(&self.reason)?;
        let payload =
            serde_json::to_vec(&self.payload).map_err(|err| format!("invalid payload: {err}"))?;
        if payload.is_empty() || payload.len() > MAX_APPROVAL_PAYLOAD_BYTES {
            return Err(format!(
                "payload must be 1-{MAX_APPROVAL_PAYLOAD_BYTES} bytes"
            ));
        }
        if self.expires_at <= now {
            return Err("approval expires_at must be in the future".to_string());
        }
        if self.expires_at > now.saturating_add(MAX_APPROVAL_REQUEST_TTL_SECS) {
            return Err(format!(
                "approval lifetime exceeds {MAX_APPROVAL_REQUEST_TTL_SECS} seconds"
            ));
        }
        Ok(())
    }
}

impl LinkAccountInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_opaque_id(&self.principal_id, "principal_id")?;
        validate_opaque_id(&self.proof_binding_id, "proof_binding_id")?;
        validate_opaque_id(&self.chain_namespace, "chain_namespace")?;
        validate_opaque_id(&self.address, "address")?;
        validate_opaque_id(&self.proof_type, "proof_type")?;
        if is_managed_proof_type(&self.proof_type) {
            return Err(
                "managed accounts must be created through create_managed_account".to_string(),
            );
        }
        let Some(connector_id) = self.connector_id.as_deref() else {
            return Err("external wallet links require a connector_id".to_string());
        };
        validate_opaque_id(connector_id, "connector_id")?;
        if let Some(label) = &self.label {
            validate_label(label)?;
        }
        Ok(())
    }
}

impl CreateManagedAccountInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_opaque_id(&self.principal_id, "principal_id")?;
        validate_managed_chain_namespace(&self.chain_namespace)?;
        if let Some(label) = &self.label {
            validate_label(label)?;
        }
        Ok(())
    }
}

impl SetDefaultAccountInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_opaque_id(&self.principal_id, "principal_id")?;
        validate_opaque_id(&self.chain_namespace, "chain_namespace")?;
        validate_signing_intent(&self.intent)?;
        validate_opaque_id(&self.account_id, "account_id")?;
        Ok(())
    }
}

impl ImportManagedRecoveryKeyInput {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_opaque_id(&self.principal_id, "principal_id")?;
        if let Some(label) = &self.label {
            validate_label(label)?;
        }
        Ok(())
    }
}

pub(super) fn validate_default_account_lookup(
    principal_id: &str,
    chain_namespace: &str,
    intent: &str,
) -> Result<(), String> {
    validate_opaque_id(principal_id, "principal_id")?;
    validate_opaque_id(chain_namespace, "chain_namespace")?;
    validate_signing_intent(intent)?;
    Ok(())
}

pub(super) fn validate_opaque_id(value: &str, label: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 {
        return Err(format!("{label} must be 1-256 characters"));
    }
    if value == "." || value == ".." {
        return Err(format!("{label} must be opaque"));
    }
    if value.chars().any(|ch| {
        ch.is_control() || ch == '/' || ch == '\\' || ch == '"' || ch == '\'' || ch.is_whitespace()
    }) {
        return Err(format!("{label} contains unsupported characters"));
    }
    Ok(())
}

pub(super) fn validate_evm_chain_namespace(value: &str) -> Result<(), String> {
    validate_opaque_id(value, "chain_namespace")?;
    let chain_id = value
        .strip_prefix("eip155:")
        .ok_or_else(|| "managed EVM wallets require an eip155 chain namespace".to_string())?;
    let chain_id = chain_id
        .parse::<u64>()
        .map_err(|_| "managed wallet chain ID must be numeric".to_string())?;
    if chain_id == 0 {
        return Err("managed wallet chain ID must be non-zero".to_string());
    }
    Ok(())
}

pub(super) fn validate_managed_chain_namespace(value: &str) -> Result<(), String> {
    managed_proof_type(value).map(|_| ())
}

pub(super) fn is_managed_proof_type(value: &str) -> bool {
    matches!(
        value,
        MANAGED_EVM_PROOF_TYPE | MANAGED_BTC_P2WPKH_PROOF_TYPE
    )
}

pub(super) fn managed_proof_type(chain_namespace: &str) -> Result<&'static str, String> {
    validate_opaque_id(chain_namespace, "chain_namespace")?;
    if chain_namespace.starts_with("eip155:") {
        validate_evm_chain_namespace(chain_namespace)?;
        return Ok(MANAGED_EVM_PROOF_TYPE);
    }
    if chain_namespace == BITCOIN_MAINNET_CHAIN_NAMESPACE {
        return Ok(MANAGED_BTC_P2WPKH_PROOF_TYPE);
    }
    Err("managed wallets support EVM chains and Bitcoin mainnet P2WPKH".to_string())
}

pub(super) fn managed_key_scope(chain_namespace: &str) -> Result<&'static str, String> {
    match managed_proof_type(chain_namespace)? {
        MANAGED_EVM_PROOF_TYPE => Ok("eip155"),
        MANAGED_BTC_P2WPKH_PROOF_TYPE => Ok("bitcoin:p2wpkh"),
        _ => Err("unsupported managed wallet proof type".to_string()),
    }
}

pub(super) fn chain_namespaces_compatible(
    account_chain_namespace: &str,
    requested_chain_namespace: &str,
) -> bool {
    account_chain_namespace == requested_chain_namespace
        || (account_chain_namespace.starts_with("eip155:")
            && requested_chain_namespace.starts_with("eip155:"))
}

pub(super) fn validate_label(value: &str) -> Result<(), String> {
    if value.len() > 80 || value.chars().any(char::is_control) {
        return Err(
            "label must be 80 characters or fewer and contain no control characters".into(),
        );
    }
    Ok(())
}

pub(super) fn validate_reason(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 160 || value.chars().any(char::is_control) {
        return Err("reason must be 1-160 characters and contain no control characters".into());
    }
    Ok(())
}

pub(super) fn validate_signing_intent(value: &str) -> Result<(), String> {
    match value {
        "auth_challenge"
        | "capability_grant"
        | "credential"
        | "publish_envelope"
        | "transaction_intent"
        | "browser_connect"
        | "browser_personal_sign"
        | "browser_typed_data_sign"
        | "bitcoin_bip322_proof"
        | "revocation" => Ok(()),
        _ => Err("unsupported signing intent".to_string()),
    }
}

pub(super) fn validate_domain(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 253
        || value.contains('/')
        || value.contains('@')
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err("invalid SIWE domain".to_string());
    }
    Ok(())
}

pub(super) fn validate_uri(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() > 512
        || !(value.starts_with("https://") || value.starts_with("http://"))
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err("invalid SIWE URI".to_string());
    }
    Ok(())
}

pub(super) fn validate_resource(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() > 512
        || !value.starts_with("elastos://")
        || value
            .chars()
            .any(|ch| ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err("invalid wallet proof resource".to_string());
    }
    Ok(())
}

pub(super) fn validate_hash(value: &str, label: &str) -> Result<(), String> {
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| format!("{label} must start with 0x"))?;
    if raw.len() != 64 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{label} must be a 32-byte hex digest"));
    }
    Ok(())
}

pub(super) fn validate_signature(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() < 8 || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err("signature must be 8-4096 characters and contain no control characters".into());
    }
    Ok(())
}

pub(super) fn value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    bytes_hash(&bytes)
}

pub(super) fn wallet_authority_binding(authority: &WalletAuthorityV2) -> String {
    value_hash(&json!({
        "principal_id": authority.principal_id,
        "session_id": authority.session_id,
        "proof_binding_id": authority.proof_binding_id,
        "grant_id": authority.grant_id,
        "actor": authority.actor,
        "launch_id": authority.launch_id,
    }))
}

pub(super) fn bytes_hash(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(sha2::Sha256::digest(bytes)))
}

pub(super) fn managed_key_aad(
    account_id: &str,
    principal_id: &str,
    chain_namespace: &str,
    address: &str,
) -> String {
    format!(
        "elastos.wallet.managed_secret/v1\n{account_id}\n{principal_id}\n{chain_namespace}\n{}",
        normalize_evm_address(address)
    )
}

pub(super) fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn random_hex(bytes_len: usize) -> String {
    let mut bytes = vec![0u8; bytes_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
