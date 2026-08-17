use super::*;

pub(super) struct ConnectorHandoffCompletion<'a> {
    pub(super) principal_id: &'a str,
    pub(super) session_id: &'a str,
    pub(super) request_id: &'a str,
    pub(super) connector_id: &'a str,
    pub(super) payload_hash: &'a str,
    pub(super) signature: Option<&'a str>,
    pub(super) signature_type: Option<&'a str>,
    pub(super) public_key: Option<&'a str>,
    pub(super) signer: &'a str,
    pub(super) transaction_hash: Option<&'a str>,
}

impl WalletProvider {
    pub(super) fn idempotent_transaction_effect_replay(
        &self,
        wallet_request: &WalletProviderRequestV2,
    ) -> Option<Response> {
        match &wallet_request.operation {
            WalletProviderOperationV2::RequestApproval { .. } => {
                let existing = self
                    .store
                    .approval_requests
                    .iter()
                    .find(|request| request.request_id == wallet_request.request_id)?;
                let authority_binding = wallet_authority_binding(&wallet_request.authority);
                if existing.principal_id != wallet_request.authority.principal_id
                    || existing.wallet_request_sha256 != wallet_request.request_sha256
                    || existing.authority_binding != authority_binding
                {
                    return Some(Response::error(
                        "approval_identity_conflict",
                        "Wallet approval identity was reused with substituted semantics or authority",
                    ));
                }
                Some(Response::ok(json!({
                    "approval_request": existing,
                    "requires_approval": existing.status == ApprovalStatus::Pending,
                    "signature": Value::Null,
                })))
            }
            WalletProviderOperationV2::AttachValidatedChainOutcome { outcome } => {
                let existing = self
                    .store
                    .approval_requests
                    .iter()
                    .find(|request| request.request_id == outcome.approval_request_id)?;
                if let Err(err) =
                    validate_chain_outcome_target(existing, &wallet_request.authority, outcome)
                {
                    return Some(Response::error("chain_outcome_conflict", err));
                }
                existing.validated_chain_outcome.as_ref().map(|stored| {
                    if stored != outcome {
                        Response::error(
                            "chain_outcome_conflict",
                            "validated Chain outcome substitution was rejected",
                        )
                    } else {
                        Response::ok(json!({ "approval_request": existing }))
                    }
                })
            }
            _ => None,
        }
    }

