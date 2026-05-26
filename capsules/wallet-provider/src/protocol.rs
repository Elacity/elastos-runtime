use super::*;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    Accounts {
        principal_id: String,
        #[serde(default)]
        include_revoked: bool,
    },
    CreateManagedAccount {
        principal_id: String,
        chain_namespace: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        create_new: bool,
    },
    LinkAccount {
        principal_id: String,
        proof_binding_id: String,
        chain_namespace: String,
        address: String,
        proof_type: String,
        #[serde(default)]
        connector_id: Option<String>,
        #[serde(default)]
        label: Option<String>,
    },
    RevokeAccount {
        principal_id: String,
        account_id: String,
    },
    RenameAccount {
        principal_id: String,
        account_id: String,
        label: String,
    },
    ExportManagedSecret {
        principal_id: String,
        account_id: String,
    },
    ImportManagedSecret {
        principal_id: String,
        recovery_key: Value,
        #[serde(default)]
        label: Option<String>,
    },
    SetDefaultAccount {
        principal_id: String,
        chain_namespace: String,
        intent: String,
        account_id: String,
    },
    DefaultAccount {
        principal_id: String,
        chain_namespace: String,
        intent: String,
    },
    Challenge {
        domain: String,
        uri: String,
        address: String,
        chain_id: u64,
        #[serde(default)]
        resources: Vec<String>,
    },
    BitcoinChallenge {
        domain: String,
        uri: String,
        address: String,
        #[serde(default = "default_bitcoin_network")]
        network: String,
        #[serde(default)]
        resources: Vec<String>,
    },
    VerifyProof {
        message: String,
        signature: String,
    },
    VerifyBip322Proof {
        message: String,
        signature: String,
        #[serde(default)]
        signature_type: Option<String>,
        #[serde(default)]
        public_key: Option<String>,
    },
    VerifyContractProof {
        message: String,
        signature: String,
        erc1271_proof: Value,
    },
    #[serde(rename = "request_signature")]
    Signature {
        principal_id: String,
        #[serde(default)]
        account_id: Option<String>,
        #[serde(default)]
        chain_namespace: Option<String>,
        intent: String,
        capsule_id: String,
        resource: String,
        reason: String,
        payload: Value,
        #[serde(default)]
        expires_at: Option<u64>,
    },
    ApprovalRequests {
        principal_id: String,
        #[serde(default)]
        include_resolved: bool,
    },
    RejectApproval {
        principal_id: String,
        request_id: String,
        #[serde(default)]
        reason: Option<String>,
    },
    ApproveApproval {
        principal_id: String,
        request_id: String,
        #[serde(default)]
        reason: Option<String>,
    },
    CompleteApproval {
        principal_id: String,
        request_id: String,
        connector_id: String,
        payload_hash: String,
        #[serde(default)]
        signature: Option<String>,
        #[serde(default)]
        signature_type: Option<String>,
        #[serde(default)]
        public_key: Option<String>,
        signer: String,
        #[serde(default)]
        transaction_hash: Option<String>,
    },
    RecordTransactionHash {
        principal_id: String,
        request_id: String,
        transaction_hash: String,
    },
    SignApproved {
        principal_id: String,
        request_id: String,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    pub(super) fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    pub(super) fn empty_ok() -> Self {
        Response::Ok { data: None }
    }

    pub(super) fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}
