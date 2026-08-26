//! Transport-independent Runtime binding contract for typed model provider access.
//!
//! Runtime owns verified principal/session/capsule/grant/request authority and
//! constructs these bindings before calling the native model provider.
//! Provider-owned offer configuration, durable runs, events, journals, and
//! backend execution stay outside this crate.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;

pub const RUNTIME_CREATE_BINDING_SCHEMA: &str = "elastos.model.runtime-binding/v1";
pub const RUNTIME_ACCESS_BINDING_SCHEMA: &str = "elastos.model.runtime-access-binding/v1";
pub const MAX_RUNTIME_BINDING_ID_BYTES: usize = 256;
pub const MAX_RUNTIME_OPERATION_BYTES: usize = 128;
pub const MAX_RUNTIME_INPUT_HASH_BYTES: usize = 71;
pub const MAX_RUN_ID_BYTES: usize = 75;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError(String);

impl ContractError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ContractError {}

impl From<serde_json::Error> for ContractError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(format!("invalid model binding JSON: {error}"))
    }
}

pub type ContractResult<T> = Result<T, ContractError>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCreateBinding {
    pub schema: String,
    pub principal_id: String,
    pub session_id: String,
    pub capsule_id: String,
    pub grant_id: String,
    pub request_id: String,
    pub offer_id: String,
    pub operation: String,
    pub input_hash: String,
}