    pub(super) fn request_approval(&mut self, input: SignatureRequestInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let now = now_ts();
        if let Err(err) = input.validate(now) {
            return Response::error("invalid_request", err);
        }
        let previous_store = self.store.clone();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let account = match self.account_for_signature(&input) {
            Ok(account) => account,
            Err(response) => return response,
        };
        if let Err(response) = self.ensure_managed_account_can_sign(&account) {
            return response;
        }
        if is_managed_proof_type(&account.proof_type)
            && !managed_signing_intent_is_supported(&input.intent)
        {
            return Response::error(
                "unsupported_managed_signing_intent",
                "managed signing is implemented only for Browser account access, personal_sign, typed data, transaction, and Bitcoin BIP-322 intents",
            );
        }
        if account.chain_namespace == BITCOIN_MAINNET_CHAIN_NAMESPACE
            && input.intent != "bitcoin_bip322_proof"
        {
            return Response::error(
                "invalid_request",
                "Bitcoin accounts only support bitcoin_bip322_proof signing",
            );
        }
        if input.intent == "transaction_intent" {
            if let Err(err) = validate_eip155_transaction_intent_payload(&input.payload, &account) {
                return Response::error("invalid_transaction_intent", err);
            }
        }
        if input.intent == "browser_personal_sign" {
            if let Err(err) = validate_browser_personal_sign_payload(&input.payload, &account) {
                return Response::error("invalid_browser_personal_sign", err);
            }
        }
        if input.intent == "browser_typed_data_sign" {
            if let Err(err) = validate_browser_typed_data_sign_payload(&input.payload, &account) {
                return Response::error("invalid_browser_typed_data_sign", err);
            }
        }
        if input.intent == "browser_account_access" {
            if let Err(err) = validate_browser_account_access_payload(
                &input.payload,
                &account,
                &input.chain_namespace,
                &input.session_id,
                &input.launch_id,
                input.proof_binding_id.as_deref(),
                &input.requested_by_actor,
                now,
            ) {
                return Response::error("invalid_browser_account_access", err);
            }
        }
        if input.intent == "bitcoin_bip322_proof" {
            if account.chain_namespace != BITCOIN_MAINNET_CHAIN_NAMESPACE
                || !matches!(
                    account.proof_type.as_str(),
                    MANAGED_BTC_P2WPKH_PROOF_TYPE | "bip322_simple" | "bitcoin_signed_message"
                )
            {
                return Response::error(
                    "invalid_request",
                    "Bitcoin proof signing requires a supported Bitcoin account",
                );
            }
            if let Err(err) =
                self.validate_bitcoin_challenge_for_signing(&input.payload, &account, now)
            {
                return Response::error("invalid_bitcoin_bip322_proof", err);
            }
        }

        let active_request_count = self
            .store
            .approval_requests
            .iter()
            .filter(|request| request.principal_id == input.principal_id)
            .filter(|request| approval_authority_is_active(request, now))
            .count();
        if active_request_count >= MAX_ACTIVE_APPROVAL_REQUESTS_PER_PRINCIPAL {
            return Response::error(
                "approval_limit_reached",
                "too many active wallet approval requests for this principal",
            );
        }

        let request = WalletApprovalRequest {
            schema: "elastos.wallet.approval_request/v1".to_string(),
            request_id: input.request_id,
            wallet_request_sha256: input.wallet_request_sha256,
            authority_binding: input.authority_binding,
            kind: "signature".to_string(),
            status: ApprovalStatus::Pending,
            principal_id: input.principal_id,
            account_id: account.account_id,
            proof_binding_id: account.proof_binding_id,
            chain_namespace: input.chain_namespace,
            address: account.address,
            proof_type: account.proof_type,
            connector_id: account.connector_id,
            intent: input.intent,
            session_id: input.session_id,
            launch_id: input.launch_id,
            requested_by_actor: input.requested_by_actor,
            resource: input.resource,
            reason: input.reason,
            payload_hash: value_hash(&input.payload),
            payload: input.payload,
            created_at: now,
            expires_at: input.expires_at,
            resolved_at: None,
            rejection_reason: None,
            approved_at: None,
            approval_reason: None,
            completed_at: None,
            signature_receipt: None,
            signed_result: None,
            validated_chain_outcome: None,
        };
        self.store.approval_requests.push(request.clone());
        if let Err(err) = self.save() {
            self.recover_store_after_save_failure(previous_store);
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "approval_request": request,
            "requires_approval": true,
            "signature": Value::Null,
        }))
    }

    pub(super) fn attach_validated_chain_outcome(
        &mut self,
        authority: &WalletAuthorityV2,
        outcome: &ValidatedChainOutcomeV1,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let Some(request_index) = self
            .store
            .approval_requests
            .iter()
            .position(|request| request.request_id == outcome.approval_request_id)
        else {
            return Response::error("not_found", "wallet approval request not found");
        };
        let request = self.store.approval_requests[request_index].clone();
        if let Err(err) = validate_chain_outcome_target(&request, authority, outcome) {
            return Response::error("chain_outcome_conflict", err);
        }
        if let Some(stored) = request.validated_chain_outcome.as_ref() {
            return if stored == outcome {
                Response::ok(json!({ "approval_request": request }))
            } else {
                Response::error(
                    "chain_outcome_conflict",
                    "validated Chain outcome substitution was rejected",
                )
            };
        }
        self.store.approval_requests[request_index].validated_chain_outcome = Some(outcome.clone());
        let request = self.store.approval_requests[request_index].clone();
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "approval_request": request }))
    }

    pub(super) fn approval_requests(
        &mut self,
        principal_id: &str,
        include_resolved: bool,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        let store = prune_store(self.store.clone(), now);
        let requests = store
            .approval_requests
            .iter()
            .filter(|request| request.principal_id == principal_id)
            .filter(|request| include_resolved || request.status == ApprovalStatus::Pending)
            .collect::<Vec<_>>();
        Response::ok(json!({ "approval_requests": requests }))
    }

    pub(super) fn reject_approval(
        &mut self,
        principal_id: &str,
        session_id: &str,
        actor: &str,
        request_id: &str,
        reason: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(session_id, "session_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(actor, "actor") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(request_id, "request_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_reason(reason) {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let Some(request) = self.store.approval_requests.iter_mut().find(|request| {
            request.principal_id == principal_id && request.request_id == request_id
        }) else {
            return Response::error("not_found", "wallet approval request not found");
        };
        if request.session_id != session_id {
            return Response::error(
                "invalid_request",
                "wallet approval request belongs to a different Runtime session",
            );
        }
        if actor.starts_with("wallet-") && request.connector_id.as_deref() != Some(actor) {
            return Response::error(
                "invalid_request",
                "wallet approval request belongs to a different connector",
            );
        }
        if request.status != ApprovalStatus::Pending {
            return Response::error("invalid_request", "wallet approval request is not pending");
        }
        request.status = ApprovalStatus::Rejected;
        request.resolved_at = Some(now);
        request.rejection_reason = Some(reason.trim().to_string());
        let request = request.clone();
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "approval_request": request }))
    }

    pub(super) fn approve_connector_handoff(
        &mut self,
        principal_id: &str,
        session_id: &str,
        actor: &str,
        request_id: &str,
        reason: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(session_id, "session_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(actor, "actor") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(request_id, "request_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_reason(reason) {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let Some(request) = self.store.approval_requests.iter_mut().find(|request| {
            request.principal_id == principal_id && request.request_id == request_id
        }) else {
            return Response::error("not_found", "wallet approval request not found");
        };
        if request.session_id != session_id {
            return Response::error(
                "invalid_request",
                "wallet approval request belongs to a different Runtime session",
            );
        }
        if request.connector_id.as_deref() != Some(actor)
            || is_managed_proof_type(&request.proof_type)
        {
            return Response::error(
                "invalid_request",
                "wallet connector handoff authority does not match the approval",
            );
        }
        if request.status != ApprovalStatus::Pending {
            return Response::error("invalid_request", "wallet approval request is not pending");
        }
        request.status = ApprovalStatus::Approved;
        request.approved_at = Some(now);
        request.approval_reason = Some(reason.trim().to_string());
        let request = request.clone();
        let handoff = match external_wallet_handoff(&request) {
            Ok(handoff) => handoff,
            Err(err) => return Response::error("invalid_request", err),
        };
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "approval_request": request,
            "handoff": handoff,
            "signature": Value::Null,
        }))
    }

    pub(super) fn approve_and_sign_managed(
        &mut self,
        principal_id: &str,
        session_id: &str,
        request_id: &str,
        reason: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(session_id, "session_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(request_id, "request_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_reason(reason) {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let Some(request) = self.store.approval_requests.iter_mut().find(|request| {
            request.principal_id == principal_id && request.request_id == request_id
        }) else {
            return Response::error("not_found", "wallet approval request not found");
        };
        expire_approval_if_elapsed(request, now);
        if request.status == ApprovalStatus::Expired {
            return Response::error("invalid_request", "wallet approval request expired");
        }
        if request.status != ApprovalStatus::Pending {
            return Response::error("invalid_request", "wallet approval request is not pending");
        }
        if request.session_id != session_id {
            return Response::error(
                "invalid_request",
                "managed Wallet approval belongs to a different Runtime session",
            );
        }
        if !is_managed_proof_type(&request.proof_type) || request.connector_id.is_some() {
            return Response::error(
                "external_wallet_required",
                "connector approvals require a typed connector handoff",
            );
        }
        request.status = ApprovalStatus::Approved;
        request.approved_at = Some(now);
        request.approval_reason = Some(reason.trim().to_string());
        self.sign_managed_approval(principal_id, request_id)
    }

    pub(super) fn complete_connector_handoff(
        &mut self,
        completion: ConnectorHandoffCompletion<'_>,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(completion.principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(completion.session_id, "session_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(completion.request_id, "request_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(completion.connector_id, "connector_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_hash(completion.payload_hash, "payload_hash") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(completion.signer, "signer") {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let Some(request_index) = self.store.approval_requests.iter().position(|request| {
            request.principal_id == completion.principal_id
                && request.request_id == completion.request_id
        }) else {
            return Response::error("not_found", "wallet approval request not found");
        };
        {
            let request = &mut self.store.approval_requests[request_index];
            expire_approval_if_elapsed(request, now);
            if request.status == ApprovalStatus::Expired {
                return Response::error("invalid_request", "wallet approval request expired");
            }
            if request.status != ApprovalStatus::Approved {
                return Response::error(
                    "invalid_request",
                    "wallet approval request must be approved before completion",
                );
            }
            if request.session_id != completion.session_id {
                return Response::error(
                    "invalid_request",
                    "wallet approval request belongs to a different Runtime session",
                );
            }
            if request.connector_id.as_deref() != Some(completion.connector_id) {
                return Response::error(
                    "invalid_request",
                    "wallet approval request belongs to a different connector",
                );
            }
            if request.payload_hash != completion.payload_hash {
                return Response::error("invalid_request", "wallet approval payload hash mismatch");
            }
            if !request.address.eq_ignore_ascii_case(completion.signer)
                && request.account_id != completion.signer
            {
                return Response::error("invalid_request", "wallet signature signer mismatch");
            }
        }
        let request_snapshot = self.store.approval_requests[request_index].clone();
        let (signature_hash, signed_result) = if request_snapshot.intent == "transaction_intent" {
            if completion.signature.is_some() {
                return Response::error(
                    "invalid_request",
                    "external transaction completion must not include signature",
                );
            }
            let Some(transaction_hash) = completion.transaction_hash else {
                return Response::error(
                    "invalid_request",
                    "external transaction completion requires transaction_hash",
                );
            };
            if let Err(err) = validate_hash(transaction_hash, "transaction_hash") {
                return Response::error("invalid_request", err);
            }
            (
                bytes_hash(transaction_hash.as_bytes()),
                Some(external_transaction_result(
                    &request_snapshot,
                    transaction_hash,
                )),
            )
        } else if request_snapshot.intent == "bitcoin_bip322_proof" {
            let Some(signature) = completion.signature else {
                return Response::error(
                    "invalid_request",
                    "external signature completion requires signature",
                );
            };
            if let Err(err) = validate_signature(signature) {
                return Response::error("invalid_request", err);
            }
            let account = LinkedAccount {
                account_id: request_snapshot.account_id.clone(),
                principal_id: request_snapshot.principal_id.clone(),
                proof_binding_id: request_snapshot.proof_binding_id.clone(),
                chain_namespace: request_snapshot.chain_namespace.clone(),
                address: request_snapshot.address.clone(),
                proof_type: request_snapshot.proof_type.clone(),
                connector_id: request_snapshot.connector_id.clone(),
                label: None,
                linked_at: request_snapshot.created_at,
                revoked_at: None,
            };
            if let Err(err) = self.validate_bitcoin_challenge_for_signing(
                &request_snapshot.payload,
                &account,
                now,
            ) {
                return Response::error("invalid_bitcoin_bip322_proof", err);
            }
            let message = match external_signature_message(&request_snapshot) {
                Ok(message) => message,
                Err(err) => return Response::error("invalid_bitcoin_bip322_proof", err),
            };
            let signature_type = completion.signature_type.unwrap_or_else(|| {
                bitcoin_signature_type_for_proof_type(&request_snapshot.proof_type)
            });
            if let Err(err) = verify_bitcoin_proof_for_type(
                request_snapshot.proof_type.as_str(),
                signature_type,
                "bitcoin",
                &request_snapshot.address,
                &message,
                signature,
                completion.public_key,
            ) {
                return Response::error("invalid_bitcoin_proof", err);
            }
            (bytes_hash(signature.as_bytes()), None)
        } else {
            let Some(signature) = completion.signature else {
                return Response::error(
                    "invalid_request",
                    "external signature completion requires signature",
                );
            };
            if let Err(err) = validate_signature(signature) {
                return Response::error("invalid_request", err);
            }
            let recovered = if request_snapshot.intent == "browser_typed_data_sign" {
                match eip712_payload_hash(&request_snapshot.payload)
                    .and_then(|hash| recover_evm_address_from_hash(&hash, signature))
                {
                    Ok(recovered) => recovered,
                    Err(err) => return Response::error("invalid_signature", err),
                }
            } else {
                let message = match external_signature_message(&request_snapshot) {
                    Ok(message) => message,
                    Err(err) => return Response::error("invalid_request", err),
                };
                if request_snapshot.intent == "browser_personal_sign"
                    || request_snapshot.intent == "ddrm_delegation_sign"
                {
                    let message = match browser_personal_sign_message_bytes(&message) {
                        Ok(message) => message,
                        Err(err) => return Response::error("invalid_request", err),
                    };
                    let hash = ethereum_signed_message_hash(&message);
                    match recover_evm_address_from_hash(&hash, signature) {
                        Ok(recovered) => recovered,
                        Err(err) => return Response::error("invalid_signature", err),
                    }
                } else {
                    match recover_evm_address(&message, signature) {
                        Ok((recovered, _)) => recovered,
                        Err(err) => return Response::error("invalid_signature", err),
                    }
                }
            };
            if normalize_evm_address(&recovered) != normalize_evm_address(&request_snapshot.address)
            {
                return Response::error("invalid_signature", "wallet signature signer mismatch");
            }
            (
                bytes_hash(signature.as_bytes()),
                if request_snapshot.intent == "browser_typed_data_sign" {
                    browser_typed_data_sign_result(&request_snapshot, signature)
                } else {
                    browser_personal_sign_result(&request_snapshot, signature)
                },
            )
        };
        let receipt = WalletSignatureReceipt {
            schema: "elastos.wallet.signature_receipt/v1".to_string(),
            request_id: request_snapshot.request_id.clone(),
            signer: completion.signer.to_string(),
            payload_hash: completion.payload_hash.to_string(),
            signature_hash,
            completed_at: now,
        };
        let request = &mut self.store.approval_requests[request_index];
        request.status = ApprovalStatus::Completed;
        request.resolved_at = Some(now);
        request.completed_at = Some(now);
        request.signature_receipt = Some(receipt.clone());
        request.signed_result = signed_result;
        let request = request.clone();
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "approval_request": request,
            "signature_receipt": receipt,
        }))
    }

    fn sign_managed_approval(&mut self, principal_id: &str, request_id: &str) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(request_id, "request_id") {
            return Response::error("invalid_request", err);
        }
        let previous_store = self.store.clone();
        let now = now_ts();
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let request = match self.store.approval_requests.iter_mut().find(|request| {
            request.principal_id == principal_id && request.request_id == request_id
        }) {
            Some(request) => {
                expire_approval_if_elapsed(request, now);
                request.clone()
            }
            None => return Response::error("not_found", "wallet approval request not found"),
        };
        if request.status == ApprovalStatus::Expired {
            return Response::error("invalid_request", "wallet approval request expired");
        }
        if request.status != ApprovalStatus::Approved {
            return Response::error(
                "invalid_request",
                "wallet approval request must be approved before managed signing",
            );
        }
        if value_hash(&request.payload) != request.payload_hash {
            return Response::error(
                "signing_error",
                "wallet approval payload no longer matches the reviewed payload hash",
            );
        }
        if !managed_signing_intent_is_supported(&request.intent) {
            return Response::error(
                "unsupported_managed_signing_intent",
                "managed signing is implemented only for Browser account access, personal_sign, typed data, transaction, and Bitcoin BIP-322 intents",
            );
        }
        let Some(account) = self.store.accounts.iter().find(|account| {
            account.principal_id == principal_id
                && account.account_id == request.account_id
                && account.revoked_at.is_none()
        }) else {
            return Response::error("not_found", "active linked account not found");
        };
        if !is_managed_proof_type(&account.proof_type) {
            return Response::error(
                "external_wallet_required",
                "approved request requires an external wallet signature handoff",
            );
        }
        if request.proof_binding_id != account.proof_binding_id
            || request.chain_namespace != account.chain_namespace
            || request.address != account.address
            || request.proof_type != account.proof_type
            || request.connector_id != account.connector_id
        {
            return Response::error(
                "signing_error",
                "wallet approval authority no longer matches its managed account",
            );
        }
        if request.intent == "browser_personal_sign" {
            if let Err(err) = validate_browser_personal_sign_payload(&request.payload, account) {
                return Response::error("invalid_browser_personal_sign", err);
            }
        }
        if request.intent == "browser_typed_data_sign" {
            if let Err(err) = validate_browser_typed_data_sign_payload(&request.payload, account) {
                return Response::error("invalid_browser_typed_data_sign", err);
            }
        }
        if request.intent == "transaction_intent" {
            if let Err(err) = validate_eip155_transaction_intent_payload(&request.payload, account)
            {
                return Response::error("invalid_transaction_intent", err);
            }
        }
        if request.intent == "bitcoin_bip322_proof" {
            if let Err(err) =
                self.validate_bitcoin_challenge_for_signing(&request.payload, account, now)
            {
                return Response::error("invalid_bitcoin_bip322_proof", err);
            }
        }
        let signing_key = match self.managed_signing_key_for_account(account) {
            Ok(signing_key) => signing_key,
            Err(err) => return Response::error("managed_key_unavailable", err),
        };
        let signed = match sign_managed_approval(&signing_key, &request) {
            Ok(signed) => signed,
            Err(err) => return Response::error("signing_error", err),
        };
        let signature_receipt = WalletSignatureReceipt {
            schema: "elastos.wallet.signature_receipt/v1".to_string(),
            request_id: request.request_id.clone(),
            signer: request.address.clone(),
            payload_hash: request.payload_hash.clone(),
            signature_hash: bytes_hash(signed.authority.as_bytes()),
            completed_at: now,
        };
        let Some(stored_request) =
            self.store.approval_requests.iter_mut().find(|stored| {
                stored.principal_id == principal_id && stored.request_id == request_id
            })
        else {
            return Response::error("not_found", "wallet approval request not found");
        };
        stored_request.status = ApprovalStatus::Completed;
        stored_request.resolved_at = Some(now);
        stored_request.completed_at = Some(now);
        stored_request.signature_receipt = Some(signature_receipt.clone());
        stored_request.signed_result = managed_signed_result(&stored_request.clone(), &signed);
        let stored_request = stored_request.clone();
        if let Err(err) = self.save() {
            self.recover_store_after_save_failure(previous_store);
            return Response::error("storage_error", err);
        }
        let mut response = json!({
            "approval_request": stored_request,
            "signature_receipt": signature_receipt,
            "signed_payload": signed.payload,
        });
        match signed.kind {
            ManagedSignatureKind::Message => {
                response["signature"] = Value::String(signed.authority);
            }
            ManagedSignatureKind::Transaction => {
                response["signed_transaction"] = Value::String(signed.authority);
            }
        }
        Response::ok(response)
    }
}

