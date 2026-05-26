use super::*;
use ::bitcoin::{
    absolute,
    consensus::encode::{deserialize, serialize},
    hashes::hash160,
    hashes::sha256,
    hashes::sha256d,
    hashes::Hash,
    hashes::HashEngine,
    opcodes::all::OP_RETURN,
    script::Builder,
    secp256k1::{Message, Secp256k1, XOnlyPublicKey},
    sighash::{EcdsaSighashType, Prevouts, SighashCache},
    taproot::Signature as TaprootSignature,
    Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
};
use k256::ecdsa::{
    signature::hazmat::PrehashVerifier, Signature as EcdsaSignature, SigningKey, VerifyingKey,
};
use std::str::FromStr;

pub(crate) const BITCOIN_PROOF_BIP322_SIMPLE: &str = "bip322_simple";
pub(crate) const BITCOIN_PROOF_SIGNED_MESSAGE: &str = "bitcoin_signed_message";

pub(crate) fn btc_p2wpkh_address_for_signing_key(
    signing_key: &SigningKey,
) -> Result<String, String> {
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(true);
    let secp_pubkey = ::bitcoin::secp256k1::PublicKey::from_slice(encoded.as_bytes())
        .map_err(|err| format!("invalid managed Bitcoin public key: {err}"))?;
    Ok(::bitcoin::Address::p2wpkh(
        &::bitcoin::CompressedPublicKey(secp_pubkey),
        ::bitcoin::KnownHrp::Mainnet,
    )
    .to_string())
}

pub(crate) fn sign_bip322_simple_p2wpkh_approval(
    signing_key: &SigningKey,
    request: &WalletApprovalRequest,
) -> Result<(String, Value), String> {
    validate_bitcoin_bip322_proof_payload(
        &request.payload,
        &LinkedAccount {
            account_id: request.account_id.clone(),
            principal_id: request.principal_id.clone(),
            proof_binding_id: request.proof_binding_id.clone(),
            chain_namespace: request.chain_namespace.clone(),
            address: request.address.clone(),
            proof_type: MANAGED_BTC_P2WPKH_PROOF_TYPE.to_string(),
            connector_id: None,
            label: None,
            linked_at: 0,
            revoked_at: None,
        },
    )?;
    let message = request
        .payload
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Bitcoin BIP-322 payload missing message".to_string())?;
    let address = bitcoin_address(&request.address, Network::Bitcoin)?;
    let script_pubkey = address.script_pubkey();
    let script_bytes = script_pubkey.as_bytes();
    if script_bytes.len() != 22 || script_bytes[0] != 0x00 || script_bytes[1] != 0x14 {
        return Err("managed Bitcoin account must be native P2WPKH".to_string());
    }

    let message_hash = bip322_message_hash(message.as_bytes());
    let to_spend = bip322_to_spend_tx(&message_hash, script_pubkey.clone());
    let mut to_sign = bip322_to_sign_tx(to_spend.compute_txid(), Witness::new());
    let sighash = SighashCache::new(&mut to_sign)
        .p2wpkh_signature_hash(0, &script_pubkey, Amount::ZERO, EcdsaSighashType::All)
        .map_err(|err| format!("BIP-322 sighash failed: {err}"))?;
    let (signature, _) = signing_key
        .sign_prehash_recoverable(sighash.as_byte_array())
        .map_err(|err| format!("BIP-322 signing failed: {err}"))?;
    let mut signature_der = signature.to_der().as_bytes().to_vec();
    signature_der.push(EcdsaSighashType::All as u8);
    let pubkey = signing_key.verifying_key().to_encoded_point(true);
    let mut witness = Witness::new();
    witness.push(signature_der);
    witness.push(pubkey.as_bytes());
    let signature = BASE64_STANDARD.encode(serialize(&witness));
    let payload = json!({
        "schema": "elastos.wallet.bip322_signature_payload/v1",
        "request_id": request.request_id.clone(),
        "principal_id": request.principal_id.clone(),
        "account_id": request.account_id.clone(),
        "chain_namespace": request.chain_namespace.clone(),
        "address": request.address.clone(),
        "intent": request.intent.clone(),
        "capsule_id": request.capsule_id.clone(),
        "resource": request.resource.clone(),
        "reason": request.reason.clone(),
        "message_hash": format!("0x{}", hex::encode(message_hash)),
        "signature_type": "bip322_simple",
    });
    Ok((signature, payload))
}

