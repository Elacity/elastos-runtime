use super::*;
use bitcoin::key::TapTweak;

pub(super) fn init_provider(dir: &Path) -> WalletProvider {
    let mut provider = WalletProvider::new();
    let response = provider.handle(Request::Init {
        config: json!({ "base_path": dir.display().to_string() }),
    });
    match response {
        Response::Ok { .. } => provider,
        other => panic!("expected init ok, got {other:?}"),
    }
}

pub(super) fn test_address(signing_key: &SigningKey) -> String {
    let verifying_key = signing_key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let digest = Keccak256::digest(&encoded.as_bytes()[1..]);
    format!("0x{}", hex::encode(&digest[12..]))
}

pub(super) fn sign_message(signing_key: &SigningKey, message: &str) -> String {
    sign_message_bytes(signing_key, message.as_bytes())
}

pub(super) fn sign_message_bytes(signing_key: &SigningKey, message: &[u8]) -> String {
    let hash = ethereum_signed_message_hash(message);
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&hash)
        .expect("test signature should be recoverable");
    let mut bytes = signature.to_bytes().to_vec();
    bytes.push(recovery_id.to_byte());
    format!("0x{}", hex::encode(bytes))
}

pub(super) fn bip322_test_signing_key() -> SigningKey {
    SigningKey::from_slice(&[7_u8; 32]).expect("deterministic test key")
}

pub(super) fn bip322_test_address(signing_key: &SigningKey) -> String {
    let pubkey = signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec();
    let secp_pubkey =
        bitcoin::secp256k1::PublicKey::from_slice(&pubkey).expect("deterministic test public key");
    bitcoin::Address::p2wpkh(
        &bitcoin::CompressedPublicKey(secp_pubkey),
        bitcoin::KnownHrp::Mainnet,
    )
    .to_string()
}

pub(super) fn bitcoin_p2pkh_test_address(signing_key: &SigningKey) -> String {
    let pubkey = signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec();
    let compressed =
        bitcoin::CompressedPublicKey::from_slice(&pubkey).expect("deterministic test public key");
    bitcoin::Address::p2pkh(compressed, bitcoin::Network::Bitcoin).to_string()
}

pub(super) fn bitcoin_p2shwpkh_test_address(signing_key: &SigningKey) -> String {
    let pubkey = signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec();
    let compressed =
        bitcoin::CompressedPublicKey::from_slice(&pubkey).expect("deterministic test public key");
    bitcoin::Address::p2shwpkh(&compressed, bitcoin::Network::Bitcoin).to_string()
}

pub(super) fn bitcoin_test_public_key(signing_key: &SigningKey) -> String {
    hex::encode(
        signing_key
            .verifying_key()
            .to_encoded_point(true)
            .as_bytes(),
    )
}

pub(super) fn sign_bitcoin_message(signing_key: &SigningKey, message: &str) -> String {
    let hash = bitcoin_signed_message_hash(message.as_bytes());
    let (signature, recovery_id) = signing_key
        .sign_prehash_recoverable(&hash)
        .expect("test Bitcoin message signature");
    let mut bytes = vec![27 + recovery_id.to_byte()];
    bytes.extend_from_slice(signature.to_bytes().as_ref());
    BASE64_STANDARD.encode(bytes)
}

pub(super) fn sign_bip322_simple_p2wpkh(
    signing_key: &SigningKey,
    address: &str,
    message: &str,
) -> String {
    let address = bitcoin_address(address, Network::Bitcoin).expect("test vector address");
    let script_pubkey = address.script_pubkey();
    let message_hash = bip322_message_hash(message.as_bytes());
    let to_spend = bip322_to_spend_tx(&message_hash, script_pubkey.clone());
    let mut to_sign = bip322_to_sign_tx(to_spend.compute_txid(), Witness::new());
    let sighash = SighashCache::new(&mut to_sign)
        .p2wpkh_signature_hash(0, &script_pubkey, Amount::ZERO, EcdsaSighashType::All)
        .expect("test BIP-322 sighash");
    let (signature, _) = signing_key
        .sign_prehash_recoverable(sighash.as_byte_array())
        .expect("test BIP-322 signature");
    let mut signature_bytes = signature.to_der().as_bytes().to_vec();
    signature_bytes.push(EcdsaSighashType::All as u8);
    let pubkey = signing_key
        .verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec();
    let mut witness = Witness::new();
    witness.push(signature_bytes);
    witness.push(pubkey);
    BASE64_STANDARD.encode(serialize(&witness))
}

