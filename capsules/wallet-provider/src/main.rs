//! ElastOS Wallet Provider Capsule
//!
//! Wallet authority boundary. It persists linked account records,
//! issues/verifies Runtime-bound wallet proof challenges, records typed signing
//! approval requests, and rejects effects that do not have an approval path.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, KeyInit, Nonce};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use elastos_auth::{
    ethereum_signed_message_hash, normalize_evm_address, recover_evm_address, validate_evm_address,
    verify_siwe_challenge, AuthChallengeInput, AuthChallengeV1, ProofBinding,
};
use elastos_protected_content_contracts::{
    AtomicReplayClaimer, CanonicalContract as ProtectedContentCanonicalContract, ReplayClaimError,
    ReplayClaimKeyV1, RightsRequestV1, RightsVerificationContextV1, WalletAddress,
    WalletSignedRightsRequestV1, RIGHTS_CLOCK_SKEW_SECS,
};
use elastos_wallet_contract::{
    ManagedRecoveryKeyEntryV1, ManagedRecoverySetV1, ProtectedContentRightsSignatureResultV1,
    ValidatedChainOutcomeBindingV1, ValidatedChainOutcomeV1, WalletAuthorityV2,
    WalletProviderOperationV2, WalletProviderRequestV2, WalletProviderResponseV2, WalletResultV2,
    MAX_PROTECTED_CONTENT_RIGHTS_WIRE_BYTES, PROTECTED_CONTENT_RIGHTS_SIGNATURE_INTENT,
    PROTECTED_CONTENT_RIGHTS_SIGNATURE_RESOURCE, WALLET_BUS_OPERATION, WALLET_PROTOCOL_VERSION,
};
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod account;
mod approval;
mod crypto;
mod models;
mod protocol;
mod storage;
mod validation;

use approval::*;
use crypto::*;
use models::*;
use protocol::*;
use storage::*;
use validation::*;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const AUTH_CHALLENGE_TTL_SECS: u64 = 5 * 60;
#[cfg(test)]
const APPROVAL_REQUEST_TTL_SECS: u64 = 10 * 60;
const MAX_APPROVAL_REQUEST_TTL_SECS: u64 = 30 * 60;
const MAX_APPROVAL_PAYLOAD_BYTES: usize = 32 * 1024;
const MAX_RESOLVED_APPROVAL_HISTORY_PER_PRINCIPAL: usize = 100;
const MAX_ACTIVE_APPROVAL_REQUESTS_PER_PRINCIPAL: usize = 100;
const MAX_BROWSER_ACCOUNT_ACCESS_TTL_SECS: u64 = 12 * 60 * 60;
const MAX_EFFECTFUL_LIFECYCLE_HISTORY: usize = 512;
const MANAGED_EVM_PROOF_TYPE: &str = "managed_evm";
const MANAGED_BTC_P2WPKH_PROOF_TYPE: &str = "managed_btc_p2wpkh";
const WALLET_KEY_FILE: &str = "wallet-key.hex";
const BITCOIN_MAINNET_BIP122: &str = "000000000019d6689c085ae165831e93";
const BITCOIN_MAINNET_CHAIN_NAMESPACE: &str = "bip122:000000000019d6689c085ae165831e93";
const BITCOIN_CHALLENGE_SCHEMA: &str = "elastos.wallet.bitcoin_challenge/v1";

struct WalletProvider {
    store_path: Option<PathBuf>,
    storage_key: Option<[u8; 32]>,
    store: WalletStore,
    defer_save: bool,
}

impl WalletProvider {
    fn new() -> Self {
        Self {
            store_path: None,
            storage_key: None,
            store: WalletStore::default(),
            defer_save: false,
        }
    }

    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::WalletContract {
                request,
                runtime_invocation,
            } => self.handle_wallet_contract(request, *runtime_invocation),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn handle_wallet_contract(
        &mut self,
        request_value: Value,
        runtime_invocation: RuntimeInvocationEnvelope,
    ) -> Response {
        if let Err(err) = runtime_invocation.validate_wallet_contract() {
            return Response::error("invalid_runtime_invocation", err);
        }
        let request_bytes = match serde_json::to_vec(&request_value) {
            Ok(bytes) => bytes,
            Err(err) => return Response::error("invalid_wallet_contract", err.to_string()),
        };
        let now = now_ts();
        let request = match WalletProviderRequestV2::decode_at(&request_bytes, now) {
            Ok(request) => request,
            Err(err) => return Response::error("invalid_wallet_contract", err.to_string()),
        };
        let result = if request.is_effectful() {
            self.execute_effectful_wallet_request(&request, now)
        } else {
            response_into_wallet_result(self.dispatch_wallet_operation(&request))
        };
        let response = WalletProviderResponseV2::for_request(&request, result);
        match serde_json::to_value(response) {
            Ok(value) => Response::ok(value),
            Err(err) => Response::error("wallet_contract_response", err.to_string()),
        }
    }