#[derive(Debug)]
pub(crate) struct BitcoinProofResult {
    pub(crate) chain_namespace: String,
    pub(crate) address: String,
    pub(crate) proof_type: &'static str,
    pub(crate) proof_strength: &'static str,
    pub(crate) message_hash: [u8; 32],
}

pub(crate) fn verify_bip322_simple(
    network: &str,
    address: &str,
    message: &str,
    signature: &str,
) -> Result<BitcoinProofResult, String> {
    let network = bitcoin_network(network)?;
    let address = bitcoin_address(address, network)?;
    let script_pubkey = address.script_pubkey();
    let script_bytes = script_pubkey.as_bytes();
    let message_hash = bip322_message_hash(message.as_bytes());
    let witness = bip322_simple_witness(signature)?;
    if script_bytes.len() == 22 && script_bytes[0] == 0x00 && script_bytes[1] == 0x14 {
        return verify_bip322_simple_p2wpkh(network, address, script_pubkey, message_hash, witness);
    }
    if script_bytes.len() == 34 && script_bytes[0] == 0x51 && script_bytes[1] == 0x20 {
        return verify_bip322_simple_p2tr(network, address, script_pubkey, message_hash, witness);
    }
    Err(
        "BIP-322 simple verification supports Native SegWit P2WPKH and Taproot P2TR accounts only; Legacy and Nested SegWit require bitcoin_signed_message proof"
            .to_string(),
    )
}

pub(crate) fn verify_bitcoin_signed_message(
    network: &str,
    address: &str,
    message: &str,
    signature: &str,
    public_key: &str,
) -> Result<BitcoinProofResult, String> {
    let network = bitcoin_network(network)?;
    let address = bitcoin_address(address, network)?;
    let script_pubkey = address.script_pubkey();
    if !script_pubkey.is_p2pkh() && !script_pubkey.is_p2sh() {
        return Err(
            "Bitcoin signed-message proof supports Legacy P2PKH and Nested SegWit P2SH-P2WPKH accounts only".to_string(),
        );
    }
    let public_key_bytes = hex::decode(public_key.trim())
        .map_err(|err| format!("invalid Bitcoin public key: {err}"))?;
    let compressed_key = ::bitcoin::CompressedPublicKey::from_slice(&public_key_bytes)
        .map_err(|err| format!("invalid compressed Bitcoin public key: {err}"))?;
    let expected_address = if script_pubkey.is_p2pkh() {
        ::bitcoin::Address::p2pkh(compressed_key, network)
    } else {
        ::bitcoin::Address::p2shwpkh(&compressed_key, network)
    };
    if expected_address.to_string() != address.to_string() {
        return Err("Bitcoin public key does not match selected address".to_string());
    }

    let message_hash = bitcoin_signed_message_hash(message.as_bytes());
    let signature = bitcoin_compact_signature(signature)?;
    let verifying_key = VerifyingKey::from_sec1_bytes(&public_key_bytes)
        .map_err(|err| format!("invalid Bitcoin public key: {err}"))?;
    verifying_key
        .verify_prehash(&message_hash, &signature)
        .map_err(|_| "invalid Bitcoin signed-message signature".to_string())?;

    Ok(BitcoinProofResult {
        chain_namespace: bitcoin_chain_namespace(network),
        address: address.to_string(),
        proof_type: BITCOIN_PROOF_SIGNED_MESSAGE,
        proof_strength: "standard",
        message_hash,
    })
}

pub(crate) fn verify_bitcoin_proof_for_type(
    expected_proof_type: &str,
    requested_signature_type: &str,
    network: &str,
    address: &str,
    message: &str,
    signature: &str,
    public_key: Option<&str>,
) -> Result<BitcoinProofResult, String> {
    match (expected_proof_type, requested_signature_type) {
        (BITCOIN_PROOF_BIP322_SIMPLE, BITCOIN_PROOF_BIP322_SIMPLE)
        | (BITCOIN_PROOF_BIP322_SIMPLE, "bip322-simple") => {
            verify_bip322_simple(network, address, message, signature)
        }
        (BITCOIN_PROOF_SIGNED_MESSAGE, BITCOIN_PROOF_SIGNED_MESSAGE)
        | (BITCOIN_PROOF_SIGNED_MESSAGE, "ecdsa") => verify_bitcoin_signed_message(
            network,
            address,
            message,
            signature,
            public_key.ok_or_else(|| "Bitcoin signed-message proof missing public key".to_string())?,
        ),
        _ => Err(format!(
            "Bitcoin signature type {requested_signature_type} does not match required proof type {expected_proof_type}"
        )),
    }
}

