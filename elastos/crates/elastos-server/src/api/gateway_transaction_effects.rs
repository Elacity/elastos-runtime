use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use elastos_runtime::auth::RuntimeAuditEventV1;
use elastos_wallet_contract::{
    PublicNetwork, ValidatedChainOutcomeBindingV1, ValidatedChainOutcomeV1,
    WalletProviderOperationV2, WalletProviderRequestV2, VALIDATED_CHAIN_OUTCOME_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use tokio::sync::Mutex;

use super::*;

const TRANSACTION_EFFECT_STORE_SCHEMA: &str = "elastos.runtime.transaction-effect-store/v1";
const TRANSACTION_EFFECT_SCHEMA: &str = "elastos.runtime.transaction-effect/v1";
const TRANSACTION_EFFECT_STORE_RELATIVE_PATH: &str =
    ".AppData/ElastOS/Runtime/transaction-effects.json";
const MAX_TRANSACTION_EFFECTS: usize = 128;
const MAX_TRANSACTION_EFFECT_STORE_BYTES: usize = 8 * 1024 * 1024;
const MAX_TRANSACTION_INTENT_BYTES: usize = 32 * 1024;
const MAX_SIGNED_TRANSACTION_BYTES: usize = 256 * 1024;
const MAX_TRANSACTION_RECEIPT_BYTES: usize = 128 * 1024;

pub(in crate::api::gateway) const NATIVE_TRANSACTION_SOURCE: &str = "native_wallet";
pub(in crate::api::gateway) const BROWSER_TRANSACTION_SOURCE: &str = "browser_wallet";

type TransactionEffectLockKey = (PathBuf, String);
type TransactionEffectLockMap = HashMap<TransactionEffectLockKey, Weak<Mutex<()>>>;

static TRANSACTION_EFFECT_LOCKS: OnceLock<Mutex<TransactionEffectLockMap>> = OnceLock::new();

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct RuntimeTransactionRequest {
    pub(in crate::api::gateway) source: &'static str,
    pub(in crate::api::gateway) effect_id: String,
    pub(in crate::api::gateway) request_sha256: String,
    pub(in crate::api::gateway) account_id: String,
    pub(in crate::api::gateway) address: String,
    pub(in crate::api::gateway) chain_namespace: String,
    pub(in crate::api::gateway) network: String,
    pub(in crate::api::gateway) to: String,
    pub(in crate::api::gateway) value: String,
    pub(in crate::api::gateway) data: String,
    pub(in crate::api::gateway) approval_reason: String,
    pub(in crate::api::gateway) metadata: Value,
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct RuntimeTransactionApproval {
    pub(in crate::api::gateway) effect_id: String,
    pub(in crate::api::gateway) approval_request: Value,
}

pub(in crate::api::gateway) struct RuntimeManagedTransactionApproval<'a> {
    pub(in crate::api::gateway) context: &'a HomeLaunchTokenContext,
    pub(in crate::api::gateway) reason: &'a str,
    pub(in crate::api::gateway) capsule_id: &'static str,
}

pub(in crate::api::gateway) enum RuntimeTransactionLookup<'a> {
    EffectId(&'a str),
    ApprovalId(&'a str),
}

#[derive(Debug, Clone)]
pub(in crate::api::gateway) struct RuntimeTransactionCompletion {
    pub(in crate::api::gateway) effect_id: String,
    pub(in crate::api::gateway) approval_request_id: String,
    pub(in crate::api::gateway) transaction_hash: String,
    pub(in crate::api::gateway) approval_request: Value,
    pub(in crate::api::gateway) validated_chain_outcome: Option<ValidatedChainOutcomeV1>,
    pub(in crate::api::gateway) signed_result: Option<Value>,
    pub(in crate::api::gateway) receipt: Option<Value>,
    pub(in crate::api::gateway) completion_pending: bool,
    pub(in crate::api::gateway) completion_error: Option<String>,
    pub(in crate::api::gateway) already_confirmed: bool,
    pub(in crate::api::gateway) externally_completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TransactionAuthorityBinding {
    principal_id: String,
    session_id: String,
    proof_binding_id: Option<String>,
    grant_id: String,
    actor: String,
    launch_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TransactionEffectState {
    Prepared,
    ApprovalPending,
    Signed,
    BroadcastInFlight,
    ReceiptConfirmed,
    CompletionPending,
    Complete,
    Indeterminate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RuntimeTransactionWalletBinding {
    ManagedSigned {
        signed_transaction_sha256: String,
    },
    ExternalConnector {
        connector_id: String,
        originating_address: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeTransactionEffect {
    schema: String,
    effect_id: String,
    source: String,
    authority: TransactionAuthorityBinding,
    request_sha256: String,
    request_binding: Value,
    approval_request_id: String,
    wallet_request_sha256: String,
    approval_expires_at: u64,
    approval_reason: String,
    account_id: String,
    address: String,
    chain_namespace: String,
    network: String,
    intent: Value,
    state: TransactionEffectState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signed_transaction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wallet_binding: Option<RuntimeTransactionWalletBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wallet_transaction_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signed_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<Value>,
    #[serde(default)]
    requested_audit_completed: bool,
    #[serde(default)]
    projection_completed: bool,
    #[serde(default)]
    completion_audit_completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion_error: Option<String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeTransactionEffectStore {
    schema: String,
    principal_id: String,
    effects: Vec<RuntimeTransactionEffect>,
}

impl RuntimeTransactionEffectStore {
    fn empty(principal_id: &str) -> Self {
        Self {
            schema: TRANSACTION_EFFECT_STORE_SCHEMA.to_string(),
            principal_id: principal_id.to_string(),
            effects: Vec::new(),
        }
    }

    fn validate(&self, principal_id: &str) -> anyhow::Result<()> {
        if self.schema != TRANSACTION_EFFECT_STORE_SCHEMA || self.principal_id != principal_id {
            anyhow::bail!("transaction effect store binding mismatch");
        }
        if self.effects.len() > MAX_TRANSACTION_EFFECTS {
            anyhow::bail!("transaction effect store exceeds its bounded capacity");
        }
        let mut effect_ids = std::collections::HashSet::new();
        let mut approval_ids = std::collections::HashSet::new();
        for effect in &self.effects {
            effect.validate(principal_id)?;
            if !effect_ids.insert(effect.effect_id.as_str())
                || !approval_ids.insert(effect.approval_request_id.as_str())
            {
                anyhow::bail!("transaction effect store contains duplicate identities");
            }
        }
        Ok(())
    }

    fn prepare_capacity(&mut self) -> anyhow::Result<()> {
        if self.effects.len() < MAX_TRANSACTION_EFFECTS {
            return Ok(());
        }
        let Some((index, _)) = self
            .effects
            .iter()
            .enumerate()
            .filter(|(_, effect)| effect.state == TransactionEffectState::Complete)
            .min_by_key(|(_, effect)| effect.updated_at)
        else {
            anyhow::bail!("transaction effect capacity is full");
        };
        self.effects.remove(index);
        Ok(())
    }
}

impl RuntimeTransactionEffect {
    fn validate(&self, principal_id: &str) -> anyhow::Result<()> {
        if self.schema != TRANSACTION_EFFECT_SCHEMA
            || self.authority.principal_id != principal_id
            || !valid_effect_id(&self.effect_id)
            || !valid_sha256(&self.request_sha256)
            || !valid_bounded_text(&self.approval_request_id, 256)
            || !valid_tagged_sha256(&self.wallet_request_sha256, "request")
            || self.approval_expires_at == 0
            || !valid_bounded_text(&self.approval_reason, 2048)
            || !valid_bounded_text(&self.account_id, 256)
            || !valid_bounded_text(&self.address, 256)
            || !valid_bounded_text(&self.chain_namespace, 64)
            || !valid_bounded_text(&self.network, 128)
            || self.created_at == 0
            || self.updated_at < self.created_at
        {
            anyhow::bail!("transaction effect failed structural validation");
        }
        validate_authority_binding(&self.authority)?;
        validate_bounded_object(
            "transaction request binding",
            &self.request_binding,
            MAX_TRANSACTION_INTENT_BYTES,
        )?;
        validate_bounded_object(
            "transaction intent",
            &self.intent,
            MAX_TRANSACTION_INTENT_BYTES,
        )?;
        if wallet_chain_namespace_network(&self.chain_namespace) != Some(self.network.as_str()) {
            anyhow::bail!("transaction effect Chain namespace and network mismatch");
        }
        if let Some(approval) = self.approval_snapshot.as_ref() {
            validate_bounded_object(
                "transaction approval snapshot",
                approval,
                MAX_TRANSACTION_RECEIPT_BYTES,
            )?;
        }
        if let Some(signed_transaction) = self.signed_transaction.as_deref() {
            canonical_signed_transaction(signed_transaction)?;
        }
        if let Some(binding) = self.wallet_binding.as_ref() {
            binding.validate()?;
        }
        if let Some(hash) = self.wallet_transaction_hash.as_deref() {
            validate_transaction_hash(hash)?;
        }
        if let Some(signed_transaction) = self.signed_transaction.as_deref() {
            let exact = validate_signed_evm_transaction_authority(self, signed_transaction)?;
            match self.wallet_binding.as_ref() {
                Some(RuntimeTransactionWalletBinding::ManagedSigned {
                    signed_transaction_sha256,
                }) if signed_transaction_sha256 == &exact.sha256 => {}
                Some(RuntimeTransactionWalletBinding::ManagedSigned { .. }) => {
                    anyhow::bail!("signed transaction digest differs from durable Wallet binding");
                }
                _ => {}
            }
            if self
                .wallet_transaction_hash
                .as_deref()
                .is_some_and(|hash| !hash.eq_ignore_ascii_case(&exact.transaction_hash))
            {
                anyhow::bail!("signed transaction hash differs from durable Wallet binding");
            }
        }
        if let Some(signed_result) = self.signed_result.as_ref() {
            validate_bounded_object(
                "transaction signed result",
                signed_result,
                MAX_TRANSACTION_RECEIPT_BYTES,
            )?;
        }
        if let Some(receipt) = self.receipt.as_ref() {
            validate_bounded_object(
                "transaction receipt",
                receipt,
                MAX_TRANSACTION_RECEIPT_BYTES,
            )?;
            validate_chain_result(
                receipt,
                &self.network,
                self.required_wallet_hash()?,
                false,
                self.external_originating_address(),
            )?;
        }
        match self.state {
            TransactionEffectState::Prepared | TransactionEffectState::ApprovalPending => {
                if self.signed_transaction.is_some()
                    || self.wallet_binding.is_some()
                    || self.wallet_transaction_hash.is_some()
                {
                    anyhow::bail!("unsigned transaction effect contains signed authority");
                }
            }
            TransactionEffectState::Signed => {
                if self.signed_transaction.is_none()
                    || !matches!(
                        self.wallet_binding.as_ref(),
                        Some(RuntimeTransactionWalletBinding::ManagedSigned { .. })
                    )
                    || self.wallet_transaction_hash.is_none()
                {
                    anyhow::bail!(
                        "signed transaction effect is missing its managed Wallet binding"
                    );
                }
            }
            TransactionEffectState::BroadcastInFlight
            | TransactionEffectState::ReceiptConfirmed
            | TransactionEffectState::CompletionPending
            | TransactionEffectState::Complete
            | TransactionEffectState::Indeterminate => {
                if self.signed_transaction.is_some()
                    || self.wallet_binding.is_none()
                    || self.wallet_transaction_hash.is_none()
                {
                    anyhow::bail!(
                        "post-dispatch transaction effect retained raw or missing Wallet authority"
                    );
                }
            }
        }
        if matches!(
            self.state,
            TransactionEffectState::ReceiptConfirmed
                | TransactionEffectState::CompletionPending
                | TransactionEffectState::Complete
        ) && self.receipt.is_none()
        {
            anyhow::bail!("confirmed transaction effect is missing its Chain result");
        }
        if self.state == TransactionEffectState::Complete
            && (!self.requested_audit_completed
                || !self.projection_completed
                || !self.completion_audit_completed)
        {
            anyhow::bail!("complete transaction effect has unfinished completion work");
        }
        Ok(())
    }

    fn required_wallet_hash(&self) -> anyhow::Result<&str> {
        self.wallet_transaction_hash
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("transaction effect is missing Wallet hash"))
    }

    fn external_originating_address(&self) -> Option<&str> {
        match self.wallet_binding.as_ref() {
            Some(RuntimeTransactionWalletBinding::ExternalConnector {
                originating_address,
                ..
            }) => Some(originating_address),
            _ => None,
        }
    }

    fn externally_completed(&self) -> bool {
        matches!(
            self.wallet_binding.as_ref(),
            Some(RuntimeTransactionWalletBinding::ExternalConnector { .. })
        )
    }
}

impl RuntimeTransactionWalletBinding {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::ManagedSigned {
                signed_transaction_sha256,
            } => {
                if !valid_prefixed_sha256(signed_transaction_sha256) {
                    anyhow::bail!("transaction effect signed transaction digest is invalid");
                }
            }
            Self::ExternalConnector {
                connector_id,
                originating_address,
            } => {
                if !valid_bounded_text(connector_id, 128) {
                    anyhow::bail!("transaction effect connector binding is invalid");
                }
                validate_wallet_evm_address(originating_address, "originating")
                    .map_err(|(_, message)| anyhow::anyhow!(message))?;
            }
        }
        Ok(())
    }
}

pub(in crate::api::gateway) fn runtime_transaction_request_sha256(
    value: &Value,
) -> anyhow::Result<String> {
    let canonical = canonical_json(value.clone());
    let bytes = serde_json::to_vec(&canonical)?;
    if bytes.len() > MAX_TRANSACTION_INTENT_BYTES {
        anyhow::bail!("transaction request exceeds its bounded size");
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(in crate::api::gateway) fn runtime_transaction_effect_id(
    source: &str,
    authority: &RuntimeWalletAuthority,
    stable_binding: &Value,
) -> anyhow::Result<String> {
    if !matches!(
        source,
        NATIVE_TRANSACTION_SOURCE | BROWSER_TRANSACTION_SOURCE
    ) {
        anyhow::bail!("unsupported transaction effect source");
    }
    let authority = transaction_authority(authority);
    let digest = runtime_transaction_request_sha256(&json!({
        "domain": "elastos.runtime.transaction-effect/v1",
        "source": source,
        "authority": authority,
        "stable_binding": stable_binding,
    }))?;
    Ok(format!("transaction-effect:sha256:{digest}"))
}

pub(in crate::api::gateway) fn exact_runtime_transaction_effect_id(
    source: &str,
    principal_id: &str,
    request_sha256: &str,
    request_binding: &Value,
) -> anyhow::Result<String> {
    if !matches!(
        source,
        NATIVE_TRANSACTION_SOURCE | BROWSER_TRANSACTION_SOURCE
    ) {
        anyhow::bail!("unsupported transaction effect source");
    }
    if !valid_bounded_text(principal_id, 256) || !valid_sha256(request_sha256) {
        anyhow::bail!("invalid exact transaction effect binding");
    }
    let digest = runtime_transaction_request_sha256(&json!({
        "domain": "elastos.runtime.exact-transaction-effect/v1",
        "source": source,
        "principal_id": principal_id,
        "request_sha256": request_sha256,
        "request_binding": request_binding,
    }))?;
    Ok(format!("transaction-effect:sha256:{digest}"))
}

fn bind_transaction_intent_runtime_context(
    intent: &mut Value,
    request: &RuntimeTransactionRequest,
    authority: &RuntimeWalletAuthority,
    include_metadata: bool,
) -> anyhow::Result<()> {
    let Some(intent_object) = intent.as_object_mut() else {
        anyhow::bail!("transaction intent must be a JSON object");
    };
    if include_metadata {
        if let Some(metadata) = request.metadata.as_object() {
            for (key, value) in metadata {
                intent_object.insert(key.clone(), value.clone());
            }
        }
    }
    intent_object.insert(
        "principal_id".to_string(),
        json!(authority.verified_context().principal_id()),
    );
    intent_object.insert(
        "session_id".to_string(),
        json!(authority.verified_context().session_id()),
    );
    intent_object.insert("effect_id".to_string(), json!(request.effect_id.as_str()));
    intent_object.insert("source".to_string(), json!(request.source));
    intent_object.insert(
        "request_sha256".to_string(),
        json!(request.request_sha256.as_str()),
    );
    intent_object.insert("account_id".to_string(), json!(request.account_id.as_str()));
    intent_object.insert(
        "chain_namespace".to_string(),
        json!(request.chain_namespace.as_str()),
    );
    intent_object.insert(
        "proof_binding_id".to_string(),
        json!(authority.verified_context().proof_binding_id()),
    );
    intent_object.insert(
        "grant_id".to_string(),
        json!(authority.verified_context().grant_id()),
    );
    intent_object.insert(
        "launch_id".to_string(),
        json!(authority.verified_context().launch_id()),
    );
    intent_object.insert(
        "requested_by_actor".to_string(),
        json!(authority.verified_context().actor()),
    );
    intent_object
        .entry("method".to_string())
        .or_insert_with(|| {
            json!(if request.source == NATIVE_TRANSACTION_SOURCE {
                "wallet_send"
            } else {
                "eth_sendTransaction"
            })
        });
    Ok(())
}

fn rebind_prepared_exact_effect(
    effect: &mut RuntimeTransactionEffect,
    authority: &RuntimeWalletAuthority,
    request: &RuntimeTransactionRequest,
) -> anyhow::Result<()> {
    effect.authority = transaction_authority(authority);
    bind_transaction_intent_runtime_context(&mut effect.intent, request, authority, false)?;
    effect.wallet_request_sha256 = wallet_operation_request_sha256(
        authority,
        &effect.approval_request_id,
        transaction_approval_operation(effect),
    )?;
    effect.updated_at = now_ts();
    effect.validate(authority.verified_context().principal_id())
}

async fn exact_wallet_approval_by_id(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    approval_request_id: &str,
) -> Result<Option<Value>, (StatusCode, String)> {
    let data = runtime_wallet_data(
        state,
        authority,
        WalletProviderOperationV2::ListApprovals {
            include_resolved: true,
        },
    )
    .await
    .map_err(wallet_unavailable)?;
    let approvals = data
        .get("approval_requests")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Wallet approval store returned an invalid approvals list".to_string(),
            )
        })?;
    let mut matched = None;
    for approval in approvals {
        if !approval.is_object() {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "Wallet approval store returned a malformed approval entry".to_string(),
            ));
        }
        if approval.get("request_id").and_then(Value::as_str) == Some(approval_request_id) {
            if matched.is_some() {
                return Err((
                    StatusCode::CONFLICT,
                    "Wallet approval store returned duplicate exact approval identities"
                        .to_string(),
                ));
            }
            matched = Some(approval.clone());
        }
    }
    Ok(matched)
}

pub(in crate::api::gateway) async fn ensure_exact_runtime_transaction_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request: RuntimeTransactionRequest,
) -> Result<RuntimeTransactionApproval, (StatusCode, String)> {
    validate_transaction_request(authority, &request)?;
    let principal_id = authority.verified_context().principal_id();
    let lock = transaction_effect_lock(&state.data_dir, principal_id).await;
    let _guard = lock.lock().await;
    let mut store = load_transaction_effect_store(state, principal_id)?;
    let request_binding = transaction_request_binding(&request);
    let authority_binding = transaction_authority(authority);
    let expected_effect_id = exact_runtime_transaction_effect_id(
        request.source,
        principal_id,
        &request.request_sha256,
        &request_binding,
    )
    .map_err(internal_error)?;
    if request.effect_id != expected_effect_id {
        return Err((
            StatusCode::CONFLICT,
            "exact Runtime transaction effect identity does not match the verified principal and request binding".to_string(),
        ));
    }
    let expected_approval_request_id = wallet_request_id(&request.effect_id, "approval");
    let now = now_ts();

    let existing_effect = if let Some(index) = store
        .effects
        .iter()
        .position(|effect| effect.effect_id == request.effect_id)
    {
        let effect = &store.effects[index];
        if effect.approval_request_id != expected_approval_request_id
            || effect.source != request.source
            || effect.authority.principal_id != authority_binding.principal_id
            || effect.request_sha256 != request.request_sha256
            || effect.request_binding != request_binding
            || effect.account_id != request.account_id
            || !effect.address.eq_ignore_ascii_case(&request.address)
            || effect.chain_namespace != request.chain_namespace
            || effect.network != request.network
        {
            return Err((
                StatusCode::CONFLICT,
                "exact Runtime transaction effect identity was reused with substituted bindings"
                    .to_string(),
            ));
        }
        true
    } else {
        let mut intent = prepare_chain_transaction(state, &request).await?;
        bind_transaction_intent_runtime_context(&mut intent, &request, authority, true)
            .map_err(internal_error)?;
        let mut effect = RuntimeTransactionEffect {
            schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
            effect_id: request.effect_id.clone(),
            source: request.source.to_string(),
            authority: authority_binding,
            request_sha256: request.request_sha256.clone(),
            request_binding,
            approval_request_id: expected_approval_request_id.clone(),
            wallet_request_sha256: String::new(),
            approval_expires_at: now.saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
            approval_reason: request.approval_reason.clone(),
            account_id: request.account_id.clone(),
            address: request.address.clone(),
            chain_namespace: request.chain_namespace.clone(),
            network: request.network.clone(),
            intent,
            state: TransactionEffectState::Prepared,
            approval_snapshot: None,
            signed_transaction: None,
            wallet_binding: None,
            wallet_transaction_hash: None,
            signed_result: None,
            receipt: None,
            requested_audit_completed: false,
            projection_completed: false,
            completion_audit_completed: false,
            completion_error: None,
            created_at: now,
            updated_at: now,
        };
        effect.wallet_request_sha256 = wallet_operation_request_sha256(
            authority,
            &effect.approval_request_id,
            transaction_approval_operation(&effect),
        )
        .map_err(internal_error)?;
        effect.validate(principal_id).map_err(internal_error)?;
        store.prepare_capacity().map_err(conflict_error)?;
        if store
            .effects
            .iter()
            .any(|effect| effect.approval_request_id == expected_approval_request_id)
        {
            return Err((
                StatusCode::CONFLICT,
                "exact Runtime transaction approval identity was reused with substituted bindings"
                    .to_string(),
            ));
        }
        store.effects.push(effect);
        save_transaction_effect_store(state, &store)?;
        false
    };
    let effect_index = store
        .effects
        .iter()
        .position(|effect| effect.effect_id == request.effect_id)
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "exact Runtime transaction effect disappeared during approval recovery".to_string(),
            )
        })?;

    let approval = if existing_effect {
        ensure_requested_transaction_audit(state, &mut store, effect_index)?;
        match exact_wallet_approval_by_id(
            state,
            authority,
            &store.effects[effect_index].approval_request_id,
        )
        .await?
        {
            Some(approval) => {
                let effect = &store.effects[effect_index];
                validate_approval_snapshot(effect, authority, &approval)?;
                let effect = &mut store.effects[effect_index];
                effect.approval_snapshot = Some(approval.clone());
                if effect.state == TransactionEffectState::Prepared {
                    effect.state = TransactionEffectState::ApprovalPending;
                }
                effect.updated_at = now_ts();
                save_transaction_effect_store(state, &store)?;
                approval
            }
            None => {
                let effect = &mut store.effects[effect_index];
                if effect.approval_snapshot.is_some() {
                    return Err((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "exact Runtime transaction approval is missing from the Wallet approval store"
                            .to_string(),
                    ));
                }
                if effect.state != TransactionEffectState::Prepared {
                    return Err((
                        StatusCode::CONFLICT,
                        "exact Runtime transaction approval cannot be recreated after durable progression beyond the prepared state"
                            .to_string(),
                    ));
                }
                if effect.approval_expires_at <= now_ts() {
                    return Err((
                        StatusCode::CONFLICT,
                        "exact Runtime transaction approval expired before durable recovery"
                            .to_string(),
                    ));
                }
                rebind_prepared_exact_effect(effect, authority, &request)
                    .map_err(internal_error)?;
                save_transaction_effect_store(state, &store)?;
                ensure_effect_wallet_approval(state, authority, &mut store, effect_index).await?
            }
        }
    } else {
        ensure_effect_wallet_approval(state, authority, &mut store, effect_index).await?
    };
    Ok(RuntimeTransactionApproval {
        effect_id: request.effect_id,
        approval_request: approval,
    })
}