fn validate_browser_account_access_payload(
    payload: &Value,
    account: &LinkedAccount,
    requested_chain_namespace: &str,
    session_id: &str,
    launch_id: &str,
    request_proof_binding_id: Option<&str>,
    requested_by_actor: &str,
    now: u64,
) -> Result<(), String> {
    const FIELDS: &[&str] = &[
        "schema",
        "permission",
        "principal_id",
        "session_id",
        "launch_id",
        "proof_binding_id",
        "origin",
        "page_url",
        "account_id",
        "requested_chain_namespace",
        "chain_namespaces",
        "address",
        "grant_expires_at",
        "requires_wallet_approval",
    ];
    const SUPPORTED_CHAIN_NAMESPACES: &[&str] = &["eip155:20", "eip155:8453"];

    let object = payload
        .as_object()
        .ok_or_else(|| "Browser account access payload must be an object".to_string())?;
    if object.len() != FIELDS.len() || !object.keys().all(|key| FIELDS.contains(&key.as_str())) {
        return Err("Browser account access payload fields are invalid".to_string());
    }
    let text = |field: &str| {
        object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("Browser account access payload missing {field}"))
    };
    if text("schema")? != "elastos.browser.account-access-request/v1" {
        return Err("Browser account access payload schema is invalid".to_string());
    }
    if text("permission")? != "eth_accounts" {
        return Err("Browser account access permission must be eth_accounts".to_string());
    }
    if requested_by_actor != "browser" {
        return Err("Browser account access must be requested by Browser".to_string());
    }
    if !is_managed_proof_type(&account.proof_type) || account.connector_id.is_some() {
        return Err("Browser account access requires a Runtime-managed account".to_string());
    }
    if text("principal_id")? != account.principal_id {
        return Err("Browser account access principal does not match selected account".to_string());
    }
    if text("session_id")? != session_id || text("launch_id")? != launch_id {
        return Err(
            "Browser account access session or launch authority does not match".to_string(),
        );
    }
    let proof_binding_id = text("proof_binding_id")?;
    validate_opaque_id(proof_binding_id, "proof_binding_id")?;
    if Some(proof_binding_id) != request_proof_binding_id {
        return Err("Browser account access proof binding does not match authority".to_string());
    }
    if text("account_id")? != account.account_id {
        return Err("Browser account access account does not match selected account".to_string());
    }
    let requested_payload_chain = text("requested_chain_namespace")?;
    if requested_payload_chain != requested_chain_namespace
        || !chain_namespaces_compatible(&account.chain_namespace, requested_payload_chain)
    {
        return Err("Browser account access chain does not match selected account".to_string());
    }
    let chain_namespaces = object
        .get("chain_namespaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "Browser account access chain_namespaces are missing".to_string())?;
    if chain_namespaces.len() != SUPPORTED_CHAIN_NAMESPACES.len()
        || !chain_namespaces
            .iter()
            .zip(SUPPORTED_CHAIN_NAMESPACES)
            .all(|(actual, expected)| actual.as_str() == Some(*expected))
        || !SUPPORTED_CHAIN_NAMESPACES.contains(&requested_payload_chain)
        || !SUPPORTED_CHAIN_NAMESPACES
            .iter()
            .all(|namespace| chain_namespaces_compatible(&account.chain_namespace, namespace))
    {
        return Err(
            "Browser account access chain_namespaces do not match the supported network set"
                .to_string(),
        );
    }
    if !text("address")?.eq_ignore_ascii_case(&account.address) {
        return Err("Browser account access address does not match selected account".to_string());
    }
    let origin = text("origin")?;
    let page_url = text("page_url")?;
    if origin.len() > 512
        || page_url.len() > 4096
        || !(origin.starts_with("https://") || origin.starts_with("http://"))
        || !(page_url.starts_with("https://") || page_url.starts_with("http://"))
        || origin.ends_with('/')
        || origin.chars().any(char::is_whitespace)
        || page_url.chars().any(char::is_whitespace)
        || !page_url.strip_prefix(origin).is_some_and(|suffix| {
            suffix.is_empty()
                || suffix.starts_with('/')
                || suffix.starts_with('?')
                || suffix.starts_with('#')
        })
    {
        return Err("Browser account access page URL or origin is invalid".to_string());
    }
    if object
        .get("requires_wallet_approval")
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("Browser account access must require wallet approval".to_string());
    }
    let grant_expires_at = object
        .get("grant_expires_at")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Browser account access grant expiry is missing".to_string())?;
    if grant_expires_at <= now
        || grant_expires_at > now.saturating_add(MAX_BROWSER_ACCOUNT_ACCESS_TTL_SECS)
    {
        return Err(
            "Browser account access grant expiry is outside the allowed window".to_string(),
        );
    }
    Ok(())
}