pub(crate) fn bitcoin_signature_type_for_proof_type(proof_type: &str) -> &'static str {
    match proof_type {
        BITCOIN_PROOF_SIGNED_MESSAGE => "ecdsa",
        _ => BITCOIN_PROOF_BIP322_SIMPLE,
    }
}

fn bitcoin_compact_signature(signature: &str) -> Result<EcdsaSignature, String> {
    let signature_bytes = BASE64_STANDARD
        .decode(signature)
        .map_err(|err| format!("invalid Bitcoin signed-message base64 signature: {err}"))?;
    let compact = match signature_bytes.len() {
        65 => &signature_bytes[1..65],
        64 => signature_bytes.as_slice(),
        _ => return Err("Bitcoin signed-message signature must be 64 or 65 bytes".to_string()),
    };
    EcdsaSignature::try_from(compact)
        .map_err(|err| format!("invalid Bitcoin signed-message signature: {err}"))
}

pub(crate) fn bitcoin_signed_message_hash(message: &[u8]) -> [u8; 32] {
    let mut bytes = b"\x18Bitcoin Signed Message:\n".to_vec();
    push_compact_size(message.len() as u64, &mut bytes);
    bytes.extend_from_slice(message);
    sha256d::Hash::hash(&bytes).to_byte_array()
}

fn push_compact_size(value: u64, bytes: &mut Vec<u8>) {
    if value < 253 {
        bytes.push(value as u8);
    } else if value <= 0xffff {
        bytes.push(253);
        bytes.extend_from_slice(&(value as u16).to_le_bytes());
    } else if value <= 0xffff_ffff {
        bytes.push(254);
        bytes.extend_from_slice(&(value as u32).to_le_bytes());
    } else {
        bytes.push(255);
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) fn bitcoin_proof_type_for_address(
    network: &str,
    address: &str,
) -> Result<&'static str, String> {
    let network = bitcoin_network(network)?;
    let address = bitcoin_address(address, network)?;
    let script_pubkey = address.script_pubkey();
    let script_bytes = script_pubkey.as_bytes();
    if (script_bytes.len() == 22 && script_bytes[0] == 0x00 && script_bytes[1] == 0x14)
        || (script_bytes.len() == 34 && script_bytes[0] == 0x51 && script_bytes[1] == 0x20)
    {
        return Ok(BITCOIN_PROOF_BIP322_SIMPLE);
    }
    if script_pubkey.is_p2pkh() || script_pubkey.is_p2sh() {
        return Ok(BITCOIN_PROOF_SIGNED_MESSAGE);
    }
    Err("unsupported Bitcoin address type".to_string())
}

fn bip322_simple_witness(signature: &str) -> Result<Witness, String> {
    let witness_bytes = BASE64_STANDARD
        .decode(signature)
        .map_err(|err| format!("invalid BIP-322 base64 signature: {err}"))?;
    deserialize(&witness_bytes).map_err(|err| format!("invalid BIP-322 simple witness: {err}"))
}