pub(in crate::api::gateway) async fn ensure_runtime_transaction_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request: RuntimeTransactionRequest,
) -> Result<RuntimeTransactionApproval, (StatusCode, String)> {
    validate_transaction_request(authority, &request)?;
    let principal_id = authority.verified_context().principal_id();
    let lock = transaction_effect_lock(&state.data_dir, principal_id).await;
    let _guard = lock.lock().await;
    let mut store = load_transaction_effect_store(state, principal_id)?;
    let request_binding = transaction_request_binding(&request);
    let authority_binding = transaction_authority(authority);
    let now = now_ts();

    let effect_index = if let Some(index) = store
        .effects
        .iter()
        .position(|effect| effect.effect_id == request.effect_id)
    {
        let effect = &store.effects[index];
        if effect.source != request.source
            || effect.authority != authority_binding
            || effect.request_sha256 != request.request_sha256
            || effect.request_binding != request_binding
            || effect.account_id != request.account_id
            || !effect.address.eq_ignore_ascii_case(&request.address)
            || effect.chain_namespace != request.chain_namespace
            || effect.network != request.network
        {
            return Err((
                StatusCode::CONFLICT,
                "transaction effect identity was reused with substituted bindings".to_string(),
            ));
        }
        index
    } else {
        let mut intent = prepare_chain_transaction(state, &request).await?;
        bind_transaction_intent_runtime_context(&mut intent, &request, authority, true)
            .map_err(internal_error)?;
        let approval_request_id = wallet_request_id(&request.effect_id, "approval");
        let mut effect = RuntimeTransactionEffect {
            schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
            effect_id: request.effect_id.clone(),
            source: request.source.to_string(),
            authority: authority_binding,
            request_sha256: request.request_sha256.clone(),
            request_binding,
            approval_request_id,
            wallet_request_sha256: String::new(),
            approval_expires_at: now.saturating_add(WALLET_APPROVAL_REQUEST_TTL_SECS),
            approval_reason: request.approval_reason.clone(),
            account_id: request.account_id.clone(),
            address: request.address.clone(),
            chain_namespace: request.chain_namespace.clone(),
            network: request.network.clone(),
            intent,
            state: TransactionEffectState::Prepared,
            approval_snapshot: None,
            signed_transaction: None,
            wallet_binding: None,
            wallet_transaction_hash: None,
            signed_result: None,
            receipt: None,
            requested_audit_completed: false,
            projection_completed: false,
            completion_audit_completed: false,
            completion_error: None,
            created_at: now,
            updated_at: now,
        };
        let approval_operation = transaction_approval_operation(&effect);
        effect.wallet_request_sha256 = wallet_operation_request_sha256(
            authority,
            &effect.approval_request_id,
            approval_operation,
        )
        .map_err(internal_error)?;
        effect.validate(principal_id).map_err(internal_error)?;
        store.prepare_capacity().map_err(conflict_error)?;
        store.effects.push(effect);
        save_transaction_effect_store(state, &store)?;
        store.effects.len() - 1
    };

    let approval =
        ensure_effect_wallet_approval(state, authority, &mut store, effect_index).await?;
    Ok(RuntimeTransactionApproval {
        effect_id: request.effect_id,
        approval_request: approval,
    })
}

pub(in crate::api::gateway) async fn resume_runtime_native_transaction_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    effect_id: &str,
    step_up_id: &str,
    request_sha256: &str,
) -> Result<RuntimeTransactionApproval, (StatusCode, String)> {
    if !valid_bounded_text(step_up_id, 256) || !valid_sha256(request_sha256) {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid recovered transaction step-up identity".to_string(),
        ));
    }
    let stable_binding = json!({
        "step_up_id": step_up_id,
        "request_sha256": request_sha256,
    });
    let expected_effect_id =
        runtime_transaction_effect_id(NATIVE_TRANSACTION_SOURCE, authority, &stable_binding)
            .map_err(internal_error)?;
    if expected_effect_id != effect_id {
        return Err((
            StatusCode::CONFLICT,
            "recovered transaction step-up identity does not match its effect".to_string(),
        ));
    }

    let principal_id = authority.verified_context().principal_id();
    let lock = transaction_effect_lock(&state.data_dir, principal_id).await;
    let _guard = lock.lock().await;
    let mut store = load_transaction_effect_store(state, principal_id)?;
    let effect_index = store
        .effects
        .iter()
        .position(|effect| effect.effect_id == effect_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "recovered transaction step-up has no durable Runtime effect".to_string(),
            )
        })?;
    let effect = &store.effects[effect_index];
    if effect.source != NATIVE_TRANSACTION_SOURCE
        || effect.authority != transaction_authority(authority)
        || effect.request_sha256 != request_sha256
    {
        return Err((
            StatusCode::CONFLICT,
            "recovered transaction step-up does not match the durable Runtime effect".to_string(),
        ));
    }
    ensure_requested_transaction_audit(state, &mut store, effect_index)?;
    let approval = match store.effects[effect_index].approval_snapshot.clone() {
        Some(approval) => approval,
        None => ensure_effect_wallet_approval(state, authority, &mut store, effect_index).await?,
    };
    Ok(RuntimeTransactionApproval {
        effect_id: effect_id.to_string(),
        approval_request: approval,
    })
}

async fn ensure_effect_wallet_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    store: &mut RuntimeTransactionEffectStore,
    effect_index: usize,
) -> Result<Value, (StatusCode, String)> {
    ensure_requested_transaction_audit(state, store, effect_index)?;
    let effect = store.effects[effect_index].clone();
    let approval_data = runtime_wallet_data_with_request_id(
        state,
        authority,
        effect.approval_request_id.clone(),
        transaction_approval_operation(&effect),
    )
    .await
    .map_err(wallet_unavailable)?;
    let approval = approval_data
        .get("approval_request")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider returned an invalid transaction approval".to_string(),
            )
        })?;
    validate_approval_snapshot(&effect, authority, &approval)?;
    let effect = &mut store.effects[effect_index];
    effect.approval_snapshot = Some(approval.clone());
    if effect.state == TransactionEffectState::Prepared {
        effect.state = TransactionEffectState::ApprovalPending;
    }
    effect.updated_at = now_ts();
    save_transaction_effect_store(state, store)?;
    Ok(approval)
}

fn ensure_requested_transaction_audit(
    state: &GatewayState,
    store: &mut RuntimeTransactionEffectStore,
    effect_index: usize,
) -> Result<(), (StatusCode, String)> {
    if store.effects[effect_index].requested_audit_completed {
        return Ok(());
    }
    append_transaction_effect_audit(state, &store.effects[effect_index], "requested", None)
        .map_err(|err| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("transaction requested audit unavailable: {err}"),
            )
        })?;
    let effect = &mut store.effects[effect_index];
    effect.requested_audit_completed = true;
    effect.updated_at = now_ts();
    save_transaction_effect_store(state, store)
}