    fn execute_effectful_wallet_request(
        &mut self,
        request: &WalletProviderRequestV2,
        now: u64,
    ) -> WalletResultV2 {
        prune_expired_lifecycles(&mut self.store, now);
        if let Some(response) = self.idempotent_transaction_effect_replay(request) {
            return response_into_wallet_result(response);
        }
        if self
            .store
            .consumed_lifecycles
            .iter()
            .any(|record| record.lifecycle_id == request.lifecycle_id)
        {
            return WalletResultV2::Error {
                code: "lifecycle_replay".to_string(),
                message: "Wallet effectful lifecycle was already consumed".to_string(),
            };
        }
        if self.store.consumed_lifecycles.len() >= MAX_EFFECTFUL_LIFECYCLE_HISTORY {
            return WalletResultV2::Error {
                code: "lifecycle_capacity".to_string(),
                message: "Wallet effectful lifecycle capacity is full".to_string(),
            };
        }

        let snapshot = self.store.clone();
        self.defer_save = true;
        let response = self.dispatch_wallet_operation(request);
        self.defer_save = false;
        if !response.is_ok() {
            self.store = snapshot;
            return response_into_wallet_result(response);
        }

        self.store
            .consumed_lifecycles
            .push(ConsumedWalletLifecycle {
                lifecycle_id: request.lifecycle_id.clone(),
                request_sha256: request.request_sha256.clone(),
                request_expires_at: request.expires_at,
                consumed_at: now,
            });
        self.store = prune_store(std::mem::take(&mut self.store), now);
        if let Err(err) = self.save() {
            self.recover_store_after_save_failure(snapshot);
            return WalletResultV2::Error {
                code: "storage_error".to_string(),
                message: err,
            };
        }
        response_into_wallet_result(response)
    }

