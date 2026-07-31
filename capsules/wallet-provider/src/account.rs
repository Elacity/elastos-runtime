use super::*;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;
use serde_json::{json, Value};

impl WalletProvider {
    pub(super) fn accounts(&self, principal_id: &str, include_revoked: bool) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        let accounts = self
            .store
            .accounts
            .iter()
            .filter(|account| account.principal_id == principal_id)
            .filter(|account| include_revoked || account.revoked_at.is_none())
            .map(|account| self.account_summary(account))
            .collect::<Vec<_>>();
        let default_accounts = self
            .store
            .default_accounts
            .iter()
            .filter(|default| default.principal_id == principal_id)
            .filter(|default| {
                self.active_account(&default.principal_id, &default.account_id)
                    .map(|account| account.chain_namespace == default.chain_namespace)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        Response::ok(json!({
            "accounts": accounts,
            "default_accounts": default_accounts,
        }))
    }

    fn account_summary(&self, account: &LinkedAccount) -> Value {
        let (signing_available, signing_status) = self.account_signing_status(account);
        json!({
            "account_id": account.account_id,
            "principal_id": account.principal_id,
            "proof_binding_id": account.proof_binding_id,
            "chain_namespace": account.chain_namespace,
            "address": account.address,
            "proof_type": account.proof_type,
            "connector_id": account.connector_id,
            "label": account.label,
            "linked_at": account.linked_at,
            "revoked_at": account.revoked_at,
            "signing_available": signing_available,
            "signing_status": signing_status,
        })
    }

    fn account_signing_status(&self, account: &LinkedAccount) -> (bool, &'static str) {
        if !is_managed_proof_type(&account.proof_type) {
            return if account.connector_id.is_some() {
                (true, "external_connector_available")
            } else {
                (false, "external_connector_required")
            };
        }
        let Some(secret) = self.store.managed_wallets.iter().find(|secret| {
            secret.principal_id == account.principal_id
                && secret.account_id == account.account_id
                && secret.revoked_at.is_none()
        }) else {
            return (false, "managed_key_missing");
        };
        match self.decrypt_managed_key(secret) {
            Ok(_) => (true, "managed_key_available"),
            Err(_) => (false, "managed_key_unavailable"),
        }
    }

    pub(super) fn create_managed_account(&mut self, input: CreateManagedAccountInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        if self.storage_key.is_none() {
            return Response::error(
                "not_initialized",
                "wallet-provider managed wallet storage is not initialized",
            );
        }
        let proof_type = match managed_proof_type(&input.chain_namespace) {
            Ok(proof_type) => proof_type,
            Err(err) => return Response::error("invalid_request", err),
        };
        let key_scope = match managed_key_scope(&input.chain_namespace) {
            Ok(scope) => scope,
            Err(err) => return Response::error("invalid_request", err),
        };
        if !input.create_new {
            if let Some(existing) = self.store.accounts.iter().find(|account| {
                account.principal_id == input.principal_id
                    && account.chain_namespace == input.chain_namespace
                    && account.proof_type == proof_type
                    && account.revoked_at.is_none()
            }) {
                let (signing_available, _) = self.account_signing_status(existing);
                if signing_available {
                    return Response::ok(json!({ "account": existing, "created": false }));
                }
            }
        }

        let now = now_ts();
        let signing_key = if input.create_new {
            SigningKey::random(&mut OsRng)
        } else {
            self.store
                .managed_wallets
                .iter()
                .filter(|secret| {
                    secret.principal_id == input.principal_id
                        && secret.revoked_at.is_none()
                        && managed_key_scope(&secret.chain_namespace)
                            .map(|scope| scope == key_scope)
                            .unwrap_or(false)
                })
                .find_map(|secret| self.decrypt_managed_key(secret).ok())
                .unwrap_or_else(|| SigningKey::random(&mut OsRng))
        };
        let address = match managed_address_for_signing_key(&signing_key, &input.chain_namespace) {
            Ok(address) => address,
            Err(err) => return Response::error("invalid_request", err),
        };
        let account_id = account_id(&input.chain_namespace, &address);
        let private_key_bytes = signing_key.to_bytes();
        let secret = match self.encrypt_managed_key(
            &account_id,
            &input.principal_id,
            &input.chain_namespace,
            &address,
            private_key_bytes.as_ref(),
            now,
        ) {
            Ok(secret) => secret,
            Err(err) => return Response::error("storage_error", err),
        };
        let account = LinkedAccount {
            account_id: account_id.clone(),
            principal_id: input.principal_id,
            proof_binding_id: format!("proof:wallet:managed:{}:{}", input.chain_namespace, address),
            chain_namespace: input.chain_namespace,
            address,
            proof_type: proof_type.to_string(),
            connector_id: None,
            label: input.label.filter(|label| !label.trim().is_empty()),
            linked_at: now,
            revoked_at: None,
        };
        self.store.managed_wallets.push(secret);
        self.store.accounts.push(account.clone());
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "account": account, "created": true }))
    }

    pub(super) fn import_managed_recovery_key(
        &mut self,
        input: ImportManagedRecoveryKeyInput,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        if self.storage_key.is_none() {
            return Response::error(
                "not_initialized",
                "wallet-provider managed wallet storage is not initialized",
            );
        }

        let recovery_key = input.recovery_key;
        let schema = recovery_key
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema != "elastos.wallet.recovery-key/v1" {
            return Response::error("invalid_request", "expected elastos.wallet.recovery-key/v1");
        }
        let account_id_value = match recovery_key.get("account_id").and_then(Value::as_str) {
            Some(value) => value,
            None => return Response::error("invalid_request", "recovery key missing account_id"),
        };
        let chain_namespace = match recovery_key.get("chain_namespace").and_then(Value::as_str) {
            Some(value) => value,
            None => {
                return Response::error("invalid_request", "recovery key missing chain_namespace")
            }
        };
        let address = match recovery_key.get("address").and_then(Value::as_str) {
            Some(value) => value,
            None => return Response::error("invalid_request", "recovery key missing address"),
        };
        let secret_type = recovery_key
            .get("secret_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if secret_type != "secp256k1_private_key_hex" {
            return Response::error("invalid_request", "unsupported wallet recovery key type");
        }
        if let Err(err) = validate_managed_chain_namespace(chain_namespace) {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(account_id_value, "account_id") {
            return Response::error("invalid_request", err);
        }
        let normalized_address = if chain_namespace.starts_with("eip155:") {
            normalize_evm_address(address)
        } else {
            address.to_string()
        };
        let expected_account_id = account_id(chain_namespace, &normalized_address);
        if account_id_value != expected_account_id {
            return Response::error(
                "invalid_request",
                "recovery key account_id does not match chain and address",
            );
        }
        let private_key_hex = match recovery_key.get("private_key_hex").and_then(Value::as_str) {
            Some(value) => value.trim(),
            None => {
                return Response::error("invalid_request", "recovery key missing private_key_hex")
            }
        };
        let private_key = match hex::decode(private_key_hex) {
            Ok(value) if value.len() == 32 => value,
            _ => return Response::error("invalid_request", "private_key_hex must be 32 bytes"),
        };
        let signing_key = match SigningKey::from_slice(&private_key) {
            Ok(signing_key) => signing_key,
            Err(_) => return Response::error("invalid_request", "invalid secp256k1 private key"),
        };
        let derived_address = match managed_address_for_signing_key(&signing_key, chain_namespace) {
            Ok(value) => value,
            Err(err) => return Response::error("invalid_request", err),
        };
        if !derived_address.eq_ignore_ascii_case(&normalized_address) {
            return Response::error(
                "invalid_request",
                "private key does not match recovery key address",
            );
        }

        let now = now_ts();
        let proof_type = match managed_proof_type(chain_namespace) {
            Ok(proof_type) => proof_type,
            Err(err) => return Response::error("invalid_request", err),
        };
        let secret = match self.encrypt_managed_key(
            &expected_account_id,
            &input.principal_id,
            chain_namespace,
            &normalized_address,
            signing_key.to_bytes().as_ref(),
            now,
        ) {
            Ok(secret) => secret,
            Err(err) => return Response::error("storage_error", err),
        };
        for secret in self.store.managed_wallets.iter_mut().filter(|secret| {
            secret.principal_id == input.principal_id
                && secret.account_id == expected_account_id
                && secret.revoked_at.is_none()
        }) {
            secret.revoked_at = Some(now);
        }
        self.store.managed_wallets.push(secret);

        let label = input.label.filter(|label| !label.trim().is_empty());
        let linked_at = self
            .store
            .accounts
            .iter()
            .find(|account| {
                account.principal_id == input.principal_id
                    && account.account_id == expected_account_id
            })
            .map(|account| account.linked_at)
            .unwrap_or(now);
        let account = LinkedAccount {
            account_id: expected_account_id.clone(),
            principal_id: input.principal_id,
            proof_binding_id: format!(
                "proof:wallet:managed:{}:{}",
                chain_namespace, normalized_address
            ),
            chain_namespace: chain_namespace.to_string(),
            address: normalized_address,
            proof_type: proof_type.to_string(),
            connector_id: None,
            label,
            linked_at,
            revoked_at: None,
        };
        if let Some(existing) = self.store.accounts.iter_mut().find(|existing| {
            existing.principal_id == account.principal_id
                && existing.account_id == expected_account_id
        }) {
            *existing = account.clone();
        } else {
            self.store.accounts.push(account.clone());
        }
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "account": account, "imported": true }))
    }

    pub(super) fn link_account(&mut self, input: LinkAccountInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        let account_id = account_id(&input.chain_namespace, &input.address);
        let account = LinkedAccount {
            account_id: account_id.clone(),
            principal_id: input.principal_id,
            proof_binding_id: input.proof_binding_id,
            chain_namespace: input.chain_namespace,
            address: input.address,
            proof_type: input.proof_type,
            connector_id: input.connector_id,
            label: input.label.filter(|label| !label.trim().is_empty()),
            linked_at: now,
            revoked_at: None,
        };

        if let Some(existing) = self.store.accounts.iter_mut().find(|existing| {
            existing.principal_id == account.principal_id && existing.account_id == account_id
        }) {
            *existing = account.clone();
        } else {
            self.store.accounts.push(account.clone());
        }
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "account": account }))
    }

    pub(super) fn revoke_account(&mut self, principal_id: &str, account_id: &str) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(account_id, "account_id") {
            return Response::error("invalid_request", err);
        }
        let now = now_ts();
        let Some(account) = self.store.accounts.iter_mut().find(|account| {
            account.principal_id == principal_id && account.account_id == account_id
        }) else {
            return Response::error("not_found", "linked account not found");
        };
        account.revoked_at = Some(now);
        let account = account.clone();
        for secret in
            self.store.managed_wallets.iter_mut().filter(|secret| {
                secret.account_id == account_id && secret.principal_id == principal_id
            })
        {
            secret.revoked_at = Some(now);
        }
        self.store.default_accounts.retain(|default| {
            !(default.principal_id == principal_id && default.account_id == account_id)
        });
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "account": account }))
    }

    pub(super) fn rename_account(
        &mut self,
        principal_id: &str,
        account_id: &str,
        label: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(account_id, "account_id") {
            return Response::error("invalid_request", err);
        }
        let label = label.trim();
        if label.is_empty() {
            return Response::error("invalid_request", "label is required");
        }
        if let Err(err) = validate_label(label) {
            return Response::error("invalid_request", err);
        }
        let Some(account) = self.store.accounts.iter_mut().find(|account| {
            account.principal_id == principal_id
                && account.account_id == account_id
                && account.revoked_at.is_none()
        }) else {
            return Response::error("not_found", "active linked account not found");
        };
        account.label = Some(label.to_string());
        let account = account.clone();
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({ "account": account }))
    }

    pub(super) fn export_managed_recovery_key(
        &self,
        principal_id: &str,
        account_id: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_opaque_id(principal_id, "principal_id") {
            return Response::error("invalid_request", err);
        }
        if let Err(err) = validate_opaque_id(account_id, "account_id") {
            return Response::error("invalid_request", err);
        }
        let Some(account) = self.store.accounts.iter().find(|account| {
            account.principal_id == principal_id
                && account.account_id == account_id
                && account.revoked_at.is_none()
        }) else {
            return Response::error("not_found", "active linked account not found");
        };
        if !is_managed_proof_type(&account.proof_type) {
            return Response::error(
                "external_wallet_required",
                "recovery key is available only for passkey-managed accounts",
            );
        }
        let Some(secret) = self.store.managed_wallets.iter().find(|secret| {
            secret.principal_id == principal_id
                && secret.account_id == account_id
                && secret.revoked_at.is_none()
        }) else {
            return Response::error("not_found", "managed wallet key not found");
        };
        let signing_key = match self.decrypt_managed_key(secret) {
            Ok(signing_key) => signing_key,
            Err(err) => return Response::error("storage_error", err),
        };
        Response::ok(json!({
            "schema": "elastos.wallet.recovery-key/v1",
            "account_id": account.account_id,
            "chain_namespace": account.chain_namespace,
            "address": account.address,
            "secret_type": "secp256k1_private_key_hex",
            "private_key_hex": hex::encode(signing_key.to_bytes()),
            "note": "This account was created as an encrypted signing key, not a BIP39 seed phrase."
        }))
    }

    pub(super) fn set_default_account(&mut self, input: SetDefaultAccountInput) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = input.validate() {
            return Response::error("invalid_request", err);
        }
        let Some(account) = self
            .active_account(&input.principal_id, &input.account_id)
            .cloned()
        else {
            return Response::error("not_found", "active linked account not found");
        };
        if account.chain_namespace != input.chain_namespace {
            return Response::error(
                "invalid_request",
                "default wallet chain must match the linked account",
            );
        }
        let now = now_ts();
        let default_account = DefaultWalletAccount {
            schema: "elastos.wallet.default_account/v1".to_string(),
            principal_id: input.principal_id,
            chain_namespace: input.chain_namespace,
            intent: input.intent,
            account_id: input.account_id,
            set_at: now,
        };
        self.store.default_accounts.retain(|existing| {
            if existing.principal_id != default_account.principal_id
                || existing.intent != default_account.intent
            {
                return true;
            }
            if is_eip155_namespace(&default_account.chain_namespace)
                && is_eip155_namespace(&existing.chain_namespace)
            {
                return false;
            }
            existing.chain_namespace != default_account.chain_namespace
        });
        self.store.default_accounts.push(default_account.clone());
        if let Err(err) = self.save() {
            return Response::error("storage_error", err);
        }
        Response::ok(json!({
            "default_account": default_account,
            "account": account,
        }))
    }

    pub(super) fn default_account(
        &self,
        principal_id: &str,
        chain_namespace: &str,
        intent: &str,
    ) -> Response {
        if let Err(response) = self.ensure_initialized() {
            return response;
        }
        if let Err(err) = validate_default_account_lookup(principal_id, chain_namespace, intent) {
            return Response::error("invalid_request", err);
        }
        let Some(default) = self.default_account_record(principal_id, chain_namespace, intent)
        else {
            return Response::error("not_found", "default linked account not set");
        };
        let Some(account) = self.active_account(principal_id, &default.account_id) else {
            return Response::error("not_found", "default linked account is not active");
        };
        Response::ok(json!({
            "default_account": default,
            "account": account,
        }))
    }

    pub(super) fn active_account(
        &self,
        principal_id: &str,
        account_id: &str,
    ) -> Option<&LinkedAccount> {
        self.store.accounts.iter().find(|account| {
            account.principal_id == principal_id
                && account.account_id == account_id
                && account.revoked_at.is_none()
        })
    }

    pub(super) fn default_account_record(
        &self,
        principal_id: &str,
        chain_namespace: &str,
        intent: &str,
    ) -> Option<&DefaultWalletAccount> {
        if is_eip155_namespace(chain_namespace) {
            return self
                .store
                .default_accounts
                .iter()
                .filter(|default| {
                    default.principal_id == principal_id
                        && default.intent == intent
                        && is_eip155_namespace(&default.chain_namespace)
                })
                .filter(|default| {
                    self.active_account(principal_id, &default.account_id)
                        .map(|account| {
                            chain_namespaces_compatible(&account.chain_namespace, chain_namespace)
                        })
                        .unwrap_or(false)
                })
                .max_by_key(|default| default.set_at);
        }
        self.store.default_accounts.iter().find(|default| {
            default.principal_id == principal_id
                && default.chain_namespace == chain_namespace
                && default.intent == intent
        })
    }
}

fn is_eip155_namespace(value: &str) -> bool {
    value.starts_with("eip155:")
}