pub(in crate::api::gateway) async fn complete_runtime_transaction_effect(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    lookup: RuntimeTransactionLookup<'_>,
    exact_approved_request: Option<&RuntimeTransactionRequest>,
    managed_approval: Option<RuntimeManagedTransactionApproval<'_>>,
) -> Result<RuntimeTransactionCompletion, (StatusCode, String)> {
    let approval_resume = match (&lookup, exact_approved_request) {
        (RuntimeTransactionLookup::ApprovalId(approval_request_id), Some(request)) => {
            validate_transaction_request(authority, request)?;
            Some((*approval_request_id, request))
        }
        (RuntimeTransactionLookup::EffectId(_), Some(_)) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "exact approved-request recovery requires approval-id lookup".to_string(),
            ));
        }
        (_, None) => None,
    };
    let principal_id = authority.verified_context().principal_id();
    let lock = transaction_effect_lock(&state.data_dir, principal_id).await;
    let _guard = lock.lock().await;
    let mut store = load_transaction_effect_store(state, principal_id)?;
    let effect_index = if let Some((approval_request_id, request)) = approval_resume {
        approved_request_effect_index(&store, approval_request_id, request)?
    } else {
        match find_effect_index(&store, &lookup) {
            Some(index) => index,
            None => match lookup {
                RuntimeTransactionLookup::ApprovalId(request_id) => {
                    create_browser_effect_from_approval(state, authority, &mut store, request_id)
                        .await?
                }
                RuntimeTransactionLookup::EffectId(_) => {
                    return Err((
                        StatusCode::NOT_FOUND,
                        "Runtime transaction effect not found".to_string(),
                    ));
                }
            },
        }
    };
    if approval_resume.is_some() {
        require_effect_principal(&store.effects[effect_index], authority)?;
    } else {
        require_effect_authority(&store.effects[effect_index], authority)?;
    }
    ensure_requested_transaction_audit(state, &mut store, effect_index)?;
    let already_confirmed = store.effects[effect_index].receipt.is_some();

    if store.effects[effect_index].receipt.is_none()
        && matches!(
            store.effects[effect_index].state,
            TransactionEffectState::BroadcastInFlight | TransactionEffectState::Indeterminate
        )
    {
        reconcile_transaction_effect(state, &mut store, effect_index).await?;
    }

    if store.effects[effect_index].receipt.is_none() {
        let approval = if let Some(managed) = managed_approval.as_ref() {
            match approve_managed_wallet_request(
                state,
                &state.data_dir,
                managed.context,
                authority,
                &store.effects[effect_index].approval_request_id,
                managed.reason,
                managed.capsule_id,
            )
            .await
            {
                Ok(outcome) => outcome.approval_request.ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Wallet signing response is missing approval state".to_string(),
                    )
                })?,
                Err(approve_err) => wallet_approval(
                    state,
                    authority,
                    &store.effects[effect_index].approval_request_id,
                )
                .await
                .map_err(|err| {
                    (
                        err.0,
                        format!("{approve_err}; Wallet status recovery failed: {}", err.1),
                    )
                })?,
            }
        } else {
            wallet_approval(
                state,
                authority,
                &store.effects[effect_index].approval_request_id,
            )
            .await?
        };
        validate_approval_snapshot(&store.effects[effect_index], authority, &approval)?;
        if approval.get("status").and_then(Value::as_str) != Some("completed") {
            return Err((
                StatusCode::BAD_REQUEST,
                "transaction approval is not completed".to_string(),
            ));
        }
        let signed_result = approval
            .get("signed_result")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "completed Wallet approval is missing signed result".to_string(),
                )
            })?;
        validate_signed_result_binding(&store.effects[effect_index], &approval, &signed_result)?;
        if let Some(external_hash) = signed_result
            .get("transaction_hash")
            .and_then(Value::as_str)
            .filter(|_| signed_result.get("signed_transaction").is_none())
        {
            validate_transaction_hash(external_hash).map_err(bad_gateway_error)?;
            let connector_id = required_value_str(&approval, "connector_id")?.to_string();
            let effect = &mut store.effects[effect_index];
            effect.approval_snapshot = Some(approval);
            effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ExternalConnector {
                connector_id,
                originating_address: effect.address.clone(),
            });
            effect.wallet_transaction_hash = Some(external_hash.to_string());
            effect.signed_result = Some(signed_result);
            effect.state = TransactionEffectState::BroadcastInFlight;
            effect.completion_error = None;
            effect.updated_at = now_ts();
            save_transaction_effect_store(state, &store)?;
            reconcile_transaction_effect(state, &mut store, effect_index).await?;
        } else {
            let signed_transaction = signed_result
                .get("signed_transaction")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "completed Wallet approval is missing signed transaction".to_string(),
                    )
                })?;
            let wallet_hash = signed_result
                .get("transaction_hash")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "completed Wallet approval is missing Wallet-computed transaction hash"
                            .to_string(),
                    )
                })?;
            validate_transaction_hash(wallet_hash).map_err(bad_gateway_error)?;
            let exact = validate_signed_evm_transaction_authority(
                &store.effects[effect_index],
                signed_transaction,
            )
            .map_err(bad_gateway_error)?;
            if !wallet_hash.eq_ignore_ascii_case(&exact.transaction_hash) {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    "Wallet transaction hash does not match the exact signed transaction"
                        .to_string(),
                ));
            }
            let effect = &mut store.effects[effect_index];
            effect.approval_snapshot = Some(approval);
            effect.signed_transaction = Some(exact.canonical);
            effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ManagedSigned {
                signed_transaction_sha256: exact.sha256,
            });
            effect.wallet_transaction_hash = Some(exact.transaction_hash);
            effect.signed_result = Some(signed_result);
            effect.state = TransactionEffectState::Signed;
            effect.completion_error = None;
            effect.updated_at = now_ts();
            save_transaction_effect_store(state, &store)?;

            let signed_transaction = store.effects[effect_index]
                .signed_transaction
                .clone()
                .expect("validated signed transaction");
            let network = store.effects[effect_index].network.clone();
            let expected_hash = store.effects[effect_index]
                .required_wallet_hash()
                .map_err(internal_error)?
                .to_string();
            store.effects[effect_index].signed_transaction = None;
            store.effects[effect_index].state = TransactionEffectState::BroadcastInFlight;
            store.effects[effect_index].updated_at = now_ts();
            save_transaction_effect_store(state, &store)?;
            let receipt = wallet_chain_provider_data(
                state,
                json!({
                    "op": "broadcast_transaction",
                    "network": network,
                    "signed_transaction": signed_transaction,
                }),
            )
            .await?;
            if let Err(err) = validate_chain_result(&receipt, &network, &expected_hash, true, None)
            {
                let effect = &mut store.effects[effect_index];
                effect.state = TransactionEffectState::Indeterminate;
                effect.completion_error = Some(err.to_string());
                effect.updated_at = now_ts();
                save_transaction_effect_store(state, &store)?;
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("Chain broadcast result failed exact validation: {err}"),
                ));
            }
            let effect = &mut store.effects[effect_index];
            effect.receipt = Some(receipt);
            effect.state = TransactionEffectState::ReceiptConfirmed;
            effect.completion_error = None;
            effect.updated_at = now_ts();
            save_transaction_effect_store(state, &store)?;
        }
    }

    finish_transaction_completion(state, authority, &mut store, effect_index).await;
    let effect = &store.effects[effect_index];
    let completion = RuntimeTransactionCompletion {
        effect_id: effect.effect_id.clone(),
        approval_request_id: effect.approval_request_id.clone(),
        transaction_hash: effect
            .required_wallet_hash()
            .map_err(internal_error)?
            .to_string(),
        approval_request: effect
            .approval_snapshot
            .clone()
            .unwrap_or_else(|| json!({})),
        validated_chain_outcome: validated_chain_outcome(effect).ok(),
        signed_result: effect.signed_result.clone(),
        receipt: effect.receipt.clone(),
        completion_pending: effect.state != TransactionEffectState::Complete,
        completion_error: effect.completion_error.clone(),
        already_confirmed,
        externally_completed: effect.externally_completed(),
    };
    if let Err(err) = save_transaction_effect_store(state, &store) {
        tracing::warn!(
            effect_id = %completion.effect_id,
            error = %err.1,
            "transaction completion state remains retryable after persisted Chain receipt"
        );
    }
    Ok(completion)
}

async fn finish_transaction_completion(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    store: &mut RuntimeTransactionEffectStore,
    effect_index: usize,
) {
    let mut errors = Vec::new();
    if !store.effects[effect_index].projection_completed {
        let effect = store.effects[effect_index].clone();
        let outcome = match validated_chain_outcome(&effect) {
            Ok(outcome) => Some(outcome),
            Err(err) => {
                errors.push(err.to_string());
                store.effects[effect_index].state = TransactionEffectState::CompletionPending;
                store.effects[effect_index].completion_error = Some(errors.join("; "));
                None
            }
        };
        if let Some(outcome) = outcome {
            match runtime_wallet_data_with_request_id(
                state,
                authority,
                wallet_request_id(&effect.effect_id, "chain-outcome"),
                WalletProviderOperationV2::AttachValidatedChainOutcome { outcome },
            )
            .await
            {
                Ok(data) => {
                    store.effects[effect_index].projection_completed = true;
                    if let Some(approval) = data
                        .get("approval_request")
                        .filter(|value| value.is_object())
                        .cloned()
                    {
                        store.effects[effect_index].approval_snapshot = Some(approval);
                    }
                }
                Err(err) => errors.push(format!("Wallet outcome projection pending: {err}")),
            }
        }
    }
    if !store.effects[effect_index].completion_audit_completed {
        match append_transaction_effect_audit(
            state,
            &store.effects[effect_index],
            "completed",
            store.effects[effect_index]
                .wallet_transaction_hash
                .as_deref(),
        ) {
            Ok(()) => store.effects[effect_index].completion_audit_completed = true,
            Err(err) => errors.push(format!("transaction audit pending: {err}")),
        }
    }
    let effect = &mut store.effects[effect_index];
    effect.updated_at = now_ts();
    if effect.requested_audit_completed
        && effect.projection_completed
        && effect.completion_audit_completed
    {
        effect.state = TransactionEffectState::Complete;
        effect.completion_error = None;
    } else {
        effect.state = TransactionEffectState::CompletionPending;
        effect.completion_error = Some(errors.join("; "));
    }
}

async fn reconcile_transaction_effect(
    state: &GatewayState,
    store: &mut RuntimeTransactionEffectStore,
    effect_index: usize,
) -> Result<(), (StatusCode, String)> {
    let network = store.effects[effect_index].network.clone();
    let expected_hash = store.effects[effect_index]
        .required_wallet_hash()
        .map_err(internal_error)?
        .to_string();
    let external_originating_address = store.effects[effect_index]
        .external_originating_address()
        .map(ToString::to_string);
    let lookup_operations: &[&str] = if external_originating_address.is_some() {
        &["transaction"]
    } else {
        &["receipt", "transaction"]
    };
    let mut lookup_errors = Vec::new();
    for op in lookup_operations {
        match wallet_chain_provider_data(
            state,
            json!({
                "op": op,
                "network": network,
                "hash": expected_hash,
            }),
        )
        .await
        {
            Ok(result) => {
                if let Err(err) = validate_chain_result(
                    &result,
                    &network,
                    &expected_hash,
                    false,
                    external_originating_address.as_deref(),
                ) {
                    let effect = &mut store.effects[effect_index];
                    effect.state = TransactionEffectState::Indeterminate;
                    effect.completion_error = Some(err.to_string());
                    effect.updated_at = now_ts();
                    save_transaction_effect_store(state, store)?;
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("Chain reconciliation result failed exact validation: {err}"),
                    ));
                }
                let found = match *op {
                    "receipt" => result.get("receipt").is_some_and(|value| !value.is_null()),
                    _ => result
                        .get("transaction")
                        .is_some_and(|value| !value.is_null()),
                };
                if found {
                    let effect = &mut store.effects[effect_index];
                    effect.receipt = Some(result);
                    effect.state = TransactionEffectState::ReceiptConfirmed;
                    effect.completion_error = None;
                    effect.updated_at = now_ts();
                    save_transaction_effect_store(state, store)?;
                    return Ok(());
                }
            }
            Err((_, err)) => lookup_errors.push(format!("{op}: {err}")),
        }
    }
    let message = if lookup_errors.is_empty() {
        "Chain could not independently determine the prior broadcast outcome".to_string()
    } else {
        format!(
            "Chain reconciliation was unavailable: {}",
            lookup_errors.join("; ")
        )
    };
    let effect = &mut store.effects[effect_index];
    effect.state = TransactionEffectState::Indeterminate;
    effect.completion_error = Some(message.clone());
    effect.updated_at = now_ts();
    save_transaction_effect_store(state, store)?;
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        if external_originating_address.is_some() {
            format!("{message}; Runtime never rebroadcasts connector transactions")
        } else {
            format!("{message}; Runtime will not rebroadcast this transaction")
        },
    ))
}

async fn create_browser_effect_from_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    store: &mut RuntimeTransactionEffectStore,
    request_id: &str,
) -> Result<usize, (StatusCode, String)> {
    let approval = wallet_approval(state, authority, request_id).await?;
    let actor = approval_actor(&approval);
    if actor != Some(BROWSER_CAPSULE_ID)
        || approval.get("principal_id").and_then(Value::as_str)
            != Some(authority.verified_context().principal_id())
        || approval.get("intent").and_then(Value::as_str) != Some("transaction_intent")
    {
        return Err((
            StatusCode::NOT_FOUND,
            "browser wallet transaction approval not found".to_string(),
        ));
    }
    let account_id = required_value_str(&approval, "account_id")?;
    let address = required_value_str(&approval, "address")?;
    let chain_namespace = required_value_str(&approval, "chain_namespace")?;
    let network = wallet_chain_namespace_network(chain_namespace).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Browser transaction approval uses an unsupported eip155 chain".to_string(),
        )
    })?;
    let intent = approval
        .get("payload")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Browser transaction approval is missing its Chain intent".to_string(),
            )
        })?;
    let stable = json!({
        "approval_request_id": request_id,
        "payload_hash": approval.get("payload_hash"),
    });
    let effect_id = runtime_transaction_effect_id(BROWSER_TRANSACTION_SOURCE, authority, &stable)
        .map_err(internal_error)?;
    let request_sha256 = runtime_transaction_request_sha256(&stable).map_err(internal_error)?;
    let now = now_ts();
    let effect = RuntimeTransactionEffect {
        schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
        effect_id,
        source: BROWSER_TRANSACTION_SOURCE.to_string(),
        authority: transaction_authority(authority),
        request_sha256,
        request_binding: stable,
        approval_request_id: request_id.to_string(),
        wallet_request_sha256: required_value_str(&approval, "wallet_request_sha256")?.to_string(),
        approval_expires_at: approval
            .get("expires_at")
            .and_then(Value::as_u64)
            .unwrap_or(now),
        approval_reason: approval
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("Browser transaction approval")
            .to_string(),
        account_id: account_id.to_string(),
        address: address.to_string(),
        chain_namespace: chain_namespace.to_string(),
        network: network.to_string(),
        intent,
        state: TransactionEffectState::ApprovalPending,
        approval_snapshot: Some(approval),
        signed_transaction: None,
        wallet_binding: None,
        wallet_transaction_hash: None,
        signed_result: None,
        receipt: None,
        requested_audit_completed: false,
        projection_completed: false,
        completion_audit_completed: false,
        completion_error: None,
        created_at: now,
        updated_at: now,
    };
    let expected_wallet_request_sha256 = wallet_operation_request_sha256(
        authority,
        &effect.approval_request_id,
        transaction_approval_operation(&effect),
    )
    .map_err(internal_error)?;
    if effect.wallet_request_sha256 != expected_wallet_request_sha256 {
        return Err((
            StatusCode::CONFLICT,
            "Browser Wallet approval has a stale or substituted operation binding".to_string(),
        ));
    }
    store.prepare_capacity().map_err(conflict_error)?;
    store.effects.push(effect);
    save_transaction_effect_store(state, store)?;
    Ok(store.effects.len() - 1)
}

async fn wallet_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> Result<Value, (StatusCode, String)> {
    let data = runtime_wallet_data(
        state,
        authority,
        WalletProviderOperationV2::ListApprovals {
            include_resolved: true,
        },
    )
    .await
    .map_err(wallet_unavailable)?;
    data.get("approval_requests")
        .and_then(Value::as_array)
        .and_then(|approvals| {
            approvals.iter().find(|approval| {
                approval.get("request_id").and_then(Value::as_str) == Some(request_id)
            })
        })
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Wallet transaction approval not found".to_string(),
            )
        })
}

async fn prepare_chain_transaction(
    state: &GatewayState,
    request: &RuntimeTransactionRequest,
) -> Result<Value, (StatusCode, String)> {
    let expected_chain_id = request
        .chain_namespace
        .strip_prefix("eip155:")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "transaction request has an invalid EVM namespace".to_string(),
            )
        })?;
    let intent = wallet_chain_provider_data(
        state,
        json!({
            "op": "prepare_transaction",
            "network": request.network,
            "from": request.address,
            "to": request.to,
            "value": request.value,
            "data": request.data,
        }),
    )
    .await?;
    if intent.get("schema").and_then(Value::as_str)
        != Some("elastos.chain.unsigned_transaction_intent/v1")
        || intent.get("transaction_type").and_then(Value::as_str) != Some("eip155_legacy")
        || intent
            .get("network")
            .and_then(|network| network.get("id"))
            .and_then(Value::as_str)
            != Some(request.network.as_str())
        || intent
            .get("network")
            .and_then(|network| network.get("chain_id"))
            .and_then(Value::as_u64)
            != Some(expected_chain_id)
        || intent.get("chain_id").and_then(Value::as_u64) != Some(expected_chain_id)
        || !intent
            .get("from")
            .and_then(Value::as_str)
            .is_some_and(|from| from.eq_ignore_ascii_case(&request.address))
        || !intent
            .get("to")
            .and_then(Value::as_str)
            .is_some_and(|to| to.eq_ignore_ascii_case(&request.to))
        || intent.get("value").and_then(Value::as_str) != Some(request.value.as_str())
        || intent.get("data").and_then(Value::as_str) != Some(request.data.as_str())
        || intent
            .get("requires_wallet_approval")
            .and_then(Value::as_bool)
            != Some(true)
        || intent.get("wallet_intent").and_then(Value::as_str) != Some("transaction_intent")
    {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Chain transaction intent failed exact request validation".to_string(),
        ));
    }
    validate_bounded_object(
        "Chain transaction intent",
        &intent,
        MAX_TRANSACTION_INTENT_BYTES,
    )
    .map_err(bad_gateway_error)?;
    for field in ["nonce", "gas_price", "gas_limit", "value"] {
        exact_payload_quantity(&intent, field).map_err(bad_gateway_error)?;
    }
    let to = exact_payload_bytes(&intent, "to").map_err(bad_gateway_error)?;
    if to.len() != 20 {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Chain transaction intent has an invalid EVM destination".to_string(),
        ));
    }
    exact_payload_bytes(&intent, "data").map_err(bad_gateway_error)?;
    Ok(intent)
}