    fn dispatch_wallet_operation(&mut self, request: &WalletProviderRequestV2) -> Response {
        let principal_id = request.authority.principal_id.as_str();
        let session_id = request.authority.session_id.as_str();
        let actor = request.authority.actor.as_str();
        match &request.operation {
            WalletProviderOperationV2::ListAccounts { include_revoked } => {
                self.accounts(principal_id, *include_revoked)
            }
            WalletProviderOperationV2::CreateManagedAccount {
                chain_namespace,
                label,
                create_new,
            } => self.create_managed_account(CreateManagedAccountInput {
                principal_id: principal_id.to_string(),
                chain_namespace: chain_namespace.clone(),
                label: label.clone(),
                create_new: *create_new,
            }),
            WalletProviderOperationV2::LinkVerifiedAccount {
                proof_binding_id,
                chain_namespace,
                address,
                proof_type,
                label,
            } => self.link_account(LinkAccountInput {
                principal_id: principal_id.to_string(),
                proof_binding_id: proof_binding_id.clone(),
                chain_namespace: chain_namespace.clone(),
                address: address.clone(),
                proof_type: proof_type.clone(),
                connector_id: Some(actor.to_string()),
                label: label.clone(),
            }),
            WalletProviderOperationV2::RevokeAccount { account_id } => {
                self.revoke_account(principal_id, account_id)
            }
            WalletProviderOperationV2::RenameAccount { account_id, label } => {
                self.rename_account(principal_id, account_id, label)
            }
            WalletProviderOperationV2::SetDefaultAccount {
                chain_namespace,
                intent,
                account_id,
            } => self.set_default_account(SetDefaultAccountInput {
                principal_id: principal_id.to_string(),
                chain_namespace: chain_namespace.clone(),
                intent: intent.clone(),
                account_id: account_id.clone(),
            }),
            WalletProviderOperationV2::DefaultAccount {
                chain_namespace,
                intent,
            } => self.default_account(principal_id, chain_namespace, intent),
            WalletProviderOperationV2::Challenge {
                domain,
                uri,
                address,
                chain_id,
                resources,
            } => self.challenge(ChallengeInput {
                domain: domain.clone(),
                uri: uri.clone(),
                address: address.clone(),
                chain_id: *chain_id,
                resources: resources.clone(),
            }),
            WalletProviderOperationV2::BitcoinChallenge {
                domain,
                uri,
                address,
                network,
                resources,
            } => self.bitcoin_challenge(BitcoinChallengeInput {
                domain: domain.clone(),
                uri: uri.clone(),
                address: address.clone(),
                network: network.as_str().to_string(),
                resources: resources.clone(),
            }),
            WalletProviderOperationV2::VerifyProof { message, signature } => {
                self.verify_proof(message, signature)
            }
            WalletProviderOperationV2::VerifyContractProof {
                message,
                signature,
                evidence,
            } => match serde_json::to_value(evidence) {
                Ok(evidence) => self.verify_contract_proof(message, signature, &evidence),
                Err(err) => Response::error("invalid_contract_proof", err.to_string()),
            },
            WalletProviderOperationV2::VerifyBip322Proof {
                message,
                signature,
                signature_type,
                public_key,
            } => self.verify_bitcoin_proof(
                message,
                signature,
                Some(signature_type),
                public_key.as_deref(),
            ),
            WalletProviderOperationV2::RequestProtectedContentRightsSignature {
                account_id,
                canonical_rights_request_hex,
                reason,
            } => self.request_protected_content_rights_signature(
                request,
                account_id,
                canonical_rights_request_hex,
                reason,
            ),
            WalletProviderOperationV2::RequestApproval {
                account_id,
                chain_namespace,
                intent,
                resource,
                reason,
                payload,
                expires_at,
            } => self.request_approval(SignatureRequestInput {
                request_id: request.request_id.clone(),
                wallet_request_sha256: request.request_sha256.clone(),
                authority_binding: wallet_authority_binding(&request.authority),
                principal_id: principal_id.to_string(),
                session_id: session_id.to_string(),
                launch_id: request.authority.launch_id.clone(),
                proof_binding_id: request.authority.proof_binding_id.clone(),
                account_id: account_id.clone(),
                chain_namespace: chain_namespace.clone(),
                intent: intent.clone(),
                requested_by_actor: actor.to_string(),
                resource: resource.clone(),
                reason: reason.clone(),
                payload: payload.clone(),
                expires_at: *expires_at,
            }),
            WalletProviderOperationV2::AttachValidatedChainOutcome { outcome } => {
                self.attach_validated_chain_outcome(&request.authority, outcome)
            }
            WalletProviderOperationV2::ListApprovals { include_resolved } => {
                self.approval_requests(principal_id, *include_resolved)
            }
            WalletProviderOperationV2::RejectApproval { request_id, reason } => {
                self.reject_approval(principal_id, session_id, actor, request_id, reason)
            }
            WalletProviderOperationV2::ApproveAndSignManaged { request_id, reason } => {
                self.approve_and_sign_managed(principal_id, session_id, request_id, reason)
            }
            WalletProviderOperationV2::ApproveConnectorHandoff { request_id, reason } => {
                self.approve_connector_handoff(principal_id, session_id, actor, request_id, reason)
            }
            WalletProviderOperationV2::CompleteConnectorHandoff {
                request_id,
                payload_hash,
                signature,
                signature_type,
                public_key,
                signer,
                transaction_hash,
            } => self.complete_connector_handoff(ConnectorHandoffCompletion {
                principal_id,
                session_id,
                request_id,
                connector_id: actor,
                payload_hash,
                signature: signature.as_deref(),
                signature_type: signature_type.as_deref(),
                public_key: public_key.as_deref(),
                signer,
                transaction_hash: transaction_hash.as_deref(),
            }),
            WalletProviderOperationV2::ExportManagedRecoveryKey { account_id } => {
                self.export_managed_recovery_key(principal_id, account_id)
            }
            WalletProviderOperationV2::ImportManagedRecoveryKey {
                recovery_key,
                label,
            } => self.import_managed_recovery_key(ImportManagedRecoveryKeyInput {
                principal_id: principal_id.to_string(),
                recovery_key: recovery_key.clone(),
                label: label.clone(),
            }),
            WalletProviderOperationV2::ExportManagedRecoverySet {} => {
                self.export_managed_recovery_set(principal_id)
            }
            WalletProviderOperationV2::ImportManagedRecoverySet { recovery_set } => {
                self.import_managed_recovery_set(principal_id, recovery_set)
            }
        }
    }

