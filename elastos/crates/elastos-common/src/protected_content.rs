//! Shared protected-content contracts.
//!
//! These structs are data contracts only. Rights checks, key release, decrypt,
//! and render authority belong to provider capsules behind runtime
//! capabilities, not to app/viewer/content capsules.

use serde::{Deserialize, Serialize};

pub const SEALED_OBJECT_SCHEMA: &str = "elastos.sealed.object/v1";
pub const RIGHTS_POLICY_SCHEMA: &str = "elastos.rights.policy/v1";
pub const KEY_RELEASE_REQUEST_SCHEMA: &str = "elastos.key_release.request/v1";
pub const DECRYPT_SESSION_REQUEST_SCHEMA: &str = "elastos.decrypt.session.request/v1";
pub const DECRYPT_SESSION_SCHEMA: &str = "elastos.decrypt.session/v1";
pub const RELEASE_RECEIPT_SCHEMA: &str = "elastos.release.receipt/v1";
pub const RIGHTS_DECISION_RECEIPT_SCHEMA: &str = "elastos.rights.decision.receipt/v1";
pub const PROTECTED_CONTENT_ACTIONS: &[&str] = &["view", "stream", "download", "execute"];
pub const PROTECTED_CONTENT_OUTPUTS: &[&str] = &["rendered", "stream", "working_copy"];
pub const DEFAULT_PROTECTED_CONTENT_CIPHER: &str = "aes-256-gcm";
pub const DEFAULT_PROTECTED_CONTENT_SIGNATURES: &[&str] = &["ed25519", "ml-dsa-65"];
pub const DEFAULT_PROTECTED_CONTENT_KEMS: &[&str] = &["x25519", "ml-kem-768"];
pub const DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME: &str = "shamir-t-of-n";
pub const ALLOWED_PROTECTED_CONTENT_CIPHERS: &[&str] = &["aes-256-gcm", "chacha20-poly1305"];
pub const ALLOWED_PROTECTED_CONTENT_SIGNATURES: &[&str] =
    &["ed25519", "ml-dsa-65", "slh-dsa-sha2-256s"];
pub const REQUIRED_PROTECTED_CONTENT_PQ_SIGNATURES: &[&str] = &["ml-dsa-65", "slh-dsa-sha2-256s"];
pub const REQUIRED_PROTECTED_CONTENT_HYBRID_KEMS: &[&str] = &["x25519", "ml-kem-768"];
pub const ALLOWED_PROTECTED_CONTENT_SHARE_SCHEMES: &[&str] = &["shamir-t-of-n"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SealedObjectV1 {
    pub schema: String,
    pub payload_cid: String,
    pub rights_policy_cid: String,
    pub availability_receipt_cid: String,
    pub key_envelope: KeyEnvelopeV1,
    pub viewer: ViewerRequirementV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyEnvelopeV1 {
    pub scheme: String,
    pub kid: String,
    pub wrapped_cek: String,
    pub policy_hash: String,
    pub algorithms: KeyEnvelopeAlgorithmsV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyEnvelopeAlgorithmsV1 {
    pub cipher: String,
    pub signature: Vec<String>,
    pub kem: Vec<String>,
    pub share_scheme: String,
}

pub fn validate_protected_content_key_envelope_algorithms(
    algorithms: &KeyEnvelopeAlgorithmsV1,
) -> Result<(), String> {
    require_allowed_algorithm(
        &algorithms.cipher,
        ALLOWED_PROTECTED_CONTENT_CIPHERS,
        "key_envelope.algorithms.cipher",
    )?;
    require_allowed_algorithm(
        &algorithms.share_scheme,
        ALLOWED_PROTECTED_CONTENT_SHARE_SCHEMES,
        "key_envelope.algorithms.share_scheme",
    )?;
    require_non_empty_algorithm_list(&algorithms.signature, "key_envelope.algorithms.signature")?;
    require_non_empty_algorithm_list(&algorithms.kem, "key_envelope.algorithms.kem")?;
    require_all_allowed_algorithms(
        &algorithms.signature,
        ALLOWED_PROTECTED_CONTENT_SIGNATURES,
        "key_envelope.algorithms.signature",
    )?;
    require_any_algorithm(
        &algorithms.signature,
        REQUIRED_PROTECTED_CONTENT_PQ_SIGNATURES,
        "key_envelope.algorithms.signature",
    )?;
    require_all_required_algorithms(
        &algorithms.kem,
        REQUIRED_PROTECTED_CONTENT_HYBRID_KEMS,
        "key_envelope.algorithms.kem",
    )
}

fn require_non_empty_algorithm_list(values: &[String], field: &str) -> Result<(), String> {
    if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
        Err(format!("{field} must be a non-empty list"))
    } else {
        Ok(())
    }
}

fn require_allowed_algorithm(value: &str, allowed: &[&str], field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} is required"));
    }
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("{field} uses unsupported algorithm: {value}"))
    }
}

