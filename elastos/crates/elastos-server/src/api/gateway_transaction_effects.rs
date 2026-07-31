use std::path::PathBuf;
use std::sync::OnceLock;

use elastos_runtime::auth::RuntimeAuditEventV1;
use elastos_wallet_contract::{
    PublicNetwork, ValidatedChainOutcomeBindingV1, ValidatedChainOutcomeV1,
    WalletProviderOperationV2, WalletProviderRequestV2, VALIDATED_CHAIN_OUTCOME_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

static TRANSACTION_EFFECT_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

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

pub(in crate::api::gateway) async fn ensure_runtime_transaction_approval(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request: RuntimeTransactionRequest,
) -> Result<RuntimeTransactionApproval, (StatusCode, String)> {
    validate_transaction_request(authority, &request)?;
    let _guard = transaction_effect_guard().lock().await;
    let principal_id = authority.verified_context().principal_id();
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
        if let Some(intent_object) = intent.as_object_mut() {
            if let Some(metadata) = request.metadata.as_object() {
                for (key, value) in metadata {
                    intent_object.insert(key.clone(), value.clone());
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
        }
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

    let _guard = transaction_effect_guard().lock().await;
    let principal_id = authority.verified_context().principal_id();
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
    managed_approval: Option<RuntimeManagedTransactionApproval<'_>>,
) -> Result<RuntimeTransactionCompletion, (StatusCode, String)> {
    let _guard = transaction_effect_guard().lock().await;
    let principal_id = authority.verified_context().principal_id();
    let mut store = load_transaction_effect_store(state, principal_id)?;
    let effect_index = match find_effect_index(&store, &lookup) {
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
    };
    require_effect_authority(&store.effects[effect_index], authority)?;
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
            let (canonical_signed, digest) =
                canonical_signed_transaction(signed_transaction).map_err(bad_gateway_error)?;
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
            let effect = &mut store.effects[effect_index];
            effect.approval_snapshot = Some(approval);
            effect.signed_transaction = Some(canonical_signed);
            effect.wallet_binding = Some(RuntimeTransactionWalletBinding::ManagedSigned {
                signed_transaction_sha256: digest,
            });
            effect.wallet_transaction_hash = Some(wallet_hash.to_string());
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
        || intent
            .get("network")
            .and_then(|network| network.get("id"))
            .and_then(Value::as_str)
            != Some(request.network.as_str())
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

fn transaction_request_binding(request: &RuntimeTransactionRequest) -> Value {
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

fn transaction_effect_guard() -> &'static Mutex<()> {
    TRANSACTION_EFFECT_GUARD.get_or_init(|| Mutex::new(()))
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

fn wallet_request_id(effect_id: &str, phase: &str) -> String {
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