fn validated_chain_outcome(
    effect: &RuntimeTransactionEffect,
) -> anyhow::Result<ValidatedChainOutcomeV1> {
    Ok(ValidatedChainOutcomeV1 {
        schema: VALIDATED_CHAIN_OUTCOME_SCHEMA.to_string(),
        approval_request_id: effect.approval_request_id.clone(),
        account_id: effect.account_id.clone(),
        chain_namespace: effect.chain_namespace.clone(),
        network: PublicNetwork::new(effect.network.clone())
            .map_err(|err| anyhow::anyhow!(err.to_string()))?,
        binding: match effect
            .wallet_binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("transaction effect missing Wallet binding"))?
        {
            RuntimeTransactionWalletBinding::ManagedSigned {
                signed_transaction_sha256,
            } => ValidatedChainOutcomeBindingV1::ManagedSigned {
                signed_transaction_sha256: signed_transaction_sha256.clone(),
            },
            RuntimeTransactionWalletBinding::ExternalConnector {
                connector_id,
                originating_address,
            } => ValidatedChainOutcomeBindingV1::ExternalConnector {
                connector_id: connector_id.clone(),
                originating_address: originating_address.clone(),
            },
        },
        transaction_hash: effect.required_wallet_hash()?.to_string(),
        chain_observation: effect
            .receipt
            .clone()
            .ok_or_else(|| anyhow::anyhow!("transaction effect missing Chain observation"))?,
        confirmed_at: effect.updated_at,
    })
}

fn validate_transaction_request(
    authority: &RuntimeWalletAuthority,
    request: &RuntimeTransactionRequest,
) -> Result<(), (StatusCode, String)> {
    if !matches!(
        request.source,
        NATIVE_TRANSACTION_SOURCE | BROWSER_TRANSACTION_SOURCE
    ) || !valid_effect_id(&request.effect_id)
        || !valid_sha256(&request.request_sha256)
        || !valid_bounded_text(&request.account_id, 256)
        || !valid_bounded_text(&request.address, 256)
        || !valid_bounded_text(&request.chain_namespace, 64)
        || !valid_bounded_text(&request.network, 128)
        || !valid_bounded_text(&request.approval_reason, 2048)
        || wallet_chain_namespace_network(&request.chain_namespace)
            != Some(request.network.as_str())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid Runtime transaction effect request".to_string(),
        ));
    }
    validate_wallet_evm_address(&request.address, "from")?;
    validate_wallet_evm_address(&request.to, "to")?;
    validate_bounded_object(
        "transaction metadata",
        &request.metadata,
        MAX_TRANSACTION_INTENT_BYTES,
    )
    .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    validate_authority_binding(&transaction_authority(authority)).map_err(internal_error)
}

pub(in crate::api::gateway) fn transaction_request_binding(
    request: &RuntimeTransactionRequest,
) -> Value {
    canonical_json(json!({
        "source": request.source,
        "account_id": request.account_id,
        "address": request.address.to_ascii_lowercase(),
        "chain_namespace": request.chain_namespace,
        "network": request.network,
        "to": request.to.to_ascii_lowercase(),
        "value": request.value,
        "data": request.data,
        "approval_reason": request.approval_reason,
        "metadata": request.metadata,
    }))
}

fn validate_approval_snapshot(
    effect: &RuntimeTransactionEffect,
    authority: &RuntimeWalletAuthority,
    approval: &Value,
) -> Result<(), (StatusCode, String)> {
    let expected_resource = format!("elastos://chain/{}/broadcast_transaction", effect.network);
    if approval.get("schema").and_then(Value::as_str) != Some("elastos.wallet.approval_request/v1")
        || approval.get("request_id").and_then(Value::as_str)
            != Some(effect.approval_request_id.as_str())
        || approval
            .get("wallet_request_sha256")
            .and_then(Value::as_str)
            != Some(effect.wallet_request_sha256.as_str())
        || approval.get("principal_id").and_then(Value::as_str)
            != Some(authority.verified_context().principal_id())
        || approval.get("session_id").and_then(Value::as_str)
            != Some(effect.authority.session_id.as_str())
        || approval.get("launch_id").and_then(Value::as_str)
            != Some(effect.authority.launch_id.as_str())
        || approval.get("account_id").and_then(Value::as_str) != Some(effect.account_id.as_str())
        || approval.get("chain_namespace").and_then(Value::as_str)
            != Some(effect.chain_namespace.as_str())
        || !approval
            .get("address")
            .and_then(Value::as_str)
            .is_some_and(|address| address.eq_ignore_ascii_case(&effect.address))
        || approval.get("intent").and_then(Value::as_str) != Some("transaction_intent")
        || approval_actor(approval) != Some(effect.authority.actor.as_str())
        || approval.get("resource").and_then(Value::as_str) != Some(expected_resource.as_str())
        || approval.get("reason").and_then(Value::as_str) != Some(effect.approval_reason.as_str())
        || approval.get("payload") != Some(&effect.intent)
        || approval.get("expires_at").and_then(Value::as_u64) != Some(effect.approval_expires_at)
    {
        return Err((
            StatusCode::CONFLICT,
            "Wallet transaction approval failed exact Runtime binding validation".to_string(),
        ));
    }
    if !approval
        .get("authority_binding")
        .and_then(Value::as_str)
        .is_some_and(|value| valid_bounded_text(value, 128))
    {
        return Err((
            StatusCode::CONFLICT,
            "Wallet transaction approval is missing its authority binding".to_string(),
        ));
    }
    if let Some(existing) = effect.approval_snapshot.as_ref() {
        for field in [
            "schema",
            "request_id",
            "wallet_request_sha256",
            "authority_binding",
            "kind",
            "principal_id",
            "account_id",
            "proof_binding_id",
            "chain_namespace",
            "address",
            "proof_type",
            "connector_id",
            "intent",
            "session_id",
            "launch_id",
            "requested_by_actor",
            "capsule_id",
            "resource",
            "reason",
            "payload",
            "payload_hash",
            "created_at",
            "expires_at",
        ] {
            if existing.get(field) != approval.get(field) {
                return Err((
                    StatusCode::CONFLICT,
                    format!("Wallet transaction approval changed immutable field {field}"),
                ));
            }
        }
    }
    validate_bounded_object(
        "Wallet transaction approval",
        approval,
        MAX_TRANSACTION_RECEIPT_BYTES,
    )
    .map_err(bad_gateway_error)
}

fn transaction_approval_operation(effect: &RuntimeTransactionEffect) -> WalletProviderOperationV2 {
    WalletProviderOperationV2::RequestApproval {
        account_id: effect.account_id.clone(),
        chain_namespace: effect.chain_namespace.clone(),
        intent: "transaction_intent".to_string(),
        resource: format!("elastos://chain/{}/broadcast_transaction", effect.network),
        reason: effect.approval_reason.clone(),
        payload: effect.intent.clone(),
        expires_at: effect.approval_expires_at,
    }
}

fn validate_signed_result_binding(
    effect: &RuntimeTransactionEffect,
    approval: &Value,
    signed_result: &Value,
) -> Result<(), (StatusCode, String)> {
    let has_signed_transaction = signed_result.get("signed_transaction").is_some();
    let expected_schema = if has_signed_transaction {
        "elastos.wallet.signed-transaction-result/v1"
    } else {
        "elastos.wallet.external-transaction-result/v1"
    };
    if signed_result.get("schema").and_then(Value::as_str) != Some(expected_schema)
        || signed_result.get("request_id").and_then(Value::as_str)
            != Some(effect.approval_request_id.as_str())
        || signed_result.get("method").and_then(Value::as_str) != Some("eth_sendTransaction")
        || signed_result.get("chain_namespace").and_then(Value::as_str)
            != Some(effect.chain_namespace.as_str())
        || !signed_result
            .get("signer")
            .and_then(Value::as_str)
            .is_some_and(|signer| signer.eq_ignore_ascii_case(&effect.address))
        || signed_result.get("payload_hash") != approval.get("payload_hash")
    {
        return Err((
            StatusCode::CONFLICT,
            "Wallet signed transaction result failed exact approval binding".to_string(),
        ));
    }
    validate_bounded_object(
        "Wallet signed transaction result",
        signed_result,
        MAX_TRANSACTION_RECEIPT_BYTES,
    )
    .map_err(bad_gateway_error)
}

fn wallet_operation_request_sha256(
    authority: &RuntimeWalletAuthority,
    request_id: &str,
    operation: WalletProviderOperationV2,
) -> anyhow::Result<String> {
    Ok(
        WalletProviderRequestV2::new(authority.verified_context(), request_id, 1, 2, operation)
            .map_err(|err| anyhow::anyhow!(err.to_string()))?
            .request_sha256,
    )
}

fn validate_chain_result(
    result: &Value,
    network: &str,
    expected_hash: &str,
    require_broadcast_schema: bool,
    expected_originating_address: Option<&str>,
) -> anyhow::Result<()> {
    validate_bounded_object(
        "Chain transaction result",
        result,
        MAX_TRANSACTION_RECEIPT_BYTES,
    )?;
    if require_broadcast_schema
        && result.get("schema").and_then(Value::as_str)
            != Some("elastos.chain.broadcast_receipt/v1")
    {
        anyhow::bail!("unsupported Chain broadcast receipt schema");
    }
    if result.get("network").and_then(Value::as_str) != Some(network) {
        anyhow::bail!("Chain transaction result network mismatch");
    }
    let hash = result
        .get("transaction_hash")
        .or_else(|| result.get("hash"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Chain transaction result is missing its hash"))?;
    if hash != expected_hash {
        anyhow::bail!("Chain transaction result hash does not match Wallet-computed hash");
    }
    for nested_hash in [
        result
            .get("receipt")
            .and_then(|receipt| receipt.get("transactionHash"))
            .and_then(Value::as_str),
        result
            .get("transaction")
            .and_then(|transaction| transaction.get("hash"))
            .and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    {
        if nested_hash != expected_hash {
            anyhow::bail!("Chain transaction result payload hash mismatch");
        }
    }
    if let Some(expected_originating_address) = expected_originating_address {
        if let Some(transaction) = result
            .get("transaction")
            .filter(|transaction| !transaction.is_null())
        {
            let observed_from =
                transaction
                    .get("from")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Chain transaction observation is missing its originating account"
                        )
                    })?;
            if !observed_from.eq_ignore_ascii_case(expected_originating_address) {
                anyhow::bail!("Chain transaction originating account mismatch");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExactSignedEvmTransaction {
    canonical: String,
    sha256: String,
    transaction_hash: String,
}

struct RlpItem<'a> {
    is_list: bool,
    payload: &'a [u8],
    encoded_len: usize,
}

fn parse_rlp_length(bytes: &[u8]) -> anyhow::Result<usize> {
    if bytes.is_empty() || bytes[0] == 0 {
        anyhow::bail!("RLP length is not canonical");
    }
    bytes.iter().try_fold(0usize, |length, byte| {
        length
            .checked_mul(256)
            .and_then(|length| length.checked_add(usize::from(*byte)))
            .ok_or_else(|| anyhow::anyhow!("RLP length overflows this Runtime"))
    })
}

fn parse_rlp_item(input: &[u8]) -> anyhow::Result<RlpItem<'_>> {
    let first = *input
        .first()
        .ok_or_else(|| anyhow::anyhow!("signed transaction contains truncated RLP"))?;
    if first <= 0x7f {
        return Ok(RlpItem {
            is_list: false,
            payload: &input[..1],
            encoded_len: 1,
        });
    }
    let (is_list, payload_offset, payload_len) = match first {
        0x00..=0x7f => unreachable!("single-byte RLP was handled above"),
        0x80..=0xb7 => (false, 1usize, usize::from(first - 0x80)),
        0xb8..=0xbf => {
            let length_bytes = usize::from(first - 0xb7);
            if input.len() < 1 + length_bytes {
                anyhow::bail!("signed transaction contains truncated RLP length");
            }
            let length = parse_rlp_length(&input[1..1 + length_bytes])?;
            if length < 56 {
                anyhow::bail!("signed transaction uses non-canonical long RLP bytes");
            }
            (false, 1 + length_bytes, length)
        }
        0xc0..=0xf7 => (true, 1usize, usize::from(first - 0xc0)),
        0xf8..=0xff => {
            let length_bytes = usize::from(first - 0xf7);
            if input.len() < 1 + length_bytes {
                anyhow::bail!("signed transaction contains truncated RLP list length");
            }
            let length = parse_rlp_length(&input[1..1 + length_bytes])?;
            if length < 56 {
                anyhow::bail!("signed transaction uses a non-canonical long RLP list");
            }
            (true, 1 + length_bytes, length)
        }
    };
    let encoded_len = payload_offset
        .checked_add(payload_len)
        .ok_or_else(|| anyhow::anyhow!("signed transaction RLP length overflows"))?;
    if input.len() < encoded_len {
        anyhow::bail!("signed transaction contains truncated RLP payload");
    }
    let payload = &input[payload_offset..encoded_len];
    if !is_list && payload_len == 1 && payload[0] < 0x80 {
        anyhow::bail!("signed transaction uses non-canonical single-byte RLP");
    }
    Ok(RlpItem {
        is_list,
        payload,
        encoded_len,
    })
}

fn rlp_length_prefix(length: usize, offset: u8) -> Vec<u8> {
    if length < 56 {
        return vec![offset + length as u8];
    }
    let raw = length.to_be_bytes();
    let first = raw
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(raw.len() - 1);
    let length_bytes = &raw[first..];
    let mut encoded = vec![offset + 55 + length_bytes.len() as u8];
    encoded.extend_from_slice(length_bytes);
    encoded
}

pub(in crate::api::gateway) fn rlp_encode_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }
    let mut encoded = rlp_length_prefix(bytes.len(), 0x80);
    encoded.extend_from_slice(bytes);
    encoded
}

pub(in crate::api::gateway) fn rlp_encode_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload_len = items.iter().map(Vec::len).sum::<usize>();
    let mut encoded = rlp_length_prefix(payload_len, 0xc0);
    for item in items {
        encoded.extend_from_slice(item);
    }
    encoded
}

fn canonical_unsigned_integer(bytes: &[u8], field: &str) -> anyhow::Result<()> {
    if bytes.first() == Some(&0) {
        anyhow::bail!("signed transaction {field} is not canonical");
    }
    Ok(())
}

fn validate_low_s_signature(signature: &k256::ecdsa::Signature) -> anyhow::Result<()> {
    if signature.normalize_s().is_some() {
        anyhow::bail!("signed transaction signature is not canonical low-s");
    }
    Ok(())
}

fn rlp_u64_value(bytes: &[u8], field: &str) -> anyhow::Result<u64> {
    canonical_unsigned_integer(bytes, field)?;
    if bytes.len() > std::mem::size_of::<u64>() {
        anyhow::bail!("signed transaction {field} exceeds u64");
    }
    Ok(bytes
        .iter()
        .fold(0u64, |value, byte| (value << 8) | u64::from(*byte)))
}

pub(in crate::api::gateway) fn exact_payload_quantity(
    payload: &Value,
    field: &str,
) -> anyhow::Result<Vec<u8>> {
    let value = payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("transaction payload is missing {field}"))?;
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| anyhow::anyhow!("transaction payload {field} is not 0x-prefixed"))?;
    if raw == "0" {
        return Ok(Vec::new());
    }
    if raw.is_empty()
        || raw.starts_with('0')
        || raw.len() > 64
        || !raw.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        anyhow::bail!("transaction payload {field} is not a canonical quantity");
    }
    let padded;
    let encoded = if raw.len() % 2 == 0 {
        raw
    } else {
        padded = format!("0{raw}");
        &padded
    };
    hex::decode(encoded)
        .map_err(|_| anyhow::anyhow!("transaction payload {field} contains invalid hex"))
}

pub(in crate::api::gateway) fn exact_payload_bytes(
    payload: &Value,
    field: &str,
) -> anyhow::Result<Vec<u8>> {
    let value = payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("transaction payload is missing {field}"))?;
    let raw = value
        .strip_prefix("0x")
        .ok_or_else(|| anyhow::anyhow!("transaction payload {field} is not 0x-prefixed"))?;
    if raw.len() % 2 != 0 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("transaction payload {field} contains invalid hex");
    }
    hex::decode(raw)
        .map_err(|_| anyhow::anyhow!("transaction payload {field} contains invalid hex"))
}