fn require_all_allowed_algorithms(
    values: &[String],
    allowed: &[&str],
    field: &str,
) -> Result<(), String> {
    for value in values {
        require_allowed_algorithm(value, allowed, field)?;
    }
    Ok(())
}

fn require_any_algorithm(values: &[String], required: &[&str], field: &str) -> Result<(), String> {
    if values
        .iter()
        .any(|value| required.contains(&value.as_str()))
    {
        Ok(())
    } else {
        Err(format!("{field} must include a post-quantum algorithm"))
    }
}

fn require_all_required_algorithms(
    values: &[String],
    required: &[&str],
    field: &str,
) -> Result<(), String> {
    for required_value in required {
        if !values.iter().any(|value| value == required_value) {
            return Err(format!("{field} must include {required_value}"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewerRequirementV1 {
    pub required_interface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RightsPolicyV1 {
    pub schema: String,
    pub policy_id: String,
    pub content_id: String,
    pub actions: Vec<String>,
    pub chain: RightsChainBindingV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RightsChainBindingV1 {
    pub network: String,
    pub contract: String,
    pub method: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeyReleaseRequestV1 {
    pub schema: String,
    pub request_id: String,
    pub principal_id: String,
    pub session_id: String,
    pub object_cid: String,
    pub action: String,
    pub rights_receipt: RightsDecisionReceiptV1,
    pub key_envelope: KeyEnvelopeV1,
    pub reason: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecryptSessionRequestV1 {
    pub schema: String,
    pub request_id: String,
    pub principal_id: String,
    pub session_id: String,
    pub object_cid: String,
    pub action: String,
    pub viewer_interface: String,
    pub release_receipt: ReleaseReceiptV1,
    pub output_kind: String,
    pub reason: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecryptSessionV1 {
    pub schema: String,
    pub session_id: String,
    pub object_cid: String,
    pub viewer_interface: String,
    pub output: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReceiptV1 {
    pub schema: String,
    pub request_id: String,
    pub object_cid: String,
    pub principal_id: String,
    pub session_id: String,
    pub action: String,
    pub provider: String,
    pub status: String,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RightsDecisionReceiptV1 {
    pub schema: String,
    pub request_id: String,
    pub content_id: String,
    pub principal_id: String,
    pub session_id: String,
    pub right: String,
    pub provider: String,
    pub allowed: bool,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sealed_object_round_trips_with_schema_constants() {
        let object = SealedObjectV1 {
            schema: SEALED_OBJECT_SCHEMA.to_string(),
            payload_cid: "bafybeigpayload".to_string(),
            rights_policy_cid: "bafybeigpolicy".to_string(),
            availability_receipt_cid: "bafybeigreceipt".to_string(),
            key_envelope: KeyEnvelopeV1 {
                scheme: "elastos-pq-hybrid-threshold-v0".to_string(),
                kid: "kid:test".to_string(),
                wrapped_cek: "wrapped".to_string(),
                policy_hash: "sha256:test".to_string(),
                algorithms: KeyEnvelopeAlgorithmsV1 {
                    cipher: DEFAULT_PROTECTED_CONTENT_CIPHER.to_string(),
                    signature: DEFAULT_PROTECTED_CONTENT_SIGNATURES
                        .iter()
                        .map(|algorithm| algorithm.to_string())
                        .collect(),
                    kem: DEFAULT_PROTECTED_CONTENT_KEMS
                        .iter()
                        .map(|algorithm| algorithm.to_string())
                        .collect(),
                    share_scheme: DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME.to_string(),
                },
            },
            viewer: ViewerRequirementV1 {
                required_interface: "elastos.viewer/document@1".to_string(),
            },
        };

        let json = serde_json::to_string(&object).unwrap();
        let round_trip: SealedObjectV1 = serde_json::from_str(&json).unwrap();

        assert_eq!(round_trip, object);
        assert_eq!(round_trip.schema, SEALED_OBJECT_SCHEMA);
    }

    #[test]
    fn decrypt_session_request_round_trips_with_schema_constants() {
        let request = DecryptSessionRequestV1 {
            schema: DECRYPT_SESSION_REQUEST_SCHEMA.to_string(),
            request_id: "decrypt:test".to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            object_cid: "bafybeigprotectedcontent".to_string(),
            action: PROTECTED_CONTENT_ACTIONS[0].to_string(),
            viewer_interface: "elastos.viewer/document@1".to_string(),
            release_receipt: ReleaseReceiptV1 {
                schema: RELEASE_RECEIPT_SCHEMA.to_string(),
                request_id: "key-release:test".to_string(),
                object_cid: "bafybeigprotectedcontent".to_string(),
                principal_id: "person:local:test".to_string(),
                session_id: "session:test".to_string(),
                action: PROTECTED_CONTENT_ACTIONS[0].to_string(),
                provider: "key-provider".to_string(),
                status: "released".to_string(),
                issued_at: 1_800_000_000,
                expires_at: 1_900_000_000,
            },
            output_kind: PROTECTED_CONTENT_OUTPUTS[0].to_string(),
            reason: "open protected document".to_string(),
            expires_at: 1_900_000_000,
        };

        let json = serde_json::to_string(&request).unwrap();
        let round_trip: DecryptSessionRequestV1 = serde_json::from_str(&json).unwrap();

        assert_eq!(round_trip, request);
        assert_eq!(round_trip.schema, DECRYPT_SESSION_REQUEST_SCHEMA);
    }

    #[test]
    fn key_envelope_algorithm_policy_rejects_non_pq_or_weak_metadata() {
        let mut algorithms = KeyEnvelopeAlgorithmsV1 {
            cipher: DEFAULT_PROTECTED_CONTENT_CIPHER.to_string(),
            signature: DEFAULT_PROTECTED_CONTENT_SIGNATURES
                .iter()
                .map(|algorithm| algorithm.to_string())
                .collect(),
            kem: DEFAULT_PROTECTED_CONTENT_KEMS
                .iter()
                .map(|algorithm| algorithm.to_string())
                .collect(),
            share_scheme: DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME.to_string(),
        };

        validate_protected_content_key_envelope_algorithms(&algorithms).unwrap();

        algorithms.cipher = "aes-128-gcm".to_string();
        assert!(
            validate_protected_content_key_envelope_algorithms(&algorithms)
                .unwrap_err()
                .contains("unsupported algorithm")
        );

        algorithms.cipher = DEFAULT_PROTECTED_CONTENT_CIPHER.to_string();
        algorithms.signature = vec!["ed25519".to_string()];
        assert!(
            validate_protected_content_key_envelope_algorithms(&algorithms)
                .unwrap_err()
                .contains("post-quantum")
        );

        algorithms.signature = DEFAULT_PROTECTED_CONTENT_SIGNATURES
            .iter()
            .map(|algorithm| algorithm.to_string())
            .collect();
        algorithms.kem = vec!["x25519".to_string()];
        assert!(
            validate_protected_content_key_envelope_algorithms(&algorithms)
                .unwrap_err()
                .contains("ml-kem-768")
        );
    }

    #[test]
    fn sealed_object_rejects_unknown_contract_fields() {
        let payload = json!({
            "schema": SEALED_OBJECT_SCHEMA,
            "payload_cid": "bafybeigpayload",
            "rights_policy_cid": "bafybeigpolicy",
            "availability_receipt_cid": "bafybeigreceipt",
            "key_envelope": {
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "kid": "kid:test",
                "wrapped_cek": "wrapped",
                "policy_hash": "sha256:test",
                "algorithms": {
                    "cipher": DEFAULT_PROTECTED_CONTENT_CIPHER,
                    "signature": DEFAULT_PROTECTED_CONTENT_SIGNATURES,
                    "kem": DEFAULT_PROTECTED_CONTENT_KEMS,
                    "share_scheme": DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME
                }
            },
            "viewer": {
                "required_interface": "elastos.viewer/document@1"
            },
            "raw_ipfs_gateway": "https://example.invalid/ipfs"
        });

        let err = serde_json::from_value::<SealedObjectV1>(payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"));
    }

    #[test]
    fn sealed_object_rejects_unknown_nested_key_envelope_fields() {
        let payload = json!({
            "schema": SEALED_OBJECT_SCHEMA,
            "payload_cid": "bafybeigpayload",
            "rights_policy_cid": "bafybeigpolicy",
            "availability_receipt_cid": "bafybeigreceipt",
            "key_envelope": {
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "kid": "kid:test",
                "wrapped_cek": "wrapped",
                "policy_hash": "sha256:test",
                "algorithms": {
                    "cipher": DEFAULT_PROTECTED_CONTENT_CIPHER,
                    "signature": DEFAULT_PROTECTED_CONTENT_SIGNATURES,
                    "kem": DEFAULT_PROTECTED_CONTENT_KEMS,
                    "share_scheme": DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME,
                    "frost_only": true
                }
            },
            "viewer": {
                "required_interface": "elastos.viewer/document@1"
            }
        });

        let err = serde_json::from_value::<SealedObjectV1>(payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"));
    }

    #[test]
    fn key_release_request_rejects_unknown_contract_fields() {
        let payload = json!({
            "schema": KEY_RELEASE_REQUEST_SCHEMA,
            "request_id": "key-release:test",
            "principal_id": "person:local:test",
            "session_id": "session:test",
            "object_cid": "bafybeigprotectedcontent",
            "action": "view",
            "rights_receipt": {
                "schema": RIGHTS_DECISION_RECEIPT_SCHEMA,
                "request_id": "rights:test",
                "content_id": "bafybeigprotectedcontent",
                "principal_id": "person:local:test",
                "session_id": "session:test",
                "right": "view",
                "provider": "rights-provider",
                "allowed": true,
                "issued_at": 1_800_000_000u64,
                "expires_at": 1_900_000_000u64
            },
            "key_envelope": {
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "kid": "kid:test",
                "wrapped_cek": "wrapped",
                "policy_hash": "sha256:test",
                "algorithms": {
                    "cipher": DEFAULT_PROTECTED_CONTENT_CIPHER,
                    "signature": DEFAULT_PROTECTED_CONTENT_SIGNATURES,
                    "kem": DEFAULT_PROTECTED_CONTENT_KEMS,
                    "share_scheme": DEFAULT_PROTECTED_CONTENT_SHARE_SCHEME
                }
            },
            "reason": "open protected document",
            "expires_at": 1_900_000_000u64,
            "raw_cek": "must-not-be-accepted"
        });

        let err = serde_json::from_value::<KeyReleaseRequestV1>(payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"));
    }

    #[test]
    fn decrypt_session_request_rejects_unknown_contract_fields() {
        let payload = json!({
            "schema": DECRYPT_SESSION_REQUEST_SCHEMA,
            "request_id": "decrypt:test",
            "principal_id": "person:local:test",
            "session_id": "session:test",
            "object_cid": "bafybeigprotectedcontent",
            "action": "view",
            "viewer_interface": "elastos.viewer/document@1",
            "release_receipt": {
                "schema": RELEASE_RECEIPT_SCHEMA,
                "request_id": "key-release:test",
                "object_cid": "bafybeigprotectedcontent",
                "principal_id": "person:local:test",
                "session_id": "session:test",
                "action": "view",
                "provider": "key-provider",
                "status": "released",
                "issued_at": 1_800_000_000u64,
                "expires_at": 1_900_000_000u64
            },
            "output_kind": "rendered",
            "reason": "open protected document",
            "expires_at": 1_900_000_000u64,
            "raw_plaintext": true
        });

        let err = serde_json::from_value::<DecryptSessionRequestV1>(payload)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field"));
    }
}