impl RuntimeCreateBinding {
    pub fn validate(&self, offer_id: &str, operation: &str, input: &Value) -> ContractResult<()> {
        if self.schema != RUNTIME_CREATE_BINDING_SCHEMA {
            return Err(ContractError::new(format!(
                "runtime binding schema must be {RUNTIME_CREATE_BINDING_SCHEMA}"
            )));
        }
        validate_bounded_trimmed(
            &self.principal_id,
            "principal_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        )?;
        validate_bounded_trimmed(&self.session_id, "session_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.capsule_id, "capsule_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.grant_id, "grant_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.request_id, "request_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.offer_id, "offer_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.operation, "operation", MAX_RUNTIME_OPERATION_BYTES)?;
        validate_input_hash(&self.input_hash)?;
        if self.offer_id != offer_id {
            return Err(ContractError::new(
                "runtime binding offer_id does not match request",
            ));
        }
        if self.operation != operation {
            return Err(ContractError::new(
                "runtime binding operation does not match request",
            ));
        }
        if self.input_hash != model_input_hash(input)? {
            return Err(ContractError::new(
                "runtime binding input_hash does not match request input",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAccessBinding {
    pub schema: String,
    pub principal_id: String,
    pub session_id: String,
    pub capsule_id: String,
    pub grant_id: String,
    pub request_id: String,
    pub run_id: String,
}

impl RuntimeAccessBinding {
    pub fn validate(&self, run_id: &str) -> ContractResult<()> {
        if self.schema != RUNTIME_ACCESS_BINDING_SCHEMA {
            return Err(ContractError::new(format!(
                "runtime access binding schema must be {RUNTIME_ACCESS_BINDING_SCHEMA}"
            )));
        }
        validate_bounded_trimmed(
            &self.principal_id,
            "principal_id",
            MAX_RUNTIME_BINDING_ID_BYTES,
        )?;
        validate_bounded_trimmed(&self.session_id, "session_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.capsule_id, "capsule_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.grant_id, "grant_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_bounded_trimmed(&self.request_id, "request_id", MAX_RUNTIME_BINDING_ID_BYTES)?;
        validate_run_id(&self.run_id)?;
        if self.run_id != run_id {
            return Err(ContractError::new(
                "runtime access binding run_id does not match request",
            ));
        }
        Ok(())
    }
}

pub fn model_input_hash(input: &Value) -> ContractResult<String> {
    let canonical = serde_json::to_vec(input)?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    Ok(format!("sha256:{}", hex_hash(&hasher.finalize())))
}

pub fn validate_input_hash(value: &str) -> ContractResult<()> {
    validate_bounded_trimmed(value, "input_hash", MAX_RUNTIME_INPUT_HASH_BYTES)?;
    if value.len() != MAX_RUNTIME_INPUT_HASH_BYTES
        || !value.starts_with("sha256:")
        || !value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ContractError::new(
            "input_hash must be a canonical sha256 digest",
        ));
    }
    Ok(())
}

pub fn validate_run_id(value: &str) -> ContractResult<()> {
    validate_bounded_trimmed(value, "run_id", MAX_RUN_ID_BYTES)?;
    if value.len() != MAX_RUN_ID_BYTES
        || !value.starts_with("run:sha256:")
        || !value["run:sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ContractError::new(
            "run_id must be a canonical model run identifier",
        ));
    }
    Ok(())
}

fn validate_trimmed(value: &str, label: &str) -> ContractResult<()> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(ContractError::new(format!(
            "{label} must be a trimmed non-empty string"
        )));
    }
    Ok(())
}

fn validate_bounded_trimmed(value: &str, label: &str, max_bytes: usize) -> ContractResult<()> {
    validate_trimmed(value, label)?;
    if value.len() > max_bytes {
        return Err(ContractError::new(format!(
            "{label} exceeds {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn hex_hash(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_input() -> Value {
        json!({
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0
        })
    }

    fn sample_create_binding() -> RuntimeCreateBinding {
        let input = sample_input();
        RuntimeCreateBinding {
            schema: RUNTIME_CREATE_BINDING_SCHEMA.to_string(),
            principal_id: "principal-1".to_string(),
            session_id: "session-1".to_string(),
            capsule_id: "capsule-1".to_string(),
            grant_id: "grant-1".to_string(),
            request_id: "request-1".to_string(),
            offer_id: "offer:flash-chat:pair-a".to_string(),
            operation: "text.generate".to_string(),
            input_hash: model_input_hash(&input).unwrap(),
        }
    }

    fn sample_run_id() -> String {
        "run:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string()
    }

    #[test]
    fn runtime_create_binding_serializes_exactly() {
        let binding = sample_create_binding();
        let value = serde_json::to_value(&binding).unwrap();
        assert_eq!(
            value,
            json!({
                "schema": RUNTIME_CREATE_BINDING_SCHEMA,
                "principal_id": "principal-1",
                "session_id": "session-1",
                "capsule_id": "capsule-1",
                "grant_id": "grant-1",
                "request_id": "request-1",
                "offer_id": "offer:flash-chat:pair-a",
                "operation": "text.generate",
                "input_hash": binding.input_hash,
            })
        );
    }

    #[test]
    fn runtime_access_binding_rejects_unknown_legacy_fields() {
        let error = serde_json::from_value::<RuntimeAccessBinding>(json!({
            "schema": RUNTIME_ACCESS_BINDING_SCHEMA,
            "principal_id": "principal-1",
            "session_id": "session-1",
            "capsule_id": "capsule-1",
            "grant_id": "grant-1",
            "request_id": "request-2",
            "run_id": sample_run_id(),
            "offer_id": "legacy-offer",
            "operation": "legacy-op"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn runtime_create_binding_rejects_input_mutation_hash_mismatch() {
        let input = sample_input();
        let mutated = json!({
            "messages": [{"role": "user", "content": "goodbye"}],
            "temperature": 0
        });
        let binding = sample_create_binding();
        binding
            .validate("offer:flash-chat:pair-a", "text.generate", &input)
            .unwrap();
        let error = binding
            .validate("offer:flash-chat:pair-a", "text.generate", &mutated)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("runtime binding input_hash does not match request input"));
    }

    #[test]
    fn validate_run_id_requires_canonical_identifier() {
        validate_run_id(&sample_run_id()).unwrap();
        for invalid in [
            "run:sha512:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "run:sha256:01234567",
            "run:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF",
            "run:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd/0",
        ] {
            assert!(validate_run_id(invalid).is_err(), "{invalid} should fail");
        }
    }

    #[test]
    fn binding_validation_requires_exact_request_match() {
        let input = sample_input();
        let create = sample_create_binding();
        assert!(create
            .validate("offer:h3-video:2x", "text.generate", &input)
            .is_err());
        assert!(create
            .validate("offer:flash-chat:pair-a", "image.generate", &input)
            .is_err());

        let access = RuntimeAccessBinding {
            schema: RUNTIME_ACCESS_BINDING_SCHEMA.to_string(),
            principal_id: "principal-1".to_string(),
            session_id: "session-2".to_string(),
            capsule_id: "capsule-1".to_string(),
            grant_id: "grant-2".to_string(),
            request_id: "request-3".to_string(),
            run_id: sample_run_id(),
        };
        assert!(access.validate(&sample_run_id()).is_ok());
        assert!(access
            .validate("run:sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .is_err());
    }
}
