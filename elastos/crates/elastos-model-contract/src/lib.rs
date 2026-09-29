//! Transport-independent Runtime binding contract for typed model provider access.
//!
//! Runtime owns verified principal/session/capsule/grant/request authority and
//! constructs these bindings before calling the native model provider.
//! Provider-owned offer configuration, durable runs, events, journals, and
//! backend execution stay outside this crate.

#![forbid(unsafe_code)]

pub mod decisions;

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
pub const TEXT_INPUT_V1_SCHEMA: &str = "elastos.model.input.text/v1";
pub const TEXT_INPUT_V2_SCHEMA: &str = "elastos.model.input.text/v2";
pub const MAX_TEXT_MESSAGES: usize = 256;

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

/// Stable run identity shared by Runtime recovery and the provider journal.
/// Session and grant rotation preserve ownership by principal and capsule.
pub fn model_run_id(binding: &RuntimeCreateBinding) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"elastos:model-run:v1\n");
    hasher.update(binding.principal_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(binding.capsule_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(binding.request_id.as_bytes());
    format!("run:sha256:{}", hex_hash(&hasher.finalize()))
}

pub fn model_input_hash(input: &Value) -> ContractResult<String> {
    let canonical = serde_json::to_vec(input)?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    Ok(format!("sha256:{}", hex_hash(&hasher.finalize())))
}

/// Validate the exact ordered conversation. Offer byte limits apply separately.
pub fn validate_text_input_v2(input: &Value) -> ContractResult<&[Value]> {
    let invalid = || ContractError::new("invalid text/v2 conversation");
    let object = input.as_object().ok_or_else(invalid)?;
    if object.len() != 2 || input["schema"] != TEXT_INPUT_V2_SCHEMA {
        return Err(invalid());
    }
    let messages = input["messages"].as_array().ok_or_else(invalid)?;
    if messages.is_empty() || messages.len() > MAX_TEXT_MESSAGES {
        return Err(invalid());
    }
    for (index, message) in messages.iter().enumerate() {
        let message = message.as_object().ok_or_else(invalid)?;
        if message.len() != 2
            || message
                .get("content")
                .and_then(Value::as_str)
                .is_none_or(|text| text.is_empty())
        {
            return Err(invalid());
        }
        match message.get("role").and_then(Value::as_str) {
            Some("system") if index == 0 => {}
            Some("user" | "assistant") => {}
            _ => return Err(invalid()),
        }
    }
    if messages.last().map(|message| &message["role"]) != Some(&Value::from("user")) {
        return Err(invalid());
    }
    Ok(messages)
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

    #[test]
    fn text_v2_keeps_messages_and_rejects_invalid_shapes() {
        let user = json!({"role":"user", "content":"Hello"});
        let system = json!({"role":"system", "content":"Be concise."});
        let assistant = json!({"role":"assistant", "content":"Hi"});
        for messages in [
            json!([user]),
            json!([system, user]),
            json!([system, user, assistant, user]),
        ] {
            let input = json!({"schema":TEXT_INPUT_V2_SCHEMA, "messages":messages});
            assert_eq!(
                validate_text_input_v2(&input).unwrap(),
                messages.as_array().unwrap()
            );
        }
        let valid = json!({"schema":TEXT_INPUT_V2_SCHEMA, "messages":[user]});
        let mut extra_top = valid.clone();
        extra_top["prompt"] = json!("hidden");
        let invalid_messages = vec![
            json!([]),
            json!([{"role":"user","content":""}]),
            json!([{"role":"user","content":"ok","name":"hidden"}]),
            json!([{"role":"agent","content":"ok"}]),
            json!([{"role":"tool","content":"ok"}]),
            json!([{"role":"user","content":1}]),
            json!([user, system, user]),
            json!([system, system, user]),
            json!([assistant]),
            json!([system]),
            json!(vec![user.clone(); MAX_TEXT_MESSAGES + 1]),
        ];
        assert!(validate_text_input_v2(&extra_top).is_err());
        assert!(
            validate_text_input_v2(&json!({"schema":TEXT_INPUT_V1_SCHEMA,"messages":[user]}))
                .is_err()
        );
        for messages in invalid_messages {
            assert!(
                validate_text_input_v2(&json!({"schema":TEXT_INPUT_V2_SCHEMA,"messages":messages}))
                    .is_err(),
                "{messages}"
            );
        }
        let maximum =
            json!({"schema":TEXT_INPUT_V2_SCHEMA,"messages":vec![user; MAX_TEXT_MESSAGES]});
        assert_eq!(
            validate_text_input_v2(&maximum).unwrap().len(),
            MAX_TEXT_MESSAGES
        );
    }

    #[test]
    fn text_v2_binding_detects_role_order_and_content_changes() {
        let input = json!({"schema":TEXT_INPUT_V2_SCHEMA,"messages":[
            {"role":"system","content":"Be concise."},
            {"role":"user","content":"first"},
            {"role":"assistant","content":"reply"},
            {"role":"user","content":"last"}
        ]});
        let mut binding = sample_create_binding();
        binding.input_hash = model_input_hash(&input).unwrap();
        binding
            .validate(&binding.offer_id, &binding.operation, &input)
            .unwrap();
        let mut role = input.clone();
        role["messages"][2]["role"] = json!("user");
        let mut order = input.clone();
        order["messages"].as_array_mut().unwrap().swap(1, 2);
        let mut content = input.clone();
        content["messages"][2]["content"] = json!("changed");
        for changed in [role, order, content] {
            assert!(binding
                .validate(&binding.offer_id, &binding.operation, &changed)
                .is_err());
        }
    }

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