fn validate_chain_outcome_target(
    request: &WalletApprovalRequest,
    authority: &WalletAuthorityV2,
    outcome: &ValidatedChainOutcomeV1,
) -> Result<(), String> {
    if request.principal_id != authority.principal_id
        || request.authority_binding != wallet_authority_binding(authority)
    {
        return Err("validated Chain outcome authority does not match the approval".to_string());
    }
    if request.request_id != outcome.approval_request_id
        || request.account_id != outcome.account_id
        || request.chain_namespace != outcome.chain_namespace
    {
        return Err("validated Chain outcome approval binding mismatch".to_string());
    }
    if request.status != ApprovalStatus::Completed || request.intent != "transaction_intent" {
        return Err(
            "validated Chain outcome requires a completed transaction approval".to_string(),
        );
    }
    let signed_result = request
        .signed_result
        .as_ref()
        .and_then(Value::as_object)
        .ok_or_else(|| "completed approval is missing its signed transaction result".to_string())?;
    if signed_result
        .get("transaction_hash")
        .and_then(Value::as_str)
        != Some(outcome.transaction_hash.as_str())
    {
        return Err("validated Chain outcome transaction hash mismatch".to_string());
    }
    if request
        .payload
        .get("network")
        .and_then(|network| network.get("id"))
        .and_then(Value::as_str)
        != Some(outcome.network.as_str())
    {
        return Err("validated Chain outcome network mismatch".to_string());
    }
    match &outcome.binding {
        ValidatedChainOutcomeBindingV1::ManagedSigned {
            signed_transaction_sha256,
        } => {
            if !is_managed_proof_type(&request.proof_type)
                || request.connector_id.is_some()
                || signed_result.get("schema").and_then(Value::as_str)
                    != Some("elastos.wallet.signed-transaction-result/v1")
            {
                return Err(
                    "validated managed Chain outcome requires a managed signed result".to_string(),
                );
            }
            let signed_transaction = signed_result
                .get("signed_transaction")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "completed approval is missing its signed transaction".to_string()
                })?;
            if canonical_signed_transaction_sha256(signed_transaction)?
                != *signed_transaction_sha256
            {
                return Err(
                    "validated Chain outcome signed transaction digest mismatch".to_string()
                );
            }
        }
        ValidatedChainOutcomeBindingV1::ExternalConnector {
            connector_id,
            originating_address,
        } => {
            if request.connector_id.as_deref() != Some(connector_id.as_str())
                || !request.address.eq_ignore_ascii_case(originating_address)
                || signed_result.get("schema").and_then(Value::as_str)
                    != Some("elastos.wallet.external-transaction-result/v1")
                || signed_result.get("signed_transaction").is_some()
            {
                return Err(
                    "validated external Chain outcome connector binding mismatch".to_string(),
                );
            }
        }
    }
    if signed_result.get("request_id").and_then(Value::as_str) != Some(request.request_id.as_str())
        || signed_result.get("method").and_then(Value::as_str) != Some("eth_sendTransaction")
        || signed_result.get("chain_namespace").and_then(Value::as_str)
            != Some(request.chain_namespace.as_str())
        || !signed_result
            .get("signer")
            .and_then(Value::as_str)
            .is_some_and(|signer| signer.eq_ignore_ascii_case(&request.address))
        || signed_result.get("payload_hash").and_then(Value::as_str)
            != Some(request.payload_hash.as_str())
    {
        return Err("validated Chain outcome signed result binding mismatch".to_string());
    }
    validate_chain_observation_binding(outcome)
}

