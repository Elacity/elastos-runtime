use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct LinkedAccount {
    pub(super) account_id: String,
    pub(super) principal_id: String,
    pub(super) proof_binding_id: String,
    pub(super) chain_namespace: String,
    pub(super) address: String,
    pub(super) proof_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) connector_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) label: Option<String>,
    pub(super) linked_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) revoked_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct WalletStore {
    #[serde(default)]
    pub(super) accounts: Vec<LinkedAccount>,
    #[serde(default)]
    pub(super) managed_wallets: Vec<ManagedWalletSecret>,
    #[serde(default)]
    pub(super) challenges: Vec<StoredWalletChallenge>,
    #[serde(default)]
    pub(super) bitcoin_challenges: Vec<StoredBitcoinChallenge>,
    #[serde(default)]
    pub(super) approval_requests: Vec<WalletApprovalRequest>,
    #[serde(default)]
    pub(super) default_accounts: Vec<DefaultWalletAccount>,
    #[serde(default)]
    pub(super) consumed_lifecycles: Vec<ConsumedWalletLifecycle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct ConsumedWalletLifecycle {
    pub(super) lifecycle_id: String,
    pub(super) request_sha256: String,
    #[serde(default)]
    pub(super) request_expires_at: u64,
    pub(super) consumed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct ManagedWalletSecret {
    pub(super) schema: String,
    pub(super) account_id: String,
    pub(super) principal_id: String,
    pub(super) chain_namespace: String,
    pub(super) address: String,
    pub(super) key_algorithm: String,
    pub(super) cipher: String,
    pub(super) nonce: String,
    pub(super) ciphertext: String,
    pub(super) created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) revoked_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct DefaultWalletAccount {
    pub(super) schema: String,
    pub(super) principal_id: String,
    pub(super) chain_namespace: String,
    pub(super) intent: String,
    pub(super) account_id: String,
    pub(super) set_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct StoredWalletChallenge {
    pub(super) challenge: AuthChallengeV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) consumed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredBitcoinChallenge {
    pub(super) challenge: BitcoinChallengeV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) consumed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct BitcoinChallengeV1 {
    pub(super) schema: String,
    pub(super) challenge_id: String,
    pub(super) domain: String,
    pub(super) uri: String,
    pub(super) network: String,
    pub(super) address: String,
    pub(super) nonce: String,
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
    pub(super) resources: Vec<String>,
}

impl BitcoinChallengeV1 {
    pub(super) fn message(&self) -> String {
        let resources = self
            .resources
            .iter()
            .map(|resource| format!("- {resource}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{domain} wants you to prove Bitcoin account ownership:\n{address}\n\nURI: {uri}\nVersion: 1\nNetwork: {network}\nNonce: {nonce}\nIssued At: {issued_at}\nExpiration Time: {expires_at}\nResources:\n{resources}",
            domain = self.domain,
            address = self.address,
            uri = self.uri,
            network = self.network,
            nonce = self.nonce,
            issued_at = self.issued_at,
            expires_at = self.expires_at,
            resources = resources,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ApprovalStatus {
    Pending,
    Approved,
    Completed,
    Rejected,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct WalletApprovalRequest {
    pub(super) schema: String,
    pub(super) request_id: String,
    pub(super) kind: String,
    pub(super) status: ApprovalStatus,
    pub(super) principal_id: String,
    pub(super) account_id: String,
    pub(super) proof_binding_id: String,
    pub(super) chain_namespace: String,
    pub(super) address: String,
    #[serde(default)]
    pub(super) proof_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) connector_id: Option<String>,
    pub(super) intent: String,
    #[serde(default)]
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) launch_id: String,
    #[serde(alias = "capsule_id")]
    pub(super) requested_by_actor: String,
    pub(super) resource: String,
    pub(super) reason: String,
    pub(super) payload: Value,
    pub(super) payload_hash: String,
    pub(super) created_at: u64,
    pub(super) expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) resolved_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) rejection_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) approved_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) approval_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) completed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) signature_receipt: Option<WalletSignatureReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) signed_result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct WalletSignatureReceipt {
    pub(super) schema: String,
    pub(super) request_id: String,
    pub(super) signer: String,
    pub(super) payload_hash: String,
    pub(super) signature_hash: String,
    pub(super) completed_at: u64,
}