fn exact_optional_intent_string(intent: &Value, field: &str, expected: &str) -> anyhow::Result<()> {
    if let Some(value) = intent.get(field) {
        if value.as_str() != Some(expected) {
            anyhow::bail!("transaction intent {field} authority mismatch");
        }
    }
    Ok(())
}

fn validate_extended_transaction_intent_authority(
    effect: &RuntimeTransactionEffect,
) -> anyhow::Result<()> {
    let intent = &effect.intent;
    if intent.get("effect_id").is_none() {
        return Ok(());
    }
    for (field, expected) in [
        ("effect_id", effect.effect_id.as_str()),
        ("source", effect.source.as_str()),
        ("request_sha256", effect.request_sha256.as_str()),
        ("account_id", effect.account_id.as_str()),
        ("chain_namespace", effect.chain_namespace.as_str()),
        ("principal_id", effect.authority.principal_id.as_str()),
        ("session_id", effect.authority.session_id.as_str()),
        ("grant_id", effect.authority.grant_id.as_str()),
        ("launch_id", effect.authority.launch_id.as_str()),
        ("requested_by_actor", effect.authority.actor.as_str()),
    ] {
        if intent.get(field).and_then(Value::as_str) != Some(expected) {
            anyhow::bail!("transaction intent {field} authority mismatch");
        }
    }
    let expected_method = if effect.source == NATIVE_TRANSACTION_SOURCE {
        "wallet_send"
    } else {
        "eth_sendTransaction"
    };
    if intent.get("method").and_then(Value::as_str) != Some(expected_method)
        || intent.get("proof_binding_id")
            != Some(
                &effect
                    .authority
                    .proof_binding_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            )
    {
        anyhow::bail!("transaction intent extended authority mismatch");
    }
    Ok(())
}

fn validate_signed_evm_transaction_authority(
    effect: &RuntimeTransactionEffect,
    signed_transaction: &str,
) -> anyhow::Result<ExactSignedEvmTransaction> {
    let intent = &effect.intent;
    let expected_chain_id = effect
        .chain_namespace
        .strip_prefix("eip155:")
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("transaction effect has an invalid EVM namespace"))?;
    if intent.get("schema").and_then(Value::as_str)
        != Some("elastos.chain.unsigned_transaction_intent/v1")
        || intent.get("transaction_type").and_then(Value::as_str) != Some("eip155_legacy")
        || intent.get("wallet_intent").and_then(Value::as_str) != Some("transaction_intent")
        || intent
            .get("requires_wallet_approval")
            .and_then(Value::as_bool)
            != Some(true)
        || intent.get("chain_id").and_then(Value::as_u64) != Some(expected_chain_id)
        || intent
            .get("network")
            .and_then(|network| network.get("id"))
            .and_then(Value::as_str)
            != Some(effect.network.as_str())
        || intent
            .get("network")
            .and_then(|network| network.get("chain_id"))
            .and_then(Value::as_u64)
            != Some(expected_chain_id)
        || !intent
            .get("from")
            .and_then(Value::as_str)
            .is_some_and(|address| address.eq_ignore_ascii_case(&effect.address))
    {
        anyhow::bail!("signed transaction intent does not match exact effect authority");
    }
    validate_extended_transaction_intent_authority(effect)?;
    exact_optional_intent_string(intent, "account_id", &effect.account_id)?;
    exact_optional_intent_string(intent, "chain_namespace", &effect.chain_namespace)?;

    let (canonical, sha256) = canonical_signed_transaction(signed_transaction)?;
    let bytes = hex::decode(canonical.trim_start_matches("0x"))?;
    let root = parse_rlp_item(&bytes)?;
    if !root.is_list || root.encoded_len != bytes.len() {
        anyhow::bail!("signed transaction must be one exact legacy RLP list");
    }
    let mut encoded_fields = root.payload;
    let mut fields = Vec::with_capacity(9);
    while !encoded_fields.is_empty() {
        let item = parse_rlp_item(encoded_fields)?;
        if item.is_list {
            anyhow::bail!("signed transaction fields must be RLP byte strings");
        }
        fields.push(item.payload);
        encoded_fields = &encoded_fields[item.encoded_len..];
    }
    if fields.len() != 9 {
        anyhow::bail!("signed legacy transaction must contain exactly nine fields");
    }
    for (field, name) in fields.iter().zip([
        "nonce",
        "gas_price",
        "gas_limit",
        "to",
        "value",
        "data",
        "v",
        "r",
        "s",
    ]) {
        if name != "to" && name != "data" {
            canonical_unsigned_integer(field, name)?;
        }
    }
    let expected_to = exact_payload_bytes(intent, "to")?;
    if expected_to.len() != 20
        || fields[0] != exact_payload_quantity(intent, "nonce")?
        || fields[1] != exact_payload_quantity(intent, "gas_price")?
        || fields[2] != exact_payload_quantity(intent, "gas_limit")?
        || fields[3] != expected_to
        || fields[4] != exact_payload_quantity(intent, "value")?
        || fields[5] != exact_payload_bytes(intent, "data")?
    {
        anyhow::bail!("signed transaction bytes differ from the exact reviewed transaction");
    }
    let v = rlp_u64_value(fields[6], "v")?;
    let eip155_v = v.checked_sub(35).ok_or_else(|| {
        anyhow::anyhow!("signed transaction does not use EIP-155 replay protection")
    })?;
    if eip155_v / 2 != expected_chain_id {
        anyhow::bail!("signed transaction chain id differs from the exact review");
    }
    let recovery_id = k256::ecdsa::RecoveryId::try_from((eip155_v % 2) as u8)
        .map_err(|_| anyhow::anyhow!("signed transaction has an invalid recovery id"))?;
    if fields[7].is_empty() || fields[7].len() > 32 || fields[8].is_empty() || fields[8].len() > 32
    {
        anyhow::bail!("signed transaction has invalid signature scalar widths");
    }
    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r[32 - fields[7].len()..].copy_from_slice(fields[7]);
    s[32 - fields[8].len()..].copy_from_slice(fields[8]);
    let signature = k256::ecdsa::Signature::from_scalars(r, s)
        .map_err(|_| anyhow::anyhow!("signed transaction has invalid signature scalars"))?;
    validate_low_s_signature(&signature)?;
    let chain_id_bytes = if expected_chain_id == 0 {
        Vec::new()
    } else {
        let raw = expected_chain_id.to_be_bytes();
        raw[raw
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(raw.len() - 1)..]
            .to_vec()
    };
    let signing_payload = rlp_encode_list(&[
        rlp_encode_bytes(fields[0]),
        rlp_encode_bytes(fields[1]),
        rlp_encode_bytes(fields[2]),
        rlp_encode_bytes(fields[3]),
        rlp_encode_bytes(fields[4]),
        rlp_encode_bytes(fields[5]),
        rlp_encode_bytes(&chain_id_bytes),
        rlp_encode_bytes(&[]),
        rlp_encode_bytes(&[]),
    ]);
    let signing_hash = Keccak256::digest(signing_payload);
    let verifying_key =
        k256::ecdsa::VerifyingKey::recover_from_prehash(&signing_hash, &signature, recovery_id)
            .map_err(|_| anyhow::anyhow!("signed transaction signer recovery failed"))?;
    let public_key = verifying_key.to_encoded_point(false);
    let recovered = Keccak256::digest(&public_key.as_bytes()[1..]);
    let recovered_address = format!("0x{}", hex::encode(&recovered[12..]));
    if !recovered_address.eq_ignore_ascii_case(&effect.address) {
        anyhow::bail!("signed transaction signer differs from the exact reviewed account");
    }
    let transaction_hash = format!("0x{}", hex::encode(Keccak256::digest(&bytes)));
    Ok(ExactSignedEvmTransaction {
        canonical,
        sha256,
        transaction_hash,
    })
}

fn canonical_signed_transaction(value: &str) -> anyhow::Result<(String, String)> {
    let encoded = value
        .strip_prefix("0x")
        .ok_or_else(|| anyhow::anyhow!("signed transaction must be 0x-prefixed"))?;
    if encoded.is_empty()
        || encoded.len() % 2 != 0
        || encoded.len() / 2 > MAX_SIGNED_TRANSACTION_BYTES
    {
        anyhow::bail!("signed transaction size is invalid");
    }
    let bytes = hex::decode(encoded)
        .map_err(|_| anyhow::anyhow!("signed transaction is not hexadecimal"))?;
    Ok((
        format!("0x{}", hex::encode(&bytes)),
        format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
    ))
}

fn append_transaction_effect_audit(
    state: &GatewayState,
    effect: &RuntimeTransactionEffect,
    phase: &str,
    transaction_hash: Option<&str>,
) -> anyhow::Result<()> {
    #[cfg(test)]
    if phase == "requested" && effect.approval_reason.contains("requested-audit-fails") {
        anyhow::bail!("simulated requested transaction audit failure");
    }
    #[cfg(test)]
    if phase == "completed" && effect.approval_reason.contains("audit-fails") {
        static FAILED_EFFECTS: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
            OnceLock::new();
        let mut failed = FAILED_EFFECTS
            .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
            .lock()
            .map_err(|_| anyhow::anyhow!("transaction audit failure state poisoned"))?;
        if failed.insert(effect.effect_id.clone()) {
            anyhow::bail!("simulated transaction audit failure");
        }
    }
    let event_type = match (effect.source.as_str(), phase) {
        (NATIVE_TRANSACTION_SOURCE, "requested") => "wallet.transaction.requested",
        (NATIVE_TRANSACTION_SOURCE, "completed") => "wallet.transaction.completed",
        (BROWSER_TRANSACTION_SOURCE, "requested") => "browser.wallet.transaction.requested",
        (BROWSER_TRANSACTION_SOURCE, "completed") => "browser.wallet.transaction.completed",
        _ => anyhow::bail!("invalid transaction audit phase"),
    };
    let event_id = format!("audit:transaction-effect:{}:{phase}", effect.effect_id);
    let reason = match phase {
        "requested" => format!(
            "Runtime transaction effect requested approval {} on {}",
            effect.approval_request_id, effect.network
        ),
        _ => format!(
            "Runtime transaction effect completed approval {} with Chain hash {}",
            effect.approval_request_id,
            transaction_hash.unwrap_or_default()
        ),
    };
    let event = RuntimeAuditEventV1 {
        schema: RuntimeAuditEventV1::SCHEMA.to_string(),
        event_id: event_id.clone(),
        event_type: event_type.to_string(),
        principal_id: Some(effect.authority.principal_id.clone()),
        proof_binding_id: effect.authority.proof_binding_id.clone(),
        session_id: Some(effect.authority.session_id.clone()),
        challenge_id: Some(effect.effect_id.clone()),
        capsule_id: Some(
            match effect.source.as_str() {
                NATIVE_TRANSACTION_SOURCE => WALLET_CAPSULE_ID,
                _ => BROWSER_CAPSULE_ID,
            }
            .to_string(),
        ),
        result: phase.to_string(),
        reason,
        occurred_at: effect.created_at,
        signer_did: None,
        signature: None,
    };
    if let Some(existing) = crate::auth::load_auth_state(&state.data_dir)?
        .audit
        .iter()
        .find(|existing| existing.event_id == event_id)
    {
        if existing.schema == event.schema
            && existing.event_type == event.event_type
            && existing.principal_id == event.principal_id
            && existing.proof_binding_id == event.proof_binding_id
            && existing.session_id == event.session_id
            && existing.challenge_id == event.challenge_id
            && existing.capsule_id == event.capsule_id
            && existing.result == event.result
            && existing.reason == event.reason
            && existing.occurred_at == event.occurred_at
        {
            return Ok(());
        }
        anyhow::bail!("transaction effect audit id collision");
    }
    crate::auth::append_audit_event(&state.data_dir, event)
}

async fn transaction_effect_lock(data_dir: &Path, principal_id: &str) -> Arc<Mutex<()>> {
    let key = (data_dir.to_path_buf(), principal_id.to_string());
    let locks = TRANSACTION_EFFECT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().await;
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

fn transaction_authority(authority: &RuntimeWalletAuthority) -> TransactionAuthorityBinding {
    let context = authority.verified_context();
    TransactionAuthorityBinding {
        principal_id: context.principal_id().to_string(),
        session_id: context.session_id().to_string(),
        proof_binding_id: context.proof_binding_id().map(ToString::to_string),
        grant_id: context.grant_id().to_string(),
        actor: context.actor().to_string(),
        launch_id: context.launch_id().to_string(),
    }
}

fn validate_authority_binding(authority: &TransactionAuthorityBinding) -> anyhow::Result<()> {
    for (label, value, max) in [
        ("principal", authority.principal_id.as_str(), 256),
        ("session", authority.session_id.as_str(), 256),
        ("grant", authority.grant_id.as_str(), 256),
        ("actor", authority.actor.as_str(), 128),
        ("launch", authority.launch_id.as_str(), 256),
    ] {
        if !valid_bounded_text(value, max) {
            anyhow::bail!("transaction authority {label} is invalid");
        }
    }
    if authority
        .proof_binding_id
        .as_deref()
        .is_some_and(|value| !valid_bounded_text(value, 256))
    {
        anyhow::bail!("transaction authority proof binding is invalid");
    }
    Ok(())
}

fn require_effect_authority(
    effect: &RuntimeTransactionEffect,
    authority: &RuntimeWalletAuthority,
) -> Result<(), (StatusCode, String)> {
    if effect.authority != transaction_authority(authority) {
        return Err((
            StatusCode::FORBIDDEN,
            "transaction effect belongs to different verified Runtime authority".to_string(),
        ));
    }
    Ok(())
}

fn require_effect_principal(
    effect: &RuntimeTransactionEffect,
    authority: &RuntimeWalletAuthority,
) -> Result<(), (StatusCode, String)> {
    if effect.authority.principal_id != authority.verified_context().principal_id() {
        return Err((
            StatusCode::FORBIDDEN,
            "transaction effect belongs to different verified Runtime principal".to_string(),
        ));
    }
    Ok(())
}

fn transaction_effect_location(
    state: &GatewayState,
    principal_id: &str,
) -> Result<(String, String, PathBuf), (StatusCode, String)> {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let uri = format!("{localhost_root}/{TRANSACTION_EFFECT_STORE_RELATIVE_PATH}");
    let path = rooted_localhost_fs_path(&state.data_dir, &uri).ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid Runtime transaction effect storage path".to_string(),
        )
    })?;
    Ok((localhost_root, uri, path))
}

pub(super) fn principal_root_protected_object_inventory(
    localhost_root: &str,
) -> Vec<crate::auth::PrincipalRootProtectedObjectDeclarationV1> {
    vec![
        crate::auth::PrincipalRootProtectedObjectDeclarationV1::exact(format!(
            "{localhost_root}/{TRANSACTION_EFFECT_STORE_RELATIVE_PATH}"
        )),
    ]
}

fn load_transaction_effect_store(
    state: &GatewayState,
    principal_id: &str,
) -> Result<RuntimeTransactionEffectStore, (StatusCode, String)> {
    let (localhost_root, uri, path) = transaction_effect_location(state, principal_id)?;
    if !path.is_file() {
        return Ok(RuntimeTransactionEffectStore::empty(principal_id));
    }
    let bytes = crate::auth::read_principal_root_object(
        &state.data_dir,
        principal_id,
        &localhost_root,
        &uri,
        &path,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to read Runtime transaction effects: {err}"),
        )
    })?;
    if bytes.len() > MAX_TRANSACTION_EFFECT_STORE_BYTES {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Runtime transaction effect store exceeds its bounded size".to_string(),
        ));
    }
    let store: RuntimeTransactionEffectStore = serde_json::from_slice(&bytes).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("invalid Runtime transaction effect store: {err}"),
        )
    })?;
    store.validate(principal_id).map_err(internal_error)?;
    Ok(store)
}

fn save_transaction_effect_store(
    state: &GatewayState,
    store: &RuntimeTransactionEffectStore,
) -> Result<(), (StatusCode, String)> {
    store
        .validate(&store.principal_id)
        .map_err(internal_error)?;
    let bytes = serde_json::to_vec_pretty(store).map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode Runtime transaction effects: {err}"),
        )
    })?;
    if bytes.len() > MAX_TRANSACTION_EFFECT_STORE_BYTES {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Runtime transaction effect store exceeds its bounded size".to_string(),
        ));
    }
    let (localhost_root, uri, path) = transaction_effect_location(state, &store.principal_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to create Runtime transaction effect storage: {err}"),
            )
        })?;
    }
    crate::auth::write_principal_root_object(
        &state.data_dir,
        &store.principal_id,
        &localhost_root,
        &uri,
        &path,
        &bytes,
    )
    .map_err(|err| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to persist Runtime transaction effects: {err}"),
        )
    })
}

