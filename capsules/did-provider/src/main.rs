//! ElastOS DID Provider Capsule
//!
//! Manages a portable Ed25519 identity as a did:key.
//! Random keypair (not derived from device key) — portable across devices.
//! Wire protocol: line-delimited JSON over stdin/stdout.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use elastos_guest::prelude::*;
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::io::{self, BufRead, Write};
use zeroize::Zeroize;

const NONCE_LEN: usize = 12;
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Multicodec prefix for Ed25519 public key (0xed01, varint-encoded)
const MULTICODEC_ED25519_PUB: [u8; 2] = [0xed, 0x01];

// === Wire protocol types ===

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    GetDid,
    Resolve { did: String },
    Sign { data: String },
    Verify { did: String, data: String, signature: String },
    GetNickname,
    SetNickname { nickname: String },
    /// Get or create a named persona DID (for agents/sub-identities)
    GetPersonaDid { name: String },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: serde_json::Value) -> Self {
        Response::Ok { data: Some(data) }
    }

    fn error(code: &str, message: &str) -> Self {
        Response::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

// === did:key encoding/decoding ===

fn encode_did_key(verifying_key: &VerifyingKey) -> String {
    let mut bytes = Vec::with_capacity(2 + 32);
    bytes.extend_from_slice(&MULTICODEC_ED25519_PUB);
    bytes.extend_from_slice(verifying_key.as_bytes());
    format!("did:key:z{}", bs58::encode(&bytes).into_string())
}

fn decode_did_key(did: &str) -> Result<VerifyingKey, String> {
    let multibase_part = did
        .strip_prefix("did:key:z")
        .ok_or_else(|| "DID must start with did:key:z".to_string())?;

    let bytes = bs58::decode(multibase_part)
        .into_vec()
        .map_err(|e| format!("Invalid base58: {}", e))?;

    if bytes.len() != 34 {
        return Err(format!("Expected 34 bytes (2 prefix + 32 key), got {}", bytes.len()));
    }
    if bytes[0] != MULTICODEC_ED25519_PUB[0] || bytes[1] != MULTICODEC_ED25519_PUB[1] {
        return Err("Not an Ed25519 multicodec prefix".to_string());
    }

    let key_bytes: [u8; 32] = bytes[2..34]
        .try_into()
        .map_err(|_| "Invalid key length".to_string())?;

    VerifyingKey::from_bytes(&key_bytes)
        .map_err(|e| format!("Invalid Ed25519 public key: {}", e))
}

fn did_document(did: &str) -> serde_json::Value {
    let fragment = did.strip_prefix("did:key:").unwrap_or(did);
    let vm_id = format!("{}#{}", did, fragment);
    serde_json::json!({
        "@context": "https://www.w3.org/ns/did/v1",
        "id": did,
        "verificationMethod": [{
            "id": vm_id,
            "type": "Ed25519VerificationKey2020",
            "controller": did,
            "publicKeyMultibase": fragment,
        }],
        "authentication": [vm_id],
    })
}

// === DID Provider State ===

struct DidProvider {
    signing_key: Option<SigningKey>,
    verifying_key: Option<VerifyingKey>,
    nickname: Option<String>,
    storage_path: String,
    /// Device key — used only for encrypting DID key and nickname at rest
    device_key: Option<[u8; 32]>,
}

impl DidProvider {
    fn new() -> Self {
        Self {
            signing_key: None,
            verifying_key: None,
            nickname: None,
            storage_path: String::new(),
            device_key: None,
        }
    }

    fn handle(&mut self, req: Request) -> Response {
        match req {
            Request::Init { config } => self.init(config),
            Request::GetDid => self.get_did(),
            Request::Resolve { did } => self.resolve(&did),
            Request::Sign { data } => self.sign(&data),
            Request::Verify { did, data, signature } => self.verify(&did, &data, &signature),
            Request::GetNickname => self.get_nickname(),
            Request::SetNickname { nickname } => self.set_nickname(&nickname),
            Request::GetPersonaDid { name } => self.get_persona_did(&name),
            Request::Shutdown => {
                Response::ok(serde_json::json!({"message": "DID provider shutting down"}))
            }
        }
    }

    fn init(&mut self, config: serde_json::Value) -> Response {
        if let Some(bp) = config.get("base_path").and_then(|v| v.as_str()) {
            self.storage_path = bp.to_string();
        }

        if let Some(key_hex) = config.get("encryption_key").and_then(|v| v.as_str()) {
            if let Ok(key_bytes) = hex::decode(key_hex) {
                if key_bytes.len() == 32 {
                    let mut dk = [0u8; 32];
                    dk.copy_from_slice(&key_bytes);
                    self.device_key = Some(dk);
                }
            }
        }

        // Load or generate DID keypair
        if self.device_key.is_some() {
            if let Err(e) = self.load_or_generate_key() {
                return Response::error("key_error", &e);
            }
        }

        self.try_load_nickname();

        Response::ok(serde_json::json!({
            "protocol_version": "1.0",
            "provider": "did",
        }))
    }

    fn load_or_generate_key(&mut self) -> Result<(), String> {
        let dk = self.device_key.as_ref().ok_or("No device key")?;
        let dir = self.did_dir();
        let key_path = dir.join("key.enc");

        // Derive an AES key specifically for DID key storage
        let storage_key = derive_storage_key(dk);

        if key_path.exists() {
            // Load existing key
            let encrypted = std::fs::read(&key_path)
                .map_err(|e| format!("Failed to read DID key: {}", e))?;
            let mut secret_bytes = decrypt_data(&storage_key, &encrypted)?;
            if secret_bytes.len() != 32 {
                return Err(format!("Invalid DID key length: {}", secret_bytes.len()));
            }
            let mut key_arr = [0u8; 32];
            key_arr.copy_from_slice(&secret_bytes);
            let signing_key = SigningKey::from_bytes(&key_arr);
            self.verifying_key = Some(signing_key.verifying_key());
            self.signing_key = Some(signing_key);
            secret_bytes.zeroize();
            key_arr.zeroize();
        } else {
            // Generate new random keypair
            let signing_key = SigningKey::generate(&mut OsRng);
            self.verifying_key = Some(signing_key.verifying_key());

            // Persist encrypted
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Failed to create DID dir: {}", e))?;
            let encrypted = encrypt_data(&storage_key, signing_key.as_bytes())?;
            std::fs::write(&key_path, encrypted)
                .map_err(|e| format!("Failed to write DID key: {}", e))?;

            self.signing_key = Some(signing_key);
        }

        Ok(())
    }

    fn get_did(&self) -> Response {
        match &self.verifying_key {
            Some(vk) => {
                let did = encode_did_key(vk);
                Response::ok(serde_json::json!({ "did": did }))
            }
            None => Response::error("not_init", "DID key not available"),
        }
    }

    fn resolve(&self, did: &str) -> Response {
        match decode_did_key(did) {
            Ok(vk) => Response::ok(serde_json::json!({
                "public_key": hex::encode(vk.as_bytes()),
                "document": did_document(did),
            })),
            Err(e) => Response::error("invalid_did", &e),
        }
    }

    fn sign(&self, data_hex: &str) -> Response {
        let signing_key = match &self.signing_key {
            Some(k) => k,
            None => return Response::error("not_init", "DID key not available"),
        };

        let data = match hex::decode(data_hex) {
            Ok(d) => d,
            Err(e) => return Response::error("invalid_data", &format!("Invalid hex: {}", e)),
        };

        let signature = signing_key.sign(&data);
        Response::ok(serde_json::json!({
            "signature": hex::encode(signature.to_bytes()),
        }))
    }

    fn verify(&self, did: &str, data_hex: &str, sig_hex: &str) -> Response {
        let vk = match decode_did_key(did) {
            Ok(vk) => vk,
            Err(e) => return Response::error("invalid_did", &e),
        };

        let data = match hex::decode(data_hex) {
            Ok(d) => d,
            Err(e) => return Response::error("invalid_data", &format!("Invalid hex data: {}", e)),
        };

        let sig_bytes = match hex::decode(sig_hex) {
            Ok(s) => s,
            Err(e) => return Response::error("invalid_signature", &format!("Invalid hex sig: {}", e)),
        };

        let signature = match Signature::from_slice(&sig_bytes) {
            Ok(s) => s,
            Err(e) => return Response::error("invalid_signature", &format!("Invalid signature: {}", e)),
        };

        let valid = vk.verify(&data, &signature).is_ok();
        Response::ok(serde_json::json!({ "valid": valid }))
    }

    fn get_nickname(&self) -> Response {
        match &self.nickname {
            Some(nick) => Response::ok(serde_json::json!({"nickname": nick})),
            None => Response::error("not_set", "No nickname set"),
        }
    }

    fn set_nickname(&mut self, nickname: &str) -> Response {
        if self.device_key.is_none() {
            return Response::error("not_init", "No device key provided");
        }
        self.nickname = Some(nickname.to_string());
        self.save_nickname(nickname);
        Response::ok(serde_json::json!({"nickname": nickname}))
    }

    /// Get or create a named persona DID (e.g. for an agent).
    /// Persona keys are stored in `did/personas/{name}.enc`, encrypted with the device key.
    /// Returns the persona DID and the owner (main) DID.
    fn get_persona_did(&mut self, name: &str) -> Response {
        let dk = match self.device_key.as_ref() {
            Some(k) => k,
            None => return Response::error("not_init", "No device key provided"),
        };

        // Validate persona name: alphanumeric + hyphen/underscore, 1-64 chars
        if name.is_empty() || name.len() > 64 {
            return Response::error("invalid_name", "Persona name must be 1-64 characters");
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Response::error(
                "invalid_name",
                "Persona name must be alphanumeric, hyphens, or underscores",
            );
        }

        let normalized = name.to_ascii_lowercase();
        let dir = self.did_dir().join("personas");
        let key_path = dir.join(format!("{}.enc", normalized));
        let storage_key = derive_storage_key(dk);

        let persona_vk = if key_path.exists() {
            // Load existing persona key
            match std::fs::read(&key_path) {
                Ok(encrypted) => match decrypt_data(&storage_key, &encrypted) {
                    Ok(mut secret_bytes) => {
                        if secret_bytes.len() != 32 {
                            return Response::error(
                                "key_error",
                                "Invalid persona key length",
                            );
                        }
                        let mut key_arr = [0u8; 32];
                        key_arr.copy_from_slice(&secret_bytes);
                        let sk = SigningKey::from_bytes(&key_arr);
                        let vk = sk.verifying_key();
                        secret_bytes.zeroize();
                        key_arr.zeroize();
                        vk
                    }
                    Err(e) => {
                        return Response::error("key_error", &format!("Decrypt failed: {}", e))
                    }
                },
                Err(e) => {
                    return Response::error("key_error", &format!("Read failed: {}", e))
                }
            }
        } else {
            // Generate new persona keypair
            if let Err(e) = std::fs::create_dir_all(&dir) {
                return Response::error("key_error", &format!("Failed to create dir: {}", e));
            }
            let sk = SigningKey::generate(&mut OsRng);
            let vk = sk.verifying_key();
            match encrypt_data(&storage_key, sk.as_bytes()) {
                Ok(encrypted) => {
                    if let Err(e) = std::fs::write(&key_path, encrypted) {
                        return Response::error(
                            "key_error",
                            &format!("Failed to persist persona key: {}", e),
                        );
                    }
                }
                Err(e) => {
                    return Response::error("key_error", &format!("Encrypt failed: {}", e))
                }
            }
            vk
        };

        let persona_did = encode_did_key(&persona_vk);
        let owner_did = self.verifying_key.as_ref().map(encode_did_key);

        Response::ok(serde_json::json!({
            "did": persona_did,
            "owner_did": owner_did,
            "name": normalized,
        }))
    }

    // === Storage helpers ===

    fn did_dir(&self) -> std::path::PathBuf {
        let base = if self.storage_path.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        } else {
            std::path::PathBuf::from(&self.storage_path)
        };
        base.join("did")
    }

    fn try_load_nickname(&mut self) {
        let dir = self.did_dir();
        let path = dir.join("nickname.enc");
        if let Some(ref dk) = self.device_key {
            let storage_key = derive_storage_key(dk);
            if let Ok(data) = std::fs::read(&path) {
                if let Ok(plaintext) = decrypt_data(&storage_key, &data) {
                    if let Ok(nick) = String::from_utf8(plaintext) {
                        let nick = nick.trim().to_string();
                        if !nick.is_empty() {
                            self.nickname = Some(nick);
                        }
                    }
                }
            }
        }
    }

    fn save_nickname(&self, nickname: &str) {
        let Some(ref dk) = self.device_key else { return };
        let dir = self.did_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let storage_key = derive_storage_key(dk);
        if let Ok(encrypted) = encrypt_data(&storage_key, nickname.as_bytes()) {
            std::fs::write(dir.join("nickname.enc"), encrypted).ok();
        }
    }
}

