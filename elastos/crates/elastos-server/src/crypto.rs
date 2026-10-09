//! DID key encoding, domain-separated signing, and release envelope verification.

use ed25519_dalek::Signer;
use ed25519_dalek::Verifier as Ed25519Verifier;
use elastos_runtime::signature;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

// DID key encoding and strict decoding live in elastos-identity — one source
// of truth for collaboration Profile identities.
pub use elastos_identity::{decode_did_key, encode_did_key, encode_signing_key_did};

// A retained 200-message history plus its receipts and current authority
// evidence can exceed 64 immutable mathematical inputs. This positive-only
// bound uses about 64 KiB for public key/signature/digest tuples.
const VERIFIED_SIGNATURE_CACHE_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VerifiedSignatureMath {
    public_key: [u8; 32],
    signature: [u8; 64],
    digest: [u8; 32],
}

static VERIFIED_SIGNATURES: OnceLock<Mutex<VecDeque<VerifiedSignatureMath>>> = OnceLock::new();

fn verify_signature_digest(
    key: &ed25519_dalek::VerifyingKey,
    digest: &[u8; 32],
    signature: &ed25519_dalek::Signature,
) -> Result<(), ed25519_dalek::SignatureError> {
    let cache = VERIFIED_SIGNATURES
        .get_or_init(|| Mutex::new(VecDeque::with_capacity(VERIFIED_SIGNATURE_CACHE_CAPACITY)));
    verify_signature_digest_with_cache(cache, key, digest, signature)
}

fn verify_signature_digest_with_cache(
    cache: &Mutex<VecDeque<VerifiedSignatureMath>>,
    key: &ed25519_dalek::VerifyingKey,
    digest: &[u8; 32],
    signature: &ed25519_dalek::Signature,
) -> Result<(), ed25519_dalek::SignatureError> {
    verify_signature_digest_with_cache_capacity(
        cache,
        key,
        digest,
        signature,
        VERIFIED_SIGNATURE_CACHE_CAPACITY,
    )
    .map(|_| ())
}

// Returns whether exact successful mathematics was reused. The private
// capacity argument lets isolated tests retain a small eviction working set.
fn verify_signature_digest_with_cache_capacity(
    cache: &Mutex<VecDeque<VerifiedSignatureMath>>,
    key: &ed25519_dalek::VerifyingKey,
    digest: &[u8; 32],
    signature: &ed25519_dalek::Signature,
    capacity: usize,
) -> Result<bool, ed25519_dalek::SignatureError> {
    // Only successful deterministic signature mathematics is reused. Callers
    // still parse each input and check current trust, grants, expiry, Profile
    // authority, contacts and revocations. No message or authority is retained.
    let verified = VerifiedSignatureMath {
        public_key: key.to_bytes(),
        signature: signature.to_bytes(),
        digest: *digest,
    };
    if let Ok(entries) = cache.lock() {
        if entries.contains(&verified) {
            return Ok(true);
        }
    }
    // Verification runs outside the lock; poisoning only repeats this check.
    key.verify(digest, signature)?;
    if let Ok(mut entries) = cache.lock() {
        if !entries.contains(&verified) {
            if entries.len() == capacity {
                entries.pop_front();
            }
            entries.push_back(verified);
        }
    }
    Ok(false)
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
    let did = encode_signing_key_did(sk);
    (hex::encode(sig.to_bytes()), did)
}

pub fn verify_domain_separated_signature(
    signer_did: &str,
    domain: &str,
    payload: &[u8],
    signature_hex: &str,
) -> anyhow::Result<()> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(payload);
    let digest: [u8; 32] = hasher.finalize().into();
    let verifying_key = decode_did_key(signer_did)?;
    let signature = hex::decode(signature_hex)
        .map_err(|err| anyhow::anyhow!("Invalid signature hex: {err}"))?;
    let signature = ed25519_dalek::Signature::from_slice(&signature)
        .map_err(|err| anyhow::anyhow!("Invalid Ed25519 signature: {err}"))?;
    verify_signature_digest(&verifying_key, &digest, &signature)
        .map_err(|_| anyhow::anyhow!("Domain-separated signature verification failed"))
}

/// Verify a signed JSON envelope `{ payload, signature, signer_did }`.
/// The `domain` is the domain separator used when signing.
/// Returns the parsed JSON value and signer DID on success.
pub fn verify_signed_json_envelope_against_dids(
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
    let digest: [u8; 32] = hasher.finalize().into();

    let verifying_key = decode_did_key(&signer_did)?;
    let sig_bytes =
        hex::decode(sig_hex).map_err(|e| anyhow::anyhow!("Invalid signature hex: {}", e))?;
    let sig = ed25519_dalek::Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("Invalid Ed25519 signature: {}", e))?;

    verify_signature_digest(&verifying_key, &digest, &sig)
        .map_err(|_| anyhow::anyhow!("Signed envelope signature verification failed"))?;

    Ok((envelope, signer_did))
}