fn verify_bip322_simple_p2wpkh(
    network: Network,
    address: ::bitcoin::Address,
    script_pubkey: ScriptBuf,
    message_hash: [u8; 32],
    witness: Witness,
) -> Result<BitcoinProofResult, String> {
    let script_bytes = script_pubkey.as_bytes();
    let pubkey_hash = &script_bytes[2..22];
    if witness.len() != 2 {
        return Err("BIP-322 P2WPKH proof must contain signature and public key".to_string());
    }
    let signature_with_hash_type = witness
        .iter()
        .next()
        .ok_or_else(|| "BIP-322 signature witness is missing".to_string())?;
    if signature_with_hash_type.len() < 2 {
        return Err("BIP-322 signature witness is too short".to_string());
    }
    let sighash_type = *signature_with_hash_type
        .last()
        .ok_or_else(|| "BIP-322 signature sighash flag is missing".to_string())?;
    if sighash_type != EcdsaSighashType::All as u8 {
        return Err("BIP-322 P2WPKH proof must use SIGHASH_ALL".to_string());
    }
    let signature_der = signature_with_hash_type[..signature_with_hash_type.len() - 1].to_vec();
    let pubkey = witness
        .iter()
        .nth(1)
        .ok_or_else(|| "BIP-322 public key witness is missing".to_string())?;
    let pubkey = pubkey.to_vec();
    let actual_pubkey_hash = hash160::Hash::hash(&pubkey);
    if actual_pubkey_hash.as_byte_array().as_slice() != pubkey_hash {
        return Err("BIP-322 public key does not match Bitcoin address".to_string());
    }

    let to_spend = bip322_to_spend_tx(&message_hash, script_pubkey.clone());
    let mut to_sign = bip322_to_sign_tx(to_spend.compute_txid(), witness);
    let sighash = SighashCache::new(&mut to_sign)
        .p2wpkh_signature_hash(0, &script_pubkey, Amount::ZERO, EcdsaSighashType::All)
        .map_err(|err| format!("BIP-322 sighash failed: {err}"))?;
    let verifying_key = VerifyingKey::from_sec1_bytes(&pubkey)
        .map_err(|err| format!("invalid BIP-322 public key: {err}"))?;
    let signature = EcdsaSignature::from_der(&signature_der)
        .map_err(|err| format!("invalid BIP-322 DER signature: {err}"))?;
    verifying_key
        .verify_prehash(sighash.as_byte_array(), &signature)
        .map_err(|_| "invalid BIP-322 signature".to_string())?;

    Ok(BitcoinProofResult {
        chain_namespace: bitcoin_chain_namespace(network),
        address: address.to_string(),
        proof_type: "bip322_simple",
        proof_strength: "standard",
        message_hash,
    })
}

fn verify_bip322_simple_p2tr(
    network: Network,
    address: ::bitcoin::Address,
    script_pubkey: ScriptBuf,
    message_hash: [u8; 32],
    witness: Witness,
) -> Result<BitcoinProofResult, String> {
    if witness.len() != 1 {
        return Err("BIP-322 P2TR proof must contain one Schnorr signature".to_string());
    }
    let signature_bytes = witness
        .iter()
        .next()
        .ok_or_else(|| "BIP-322 Taproot signature witness is missing".to_string())?;
    let signature = TaprootSignature::from_slice(signature_bytes)
        .map_err(|err| format!("invalid BIP-322 Taproot signature: {err}"))?;
    let script_bytes = script_pubkey.as_bytes();
    let output_key = XOnlyPublicKey::from_slice(&script_bytes[2..34])
        .map_err(|err| format!("invalid Taproot output key: {err}"))?;
    let to_spend = bip322_to_spend_tx(&message_hash, script_pubkey.clone());
    let mut to_sign = bip322_to_sign_tx(to_spend.compute_txid(), witness);
    let prevouts = vec![TxOut {
        value: Amount::ZERO,
        script_pubkey,
    }];
    let sighash = SighashCache::new(&mut to_sign)
        .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), signature.sighash_type)
        .map_err(|err| format!("BIP-322 Taproot sighash failed: {err}"))?;
    let message = Message::from_digest(sighash.to_byte_array());
    Secp256k1::verification_only()
        .verify_schnorr(&signature.signature, &message, &output_key)
        .map_err(|_| "invalid BIP-322 Taproot signature".to_string())?;

    Ok(BitcoinProofResult {
        chain_namespace: bitcoin_chain_namespace(network),
        address: address.to_string(),
        proof_type: "bip322_simple",
        proof_strength: "standard",
        message_hash,
    })
}

pub(crate) fn bip322_message_hash(message: &[u8]) -> [u8; 32] {
    let tag_hash = sha256::Hash::hash(b"BIP0322-signed-message");
    let mut engine = sha256::Hash::engine();
    engine.input(tag_hash.as_byte_array());
    engine.input(tag_hash.as_byte_array());
    engine.input(message);
    sha256::Hash::from_engine(engine).to_byte_array()
}