pub(super) fn bip322_test_taproot_secret_key() -> bitcoin::secp256k1::SecretKey {
    bitcoin::secp256k1::SecretKey::from_slice(&[8_u8; 32]).expect("deterministic test taproot key")
}

pub(super) fn bip322_test_taproot_address(secret_key: &bitcoin::secp256k1::SecretKey) -> String {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let keypair = bitcoin::secp256k1::Keypair::from_secret_key(&secp, secret_key);
    let (xonly, _) = keypair.x_only_public_key();
    bitcoin::Address::p2tr(&secp, xonly, None, bitcoin::KnownHrp::Mainnet).to_string()
}

pub(super) fn sign_bip322_simple_p2tr(
    secret_key: &bitcoin::secp256k1::SecretKey,
    address: &str,
    message: &str,
) -> String {
    let secp = bitcoin::secp256k1::Secp256k1::new();
    let keypair = bitcoin::secp256k1::Keypair::from_secret_key(&secp, secret_key);
    let tweaked_keypair: bitcoin::secp256k1::Keypair = keypair.tap_tweak(&secp, None).into();
    let address = bitcoin_address(address, Network::Bitcoin).expect("test vector address");
    let script_pubkey = address.script_pubkey();
    let message_hash = bip322_message_hash(message.as_bytes());
    let to_spend = bip322_to_spend_tx(&message_hash, script_pubkey.clone());
    let mut to_sign = bip322_to_sign_tx(to_spend.compute_txid(), Witness::new());
    let prevouts = vec![bitcoin::TxOut {
        value: Amount::ZERO,
        script_pubkey,
    }];
    let sighash = SighashCache::new(&mut to_sign)
        .taproot_key_spend_signature_hash(
            0,
            &bitcoin::sighash::Prevouts::All(&prevouts),
            bitcoin::TapSighashType::Default,
        )
        .expect("test BIP-322 Taproot sighash");
    let message = bitcoin::secp256k1::Message::from_digest(sighash.to_byte_array());
    let signature = bitcoin::taproot::Signature {
        signature: secp.sign_schnorr_no_aux_rand(&message, &tweaked_keypair),
        sighash_type: bitcoin::TapSighashType::Default,
    };
    let mut witness = Witness::new();
    witness.push(signature.to_vec());
    BASE64_STANDARD.encode(serialize(&witness))
}

pub(super) fn transaction_intent_payload(from: &str) -> Value {
    json!({
        "schema": "elastos.chain.unsigned_transaction_intent/v1",
        "transaction_type": "eip155_legacy",
        "from": from,
        "to": "0x0000000000000000000000000000000000000002",
        "value": "0x0",
        "data": "0x",
        "chain_id": 20,
        "nonce": "0x7",
        "gas_price": "0x3b9aca00",
        "gas_limit": "0x5208",
        "requires_wallet_approval": true,
        "wallet_intent": "transaction_intent"
    })
}

pub(super) fn bitcoin_bip322_payload(address: &str, message: &str) -> Value {
    json!({
        "schema": "elastos.wallet.bitcoin_bip322_request/v1",
        "wallet_intent": "bitcoin_bip322_proof",
        "chain_namespace": BITCOIN_MAINNET_CHAIN_NAMESPACE,
        "address": address,
        "message": message
    })
}

pub(super) fn erc1271_proof(message: &str, signature: &str, contract: &str, valid: bool) -> Value {
    let message_hash = ethereum_signed_message_hash(message.as_bytes());
    let signature_bytes = hex_prefixed_bytes(signature, None, "signature").unwrap();
    json!({
        "schema": "elastos.chain.erc1271_proof/v1",
        "network": {
            "id": "esc-local",
            "chain_id": 20
        },
        "chain_id": 20,
        "contract": normalize_evm_address(contract),
        "message_hash": format!("0x{}", hex::encode(message_hash)),
        "signature_hash": bytes_hash(&signature_bytes),
        "valid": valid,
        "magic_value": if valid { "0x1626ba7e" } else { "0xffffffff" },
        "checked_at": now_ts()
    })
}