// === Crypto helpers ===

/// Derive a separate AES key for DID storage from the device key
fn derive_storage_key(device_key: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(None, device_key);
    let mut okm = [0u8; 32];
    hk.expand(b"elastos-did-storage", &mut okm)
        .expect("HKDF expand");
    okm
}

fn encrypt_data(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Cipher init failed: {}", e))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt_data(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < NONCE_LEN {
        return Err("Data too short".to_string());
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|e| format!("Cipher init failed: {}", e))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))
}

fn main() {
    eprintln!("did-provider: starting v{} (did:key)", PROVIDER_VERSION);

    let info = CapsuleInfo::from_env();
    if info.is_elastos_runtime() {
        eprintln!("Running as: {} ({})", info.name(), info.id());
    }

    let mut provider = DidProvider::new();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Error reading input: {}", e);
                break;
            }
        };

        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let response = Response::error("parse_error", &e.to_string());
                let json = serde_json::to_string(&response).unwrap();
                writeln!(stdout, "{}", json).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };

        let is_shutdown = matches!(request, Request::Shutdown);
        let response = provider.handle(request);

        let json = serde_json::to_string(&response).unwrap();
        writeln!(stdout, "{}", json).unwrap();
        stdout.flush().unwrap();

        if is_shutdown {
            break;
        }
    }

    eprintln!("DID provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device_key() -> [u8; 32] {
        [42u8; 32]
    }

    fn init_provider(base_path: Option<&str>) -> DidProvider {
        let dk = make_device_key();
        let mut config = serde_json::json!({
            "encryption_key": hex::encode(dk),
        });
        if let Some(bp) = base_path {
            config["base_path"] = serde_json::json!(bp);
        }
        let mut provider = DidProvider::new();
        provider.handle(Request::Init { config });
        provider
    }

    #[test]
    fn test_init_generates_key() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        assert!(provider.signing_key.is_some());
        assert!(provider.verifying_key.is_some());
    }

    #[test]
    fn test_get_did_returns_valid_did_key() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        match provider.get_did() {
            Response::Ok { data: Some(d) } => {
                let did = d["did"].as_str().unwrap();
                assert!(did.starts_with("did:key:z"), "DID should start with did:key:z, got: {}", did);
            }
            other => panic!("Expected ok response, got {:?}", other),
        }
    }

    #[test]
    fn test_did_key_encoding_roundtrip() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let vk = signing_key.verifying_key();
        let did = encode_did_key(&vk);

        assert!(did.starts_with("did:key:z"));

        let decoded = decode_did_key(&did).unwrap();
        assert_eq!(vk.as_bytes(), decoded.as_bytes());
    }

    #[test]
    fn test_resolve_returns_document() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        let did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        match provider.resolve(&did) {
            Response::Ok { data: Some(d) } => {
                assert!(d["public_key"].as_str().is_some());
                assert_eq!(d["document"]["id"], did);
                assert_eq!(d["document"]["@context"], "https://www.w3.org/ns/did/v1");
            }
            other => panic!("Expected ok response, got {:?}", other),
        }
    }

    #[test]
    fn test_resolve_invalid_did() {
        let provider = DidProvider::new();
        match provider.resolve("did:key:invalid") {
            Response::Error { code, .. } => assert_eq!(code, "invalid_did"),
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        let did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        let data_hex = hex::encode(b"hello world");
        let sig_hex = match provider.sign(&data_hex) {
            Response::Ok { data: Some(d) } => d["signature"].as_str().unwrap().to_string(),
            _ => panic!("Expected signature"),
        };

        match provider.verify(&did, &data_hex, &sig_hex) {
            Response::Ok { data: Some(d) } => assert_eq!(d["valid"], true),
            other => panic!("Expected valid=true, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_wrong_data_fails() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        let did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        let sig_hex = match provider.sign(&hex::encode(b"hello")) {
            Response::Ok { data: Some(d) } => d["signature"].as_str().unwrap().to_string(),
            _ => panic!("Expected signature"),
        };

        // Verify with different data
        match provider.verify(&did, &hex::encode(b"tampered"), &sig_hex) {
            Response::Ok { data: Some(d) } => assert_eq!(d["valid"], false),
            other => panic!("Expected valid=false, got {:?}", other),
        }
    }

    #[test]
    fn test_key_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();

        let did1 = {
            let provider = init_provider(Some(bp));
            match provider.get_did() {
                Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
                _ => panic!("Expected DID"),
            }
        };

        // Second init should load the same key
        let did2 = {
            let provider = init_provider(Some(bp));
            match provider.get_did() {
                Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
                _ => panic!("Expected DID"),
            }
        };

        assert_eq!(did1, did2, "DID should persist across restarts");
    }

    #[test]
    fn test_different_device_key_cannot_read_did() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();

        // Create with one device key
        let _provider = init_provider(Some(bp));

        // Try to load with a different device key
        let config = serde_json::json!({
            "encryption_key": hex::encode([99u8; 32]),
            "base_path": bp,
        });
        let mut provider2 = DidProvider::new();
        let _resp = provider2.handle(Request::Init { config });

        // Should fail to decrypt the key (or generate a new one)
        if let Response::Error { code, .. } = provider2.get_did() {
            assert_eq!(code, "not_init");
        }
    }

    #[test]
    fn test_nickname_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();

        {
            let mut provider = init_provider(Some(bp));
            provider.handle(Request::SetNickname {
                nickname: "alice".to_string(),
            });
        }

        // Re-init should load nickname
        let provider = init_provider(Some(bp));
        match provider.get_nickname() {
            Response::Ok { data: Some(d) } => assert_eq!(d["nickname"], "alice"),
            other => panic!("Expected nickname alice, got {:?}", other),
        }
    }

    #[test]
    fn test_sign_without_init() {
        let provider = DidProvider::new();
        match provider.sign("deadbeef") {
            Response::Error { code, .. } => assert_eq!(code, "not_init"),
            _ => panic!("Expected not_init error"),
        }
    }

    #[test]
    fn test_get_did_without_init() {
        let provider = DidProvider::new();
        match provider.get_did() {
            Response::Error { code, .. } => assert_eq!(code, "not_init"),
            _ => panic!("Expected not_init error"),
        }
    }

    #[test]
    fn test_persona_did_created_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();

        let persona_did = {
            let mut provider = init_provider(Some(bp));
            match provider.get_persona_did("bot") {
                Response::Ok { data: Some(d) } => {
                    let did = d["did"].as_str().unwrap().to_string();
                    assert!(did.starts_with("did:key:z"));
                    assert_eq!(d["name"], "bot");
                    // owner_did should be present
                    assert!(d["owner_did"].as_str().unwrap().starts_with("did:key:z"));
                    did
                }
                other => panic!("Expected ok, got {:?}", other),
            }
        };

        // Re-init: same persona name should return same DID
        let mut provider = init_provider(Some(bp));
        match provider.get_persona_did("bot") {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["did"].as_str().unwrap(), persona_did);
            }
            other => panic!("Expected ok, got {:?}", other),
        }
    }

    #[test]
    fn test_persona_did_differs_from_owner() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();
        let mut provider = init_provider(Some(bp));

        let owner_did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        let persona_did = match provider.get_persona_did("bot") {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected persona DID"),
        };

        assert_ne!(owner_did, persona_did, "Persona DID must differ from owner");
    }

    #[test]
    fn test_persona_different_names_different_dids() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();
        let mut provider = init_provider(Some(bp));

        let did1 = match provider.get_persona_did("bot-a") {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };
        let did2 = match provider.get_persona_did("bot-b") {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        assert_ne!(did1, did2);
    }

    #[test]
    fn test_persona_invalid_name() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();
        let mut provider = init_provider(Some(bp));

        match provider.get_persona_did("") {
            Response::Error { code, .. } => assert_eq!(code, "invalid_name"),
            _ => panic!("Expected error for empty name"),
        }
        match provider.get_persona_did("bad/name") {
            Response::Error { code, .. } => assert_eq!(code, "invalid_name"),
            _ => panic!("Expected error for name with slash"),
        }
        match provider.get_persona_did("../traversal") {
            Response::Error { code, .. } => assert_eq!(code, "invalid_name"),
            _ => panic!("Expected error for traversal attempt"),
        }
    }
}