    fn init(&mut self, config: Value) -> Response {
        self.store_path = None;
        self.storage_key = None;
        self.store = WalletStore::default();
        self.defer_save = false;
        let Some(base_path) = config
            .get("base_path")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Response::error("invalid_config", "wallet-provider requires base_path");
        };

        let wallet_dir = Path::new(base_path)
            .join("ElastOS")
            .join("SystemServices")
            .join("Wallet");
        let store_path = wallet_dir.join("wallet-state.json");
        if let Err(err) = create_wallet_dir_durable(&wallet_dir) {
            return Response::error("storage_error", err);
        }
        let storage_key = match load_or_create_storage_key(&wallet_dir) {
            Ok(key) => key,
            Err(err) => return Response::error("storage_error", err),
        };
        let store = match load_store(&store_path) {
            Ok(store) => store,
            Err(err) => return Response::error("storage_error", err),
        };
        let now = now_ts();
        let mut store = prune_store(store, now);
        let pre_v2_approvals_rejected = reject_pre_v2_pending_approvals(&mut store, now);
        if pre_v2_approvals_rejected > 0 {
            if let Err(err) = save_store(&store_path, &store) {
                return Response::error("storage_error", err);
            }
        }
        self.store_path = Some(store_path);
        self.storage_key = Some(storage_key);
        self.store = store;
        Response::ok(json!({
            "provider": "wallet-provider",
            "protocol_version": WALLET_PROTOCOL_VERSION,
            "storage_configured": self.store_path.is_some(),
            "managed_wallets_configured": self.storage_key.is_some(),
            "pre_v2_approvals_rejected": pre_v2_approvals_rejected,
        }))
    }

    fn status(&self) -> Response {
        let now = now_ts();
        Response::ok(json!({
            "provider": "wallet-provider",
            "version": PROVIDER_VERSION,
            "protocol_version": WALLET_PROTOCOL_VERSION,
            "storage_configured": self.store_path.is_some(),
            "managed_wallets_configured": self.storage_key.is_some(),
            "active_account_count": self.store.accounts.iter().filter(|account| account.revoked_at.is_none()).count(),
            "managed_account_count": self.store.accounts.iter().filter(|account| account.revoked_at.is_none() && is_managed_proof_type(&account.proof_type)).count(),
            "pending_challenge_count": self.store.challenges.iter().filter(|stored| stored.consumed_at.is_none() && stored.challenge.expires_at > now).count(),
            "pending_bitcoin_challenge_count": self.store.bitcoin_challenges.iter().filter(|stored| stored.consumed_at.is_none() && stored.challenge.expires_at > now).count(),
            "pending_approval_count": self.store.approval_requests.iter().filter(|request| request.status == ApprovalStatus::Pending && request.expires_at > now).count(),
            "supported_operations": ["status"],
        }))
    }

    fn challenge(&mut self, input: ChallengeInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let challenge_id = random_hex(16);
        let mut resources = vec![format!("elastos://auth/challenge/{challenge_id}")];
        if input.resources.is_empty() {
            resources.push("elastos://wallet/account/link".to_string());
        } else {
            resources.extend(input.resources);
        }
        let challenge = AuthChallengeV1::new(AuthChallengeInput {
            challenge_id,
            domain: input.domain,
            uri: input.uri,
            address: input.address,
            chain_id: input.chain_id,
            nonce: random_hex(12),
            issued_at: now,
            ttl_secs: AUTH_CHALLENGE_TTL_SECS,
            resources,
        });
        self.store.challenges.push(StoredWalletChallenge {
            challenge: challenge.clone(),
            consumed_at: None,
        });
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "schema": AuthChallengeV1::SCHEMA,
            "challenge_id": challenge.challenge_id,
            "message": challenge.siwe_message(),
            "expires_at": challenge.expires_at,
            "resources": challenge.resources,
        }))
    }

    fn bitcoin_challenge(&mut self, input: BitcoinChallengeInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let challenge_id = random_hex(16);
        let mut resources = vec![format!("elastos://auth/bitcoin-challenge/{challenge_id}")];
        if input.resources.is_empty() {
            resources.push("elastos://wallet/account/link".to_string());
        } else {
            resources.extend(input.resources);
        }
        let challenge = BitcoinChallengeV1 {
            schema: BITCOIN_CHALLENGE_SCHEMA.to_string(),
            challenge_id,
            domain: input.domain,
            uri: input.uri,
            network: input.network,
            address: input.address,
            nonce: random_hex(12),
            issued_at: now,
            expires_at: now.saturating_add(AUTH_CHALLENGE_TTL_SECS),
            resources,
        };
        self.store.bitcoin_challenges.push(StoredBitcoinChallenge {
            challenge: challenge.clone(),
            consumed_at: None,
        });
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        let proof_type =
            match bitcoin_proof_type_for_address(&challenge.network, &challenge.address) {
                Ok(proof_type) => proof_type,
                Err(err) => return Response::error("invalid_request", err),
            };
        Response::ok(json!({
            "schema": BITCOIN_CHALLENGE_SCHEMA,
            "challenge_id": challenge.challenge_id,
            "message": challenge.message(),
            "expires_at": challenge.expires_at,
            "network": challenge.network,
            "address": challenge.address,
            "resources": challenge.resources,
            "proof_type": proof_type,
        }))
    }

    fn verify_proof(&mut self, message: &str, signature: &str) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let parsed = match elastos_auth::parse_siwe_message(message) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_proof", err),
        };
        let Some(challenge_id) = parsed
            .resources
            .iter()
            .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
        else {
            return Response::error("invalid_proof", "SIWE proof missing challenge resource");
        };
        let now = now_ts();
        let Some(stored) = self
            .store
            .challenges
            .iter_mut()
            .find(|stored| stored.challenge.challenge_id == challenge_id)
        else {
            return Response::error("not_found", "wallet proof challenge not found");
        };
        if stored.consumed_at.is_some() {
            return Response::error("invalid_proof", "wallet proof challenge already consumed");
        }
        if stored.challenge.expires_at <= now {
            return Response::error("invalid_proof", "wallet proof challenge expired");
        }
        let proof = match verify_siwe_challenge(&stored.challenge, message, signature, now) {
            Ok(proof) => proof,
            Err(err) => return Response::error("invalid_proof", err),
        };
        stored.consumed_at = Some(now);
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        let proof_binding_id = proof.binding.id();
        let chain_id = proof.binding.chain_id.unwrap_or_default();
        Response::ok(json!({
            "schema": "elastos.wallet.proof/v1",
            "proof_binding_id": proof_binding_id,
            "chain_namespace": format!("eip155:{chain_id}"),
            "address": proof.recovered_address,
            "proof_type": "siwe",
            "challenge_id": challenge_id,
            "verified_at": now,
            "message_hash": format!("0x{}", hex::encode(proof.message_hash)),
        }))
    }

    fn verify_bitcoin_proof(
        &mut self,
        message: &str,
        signature: &str,
        signature_type: Option<&str>,
        public_key: Option<&str>,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let Some(challenge_id) = message.lines().find_map(|line| {
            line.trim()
                .strip_prefix("- elastos://auth/bitcoin-challenge/")
        }) else {
            return Response::error("invalid_proof", "BIP-322 proof missing challenge resource");
        };
        let now = now_ts();
        let Some(stored) = self
            .store
            .bitcoin_challenges
            .iter_mut()
            .find(|stored| stored.challenge.challenge_id == challenge_id)
        else {
            return Response::error("not_found", "Bitcoin wallet proof challenge not found");
        };
        if stored.consumed_at.is_some() {
            return Response::error(
                "invalid_proof",
                "Bitcoin wallet proof challenge already consumed",
            );
        }
        if stored.challenge.expires_at <= now {
            return Response::error("invalid_proof", "Bitcoin wallet proof challenge expired");
        }
        if message != stored.challenge.message() {
            return Response::error("invalid_proof", "Bitcoin challenge message does not match");
        }
        let requested_signature_type = signature_type.unwrap_or(BITCOIN_PROOF_BIP322_SIMPLE);
        let expected_proof_type = match bitcoin_proof_type_for_address(
            &stored.challenge.network,
            &stored.challenge.address,
        ) {
            Ok(proof_type) => proof_type,
            Err(err) => return Response::error("invalid_request", err),
        };
        let proof = match verify_bitcoin_proof_for_type(
            expected_proof_type,
            requested_signature_type,
            &stored.challenge.network,
            &stored.challenge.address,
            message,
            signature,
            public_key,
        ) {
            Ok(proof) => proof,
            Err(err) => return Response::error("invalid_bitcoin_proof", err),
        };
        stored.consumed_at = Some(now);
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "schema": "elastos.wallet.proof/v1",
            "proof_binding_id": format!("proof:wallet:{}:{}", proof.chain_namespace, proof.address),
            "chain_namespace": proof.chain_namespace,
            "address": proof.address,
            "proof_type": proof.proof_type,
            "proof_strength": proof.proof_strength,
            "challenge_id": challenge_id,
            "verified_at": now,
            "message_hash": format!("0x{}", hex::encode(proof.message_hash)),
        }))
    }

    fn verify_contract_proof(
        &mut self,
        message: &str,
        signature: &str,
        erc1271_proof: &Value,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let parsed = match elastos_auth::parse_siwe_message(message) {
            Ok(parsed) => parsed,
            Err(err) => return Response::error("invalid_proof", err),
        };
        let Some(challenge_id) = parsed
            .resources
            .iter()
            .find_map(|resource| resource.strip_prefix("elastos://auth/challenge/"))
        else {
            return Response::error("invalid_proof", "SIWE proof missing challenge resource");
        };
        let now = now_ts();
        let Some(stored) = self
            .store
            .challenges
            .iter_mut()
            .find(|stored| stored.challenge.challenge_id == challenge_id)
        else {
            return Response::error("not_found", "wallet proof challenge not found");
        };
        if stored.consumed_at.is_some() {
            return Response::error("invalid_proof", "wallet proof challenge already consumed");
        }
        let message_hash =
            match validate_siwe_challenge_message(&stored.challenge, &parsed, message, now) {
                Ok(message_hash) => message_hash,
                Err(err) => return Response::error("invalid_proof", err),
            };
        if let Err(err) =
            validate_erc1271_chain_proof(erc1271_proof, &parsed, &message_hash, signature)
        {
            return Response::error("invalid_contract_proof", err);
        }
        stored.consumed_at = Some(now);
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        let binding = ProofBinding::evm_account(parsed.chain_id, &parsed.address, now);
        let proof_binding_id = binding.id();
        Response::ok(json!({
            "schema": "elastos.wallet.proof/v1",
            "proof_binding_id": proof_binding_id,
            "chain_namespace": format!("eip155:{}", parsed.chain_id),
            "address": parsed.address,
            "proof_type": "siwe_erc1271",
            "challenge_id": challenge_id,
            "verified_at": now,
            "message_hash": format!("0x{}", hex::encode(message_hash)),
        }))
    }

    fn encrypt_managed_key(
        &self,
        account_id: &str,
        principal_id: &str,
        chain_namespace: &str,
        address: &str,
        private_key: &[u8],
        created_at: u64,
    ) -> Result<ManagedWalletSecret, String> {
        let key = self
            .managed_storage_key(principal_id)
            .map_err(|err| err.to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|err| err.to_string())?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let aad = managed_key_aad(account_id, principal_id, chain_namespace, address);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: private_key,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                "managed wallet key could not be decrypted; recover or recreate this account"
                    .to_string()
            })?;
        Ok(ManagedWalletSecret {
            schema: "elastos.wallet.managed_secret/v1".to_string(),
            account_id: account_id.to_string(),
            principal_id: principal_id.to_string(),
            chain_namespace: chain_namespace.to_string(),
            address: address.to_string(),
            key_algorithm: "secp256k1".to_string(),
            cipher: "aes-256-gcm".to_string(),
            nonce: hex::encode(&nonce[..]),
            ciphertext: hex::encode(ciphertext),
            created_at,
            revoked_at: None,
        })
    }

    fn decrypt_managed_key(&self, secret: &ManagedWalletSecret) -> Result<SigningKey, String> {
        if secret.key_algorithm != "secp256k1" || secret.cipher != "aes-256-gcm" {
            return Err("unsupported managed wallet key envelope".to_string());
        }
        let key = self.managed_storage_key(&secret.principal_id)?;
        let nonce_bytes = hex::decode(&secret.nonce).map_err(|err| err.to_string())?;
        if nonce_bytes.len() != 12 {
            return Err("managed wallet nonce must be 12 bytes".to_string());
        }
        let nonce_array: [u8; 12] = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| "managed wallet nonce must be 12 bytes".to_string())?;
        let nonce = Nonce::from(nonce_array);
        let ciphertext = hex::decode(&secret.ciphertext).map_err(|err| err.to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|err| err.to_string())?;
        let aad = managed_key_aad(
            &secret.account_id,
            &secret.principal_id,
            &secret.chain_namespace,
            &secret.address,
        );
        let private_key = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext.as_slice(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| {
                "managed wallet key could not be decrypted; recover or recreate this account"
                    .to_string()
            })?;
        SigningKey::from_slice(&private_key).map_err(|err| err.to_string())
    }

    fn managed_signing_key_for_account(
        &self,
        account: &LinkedAccount,
    ) -> Result<SigningKey, String> {
        if account.revoked_at.is_some() {
            return Err("managed wallet account is revoked".to_string());
        }
        if account.connector_id.is_some() {
            return Err("managed wallet account cannot carry connector authority".to_string());
        }
        let expected_proof_type = managed_proof_type(&account.chain_namespace)?;
        if account.proof_type != expected_proof_type {
            return Err("managed wallet account proof type does not match its chain".to_string());
        }
        let canonical_address = |address: &str| -> Result<String, String> {
            if expected_proof_type == MANAGED_EVM_PROOF_TYPE {
                validate_evm_address(address)?;
                Ok(normalize_evm_address(address))
            } else {
                Ok(address.to_string())
            }
        };
        let account_address = canonical_address(&account.address)?;
        let mut matching_secrets = self.store.managed_wallets.iter().filter(|secret| {
            secret.revoked_at.is_none()
                && secret.principal_id == account.principal_id
                && secret.account_id == account.account_id
        });
        let secret = matching_secrets
            .next()
            .ok_or_else(|| "managed wallet key not found".to_string())?;
        if matching_secrets.next().is_some() {
            return Err("managed wallet account has multiple active signing keys".to_string());
        }
        if secret.schema != "elastos.wallet.managed_secret/v1"
            || secret.principal_id != account.principal_id
            || secret.account_id != account.account_id
            || secret.chain_namespace != account.chain_namespace
        {
            return Err("managed wallet key metadata does not match its account".to_string());
        }
        let secret_address = canonical_address(&secret.address)?;
        if secret_address != account_address {
            return Err("managed wallet key address does not match its account".to_string());
        }
        let signing_key = self.decrypt_managed_key(secret)?;
        let derived_address = canonical_address(&managed_address_for_signing_key(
            &signing_key,
            &account.chain_namespace,
        )?)?;
        if derived_address != account_address || derived_address != secret_address {
            return Err("managed wallet key derives a different account address".to_string());
        }
        let expected_account_id = account_id(&account.chain_namespace, &derived_address);
        if account.account_id != expected_account_id || secret.account_id != expected_account_id {
            return Err("managed wallet account id does not match its key authority".to_string());
        }
        let expected_proof_binding_id = format!(
            "proof:wallet:managed:{}:{}",
            account.chain_namespace, derived_address
        );
        if account.proof_binding_id != expected_proof_binding_id {
            return Err(
                "managed wallet proof binding does not match its key authority".to_string(),
            );
        }
        Ok(signing_key)
    }

    fn managed_storage_key(&self, principal_id: &str) -> Result<[u8; 32], String> {
        let storage_key = self
            .storage_key
            .as_ref()
            .ok_or_else(|| "managed wallet storage key is not initialized".to_string())?;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"elastos.wallet.managed-key.v1");
        hasher.update(storage_key);
        hasher.update(principal_id.as_bytes());
        Ok(hasher.finalize().into())
    }

    fn save(&self) -> Result<(), String> {
        if self.defer_save {
            return Ok(());
        }
        let Some(path) = &self.store_path else {
            return Err("wallet-provider is not initialized".to_string());
        };
        save_store(path, &self.store)
    }

    fn recover_store_after_save_failure(&mut self, previous_store: WalletStore) {
        let persisted_store = self
            .store_path
            .as_deref()
            .and_then(|path| load_store_if_present(path).ok().flatten());
        self.store = persisted_store.unwrap_or(previous_store);
    }

    fn account_for_signature(
        &self,
        input: &SignatureRequestInput,
    ) -> Result<LinkedAccount, Response> {
        let account = self
            .active_account(&input.principal_id, &input.account_id)
            .cloned()
            .ok_or_else(|| Response::error("not_found", "active linked account not found"))?;
        if !chain_namespaces_compatible(&account.chain_namespace, &input.chain_namespace) {
            return Err(Response::error(
                "invalid_request",
                "wallet account does not match requested chain_namespace",
            ));
        }
        Ok(account)
    }

    fn ensure_managed_account_can_sign(&self, account: &LinkedAccount) -> Result<(), Response> {
        if !is_managed_proof_type(&account.proof_type) {
            return Ok(());
        }
        self.managed_signing_key_for_account(account)
            .map(|_| ())
            .map_err(|err| {
                let code = if err == "managed wallet key not found" {
                    "managed_key_missing"
                } else {
                    "managed_key_unavailable"
                };
                Response::error(code, err)
            })
    }

    fn ensure_initialized(&self) -> Result<(), Response> {
        if self.store_path.is_some() && self.storage_key.is_some() {
            Ok(())
        } else {
            Err(Response::error(
                "not_initialized",
                "wallet-provider is not initialized",
            ))
        }
    }

    fn validate_bitcoin_challenge_for_signing(
        &self,
        payload: &Value,
        account: &LinkedAccount,
        now: u64,
    ) -> Result<(), String> {
        validate_bitcoin_bip322_proof_payload(payload, account)?;
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| "Bitcoin BIP-322 payload missing message".to_string())?;
        let challenge_id = bitcoin_challenge_id_from_message(message)
            .ok_or_else(|| "Bitcoin BIP-322 message missing Runtime challenge".to_string())?;
        let stored = self
            .store
            .bitcoin_challenges
            .iter()
            .find(|stored| stored.challenge.challenge_id == challenge_id)
            .ok_or_else(|| "Bitcoin proof challenge not found".to_string())?;
        if stored.consumed_at.is_some() {
            return Err("Bitcoin proof challenge already consumed".to_string());
        }
        if stored.challenge.expires_at <= now {
            return Err("Bitcoin proof challenge expired".to_string());
        }
        if stored.challenge.address != account.address {
            return Err("Bitcoin proof challenge address does not match account".to_string());
        }
        if stored.challenge.network != "bitcoin" {
            return Err("Bitcoin proof challenge network does not match account".to_string());
        }
        if stored.challenge.message() != message {
            return Err("Bitcoin proof message does not match challenge".to_string());
        }
        Ok(())
    }
}

fn response_into_wallet_result(response: Response) -> WalletResultV2 {
    match response {
        Response::Ok { data } => WalletResultV2::Ok {
            data: data.unwrap_or_else(|| json!({})),
        },
        Response::Error { code, message } => WalletResultV2::Error { code, message },
    }
}

fn write_response(response: &Response) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    serde_json::to_writer(&mut handle, response)?;
    handle.write_all(b"\n")?;
    handle.flush()
}

fn main() -> io::Result<()> {
    eprintln!(
        "wallet-provider: starting v{} (metadata authority)",
        PROVIDER_VERSION
    );
    let stdin = io::stdin();
    let mut provider = WalletProvider::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                write_response(&Response::error("read_error", err.to_string()))?;
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let (response, should_stop) = match serde_json::from_str::<Request>(&line) {
            Ok(request) => {
                let should_stop = matches!(request, Request::Shutdown);
                (provider.handle(request), should_stop)
            }
            Err(err) => (Response::error("invalid_request", err.to_string()), false),
        };
        write_response(&response)?;
        if should_stop {
            break;
        }
    }
    eprintln!("wallet-provider: exiting");
    Ok(())
}

#[cfg(test)]
mod tests;