#[cfg(test)]
pub(in crate::api::gateway) fn transaction_effect_store_for_test(
    state: &GatewayState,
    principal_id: &str,
) -> Value {
    serde_json::to_value(
        load_transaction_effect_store(state, principal_id)
            .expect("load Runtime transaction effect store for test"),
    )
    .expect("encode Runtime transaction effect store for test")
}

fn find_effect_index(
    store: &RuntimeTransactionEffectStore,
    lookup: &RuntimeTransactionLookup<'_>,
) -> Option<usize> {
    store.effects.iter().position(|effect| match lookup {
        RuntimeTransactionLookup::EffectId(effect_id) => effect.effect_id == *effect_id,
        RuntimeTransactionLookup::ApprovalId(request_id) => {
            effect.approval_request_id == *request_id
        }
    })
}

fn approved_request_effect_index(
    store: &RuntimeTransactionEffectStore,
    approval_request_id: &str,
    request: &RuntimeTransactionRequest,
) -> Result<usize, (StatusCode, String)> {
    let Some(index) = store
        .effects
        .iter()
        .position(|effect| effect.approval_request_id == approval_request_id)
    else {
        return Err((
            StatusCode::NOT_FOUND,
            "approved Runtime transaction effect not found".to_string(),
        ));
    };
    let effect = &store.effects[index];
    if effect.approval_snapshot.is_none() {
        return Err((
            StatusCode::CONFLICT,
            "approved Runtime transaction effect is missing its durable approval snapshot"
                .to_string(),
        ));
    }
    let expected_request_binding = transaction_request_binding(request);
    if effect.source != request.source
        || effect.request_sha256 != request.request_sha256
        || effect.request_binding != expected_request_binding
        || effect.account_id != request.account_id
        || !effect.address.eq_ignore_ascii_case(&request.address)
        || effect.chain_namespace != request.chain_namespace
        || effect.network != request.network
    {
        return Err((
            StatusCode::CONFLICT,
            "approved Runtime transaction recovery binding mismatch".to_string(),
        ));
    }
    Ok(index)
}

pub(in crate::api::gateway) fn wallet_request_id(effect_id: &str, phase: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"elastos.runtime.transaction-wallet-request/v1");
    digest.update([0]);
    digest.update(effect_id.as_bytes());
    digest.update([0]);
    digest.update(phase.as_bytes());
    let encoded = hex::encode(digest.finalize());
    format!("wallet-request:{}", &encoded[..32])
}

fn approval_actor(value: &Value) -> Option<&str> {
    value
        .get("requested_by_actor")
        .or_else(|| value.get("capsule_id"))
        .and_then(Value::as_str)
}

fn required_value_str<'a>(value: &'a Value, field: &str) -> Result<&'a str, (StatusCode, String)> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Wallet transaction approval is missing {field}"),
            )
        })
}

fn validate_bounded_object(label: &str, value: &Value, max_bytes: usize) -> anyhow::Result<()> {
    if !value.is_object() {
        anyhow::bail!("{label} must be a JSON object");
    }
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > max_bytes {
        anyhow::bail!("{label} exceeds {max_bytes} bytes");
    }
    Ok(())
}

fn validate_transaction_hash(value: &str) -> anyhow::Result<()> {
    let Some(encoded) = value.strip_prefix("0x") else {
        anyhow::bail!("transaction hash must be 0x-prefixed");
    };
    if encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("transaction hash must be a 32-byte hexadecimal digest");
    }
    Ok(())
}

fn valid_effect_id(value: &str) -> bool {
    value
        .strip_prefix("transaction-effect:sha256:")
        .is_some_and(valid_sha256)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_prefixed_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_sha256)
}

fn valid_tagged_sha256(value: &str, tag: &str) -> bool {
    value
        .strip_prefix(&format!("{tag}:sha256:"))
        .is_some_and(valid_sha256)
}

fn valid_bounded_text(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && !value.chars().any(char::is_control)
        && !value.contains(['/', '\\'])
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut entries = values.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical_json(value)))
                    .collect(),
            )
        }
        value => value,
    }
}

fn wallet_unavailable(err: anyhow::Error) -> (StatusCode, String) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        format!("wallet-provider unavailable: {err}"),
    )
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}

fn conflict_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::CONFLICT, err.to_string())
}

fn bad_gateway_error(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_GATEWAY, err.to_string())
}

#[cfg(test)]
mod exact_signed_transaction_tests {
    use super::*;
    use k256::ecdsa::SigningKey;

    fn trim_integer(bytes: &[u8]) -> &[u8] {
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len());
        &bytes[first..]
    }

    fn signing_key(byte: u8) -> SigningKey {
        SigningKey::from_bytes((&[byte; 32]).into()).unwrap()
    }

    fn address_for_key(key: &SigningKey) -> String {
        let point = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&point.as_bytes()[1..]);
        format!("0x{}", hex::encode(&digest[12..]))
    }

    fn sign_intent(intent: &Value, key: &SigningKey) -> String {
        let chain_id = intent["chain_id"].as_u64().unwrap();
        let nonce = exact_payload_quantity(intent, "nonce").unwrap();
        let gas_price = exact_payload_quantity(intent, "gas_price").unwrap();
        let gas_limit = exact_payload_quantity(intent, "gas_limit").unwrap();
        let to = exact_payload_bytes(intent, "to").unwrap();
        let value = exact_payload_quantity(intent, "value").unwrap();
        let data = exact_payload_bytes(intent, "data").unwrap();
        let chain_id_bytes = chain_id.to_be_bytes();
        let signing_payload = rlp_encode_list(&[
            rlp_encode_bytes(&nonce),
            rlp_encode_bytes(&gas_price),
            rlp_encode_bytes(&gas_limit),
            rlp_encode_bytes(&to),
            rlp_encode_bytes(&value),
            rlp_encode_bytes(&data),
            rlp_encode_bytes(trim_integer(&chain_id_bytes)),
            rlp_encode_bytes(&[]),
            rlp_encode_bytes(&[]),
        ]);
        let signing_hash = Keccak256::digest(signing_payload);
        let (signature, recovery_id) = key.sign_prehash_recoverable(&signing_hash).unwrap();
        let signature = signature.to_bytes();
        let v = chain_id * 2 + 35 + u64::from(recovery_id.to_byte());
        let v_bytes = v.to_be_bytes();
        let signed = rlp_encode_list(&[
            rlp_encode_bytes(&nonce),
            rlp_encode_bytes(&gas_price),
            rlp_encode_bytes(&gas_limit),
            rlp_encode_bytes(&to),
            rlp_encode_bytes(&value),
            rlp_encode_bytes(&data),
            rlp_encode_bytes(trim_integer(&v_bytes)),
            rlp_encode_bytes(trim_integer(&signature[..32])),
            rlp_encode_bytes(trim_integer(&signature[32..])),
        ]);
        format!("0x{}", hex::encode(signed))
    }

    fn effect_for_key(key: &SigningKey) -> RuntimeTransactionEffect {
        let address = address_for_key(key);
        let effect_id = format!("transaction-effect:sha256:{}", "a".repeat(64));
        let request_sha256 = "b".repeat(64);
        let account_id = format!("wallet:eip155:20:{address}");
        let authority = TransactionAuthorityBinding {
            principal_id: "principal:test".to_string(),
            session_id: "session:test".to_string(),
            proof_binding_id: Some("proof:test".to_string()),
            grant_id: "grant:test".to_string(),
            actor: BROWSER_CAPSULE_ID.to_string(),
            launch_id: "launch:test".to_string(),
        };
        let intent = json!({
            "schema": "elastos.chain.unsigned_transaction_intent/v1",
            "transaction_type": "eip155_legacy",
            "network": { "id": "esc-mainnet", "chain_id": 20 },
            "from": address,
            "to": "0x2222222222222222222222222222222222222222",
            "value": "0x1",
            "data": "0x",
            "chain_id": 20,
            "nonce": "0x1",
            "gas_price": "0x3b9aca00",
            "gas_limit": "0x5208",
            "requires_wallet_approval": true,
            "wallet_intent": "transaction_intent",
            "effect_id": effect_id,
            "source": BROWSER_TRANSACTION_SOURCE,
            "request_sha256": request_sha256,
            "account_id": account_id,
            "chain_namespace": "eip155:20",
            "principal_id": authority.principal_id,
            "session_id": authority.session_id,
            "proof_binding_id": authority.proof_binding_id,
            "grant_id": authority.grant_id,
            "launch_id": authority.launch_id,
            "requested_by_actor": authority.actor,
            "method": "eth_sendTransaction"
        });
        RuntimeTransactionEffect {
            schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
            effect_id,
            source: BROWSER_TRANSACTION_SOURCE.to_string(),
            authority,
            request_sha256,
            request_binding: json!({}),
            approval_request_id: "wallet-request:test".to_string(),
            wallet_request_sha256: format!("request:sha256:{}", "c".repeat(64)),
            approval_expires_at: 2,
            approval_reason: "test exact signed transaction".to_string(),
            account_id,
            address,
            chain_namespace: "eip155:20".to_string(),
            network: "esc-mainnet".to_string(),
            intent,
            state: TransactionEffectState::ApprovalPending,
            approval_snapshot: None,
            signed_transaction: None,
            wallet_binding: None,
            wallet_transaction_hash: None,
            signed_result: None,
            receipt: None,
            requested_audit_completed: false,
            projection_completed: false,
            completion_audit_completed: false,
            completion_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn exact_eip155_signature_binds_reviewed_fields_signer_chain_and_hash() {
        let key = signing_key(0x11);
        let effect = effect_for_key(&key);
        let signed = sign_intent(&effect.intent, &key);
        let exact = validate_signed_evm_transaction_authority(&effect, &signed).unwrap();
        assert_eq!(exact.canonical, signed);
        assert_eq!(
            exact.transaction_hash,
            format!(
                "0x{}",
                hex::encode(Keccak256::digest(
                    hex::decode(signed.trim_start_matches("0x")).unwrap()
                ))
            )
        );
    }

    #[test]
    fn exact_eip155_signature_rejects_reviewed_value_substitution() {
        let key = signing_key(0x11);
        let mut effect = effect_for_key(&key);
        let signed = sign_intent(&effect.intent, &key);
        effect.intent["value"] = json!("0x2");
        let err = validate_signed_evm_transaction_authority(&effect, &signed).unwrap_err();
        assert!(err.to_string().contains("exact reviewed transaction"));
    }

    #[test]
    fn exact_eip155_signature_rejects_wrong_signer_and_chain() {
        let key = signing_key(0x11);
        let effect = effect_for_key(&key);
        let wrong_signer = sign_intent(&effect.intent, &signing_key(0x12));
        assert!(
            validate_signed_evm_transaction_authority(&effect, &wrong_signer)
                .unwrap_err()
                .to_string()
                .contains("signer")
        );

        let mut wrong_chain_intent = effect.intent.clone();
        wrong_chain_intent["chain_id"] = json!(21);
        let wrong_chain = sign_intent(&wrong_chain_intent, &key);
        assert!(
            validate_signed_evm_transaction_authority(&effect, &wrong_chain)
                .unwrap_err()
                .to_string()
                .contains("chain id")
        );
    }

    #[test]
    fn pre_hardening_v1_signed_effect_remains_restart_readable() {
        let key = signing_key(0x11);
        let mut effect = effect_for_key(&key);
        for field in [
            "effect_id",
            "source",
            "request_sha256",
            "account_id",
            "chain_namespace",
            "principal_id",
            "session_id",
            "proof_binding_id",
            "grant_id",
            "launch_id",
            "requested_by_actor",
            "method",
        ] {
            effect.intent.as_object_mut().unwrap().remove(field);
        }
        let signed = sign_intent(&effect.intent, &key);
        let exact = validate_signed_evm_transaction_authority(&effect, &signed).unwrap();
        effect.signed_transaction = Some(signed);
        effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ManagedSigned {
            signed_transaction_sha256: exact.sha256,
        });
        effect.wallet_transaction_hash = Some(exact.transaction_hash);
        effect.state = TransactionEffectState::Signed;
        effect.validate(&effect.authority.principal_id).unwrap();
    }

    #[test]
    fn ethereum_integer_and_signature_canonicality_are_enforced() {
        assert!(canonical_unsigned_integer(&[], "nonce").is_ok());
        assert!(canonical_unsigned_integer(&[0], "nonce").is_err());
        let mut r = [0u8; 32];
        r[31] = 1;
        let mut high_s = [0u8; 32];
        high_s.copy_from_slice(
            &hex::decode("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140")
                .unwrap(),
        );
        let signature = k256::ecdsa::Signature::from_scalars(r, high_s).unwrap();
        assert!(validate_low_s_signature(&signature).is_err());
    }
}