pub(crate) fn bip322_to_spend_tx(message_hash: &[u8; 32], script_pubkey: ScriptBuf) -> Transaction {
    let mut script_sig = Vec::with_capacity(34);
    script_sig.push(0x00);
    script_sig.push(0x20);
    script_sig.extend_from_slice(message_hash);
    Transaction {
        version: ::bitcoin::transaction::Version(0),
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from_bytes(script_sig),
            sequence: Sequence::ZERO,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey,
        }],
    }
}

pub(crate) fn bip322_to_sign_tx(previous_txid: ::bitcoin::Txid, witness: Witness) -> Transaction {
    Transaction {
        version: ::bitcoin::transaction::Version(0),
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: previous_txid,
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ZERO,
            witness,
        }],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: Builder::new().push_opcode(OP_RETURN).into_script(),
        }],
    }
}

pub(crate) fn validate_bitcoin_address(address: &str, network: &str) -> Result<(), String> {
    bitcoin_address(address, bitcoin_network(network)?).map(|_| ())
}

pub(crate) fn validate_bitcoin_network(network: &str) -> Result<(), String> {
    bitcoin_network(network).map(|_| ())
}

pub(crate) fn bitcoin_address(
    address: &str,
    network: Network,
) -> Result<::bitcoin::Address, String> {
    ::bitcoin::Address::from_str(address)
        .map_err(|err| format!("invalid Bitcoin address: {err}"))?
        .require_network(network)
        .map_err(|err| format!("Bitcoin address network mismatch: {err}"))
}

pub(crate) fn bitcoin_network(network: &str) -> Result<Network, String> {
    match network {
        "bitcoin" | "mainnet" | "btc-mainnet" => Ok(Network::Bitcoin),
        _ => Err("unsupported Bitcoin proof network".to_string()),
    }
}

pub(crate) fn bitcoin_chain_namespace(network: Network) -> String {
    match network {
        Network::Bitcoin => format!("bip122:{BITCOIN_MAINNET_BIP122}"),
        _ => "bip122:unsupported".to_string(),
    }
}

pub(crate) fn validate_bitcoin_bip322_proof_payload(
    payload: &Value,
    account: &LinkedAccount,
) -> Result<(), String> {
    if payload.get("schema").and_then(Value::as_str)
        != Some("elastos.wallet.bitcoin_bip322_request/v1")
    {
        return Err("Bitcoin BIP-322 payload has unsupported schema".to_string());
    }
    if payload.get("wallet_intent").and_then(Value::as_str) != Some("bitcoin_bip322_proof") {
        return Err("Bitcoin BIP-322 payload has mismatched wallet_intent".to_string());
    }
    if payload.get("chain_namespace").and_then(Value::as_str)
        != Some(account.chain_namespace.as_str())
    {
        return Err("Bitcoin BIP-322 payload chain does not match account".to_string());
    }
    if payload.get("address").and_then(Value::as_str) != Some(account.address.as_str()) {
        return Err("Bitcoin BIP-322 payload address does not match account".to_string());
    }
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| "Bitcoin BIP-322 payload missing message".to_string())?;
    if message.is_empty() || message.len() > MAX_APPROVAL_PAYLOAD_BYTES {
        return Err("Bitcoin BIP-322 message size is invalid".to_string());
    }
    if !message.contains("wants you to prove Bitcoin account ownership") {
        return Err("Bitcoin BIP-322 message must be a Runtime account proof".to_string());
    }
    if !message.contains(&account.address) {
        return Err("Bitcoin BIP-322 message address does not match account".to_string());
    }
    if !message.lines().any(|line| {
        line.trim()
            .starts_with("- elastos://auth/bitcoin-challenge/")
    }) {
        return Err("Bitcoin BIP-322 message must bind a Runtime bitcoin challenge".to_string());
    }
    Ok(())
}

pub(crate) fn bitcoin_challenge_id_from_message(message: &str) -> Option<&str> {
    message.lines().find_map(|line| {
        line.trim()
            .strip_prefix("- elastos://auth/bitcoin-challenge/")
            .filter(|challenge_id| !challenge_id.trim().is_empty())
    })
}
