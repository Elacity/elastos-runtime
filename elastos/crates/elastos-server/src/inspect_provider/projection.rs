use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use super::InspectEntry;

fn forbidden_fact_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.starts_with("_runtime")
        || matches!(
            lower.as_str(),
            "adapter_ipc"
                | "authorization"
                | "bearer_token"
                | "carrier"
                | "carrier_route"
                | "client_token"
                | "connect_ticket"
                | "control_socket"
                | "control_socket_path"
                | "home_token"
                | "host_path"
                | "ipc_path"
                | "manifest_signature"
                | "mutation_handle"
                | "private_key"
                | "privatekey"
                | "raw_signature"
                | "raw_host_path"
                | "relay_ipc"
                | "secret"
                | "shell_token"
                | "signature"
                | "signature_raw"
                | "token"
        )
}

fn forbidden_fact_string(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.starts_with('\\')
        || (value.len() > 2
            && value.as_bytes()[1] == b':'
            && matches!(value.as_bytes()[2], b'/' | b'\\'))
        || lower.contains("bearer ")
        || lower.contains("file:///")
        || lower.starts_with("ticket:")
        || matches!(value, "dispatch_approved" | "revoke")
        || value.contains("/api/provider/inspect/dispatch_approved")
        || value.contains("/api/provider/inspect/revoke")
}

fn redact_fact_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !forbidden_fact_key(key))
                .map(|(key, value)| (key.clone(), redact_fact_value(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_fact_value).collect()),
        Value::String(value) if forbidden_fact_string(value) => Value::Null,
        _ => value.clone(),
    }
}

fn signature_fingerprint(signature: &str) -> String {
    hex::encode(Sha256::digest(signature.as_bytes()))
        .chars()
        .take(16)
        .collect()
}

fn provider_authority(manifest: &Value) -> Value {
    let authority = manifest
        .get("authority")
        .map(|authority| {
            json!({
                "reason": authority.get("reason").cloned().unwrap_or(Value::Null),
                "capabilities": authority.get("capabilities").cloned().unwrap_or(Value::Null),
                "audit_events": authority.get("audit_events").cloned().unwrap_or(Value::Null),
            })
        })
        .unwrap_or(Value::Null);
    redact_fact_value(&authority)
}

fn trust_state(signature_present: bool, cid_present: bool) -> &'static str {
    match (signature_present, cid_present) {
        (true, true) => "cid-with-manifest-signature",
        (true, false) => "local-manifest-signature",
        (false, true) => "cid-without-manifest-signature",
        (false, false) => "local-dev",
    }
}

fn trust_evidence(cid: Option<&String>, signature_fingerprint: Option<&String>) -> Value {
    let signature_present = signature_fingerprint.is_some();
    let cid_present = cid.is_some();
    json!({
        "schema": "elastos.inspect.trust-evidence/v1",
        "trust_state": trust_state(signature_present, cid_present),
        "cid_state": if cid_present { "cid-published" } else { "local-only" },
        "signature_state": if signature_present {
            "manifest-signature-declared"
        } else {
            "no-manifest-signature"
        },
        "manifest_signature": signature_fingerprint
            .map(|fingerprint| json!({
                "state": "declared",
                "fingerprint": fingerprint,
            }))
            .unwrap_or(Value::Null),
        "verified": false,
        "verified_by": Value::Null,
    })
}

pub(super) fn project(entry: &InspectEntry) -> Value {
    let manifest = entry
        .manifest
        .as_ref()
        .and_then(|manifest| serde_json::to_value(manifest).ok())
        .unwrap_or_else(|| json!({}));
    let signature = manifest.get("signature").and_then(Value::as_str);
    let signature_fingerprint = signature.map(signature_fingerprint);
    let provider_authority = provider_authority(&manifest);
    json!({
        "schema": "elastos.inspect.object/v1",
        "kind": if entry.id.starts_with("provider:") { "provider" } else { "capsule" },
        "id": entry.id,
        "name": entry.name,
        "state": entry.status,
        "type": entry.capsule_type,
        "manifest": {
            "schema": manifest.get("schema").cloned().unwrap_or(Value::Null),
            "version": manifest.get("version").cloned().unwrap_or(Value::Null),
            "role": manifest.get("role").cloned().unwrap_or(Value::Null),
            "entrypoint": manifest.get("entrypoint").map(redact_fact_value).unwrap_or(Value::Null),
            "provides": manifest.get("provides").map(redact_fact_value).unwrap_or(Value::Null),
        },
        "affordances": manifest.get("interfaces").map(redact_fact_value).unwrap_or_else(|| json!([])),
        "required_capabilities": manifest.get("capabilities").map(redact_fact_value).unwrap_or_else(|| json!([])),
        "storage_namespaces": manifest.pointer("/permissions/storage").map(redact_fact_value).unwrap_or(Value::Null),
        "carrier": {
            "enabled": manifest.pointer("/permissions/carrier").cloned().unwrap_or(Value::Null),
            "endpoints": [],
        },
        "provider_authority": provider_authority.clone(),
        "authority": provider_authority,
        "provenance": {
            "author": manifest.get("author").map(redact_fact_value).unwrap_or(Value::Null),
            "cid": entry.cid,
            "signature_present": signature.is_some(),
            "signature_fingerprint": signature_fingerprint.clone(),
            "signed_by": Value::Null,
        },
        "trust_evidence": trust_evidence(entry.cid.as_ref(), signature_fingerprint.as_ref()),
        "granted_capabilities": Value::Null,
        "audit": Value::Null,
        "spend_budget": Value::Null,
        "intent_proof": Value::Null,
        "audit_chain_attestation": Value::Null,
        "processes": [{ "kind": entry.capsule_type, "status": entry.status }],
    })
}