#[cfg(test)]
mod approved_request_resume_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};

    use async_trait::async_trait;
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use elastos_wallet_contract::{
        VerifiedWalletInvocationContext, WalletProviderOperationV2, WalletProviderRequestV2,
        WalletProviderResponseV2, WalletResultV2,
    };
    use k256::ecdsa::SigningKey;
    use tokio::sync::Mutex;

    use super::*;

    struct MockApprovedWalletProvider {
        approvals: Mutex<Vec<Value>>,
        list_mode: ApprovalListMode,
        list_calls: AtomicUsize,
        request_approval_calls: AtomicUsize,
        attach_calls: AtomicUsize,
        requests: Mutex<Vec<Value>>,
    }

    #[derive(Clone, Copy)]
    enum ApprovalListMode {
        Stored,
        Unavailable,
        Malformed,
    }

    impl MockApprovedWalletProvider {
        fn new(approvals: Vec<Value>) -> Self {
            Self {
                approvals: Mutex::new(approvals),
                list_mode: ApprovalListMode::Stored,
                list_calls: AtomicUsize::new(0),
                request_approval_calls: AtomicUsize::new(0),
                attach_calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn unavailable_list(mut self) -> Self {
            self.list_mode = ApprovalListMode::Unavailable;
            self
        }

        fn malformed_list(mut self) -> Self {
            self.list_mode = ApprovalListMode::Malformed;
            self
        }

        fn build_approval(
            &self,
            wallet_request: &WalletProviderRequestV2,
        ) -> Result<Value, ProviderError> {
            let WalletProviderOperationV2::RequestApproval {
                account_id,
                chain_namespace,
                intent,
                resource,
                reason,
                payload,
                expires_at,
            } = &wallet_request.operation
            else {
                return Err(ProviderError::Provider(
                    "build_approval requires Wallet RequestApproval".to_string(),
                ));
            };
            let address = payload.get("from").and_then(Value::as_str).ok_or_else(|| {
                ProviderError::Provider(
                    "RequestApproval payload is missing its originating address".to_string(),
                )
            })?;
            let payload_hash = format!(
                "sha256:{}",
                runtime_transaction_request_sha256(payload)
                    .map_err(|err| ProviderError::Provider(err.to_string()))?
            );
            Ok(json!({
                "schema": "elastos.wallet.approval_request/v1",
                "request_id": wallet_request.request_id,
                "wallet_request_sha256": wallet_request.request_sha256,
                "authority_binding": wallet_request.session_binding,
                "status": "pending",
                "intent": intent,
                "requested_by_actor": wallet_request.authority.actor,
                "resource": resource,
                "reason": reason,
                "account_id": account_id,
                "chain_namespace": chain_namespace,
                "address": address,
                "proof_binding_id": wallet_request.authority.proof_binding_id,
                "payload_hash": payload_hash,
                "payload": payload,
                "principal_id": wallet_request.authority.principal_id,
                "session_id": wallet_request.authority.session_id,
                "launch_id": wallet_request.authority.launch_id,
                "created_at": wallet_request.issued_at,
                "expires_at": expires_at,
            }))
        }
    }

    #[async_trait]
    impl Provider for MockApprovedWalletProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "test wallet provider supports only raw operations".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["wallet"]
        }

        fn name(&self) -> &'static str {
            "mock-approved-wallet-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            let wallet_request: WalletProviderRequestV2 =
                serde_json::from_value(request.get("request").cloned().ok_or_else(|| {
                    ProviderError::Provider("missing Wallet request".to_string())
                })?)
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
            let data = match wallet_request.operation {
                WalletProviderOperationV2::RequestApproval {
                    ref account_id,
                    ref chain_namespace,
                    ref intent,
                    ref resource,
                    ref reason,
                    ref payload,
                    expires_at,
                } => {
                    self.request_approval_calls.fetch_add(1, Ordering::SeqCst);
                    let mut approvals = self.approvals.lock().await;
                    let approval = if let Some(existing) = approvals.iter().find(|approval| {
                        approval.get("request_id").and_then(Value::as_str)
                            == Some(wallet_request.request_id.as_str())
                    }) {
                        if existing.get("account_id").and_then(Value::as_str)
                            != Some(account_id.as_str())
                            || existing.get("chain_namespace").and_then(Value::as_str)
                                != Some(chain_namespace.as_str())
                            || existing.get("intent").and_then(Value::as_str)
                                != Some(intent.as_str())
                            || existing.get("resource").and_then(Value::as_str)
                                != Some(resource.as_str())
                            || existing.get("reason").and_then(Value::as_str)
                                != Some(reason.as_str())
                            || existing.get("payload") != Some(payload)
                            || existing.get("expires_at").and_then(Value::as_u64)
                                != Some(expires_at)
                            || existing
                                .get("wallet_request_sha256")
                                .and_then(Value::as_str)
                                != Some(wallet_request.request_sha256.as_str())
                            || existing.get("principal_id").and_then(Value::as_str)
                                != Some(wallet_request.authority.principal_id.as_str())
                            || existing.get("session_id").and_then(Value::as_str)
                                != Some(wallet_request.authority.session_id.as_str())
                            || existing.get("launch_id").and_then(Value::as_str)
                                != Some(wallet_request.authority.launch_id.as_str())
                        {
                            return Err(ProviderError::Provider(
                                "RequestApproval binding mismatch in exact approval test"
                                    .to_string(),
                            ));
                        }
                        existing.clone()
                    } else {
                        let approval = self.build_approval(&wallet_request)?;
                        approvals.push(approval.clone());
                        approval
                    };
                    json!({ "approval_request": approval })
                }
                WalletProviderOperationV2::ListApprovals { .. } => {
                    self.list_calls.fetch_add(1, Ordering::SeqCst);
                    match self.list_mode {
                        ApprovalListMode::Stored => {
                            let approvals = self.approvals.lock().await.clone();
                            json!({ "approval_requests": approvals })
                        }
                        ApprovalListMode::Unavailable => {
                            return Err(ProviderError::Provider(
                                "wallet approval list unavailable in exact approval test"
                                    .to_string(),
                            ));
                        }
                        ApprovalListMode::Malformed => {
                            json!({ "approval_requests": "malformed" })
                        }
                    }
                }
                WalletProviderOperationV2::AttachValidatedChainOutcome { .. } => {
                    self.attach_calls.fetch_add(1, Ordering::SeqCst);
                    json!({ "attached": true })
                }
                _ => {
                    return Err(ProviderError::Provider(
                        "unsupported Wallet operation in approved resume test".to_string(),
                    ));
                }
            };
            Ok(json!({
                "status": "ok",
                "data": serde_json::to_value(WalletProviderResponseV2::for_request(
                    &wallet_request,
                    WalletResultV2::Ok { data },
                ))
                .map_err(|err| ProviderError::Provider(err.to_string()))?,
            }))
        }
    }

    struct MockBroadcastChainProvider {
        expected_network: String,
        expected_hash: String,
        calls: AtomicUsize,
        requests: Mutex<Vec<Value>>,
    }

    impl MockBroadcastChainProvider {
        fn new(expected_network: String, expected_hash: String) -> Self {
            Self {
                expected_network,
                expected_hash,
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for MockBroadcastChainProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "test chain provider supports only raw operations".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["chain"]
        }

        fn name(&self) -> &'static str {
            "mock-broadcast-chain-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            if request.get("op").and_then(Value::as_str) != Some("broadcast_transaction") {
                return Err(ProviderError::Provider(
                    "unexpected Chain operation in approved resume test".to_string(),
                ));
            }
            if request.get("network").and_then(Value::as_str)
                != Some(self.expected_network.as_str())
            {
                return Err(ProviderError::Provider(
                    "unexpected Chain network in approved resume test".to_string(),
                ));
            }
            let signed_transaction = request
                .get("signed_transaction")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProviderError::Provider(
                        "missing signed transaction in approved resume test".to_string(),
                    )
                })?;
            let raw = hex::decode(signed_transaction.trim_start_matches("0x"))
                .map_err(|err| ProviderError::Provider(err.to_string()))?;
            let observed_hash = format!("0x{}", hex::encode(Keccak256::digest(raw)));
            if !observed_hash.eq_ignore_ascii_case(&self.expected_hash) {
                return Err(ProviderError::Provider(
                    "unexpected broadcast hash in approved resume test".to_string(),
                ));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!({
                "schema": "elastos.chain.broadcast_receipt/v1",
                "network": self.expected_network,
                "transaction_hash": self.expected_hash,
                "receipt": {
                    "transactionHash": self.expected_hash,
                }
            }))
        }
    }

    struct MockPrepareChainProvider {
        expected_request: RuntimeTransactionRequest,
        intent: Value,
        calls: AtomicUsize,
        requests: Mutex<Vec<Value>>,
    }

    impl MockPrepareChainProvider {
        fn new(expected_request: RuntimeTransactionRequest, intent: Value) -> Self {
            Self {
                expected_request,
                intent,
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Provider for MockPrepareChainProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "test chain provider supports only raw operations".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["chain"]
        }

        fn name(&self) -> &'static str {
            "mock-prepare-chain-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            self.requests.lock().await.push(request.clone());
            if request.get("op").and_then(Value::as_str) != Some("prepare_transaction") {
                return Err(ProviderError::Provider(
                    "unexpected Chain operation in exact approval test".to_string(),
                ));
            }
            if request.get("network").and_then(Value::as_str)
                != Some(self.expected_request.network.as_str())
                || !request
                    .get("from")
                    .and_then(Value::as_str)
                    .is_some_and(|from| from.eq_ignore_ascii_case(&self.expected_request.address))
                || !request
                    .get("to")
                    .and_then(Value::as_str)
                    .is_some_and(|to| to.eq_ignore_ascii_case(&self.expected_request.to))
                || request.get("value").and_then(Value::as_str)
                    != Some(self.expected_request.value.as_str())
                || request.get("data").and_then(Value::as_str)
                    != Some(self.expected_request.data.as_str())
            {
                return Err(ProviderError::Provider(
                    "prepare_transaction request binding mismatch in exact approval test"
                        .to_string(),
                ));
            }
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.intent.clone())
        }
    }

    fn test_gateway_state(
        root: &std::path::Path,
        provider_registry: Option<Arc<ProviderRegistry>>,
    ) -> GatewayState {
        GatewayState {
            provider_registry,
            collaboration_chat_product_port: None,
            collaboration_presence_product_port: None,
            collaboration_discovery_service: None,
            identity_manager: Arc::new(OnceLock::new()),
            cache_dir: root.join("cache"),
            data_dir: root.join("data"),
        }
    }

    fn test_authority(
        principal_id: &str,
        session_id: &str,
        actor: &str,
        launch_id: &str,
    ) -> RuntimeWalletAuthority {
        RuntimeWalletAuthority::from_verified_context(
            VerifiedWalletInvocationContext::new(
                principal_id,
                session_id,
                Some("proof:test".to_string()),
                "grant:test",
                actor,
                launch_id,
            )
            .unwrap(),
        )
    }

    fn trim_integer(bytes: &[u8]) -> &[u8] {
        let first = bytes
            .iter()
            .position(|byte| *byte != 0)
            .unwrap_or(bytes.len());
        &bytes[first..]
    }

    fn address_for_key(key: &SigningKey) -> String {
        let point = key.verifying_key().to_encoded_point(false);
        let digest = Keccak256::digest(&point.as_bytes()[1..]);
        format!("0x{}", hex::encode(&digest[12..]))
    }

    fn sign_intent(intent: &Value, key: &SigningKey) -> (String, String, String) {
        let chain_id = intent["chain_id"].as_u64().unwrap();
        let nonce = exact_payload_quantity(intent, "nonce").unwrap();
        let gas_price = exact_payload_quantity(intent, "gas_price").unwrap();
        let gas_limit = exact_payload_quantity(intent, "gas_limit").unwrap();
        let to = exact_payload_bytes(intent, "to").unwrap();
        let value = exact_payload_quantity(intent, "value").unwrap();
        let data = exact_payload_bytes(intent, "data").unwrap();
        let chain_id_bytes = chain_id.to_be_bytes();
        let signing_payload = rlp_encode_list(&[
            rlp_encode_bytes(&nonce),
            rlp_encode_bytes(&gas_price),
            rlp_encode_bytes(&gas_limit),
            rlp_encode_bytes(&to),
            rlp_encode_bytes(&value),
            rlp_encode_bytes(&data),
            rlp_encode_bytes(trim_integer(&chain_id_bytes)),
            rlp_encode_bytes(&[]),
            rlp_encode_bytes(&[]),
        ]);
        let signing_hash = Keccak256::digest(signing_payload);
        let (signature, recovery_id) = key.sign_prehash_recoverable(&signing_hash).unwrap();
        let signature = signature.to_bytes();
        let v = chain_id * 2 + 35 + u64::from(recovery_id.to_byte());
        let v_bytes = v.to_be_bytes();
        let signed = rlp_encode_list(&[
            rlp_encode_bytes(&nonce),
            rlp_encode_bytes(&gas_price),
            rlp_encode_bytes(&gas_limit),
            rlp_encode_bytes(&to),
            rlp_encode_bytes(&value),
            rlp_encode_bytes(&data),
            rlp_encode_bytes(trim_integer(&v_bytes)),
            rlp_encode_bytes(trim_integer(&signature[..32])),
            rlp_encode_bytes(trim_integer(&signature[32..])),
        ]);
        let canonical = format!("0x{}", hex::encode(&signed));
        let transaction_hash = format!("0x{}", hex::encode(Keccak256::digest(&signed)));
        let sha256 = format!("sha256:{}", hex::encode(Sha256::digest(&signed)));
        (canonical, transaction_hash, sha256)
    }

    fn approved_resume_fixture(
        authority: &RuntimeWalletAuthority,
    ) -> (
        RuntimeTransactionRequest,
        RuntimeTransactionEffect,
        Value,
        String,
    ) {
        let key = SigningKey::from_bytes((&[0x31; 32]).into()).unwrap();
        let address = address_for_key(&key);
        let request_sha256 = runtime_transaction_request_sha256(&json!({
            "domain": "resume-test",
            "binding": "creator-mint",
        }))
        .unwrap();
        let effect_id = runtime_transaction_effect_id(
            NATIVE_TRANSACTION_SOURCE,
            authority,
            &json!({
                "approval_request_id": "wallet-request:resume-approved",
                "request_sha256": request_sha256,
            }),
        )
        .unwrap();
        let request = RuntimeTransactionRequest {
            source: NATIVE_TRANSACTION_SOURCE,
            effect_id: effect_id.clone(),
            request_sha256: request_sha256.clone(),
            account_id: format!("wallet:eip155:8453:{address}"),
            address: address.clone(),
            chain_namespace: "eip155:8453".to_string(),
            network: "base-mainnet".to_string(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0x0".to_string(),
            data: "0x1234".to_string(),
            approval_reason: "Creator mint approval".to_string(),
            metadata: json!({ "flow": "protected-content-creator-mint" }),
        };
        let intent = json!({
            "schema": "elastos.chain.unsigned_transaction_intent/v1",
            "transaction_type": "eip155_legacy",
            "network": { "id": "base-mainnet", "chain_id": 8453 },
            "from": address,
            "to": request.to,
            "value": request.value,
            "data": request.data,
            "chain_id": 8453,
            "nonce": "0x1",
            "gas_price": "0x3b9aca00",
            "gas_limit": "0x5208",
            "requires_wallet_approval": true,
            "wallet_intent": "transaction_intent",
            "method": "eth_sendTransaction"
        });
        let approval_request_id = wallet_request_id(&effect_id, "approval");
        let mut effect = RuntimeTransactionEffect {
            schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
            effect_id,
            source: NATIVE_TRANSACTION_SOURCE.to_string(),
            authority: transaction_authority(authority),
            request_sha256,
            request_binding: transaction_request_binding(&request),
            approval_request_id,
            wallet_request_sha256: format!("request:sha256:{}", "f".repeat(64)),
            approval_expires_at: now_ts().saturating_add(600),
            approval_reason: request.approval_reason.clone(),
            account_id: request.account_id.clone(),
            address: request.address.clone(),
            chain_namespace: request.chain_namespace.clone(),
            network: request.network.clone(),
            intent,
            state: TransactionEffectState::ApprovalPending,
            approval_snapshot: None,
            signed_transaction: None,
            wallet_binding: None,
            wallet_transaction_hash: None,
            signed_result: None,
            receipt: None,
            requested_audit_completed: false,
            projection_completed: false,
            completion_audit_completed: false,
            completion_error: None,
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        effect.wallet_request_sha256 = wallet_operation_request_sha256(
            authority,
            &effect.approval_request_id,
            transaction_approval_operation(&effect),
        )
        .unwrap();
        let (signed_transaction, transaction_hash, signed_sha256) =
            sign_intent(&effect.intent, &key);
        let approval = json!({
            "schema": "elastos.wallet.approval_request/v1",
            "request_id": effect.approval_request_id,
            "wallet_request_sha256": effect.wallet_request_sha256,
            "principal_id": effect.authority.principal_id,
            "session_id": effect.authority.session_id,
            "launch_id": effect.authority.launch_id,
            "account_id": effect.account_id,
            "chain_namespace": effect.chain_namespace,
            "address": effect.address,
            "intent": "transaction_intent",
            "requested_by_actor": effect.authority.actor,
            "resource": format!("elastos://chain/{}/broadcast_transaction", effect.network),
            "reason": effect.approval_reason,
            "payload": effect.intent,
            "payload_hash": format!("sha256:{}", "a".repeat(64)),
            "authority_binding": "authority-binding:test",
            "created_at": effect.created_at,
            "expires_at": effect.approval_expires_at,
            "status": "completed",
            "signed_result": {
                "schema": "elastos.wallet.signed-transaction-result/v1",
                "request_id": effect.approval_request_id,
                "method": "eth_sendTransaction",
                "chain_namespace": effect.chain_namespace,
                "signer": effect.address,
                "payload_hash": format!("sha256:{}", "a".repeat(64)),
                "signed_transaction": signed_transaction,
                "transaction_hash": transaction_hash,
            }
        });
        effect.approval_snapshot = Some(approval.clone());
        effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ManagedSigned {
            signed_transaction_sha256: signed_sha256,
        });
        effect.wallet_transaction_hash = Some(transaction_hash.clone());
        effect.signed_result = approval.get("signed_result").cloned();
        effect.wallet_binding = None;
        effect.wallet_transaction_hash = None;
        effect.signed_result = None;
        (request, effect, approval, transaction_hash)
    }

    fn exact_resume_fixture(
        authority: &RuntimeWalletAuthority,
    ) -> (
        RuntimeTransactionRequest,
        RuntimeTransactionEffect,
        Value,
        String,
    ) {
        let key = SigningKey::from_bytes((&[0x41; 32]).into()).unwrap();
        let address = address_for_key(&key);
        let request_sha256 = runtime_transaction_request_sha256(&json!({
            "domain": "exact-resume-test",
            "binding": "creator-mint",
        }))
        .unwrap();
        let mut request = RuntimeTransactionRequest {
            source: NATIVE_TRANSACTION_SOURCE,
            effect_id: String::new(),
            request_sha256: request_sha256.clone(),
            account_id: format!("wallet:eip155:8453:{address}"),
            address: address.clone(),
            chain_namespace: "eip155:8453".to_string(),
            network: "base-mainnet".to_string(),
            to: "0x2222222222222222222222222222222222222222".to_string(),
            value: "0x0".to_string(),
            data: "0x1234".to_string(),
            approval_reason: "Creator mint approval".to_string(),
            metadata: json!({ "flow": "protected-content-creator-mint" }),
        };
        let request_binding = transaction_request_binding(&request);
        request.effect_id = exact_runtime_transaction_effect_id(
            NATIVE_TRANSACTION_SOURCE,
            authority.verified_context().principal_id(),
            &request.request_sha256,
            &request_binding,
        )
        .unwrap();
        let mut intent = json!({
            "schema": "elastos.chain.unsigned_transaction_intent/v1",
            "transaction_type": "eip155_legacy",
            "network": { "id": "base-mainnet", "chain_id": 8453 },
            "from": address,
            "to": request.to,
            "value": request.value,
            "data": request.data,
            "chain_id": 8453,
            "nonce": "0x1",
            "gas_price": "0x3b9aca00",
            "gas_limit": "0x5208",
            "requires_wallet_approval": true,
            "wallet_intent": "transaction_intent",
            "method": "eth_sendTransaction"
        });
        bind_transaction_intent_runtime_context(&mut intent, &request, authority, true).unwrap();
        let approval_request_id = wallet_request_id(&request.effect_id, "approval");
        let mut effect = RuntimeTransactionEffect {
            schema: TRANSACTION_EFFECT_SCHEMA.to_string(),
            effect_id: request.effect_id.clone(),
            source: NATIVE_TRANSACTION_SOURCE.to_string(),
            authority: transaction_authority(authority),
            request_sha256,
            request_binding,
            approval_request_id,
            wallet_request_sha256: String::new(),
            approval_expires_at: now_ts().saturating_add(600),
            approval_reason: request.approval_reason.clone(),
            account_id: request.account_id.clone(),
            address: request.address.clone(),
            chain_namespace: request.chain_namespace.clone(),
            network: request.network.clone(),
            intent,
            state: TransactionEffectState::Prepared,
            approval_snapshot: None,
            signed_transaction: None,
            wallet_binding: None,
            wallet_transaction_hash: None,
            signed_result: None,
            receipt: None,
            requested_audit_completed: false,
            projection_completed: false,
            completion_audit_completed: false,
            completion_error: None,
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        effect.wallet_request_sha256 = wallet_operation_request_sha256(
            authority,
            &effect.approval_request_id,
            transaction_approval_operation(&effect),
        )
        .unwrap();
        let (signed_transaction, transaction_hash, _signed_sha256) =
            sign_intent(&effect.intent, &key);
        let approval = json!({
            "schema": "elastos.wallet.approval_request/v1",
            "request_id": effect.approval_request_id,
            "wallet_request_sha256": effect.wallet_request_sha256,
            "principal_id": effect.authority.principal_id,
            "session_id": effect.authority.session_id,
            "launch_id": effect.authority.launch_id,
            "account_id": effect.account_id,
            "chain_namespace": effect.chain_namespace,
            "address": effect.address,
            "intent": "transaction_intent",
            "requested_by_actor": effect.authority.actor,
            "resource": format!("elastos://chain/{}/broadcast_transaction", effect.network),
            "reason": effect.approval_reason,
            "payload": effect.intent,
            "payload_hash": format!("sha256:{}", "b".repeat(64)),
            "authority_binding": "authority-binding:test",
            "created_at": effect.created_at,
            "expires_at": effect.approval_expires_at,
            "status": "completed",
            "signed_result": {
                "schema": "elastos.wallet.signed-transaction-result/v1",
                "request_id": effect.approval_request_id,
                "method": "eth_sendTransaction",
                "chain_namespace": effect.chain_namespace,
                "signer": effect.address,
                "payload_hash": format!("sha256:{}", "b".repeat(64)),
                "signed_transaction": signed_transaction,
                "transaction_hash": transaction_hash,
            }
        });
        (request, effect, approval, transaction_hash)
    }

    #[tokio::test]
    async fn exact_transaction_approval_creates_once_and_pending_resume_reuses_same_approval() {
        let root = tempfile::tempdir().unwrap();
        let create_authority = test_authority(
            "principal:test",
            "session:create",
            "library",
            "launch:create",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, mut approval, _hash) = exact_resume_fixture(&create_authority);
        approval["status"] = json!("pending");
        approval.as_object_mut().unwrap().remove("signed_result");
        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![]));
        let chain = Arc::new(MockPrepareChainProvider::new(
            request.clone(),
            effect.intent.clone(),
        ));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        registry
            .register_sub_provider("chain", chain.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));

        let created =
            ensure_exact_runtime_transaction_approval(&state, &create_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(created.effect_id, request.effect_id);
        assert_eq!(
            created
                .approval_request
                .get("request_id")
                .and_then(Value::as_str),
            Some(wallet_request_id(&request.effect_id, "approval").as_str())
        );

        let resumed =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(resumed.effect_id, request.effect_id);
        assert_eq!(resumed.approval_request, created.approval_request);
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 1);
        assert_eq!(chain.calls.load(Ordering::SeqCst), 1);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert_eq!(persisted.effects.len(), 1);
        assert_eq!(
            persisted.effects[0].state,
            TransactionEffectState::ApprovalPending
        );
        assert_eq!(persisted.effects[0].authority.session_id, "session:create");
        assert_eq!(
            persisted.effects[0].approval_request_id,
            wallet_request_id(&request.effect_id, "approval")
        );
    }

    #[tokio::test]
    async fn exact_transaction_approval_adopts_prepared_effect_without_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, mut approval, _hash) = exact_resume_fixture(&original_authority);
        approval["status"] = json!("pending");
        approval.as_object_mut().unwrap().remove("signed_result");
        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![approval.clone()]));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));

        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let resumed =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(resumed.effect_id, request.effect_id);
        assert_eq!(
            resumed
                .approval_request
                .get("request_id")
                .and_then(Value::as_str),
            Some(wallet_request_id(&request.effect_id, "approval").as_str())
        );
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 0);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert_eq!(persisted.effects.len(), 1);
        assert_eq!(
            persisted.effects[0].authority.session_id,
            "session:original"
        );
        assert!(persisted.effects[0].approval_snapshot.is_some());
        assert!(persisted.effects[0].requested_audit_completed);
        assert_eq!(
            persisted.effects[0].state,
            TransactionEffectState::ApprovalPending
        );
        let audit_count = crate::auth::load_auth_state(&state.data_dir)
            .unwrap()
            .audit
            .iter()
            .filter(|event| {
                event.event_id
                    == format!("audit:transaction-effect:{}:requested", request.effect_id)
            })
            .count();
        assert_eq!(audit_count, 1);

        let resumed_again =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(resumed_again.approval_request, resumed.approval_request);
        let persisted_again = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert!(persisted_again.effects[0].requested_audit_completed);
        let audit_count_again = crate::auth::load_auth_state(&state.data_dir)
            .unwrap()
            .audit
            .iter()
            .filter(|event| {
                event.event_id
                    == format!("audit:transaction-effect:{}:requested", request.effect_id)
            })
            .count();
        assert_eq!(audit_count_again, 1);
    }

    #[tokio::test]
    async fn exact_transaction_approval_recreates_prepared_effect_after_authoritative_absence() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, _approval, _hash) = exact_resume_fixture(&original_authority);
        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![]));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));
        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let resumed =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(resumed.effect_id, request.effect_id);
        assert_eq!(
            resumed
                .approval_request
                .get("session_id")
                .and_then(Value::as_str),
            Some("session:resumed")
        );
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 1);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert_eq!(persisted.effects.len(), 1);
        assert_eq!(
            persisted.effects[0].state,
            TransactionEffectState::ApprovalPending
        );
        assert_eq!(persisted.effects[0].authority.session_id, "session:resumed");
        assert_eq!(persisted.effects[0].authority.launch_id, "launch:resumed");
        assert!(persisted.effects[0].approval_snapshot.is_some());
        assert!(persisted.effects[0].requested_audit_completed);
    }

    #[tokio::test]
    async fn exact_transaction_approval_rejects_expired_prepared_effect_before_rebind() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, mut effect, _approval, _hash) = exact_resume_fixture(&original_authority);
        effect.approval_expires_at = now_ts().saturating_sub(1);
        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![]));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));
        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let err =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 0);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert_eq!(persisted.effects.len(), 1);
        assert_eq!(
            persisted.effects[0].authority.session_id,
            "session:original"
        );
        assert!(persisted.effects[0].approval_snapshot.is_none());
    }

    #[tokio::test]
    async fn exact_transaction_approval_resumes_completed_effect_without_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, mut effect, approval, transaction_hash) =
            exact_resume_fixture(&original_authority);
        effect.approval_snapshot = Some(approval.clone());
        effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ExternalConnector {
            connector_id: "wallet-test".to_string(),
            originating_address: effect.address.clone(),
        });
        effect.wallet_transaction_hash = Some(transaction_hash.clone());
        effect.receipt = Some(json!({
            "schema": "elastos.chain.broadcast_receipt/v1",
            "network": effect.network,
            "transaction_hash": transaction_hash,
            "receipt": {
                "transactionHash": transaction_hash,
            }
        }));
        effect.state = TransactionEffectState::Complete;
        effect.requested_audit_completed = true;
        effect.projection_completed = true;
        effect.completion_audit_completed = true;

        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![approval.clone()]));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));
        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let resumed =
            ensure_exact_runtime_transaction_approval(&state, &resumed_authority, request.clone())
                .await
                .unwrap();
        assert_eq!(resumed.effect_id, request.effect_id);
        assert_eq!(resumed.approval_request, approval);
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 0);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        assert_eq!(persisted.effects.len(), 1);
        assert_eq!(persisted.effects[0].state, TransactionEffectState::Complete);
        assert_eq!(
            persisted.effects[0].authority.session_id,
            "session:original"
        );
    }

    #[tokio::test]
    async fn exact_transaction_approval_rejects_conflicting_wallet_approval_and_bad_lists() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, mut approval, _hash) = exact_resume_fixture(&original_authority);
        approval["status"] = json!("pending");
        approval.as_object_mut().unwrap().remove("signed_result");

        let conflict_wallet = Arc::new(MockApprovedWalletProvider::new(vec![{
            let mut conflicting = approval.clone();
            conflicting["wallet_request_sha256"] = json!("request:sha256:conflict");
            conflicting
        }]));
        let conflict_registry = Arc::new(ProviderRegistry::new());
        conflict_registry
            .register_sub_provider("wallet", conflict_wallet.clone())
            .await
            .unwrap();
        let conflict_state = test_gateway_state(root.path(), Some(conflict_registry));
        let mut conflict_store = RuntimeTransactionEffectStore::empty("principal:test");
        conflict_store.effects.push(effect.clone());
        save_transaction_effect_store(&conflict_state, &conflict_store).unwrap();
        let err = ensure_exact_runtime_transaction_approval(
            &conflict_state,
            &resumed_authority,
            request.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
        assert_eq!(conflict_wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            conflict_wallet
                .request_approval_calls
                .load(Ordering::SeqCst),
            0
        );

        for wallet in [
            Arc::new(MockApprovedWalletProvider::new(vec![]).unavailable_list()),
            Arc::new(MockApprovedWalletProvider::new(vec![]).malformed_list()),
            Arc::new(MockApprovedWalletProvider::new(vec![
                approval.clone(),
                approval.clone(),
            ])),
        ] {
            let isolated_root = tempfile::tempdir().unwrap();
            let registry = Arc::new(ProviderRegistry::new());
            registry
                .register_sub_provider("wallet", wallet.clone())
                .await
                .unwrap();
            let state = test_gateway_state(isolated_root.path(), Some(registry));
            let mut store = RuntimeTransactionEffectStore::empty("principal:test");
            store.effects.push(effect.clone());
            save_transaction_effect_store(&state, &store).unwrap();
            let err = ensure_exact_runtime_transaction_approval(
                &state,
                &resumed_authority,
                request.clone(),
            )
            .await
            .unwrap_err();
            assert!(err.0 == StatusCode::SERVICE_UNAVAILABLE || err.0 == StatusCode::CONFLICT);
            assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
            assert_eq!(wallet.request_approval_calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn exact_transaction_approval_rejects_substituted_bindings_and_wrong_approval_id() {
        let root = tempfile::tempdir().unwrap();
        let authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let (request, effect, _approval, _hash) = exact_resume_fixture(&authority);
        let state = test_gateway_state(root.path(), None);

        let substituted_principal = test_authority(
            "principal:other",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let err = ensure_exact_runtime_transaction_approval(
            &state,
            &substituted_principal,
            request.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);

        for mutate in [
            |request: &mut RuntimeTransactionRequest| {
                request.account_id = "wallet:eip155:8453:substituted".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.address = "0x9999999999999999999999999999999999999999".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.request_sha256 = "b".repeat(64);
            },
            |request: &mut RuntimeTransactionRequest| {
                request.data = "0xbeef".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.source = BROWSER_TRANSACTION_SOURCE;
            },
            |request: &mut RuntimeTransactionRequest| {
                request.chain_namespace = "eip155:20".to_string();
                request.network = "esc-mainnet".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.effect_id = format!("transaction-effect:sha256:{}", "c".repeat(64));
            },
        ] {
            let mut substituted = request.clone();
            mutate(&mut substituted);
            let err = ensure_exact_runtime_transaction_approval(&state, &authority, substituted)
                .await
                .unwrap_err();
            assert_eq!(err.0, StatusCode::CONFLICT);
        }

        let mut tampered_effect = effect;
        tampered_effect.approval_request_id = "wallet-request:tampered".to_string();
        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(tampered_effect);
        save_transaction_effect_store(&state, &store).unwrap();
        let err = ensure_exact_runtime_transaction_approval(&state, &authority, request)
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::CONFLICT);
    }

    #[test]
    fn approved_request_effect_index_requires_exact_bindings() {
        let authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let (request, effect, _approval, _hash) = approved_resume_fixture(&authority);
        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);

        assert_eq!(
            approved_request_effect_index(
                &store,
                store.effects[0].approval_request_id.as_str(),
                &request,
            )
            .unwrap(),
            0
        );

        let status = approved_request_effect_index(&store, "wallet-request:missing", &request)
            .unwrap_err()
            .0;
        assert_eq!(status, StatusCode::NOT_FOUND);

        for mutate in [
            |request: &mut RuntimeTransactionRequest| {
                request.account_id = "wallet:eip155:8453:substituted".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.address = "0x9999999999999999999999999999999999999999".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.request_sha256 = "b".repeat(64);
            },
            |request: &mut RuntimeTransactionRequest| {
                request.data = "0xbeef".to_string();
            },
            |request: &mut RuntimeTransactionRequest| {
                request.chain_namespace = "eip155:20".to_string();
                request.network = "esc-mainnet".to_string();
            },
        ] {
            let mut substituted = request.clone();
            mutate(&mut substituted);
            let err = approved_request_effect_index(
                &store,
                store.effects[0].approval_request_id.as_str(),
                &substituted,
            )
            .unwrap_err();
            assert_eq!(err.0, StatusCode::CONFLICT);
            assert!(err.1.contains("recovery binding mismatch"));
        }
    }

    #[tokio::test]
    async fn approved_request_resume_completes_exact_effect_after_session_restart() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let resumed_authority = test_authority(
            "principal:test",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, approval, transaction_hash) =
            approved_resume_fixture(&original_authority);
        let wallet = Arc::new(MockApprovedWalletProvider::new(vec![approval.clone()]));
        let chain = Arc::new(MockBroadcastChainProvider::new(
            request.network.clone(),
            transaction_hash.clone(),
        ));
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("wallet", wallet.clone())
            .await
            .unwrap();
        registry
            .register_sub_provider("chain", chain.clone())
            .await
            .unwrap();
        let state = test_gateway_state(root.path(), Some(registry));

        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let completion = complete_runtime_transaction_effect(
            &state,
            &resumed_authority,
            RuntimeTransactionLookup::ApprovalId(store.effects[0].approval_request_id.as_str()),
            Some(&request),
            None,
        )
        .await
        .unwrap();

        assert!(!completion.completion_pending);
        assert!(completion.completion_error.is_none());
        assert_eq!(completion.transaction_hash, transaction_hash);
        assert_eq!(
            completion
                .approval_request
                .get("session_id")
                .and_then(Value::as_str),
            Some("session:original")
        );
        assert_eq!(wallet.list_calls.load(Ordering::SeqCst), 1);
        assert_eq!(wallet.attach_calls.load(Ordering::SeqCst), 1);
        assert_eq!(chain.calls.load(Ordering::SeqCst), 1);

        let persisted = load_transaction_effect_store(&state, "principal:test").unwrap();
        let persisted_effect = persisted.effects.first().unwrap();
        assert_eq!(persisted_effect.state, TransactionEffectState::Complete);
        assert_eq!(persisted_effect.authority.session_id, "session:original");
        assert_eq!(persisted_effect.authority.launch_id, "launch:original");
        assert!(persisted_effect.receipt.is_some());
    }

    #[tokio::test]
    async fn approved_request_resume_rejects_substituted_principal() {
        let root = tempfile::tempdir().unwrap();
        let original_authority = test_authority(
            "principal:test",
            "session:original",
            "library",
            "launch:original",
        );
        let substituted_authority = test_authority(
            "principal:other",
            "session:resumed",
            "system",
            "launch:resumed",
        );
        let (request, effect, _approval, _hash) = approved_resume_fixture(&original_authority);
        let state = test_gateway_state(root.path(), None);

        let mut store = RuntimeTransactionEffectStore::empty("principal:test");
        store.effects.push(effect);
        save_transaction_effect_store(&state, &store).unwrap();

        let err = complete_runtime_transaction_effect(
            &state,
            &substituted_authority,
            RuntimeTransactionLookup::ApprovalId(store.effects[0].approval_request_id.as_str()),
            Some(&request),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }
}
