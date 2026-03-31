//! DID key encoding, domain-separated signing, and release envelope verification.

use ed25519_dalek::Signer;
use ed25519_dalek::{Verifier as Ed25519Verifier, VerifyingKey};
use elastos_runtime::signature;

// DID key encoding lives in elastos-identity — single source of truth.
pub use elastos_identity::{encode_did_key, MULTICODEC_ED25519_PUB};

pub fn decode_did_key(did: &str) -> anyhow::Result<VerifyingKey> {
    let multibase_part = did
        .strip_prefix("did:key:z")
        .ok_or_else(|| anyhow::anyhow!("DID must start with did:key:z"))?;
    let bytes = bs58::decode(multibase_part)
        .into_vec()
        .map_err(|e| anyhow::anyhow!("Invalid base58 in DID: {}", e))?;
    if bytes.len() != 34 || bytes[0] != 0xed || bytes[1] != 0x01 {
        anyhow::bail!("Invalid Ed25519 did:key encoding (expected 34 bytes with 0xed01 prefix)");
    }
    let key_bytes: [u8; 32] = bytes[2..34].try_into().unwrap();
    Ok(VerifyingKey::from_bytes(&key_bytes)?)
}

/// Sign arbitrary payload bytes with a domain separator.
/// Returns `(signature_hex, signer_did)`.
///
/// Signing input: `SHA256(domain || b"\0" || payload)` → Ed25519 sign.
pub fn domain_separated_sign(
    sk: &signature::SigningKey,
    domain: &str,
    payload: &[u8],
) -> (String, String) {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(payload);
    let digest = hasher.finalize();

    let sig = sk.sign(&digest);
    let did = encode_did_key(&sk.verifying_key());
    (hex::encode(sig.to_bytes()), did)
}

/// Verify a signed release envelope `{ payload, signature, signer_did }`.
/// The `domain` is the domain separator used when signing (e.g. "elastos.release.head.v1").
/// Returns the parsed JSON value and signer DID on success.
pub fn verify_release_envelope_against_dids(
    envelope_bytes: &[u8],
    domain: &str,
    expected_dids: &[String],
) -> anyhow::Result<(serde_json::Value, String)> {
    let envelope: serde_json::Value = serde_json::from_slice(envelope_bytes)?;

    let signer_did = envelope["signer_did"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing signer_did in envelope"))?
        .to_string();

    if !expected_dids.is_empty() && !expected_dids.iter().any(|did| did == &signer_did) {
        anyhow::bail!(
            "Signer DID mismatch: trusted set = {:?}, got {}",
            expected_dids,
            signer_did
        );
    }

    let sig_hex = envelope["signature"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing signature in envelope"))?;

    let payload = &envelope["payload"];
    if payload.is_null() {
        anyhow::bail!("Missing payload in envelope");
    }

    let canonical = serde_json::to_string(payload)?;

    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();

    let verifying_key = decode_did_key(&signer_did)?;
    let sig_bytes =
        hex::decode(sig_hex).map_err(|e| anyhow::anyhow!("Invalid signature hex: {}", e))?;
    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid Ed25519 signature: {}", e))?;

    verifying_key
        .verify(&digest, &sig)
        .map_err(|_| anyhow::anyhow!("Release envelope signature verification failed"))?;

    Ok((envelope, signer_did))
}

pub fn verify_release_envelope(
    envelope_bytes: &[u8],
    domain: &str,
    expected_did: &str,
) -> anyhow::Result<serde_json::Value> {
    verify_release_envelope_against_dids(envelope_bytes, domain, &[expected_did.to_string()])
        .map(|(envelope, _)| envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::signature::generate_keypair;

    #[test]
    fn test_encode_decode_did_roundtrip() {
        let (_, vk) = generate_keypair();
        let did = encode_did_key(&vk);
        assert!(did.starts_with("did:key:z6Mk"));
        let decoded = decode_did_key(&did).unwrap();
        assert_eq!(decoded.as_bytes(), vk.as_bytes());
    }

    #[test]
    fn test_decode_did_rejects_invalid() {
        assert!(decode_did_key("not-a-did").is_err());
        assert!(decode_did_key("did:key:z").is_err());
        assert!(decode_did_key("did:key:zBadBase58!!!").is_err());
    }

    #[test]
    fn test_domain_separated_sign_and_verify() {
        let (sk, _) = generate_keypair();
        let domain = "test.domain.v1";
        let payload = serde_json::json!({"version": "1.0", "channel": "stable"});

        // Sign the canonical JSON bytes (same as verify_release_envelope does)
        let canonical = serde_json::to_string(&payload).unwrap();
        let (sig_hex, signer_did) = domain_separated_sign(&sk, domain, canonical.as_bytes());
        assert!(signer_did.starts_with("did:key:z6Mk"));

        // Verify via envelope
        let envelope = serde_json::json!({
            "payload": payload,
            "signature": sig_hex,
            "signer_did": signer_did,
        });
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let result = verify_release_envelope(&envelope_bytes, domain, &signer_did);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_envelope_wrong_did() {
        let (sk, _) = generate_keypair();
        let (sig_hex, signer_did) = domain_separated_sign(&sk, "test", b"payload");
        let envelope = serde_json::json!({
            "payload": "payload",
            "signature": sig_hex,
            "signer_did": signer_did,
        });
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let (other_sk, _) = generate_keypair();
        let wrong_did = encode_did_key(&other_sk.verifying_key());
        let result = verify_release_envelope(&envelope_bytes, "test", &wrong_did);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_envelope_tampered_payload() {
        let (sk, _) = generate_keypair();
        let (sig_hex, signer_did) = domain_separated_sign(&sk, "test", b"original");
        let envelope = serde_json::json!({
            "payload": "tampered",
            "signature": sig_hex,
            "signer_did": signer_did,
        });
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let result = verify_release_envelope(&envelope_bytes, "test", &signer_did);
        assert!(result.is_err());
    }
}