fn canonical_signed_transaction_sha256(signed_transaction: &str) -> Result<String, String> {
    let encoded = signed_transaction
        .strip_prefix("0x")
        .ok_or_else(|| "signed transaction must be 0x-prefixed".to_string())?;
    let bytes =
        hex::decode(encoded).map_err(|_| "signed transaction must be hexadecimal".to_string())?;
    if bytes.is_empty() {
        return Err("signed transaction must not be empty".to_string());
    }
    Ok(format!(
        "sha256:{}",
        hex::encode(sha2::Sha256::digest(bytes))
    ))
}

fn validate_chain_observation_binding(outcome: &ValidatedChainOutcomeV1) -> Result<(), String> {
    let observation = outcome
        .chain_observation
        .as_object()
        .ok_or_else(|| "validated Chain observation must be an object".to_string())?;
    if observation.get("network").and_then(Value::as_str) != Some(outcome.network.as_str()) {
        return Err("validated Chain observation network mismatch".to_string());
    }
    let outer_hash = observation
        .get("transaction_hash")
        .or_else(|| observation.get("hash"))
        .and_then(Value::as_str)
        .ok_or_else(|| "validated Chain observation is missing its transaction hash".to_string())?;
    if outer_hash != outcome.transaction_hash {
        return Err("validated Chain observation transaction hash mismatch".to_string());
    }
    if let Some(nested_hash) = observation
        .get("receipt")
        .and_then(|receipt| receipt.get("transactionHash"))
        .and_then(Value::as_str)
    {
        if nested_hash != outcome.transaction_hash {
            return Err("validated Chain receipt payload hash mismatch".to_string());
        }
    }
    if let Some(nested_hash) = observation
        .get("transaction")
        .and_then(|transaction| transaction.get("hash"))
        .and_then(Value::as_str)
    {
        if nested_hash != outcome.transaction_hash {
            return Err("validated Chain transaction payload hash mismatch".to_string());
        }
    }
    if let ValidatedChainOutcomeBindingV1::ExternalConnector {
        originating_address,
        ..
    } = &outcome.binding
    {
        let observed_from = observation
            .get("transaction")
            .and_then(Value::as_object)
            .and_then(|transaction| transaction.get("from"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "validated external Chain observation is missing originating account".to_string()
            })?;
        if !observed_from.eq_ignore_ascii_case(originating_address) {
            return Err(
                "validated external Chain observation originating account mismatch".to_string(),
            );
        }
    }
    Ok(())
}