/// Verify a signed release envelope `{ payload, signature, signer_did }`.
/// The `domain` is the domain separator used when signing (e.g. "elastos.release.head.v1").
/// Returns the parsed JSON value and signer DID on success.
pub fn verify_release_envelope_against_dids(
    envelope_bytes: &[u8],
    domain: &str,
    expected_dids: &[String],
) -> anyhow::Result<(serde_json::Value, String)> {
    verify_signed_json_envelope_against_dids(envelope_bytes, domain, expected_dids)
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
        let did = encode_did_key(&vk).unwrap();
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
        let wrong_did = encode_signing_key_did(&other_sk);
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

    #[test]
    fn warmed_signature_math_rejects_changed_payload_domain_signature_and_key() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
        let domain = "test.cached.signature.v1";
        let payload = b"one immutable signed payload";
        let (signature, did) = domain_separated_sign(&key, domain, payload);
        for _ in 0..3 {
            verify_domain_separated_signature(&did, domain, payload, &signature).unwrap();
        }
        assert!(verify_domain_separated_signature(&did, domain, b"changed", &signature).is_err());
        assert!(
            verify_domain_separated_signature(&did, "different.domain", payload, &signature)
                .is_err()
        );
        let mut changed_signature = hex::decode(&signature).unwrap();
        changed_signature[0] ^= 1;
        assert!(verify_domain_separated_signature(
            &did,
            domain,
            payload,
            &hex::encode(changed_signature),
        )
        .is_err());
        let other_key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        assert!(verify_domain_separated_signature(
            &encode_signing_key_did(&other_key),
            domain,
            payload,
            &signature,
        )
        .is_err());
        for invalid_did in [format!("{did}x"), did.to_ascii_uppercase()] {
            assert!(
                verify_domain_separated_signature(&invalid_did, domain, payload, &signature)
                    .is_err()
            );
        }
        assert!(verify_domain_separated_signature(&did, domain, payload, "invalid").is_err());
        assert!(verify_domain_separated_signature(&did, domain, payload, "00").is_err());
        verify_domain_separated_signature(&did, domain, payload, &signature).unwrap();
    }

    #[test]
    fn warmed_signature_math_keeps_envelope_trust_and_input_validation_fresh() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[43; 32]);
        let domain = "test.cached.envelope.v1";
        let payload = serde_json::json!({"body": "original", "revision": 1});
        let canonical = serde_json::to_vec(&payload).unwrap();
        let (signature, did) = domain_separated_sign(&key, domain, &canonical);
        let envelope = serde_json::json!({
            "payload": payload, "signature": signature, "signer_did": did,
        });
        let bytes = serde_json::to_vec(&envelope).unwrap();
        // Both public entry points share exactly the same mathematical input.
        verify_domain_separated_signature(&did, domain, &canonical, &signature).unwrap();
        for _ in 0..3 {
            verify_signed_json_envelope_against_dids(&bytes, domain, std::slice::from_ref(&did))
                .unwrap();
        }
        let other = encode_signing_key_did(&ed25519_dalek::SigningKey::from_bytes(&[44; 32]));
        assert!(verify_signed_json_envelope_against_dids(
            &bytes,
            domain,
            std::slice::from_ref(&other)
        )
        .unwrap_err()
        .to_string()
        .contains("Signer DID mismatch"));
        assert!(verify_signed_json_envelope_against_dids(&bytes, "changed.domain", &[]).is_err());
        for altered in [
            serde_json::json!({"payload": {"body": "changed", "revision": 1}, "signature": signature, "signer_did": did}),
            serde_json::json!({"payload": payload, "signature": signature, "signer_did": other}),
            serde_json::json!({"payload": payload, "signature": signature, "signer_did": format!("{did}x")}),
            serde_json::json!({"payload": payload, "signature": "00", "signer_did": did}),
            serde_json::json!({"payload": null, "signature": signature, "signer_did": did}),
            serde_json::json!({"payload": payload, "signer_did": did}),
        ] {
            assert!(verify_signed_json_envelope_against_dids(
                &serde_json::to_vec(&altered).unwrap(),
                domain,
                &[],
            )
            .is_err());
        }
        assert!(verify_signed_json_envelope_against_dids(b"invalid JSON", domain, &[]).is_err());
        verify_signed_json_envelope_against_dids(&bytes, domain, std::slice::from_ref(&did))
            .unwrap();
    }

    #[test]
    fn signature_math_cache_keeps_only_positive_exact_inputs_in_fifo_order() {
        use sha2::Digest;
        const TEST_CAPACITY: usize = 4;
        let cache = Mutex::new(VecDeque::new());
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[45; 32]);
        let key = decode_did_key(&encode_signing_key_did(&signing_key)).unwrap();
        let mut originals = Vec::new();
        for index in 0..=TEST_CAPACITY {
            let digest: [u8; 32] = sha2::Sha256::digest(index.to_le_bytes()).into();
            let signature = signing_key.sign(&digest);
            verify_signature_digest_with_cache_capacity(
                &cache,
                &key,
                &digest,
                &signature,
                TEST_CAPACITY,
            )
            .unwrap();
            verify_signature_digest_with_cache_capacity(
                &cache,
                &key,
                &digest,
                &signature,
                TEST_CAPACITY,
            )
            .unwrap();
            originals.push(VerifiedSignatureMath {
                public_key: key.to_bytes(),
                signature: signature.to_bytes(),
                digest,
            });
            assert_eq!(cache.lock().unwrap().len(), (index + 1).min(TEST_CAPACITY));
        }
        assert_eq!(
            cache.lock().unwrap().iter().copied().collect::<Vec<_>>(),
            originals[1..],
        );
        let first = originals[0];
        let signature = ed25519_dalek::Signature::from_bytes(&first.signature);
        let mut changed_digest = first.digest;
        changed_digest[0] ^= 1;
        assert!(verify_signature_digest_with_cache_capacity(
            &cache,
            &key,
            &changed_digest,
            &signature,
            TEST_CAPACITY
        )
        .is_err());
        assert_eq!(cache.lock().unwrap().len(), TEST_CAPACITY);
        verify_signature_digest_with_cache_capacity(
            &cache,
            &key,
            &first.digest,
            &signature,
            TEST_CAPACITY,
        )
        .unwrap();
        let entries = cache.lock().unwrap();
        assert_eq!(entries.len(), TEST_CAPACITY);
        assert_eq!(entries.back(), Some(&first));
        assert!(!entries.contains(&originals[1]));
    }

    #[test]
    fn concurrent_signature_cache_eviction_preserves_exact_positive_verification() {
        use sha2::Digest;
        const TEST_CAPACITY: usize = 64;
        let cache = std::sync::Arc::new(Mutex::new(VecDeque::new()));
        std::thread::scope(|scope| {
            for worker in 0u8..8 {
                let cache = std::sync::Arc::clone(&cache);
                scope.spawn(move || {
                    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[worker + 46; 32]);
                    let key = decode_did_key(&encode_signing_key_did(&signing_key)).unwrap();
                    for index in 0u8..10 {
                        let digest: [u8; 32] = sha2::Sha256::digest([worker, index]).into();
                        let signature = signing_key.sign(&digest);
                        for _ in 0..2 {
                            verify_signature_digest_with_cache_capacity(
                                &cache,
                                &key,
                                &digest,
                                &signature,
                                TEST_CAPACITY,
                            )
                            .unwrap();
                        }
                        let mut changed_digest = digest;
                        changed_digest[0] ^= 1;
                        assert!(verify_signature_digest_with_cache_capacity(
                            &cache,
                            &key,
                            &changed_digest,
                            &signature,
                            TEST_CAPACITY,
                        )
                        .is_err());
                    }
                });
            }
        });
        let entries = cache.lock().unwrap();
        assert_eq!(entries.len(), TEST_CAPACITY);
        for entry in entries.iter() {
            let key = ed25519_dalek::VerifyingKey::from_bytes(&entry.public_key).unwrap();
            key.verify(
                &entry.digest,
                &ed25519_dalek::Signature::from_bytes(&entry.signature),
            )
            .unwrap();
        }
    }

    #[test]
    fn complete_retained_receipt_sweeps_reuse_math_with_authority_headroom() {
        use elastos_common::collaboration_protocol::{
            canonical_collaboration_acceptance_receipt_bytes,
            canonical_signed_collaboration_acceptance_receipt_bytes,
            CollaborationAcceptanceReceipt, SignedCollaborationAcceptanceReceipt,
            COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1,
            COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
        };
        use sha2::Digest;
        let signer = ed25519_dalek::SigningKey::from_bytes(&[57; 32]);
        let did = encode_signing_key_did(&signer);
        let key = decode_did_key(&did).unwrap();
        let mut inputs = Vec::new();
        // These are protocol-validated canonical tombstone receipts, with the
        // same 106 distinct immutable inputs measured in the retained fixture.
        for index in 0u64..106 {
            let payload = CollaborationAcceptanceReceipt {
                schema: COLLABORATION_ACCEPTANCE_RECEIPT_SCHEMA_V1.to_string(),
                network_id: "cached-signature-test".to_string(),
                message_envelope_sha256: format!("sha256:{:064x}", index + 1),
                conversation_id: "default".to_string(),
                sender_profile_did: did.clone(),
                message_id: format!("{:032x}", index + 1),
                message_nonce: format!("{:032x}", index + 107),
                recipient_endpoint_did: did.clone(),
                accepted_at: 1_800_000_000 + index,
            };
            let canonical = canonical_collaboration_acceptance_receipt_bytes(&payload).unwrap();
            let (signature_hex, signer_did) = domain_separated_sign(
                &signer,
                COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1,
                &canonical,
            );
            let envelope = SignedCollaborationAcceptanceReceipt {
                payload,
                signature: signature_hex.clone(),
                signer_did,
            };
            let bytes = canonical_signed_collaboration_acceptance_receipt_bytes(&envelope).unwrap();
            crate::collaboration_protocol::verify_stored_acceptance_receipt_envelope(&bytes)
                .unwrap();
            let mut hash = sha2::Sha256::new();
            hash.update(COLLABORATION_ACCEPTANCE_RECEIPT_SIGNATURE_DOMAIN_V1.as_bytes());
            hash.update(b"\0");
            hash.update(&canonical);
            let digest: [u8; 32] = hash.finalize().into();
            let signature = ed25519_dalek::Signature::from_bytes(
                &hex::decode(signature_hex).unwrap().try_into().unwrap(),
            );
            inputs.push((digest, signature));
        }
        // Ordinary signed authority domains consume the same math cache;
        // their callers retain all current trust/expiry/revocation checks.
        for domain in ["test.profile.authority.v1", "test.launch.authority.v1"] {
            let payload = b"immutable canonical authority evidence";
            let (signature_hex, _) = domain_separated_sign(&signer, domain, payload);
            let mut hash = sha2::Sha256::new();
            hash.update(domain.as_bytes());
            hash.update(b"\0");
            hash.update(payload);
            let digest: [u8; 32] = hash.finalize().into();
            let signature = ed25519_dalek::Signature::from_bytes(
                &hex::decode(signature_hex).unwrap().try_into().unwrap(),
            );
            inputs.push((digest, signature));
        }
        assert!(
            VERIFIED_SIGNATURE_CACHE_CAPACITY * std::mem::size_of::<VerifiedSignatureMath>()
                < 100 * 1024
        );
        for capacity in [64, VERIFIED_SIGNATURE_CACHE_CAPACITY] {
            let cache = Mutex::new(VecDeque::new());
            for (digest, signature) in &inputs {
                assert!(!verify_signature_digest_with_cache_capacity(
                    &cache, &key, digest, signature, capacity
                )
                .unwrap());
            }
            let hits = inputs
                .iter()
                .filter(|(digest, signature)| {
                    verify_signature_digest_with_cache_capacity(
                        &cache, &key, digest, signature, capacity,
                    )
                    .unwrap()
                })
                .count();
            assert_eq!(hits, if capacity == 64 { 0 } else { inputs.len() });
            let before = cache.lock().unwrap().len();
            let mut changed = inputs[0].0;
            changed[0] ^= 1;
            assert!(verify_signature_digest_with_cache_capacity(
                &cache,
                &key,
                &changed,
                &inputs[0].1,
                capacity
            )
            .is_err());
            assert_eq!(cache.lock().unwrap().len(), before);
        }
    }

    #[test]
    fn poisoned_signature_cache_repeats_full_verification() {
        let cache = Mutex::new(VecDeque::new());
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&[54; 32]);
        let key = decode_did_key(&encode_signing_key_did(&signing_key)).unwrap();
        let digest = [55; 32];
        let signature = signing_key.sign(&digest);
        verify_signature_digest_with_cache(&cache, &key, &digest, &signature).unwrap();
        assert!(std::panic::catch_unwind(|| {
            let _lock = cache.lock().unwrap();
            panic!("poison the isolated test cache");
        })
        .is_err());
        assert!(cache.is_poisoned());
        verify_signature_digest_with_cache(&cache, &key, &digest, &signature).unwrap();
        assert!(verify_signature_digest_with_cache(&cache, &key, &[56; 32], &signature).is_err());
    }
}
