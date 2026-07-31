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
    pub(super) fn request_approval(&mut self, input: SignatureRequestInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        let now = now_ts();
        if let Err(err) = input.validate(now) {
            return Response::error("invalid_request", err);
        }
        self.store = prune_store(std::mem::take(&mut self.store), now);
        let account = match self.account_for_signature(&input) {
            Ok(account) => account,
            Err(response) => return response,
        };
        if let Err(response) = self.ensure_managed_account_can_sign(&account) {
            return response;
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

        let request = WalletApprovalRequest {
            schema: "elastos.wallet.approval_request/v1".to_string(),
            request_id: format!("wallet-approval:{}", random_hex(16)),
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
        };
        self.store.approval_requests.push(request.clone());
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "approval_request": request,
            "requires_approval": true,
            "signature": Value::Null,
        }))
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
                if request_snapshot.intent == "browser_personal_sign" {
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
        if request.intent == "bitcoin_bip322_proof" {
            if let Err(err) =
                self.validate_bitcoin_challenge_for_signing(&request.payload, account, now)
            {
                return Response::error("invalid_bitcoin_bip322_proof", err);
            }
        }
        let Some(secret) = self.store.managed_wallets.iter().find(|secret| {
            secret.principal_id == principal_id
                && secret.account_id == request.account_id
                && secret.revoked_at.is_none()
        }) else {
            return Response::error("not_found", "managed wallet key not found");
        };
        let signing_key = match self.decrypt_managed_key(secret) {
            Ok(signing_key) => signing_key,
            Err(err) => return Response::error("storage_error", err),
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
