use elastos_auth::ethereum_signed_message_hash;
use k256::ecdsa::{RecoveryId, Signature as EcdsaSignature, SigningKey, VerifyingKey};
use sha3::{Digest, Keccak256};

pub(crate) fn evm_address_for_signing_key(signing_key: &SigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    format!("0x{}", hex::encode(&digest[12..])).to_ascii_lowercase()
}

pub(crate) fn sign_evm_message(signing_key: &SigningKey, message: &[u8]) -> Result<String, String> {
    let hash = ethereum_signed_message_hash(message);
    sign_evm_prehash(signing_key, &hash)
}

pub(crate) fn sign_evm_prehash(
    signing_key: &SigningKey,
    hash: &[u8; 32],
) -> Result<String, String> {
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(hash)
        .map_err(|err| err.to_string())?;
    let mut bytes = signature.to_bytes().to_vec();
    bytes.push(recovery_id.to_byte());
    Ok(format!("0x{}", hex::encode(bytes)))
}

pub(crate) fn recover_evm_address_from_hash(
    hash: &[u8; 32],
    signature_hex: &str,
) -> Result<String, String> {
    let signature_hex = signature_hex.strip_prefix("0x").unwrap_or(signature_hex);
    let bytes =
        hex::decode(signature_hex).map_err(|err| format!("invalid signature hex: {err}"))?;
    if bytes.len() != 65 {
        return Err("EVM signature must be 65 bytes".to_string());
    }
    let signature = EcdsaSignature::try_from(&bytes[..64])
        .map_err(|err| format!("invalid EVM signature: {err}"))?;
    let recovery_id = normalize_evm_recovery_id(bytes[64])?;
    let verifying_key = VerifyingKey::recover_from_prehash(hash, &signature, recovery_id)
        .map_err(|err| format!("failed to recover EVM signer: {err}"))?;
    let encoded = verifying_key.to_encoded_point(false);
    let public_key = encoded.as_bytes();
    if public_key.len() != 65 {
        return Err("unexpected recovered public key length".to_string());
    }
    let digest = Keccak256::digest(&public_key[1..]);
    Ok(format!("0x{}", hex::encode(&digest[12..])))
}

pub(crate) fn canonicalize_evm_signature(signature_hex: &str) -> Result<Vec<u8>, String> {
    let signature_hex = signature_hex.strip_prefix("0x").unwrap_or(signature_hex);
    let bytes =
        hex::decode(signature_hex).map_err(|err| format!("invalid signature hex: {err}"))?;
    if bytes.len() != 65 {
        return Err("EVM signature must be 65 bytes".to_string());
    }
    let signature = EcdsaSignature::try_from(&bytes[..64])
        .map_err(|err| format!("invalid EVM signature: {err}"))?;
    if signature.normalize_s().is_some() {
        return Err("EVM signature must use canonical low-S form".to_string());
    }
    let recovery_id = normalize_evm_recovery_id(bytes[64])?;
    let mut canonical = signature.to_bytes().to_vec();
    canonical.push(recovery_id.to_byte());
    Ok(canonical)
}

pub(crate) fn normalize_evm_recovery_id(v: u8) -> Result<RecoveryId, String> {
    let id = match v {
        0 | 1 => v,
        27 | 28 => v - 27,
        _ => return Err("unsupported EVM recovery id".to_string()),
    };
    RecoveryId::try_from(id).map_err(|err| format!("invalid recovery id: {err}"))
}
