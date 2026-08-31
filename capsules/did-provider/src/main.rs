//! ElastOS DID Provider Capsule
//!
//! Manages the device-backed Ed25519 identity as a did:key.
//! The main DID is derived from the shared device key so every runtime surface
//! sees the same stable node/device identity. Human accounts link to it through
//! runtime principals and proof bindings; they do not inherit this key.
//! Wire protocol: line-delimited JSON over stdin/stdout.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use elastos_guest::prelude::*;
use elastos_identity::{decode_did_key, encode_did_key, encode_signing_key_did};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};
use zeroize::Zeroize;

const NONCE_LEN: usize = 12;
const DID_RECOVERY_PROOF_SCHEMA: &str = "elastos.did.recovery-proof/v1";
const DID_RECOVERY_MAX_TTL_SECS: u64 = 15 * 60;
const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

// === Wire protocol types ===

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    GetDid,
    Resolve {
        did: String,
    },
    SignChatMessage {
        sender_id: String,
        ts: u64,
        content: String,
    },
    Verify {
        did: String,
        data: String,
        signature: String,
    },
    VerifyDidRecovery {
        did: String,
        principal_id: String,
        localhost_root: String,
        protector_id: String,
        data_key_id: String,
        nonce: String,
        issued_at: u64,
        expires_at: u64,
        signature: String,
    },
    GetNickname,
    SetNickname {
        nickname: String,
    },
    /// Get or create a named persona DID (for agents/sub-identities)
    GetPersonaDid {
        name: String,
    },
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
            Request::SignChatMessage {
                sender_id,
                ts,
                content,
            } => self.sign_chat_message(&sender_id, ts, &content),
            Request::Verify {
                did,
                data,
                signature,
            } => self.verify(&did, &data, &signature),
            Request::VerifyDidRecovery {
                did,
                principal_id,
                localhost_root,
                protector_id,
                data_key_id,
                nonce,
                issued_at,
                expires_at,
                signature,
            } => self.verify_did_recovery(
                &did,
                &principal_id,
                &localhost_root,
                &protector_id,
                &data_key_id,
                &nonce,
                issued_at,
                expires_at,
                &signature,
            ),
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
        let (signing_key, _did) = elastos_identity::derive_did(dk);
        self.verifying_key = Some(signing_key.verifying_key());
        self.signing_key = Some(signing_key);

        Ok(())
    }

    fn get_did(&self) -> Response {
        match &self.signing_key {
            Some(signing_key) => Response::ok(serde_json::json!({
                "did": encode_signing_key_did(signing_key),
            })),
            None => Response::error("not_init", "DID key not available"),
        }
    }

    fn resolve(&self, did: &str) -> Response {
        match decode_did_key(did).map_err(|e| e.to_string()) {
            Ok(vk) => Response::ok(serde_json::json!({
                "public_key": hex::encode(vk.as_bytes()),
                "document": did_document(did),
            })),
            Err(e) => Response::error("invalid_did", &e),
        }
    }

    fn sign_bytes(&self, data: &[u8]) -> Response {
        let signing_key = match &self.signing_key {
            Some(k) => k,
            None => return Response::error("not_init", "DID key not available"),
        };

        let signature = signing_key.sign(data);
        Response::ok(serde_json::json!({
            "signature": hex::encode(signature.to_bytes()),
        }))
    }

    fn sign_chat_message(&self, sender_id: &str, ts: u64, content: &str) -> Response {
        if sender_id.trim().is_empty() || ts == 0 {
            return Response::error(
                "invalid_intent",
                "chat message signing requires sender_id and timestamp",
            );
        }
        let payload_hex =
            elastos_common::chat_protocol::signing_payload_hex(sender_id, ts, content);
        let payload = match hex::decode(payload_hex) {
            Ok(payload) => payload,
            Err(e) => {
                return Response::error("invalid_intent", &format!("Invalid chat payload: {}", e))
            }
        };
        self.sign_bytes(&payload)
    }

    fn verify(&self, did: &str, data_hex: &str, sig_hex: &str) -> Response {
        let vk = match decode_did_key(did).map_err(|e| e.to_string()) {
            Ok(vk) => vk,
            Err(e) => return Response::error("invalid_did", &e),
        };

        let data = match hex::decode(data_hex) {
            Ok(d) => d,
            Err(e) => return Response::error("invalid_data", &format!("Invalid hex data: {}", e)),
        };

        let sig_bytes = match hex::decode(sig_hex) {
            Ok(s) => s,
            Err(e) => {
                return Response::error("invalid_signature", &format!("Invalid hex sig: {}", e))
            }
        };

        let signature = match Signature::from_slice(&sig_bytes) {
            Ok(s) => s,
            Err(e) => {
                return Response::error("invalid_signature", &format!("Invalid signature: {}", e))
            }
        };

        let valid = vk.verify(&data, &signature).is_ok();
        Response::ok(serde_json::json!({ "valid": valid }))
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_did_recovery(
        &self,
        did: &str,
        principal_id: &str,
        localhost_root: &str,
        protector_id: &str,
        data_key_id: &str,
        nonce: &str,
        issued_at: u64,
        expires_at: u64,
        sig_hex: &str,
    ) -> Response {
        let recovery = DidRecoveryPayload {
            did,
            principal_id,
            localhost_root,
            protector_id,
            data_key_id,
            nonce,
            issued_at,
            expires_at,
        };
        let now = now_ts();
        if let Err(err) = validate_did_recovery_request(&recovery, now) {
            return Response::error("invalid_recovery_proof", &err);
        }

        let vk = match decode_did_key(did).map_err(|e| e.to_string()) {
            Ok(vk) => vk,
            Err(e) => return Response::error("invalid_did", &e),
        };
        let sig_bytes = match hex::decode(sig_hex) {
            Ok(s) => s,
            Err(e) => {
                return Response::error("invalid_signature", &format!("Invalid hex sig: {}", e))
            }
        };
        let signature = match Signature::from_slice(&sig_bytes) {
            Ok(s) => s,
            Err(e) => {
                return Response::error("invalid_signature", &format!("Invalid signature: {}", e))
            }
        };
        let payload = recovery.to_payload();
        let valid = vk.verify(payload.as_bytes(), &signature).is_ok();
        let payload_sha256 = hex::encode(Sha256::digest(payload.as_bytes()));
        Response::ok(serde_json::json!({
            "schema": DID_RECOVERY_PROOF_SCHEMA,
            "valid": valid,
            "did": did,
            "principal_id": principal_id,
            "localhost_root": localhost_root,
            "protector_id": protector_id,
            "data_key_id": data_key_id,
            "payload_sha256": payload_sha256,
            "verified_at": now,
        }))
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
                            return Response::error("key_error", "Invalid persona key length");
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
                Err(e) => return Response::error("key_error", &format!("Read failed: {}", e)),
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
                Err(e) => return Response::error("key_error", &format!("Encrypt failed: {}", e)),
            }
            vk
        };

        let persona_did = match encode_did_key(&persona_vk).map_err(|e| e.to_string()) {
            Ok(did) => did,
            Err(e) => return Response::error("invalid_did", &e),
        };
        let owner_did = match self.signing_key.as_ref() {
            Some(signing_key) => Some(encode_signing_key_did(signing_key)),
            None => None,
        };

        Response::ok(serde_json::json!({
            "did": persona_did,
            "owner_did": owner_did,
            "name": normalized,
        }))
    }

    // === Storage helpers ===

    fn base_dir(&self) -> std::path::PathBuf {
        if self.storage_path.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        } else {
            std::path::PathBuf::from(&self.storage_path)
        }
    }

    fn did_dir(&self) -> std::path::PathBuf {
        self.base_dir().join("did")
    }

    fn try_load_nickname(&mut self) {
        if let Some(ref dk) = self.device_key {
            if let Ok(Some(nick)) =
                elastos_identity::load_nickname_with_device_key(&self.base_dir(), dk)
            {
                self.nickname = Some(nick);
            }
        }
    }

    fn save_nickname(&self, nickname: &str) {
        let Some(ref dk) = self.device_key else {
            return;
        };
        let _ = elastos_identity::save_nickname_with_device_key(&self.base_dir(), dk, nickname);
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

#[derive(Debug, Clone, Copy)]
struct DidRecoveryPayload<'a> {
    did: &'a str,
    principal_id: &'a str,
    localhost_root: &'a str,
    protector_id: &'a str,
    data_key_id: &'a str,
    nonce: &'a str,
    issued_at: u64,
    expires_at: u64,
}

impl DidRecoveryPayload<'_> {
    fn to_payload(self) -> String {
        format!(
            "ElastOS DID recovery request\n\
             schema: {DID_RECOVERY_PROOF_SCHEMA}\n\
             did: {did}\n\
             principal_id: {principal_id}\n\
             localhost_root: {localhost_root}\n\
             protector_id: {protector_id}\n\
             data_key_id: {data_key_id}\n\
             nonce: {nonce}\n\
             issued_at: {issued_at}\n\
             expires_at: {expires_at}\n",
            did = self.did,
            principal_id = self.principal_id,
            localhost_root = self.localhost_root,
            protector_id = self.protector_id,
            data_key_id = self.data_key_id,
            nonce = self.nonce,
            issued_at = self.issued_at,
            expires_at = self.expires_at,
        )
    }
}

fn validate_did_recovery_request(
    recovery: &DidRecoveryPayload<'_>,
    now: u64,
) -> Result<(), String> {
    decode_did_key(recovery.did)
        .map_err(|e| format!("invalid DID recovery subject: {e}"))?;
    validate_token_like_id(recovery.principal_id, "principal_id")?;
    validate_principal_localhost_root(recovery.principal_id, recovery.localhost_root)?;
    validate_token_like_id(recovery.protector_id, "protector_id")?;
    validate_token_like_id(recovery.data_key_id, "data_key_id")?;
    if !recovery.data_key_id.starts_with("pdek:") {
        return Err("data_key_id must start with pdek:".to_string());
    }
    validate_token_like_id(recovery.nonce, "nonce")?;
    if recovery.issued_at > now.saturating_add(300) {
        return Err("DID recovery proof issued_at is in the future".to_string());
    }
    if recovery.expires_at <= now {
        return Err("DID recovery proof expired".to_string());
    }
    if recovery.expires_at <= recovery.issued_at
        || recovery.expires_at.saturating_sub(recovery.issued_at) > DID_RECOVERY_MAX_TTL_SECS
    {
        return Err("DID recovery proof expiry is invalid".to_string());
    }
    Ok(())
}

fn validate_principal_localhost_root(
    principal_id: &str,
    localhost_root: &str,
) -> Result<(), String> {
    let Some(root_segment) = localhost_root.strip_prefix("localhost://Users/") else {
        return Err("localhost_root must be a principal-owned localhost user root".to_string());
    };
    if root_segment.is_empty()
        || root_segment == "self"
        || root_segment == "."
        || root_segment == ".."
        || root_segment.len() > 256
        || root_segment
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        return Err("localhost_root must be a principal-owned localhost user root".to_string());
    }
    if root_segment != principal_id {
        return Err("localhost_root must match the DID recovery principal".to_string());
    }
    Ok(())
}

fn validate_token_like_id(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.len() > 512
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|ch| ch == '/' || ch == '\\' || ch.is_ascii_control() || ch.is_ascii_whitespace())
    {
        Err(format!("{field} is invalid"))
    } else {
        Ok(())
    }
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
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

    const NONCANONICAL_ALIAS_DID: &str = "did:key:z2DQYePVrytqCLfdq5VSDGtksAZC9NAt75iCed5WwSKKVXt";

    fn make_device_key() -> [u8; 32] {
        [42u8; 32]
    }

    fn init_provider_with_device_key(base_path: Option<&str>, dk: [u8; 32]) -> DidProvider {
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

    fn init_provider(base_path: Option<&str>) -> DidProvider {
        init_provider_with_device_key(base_path, make_device_key())
    }

    fn write_runtime_device_key(base_dir: &std::path::Path, dk: [u8; 32]) {
        let identity_dir = base_dir.join("identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(identity_dir.join("device.key"), dk).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                identity_dir.join("device.key"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }

    fn overlong_did(did: &str) -> String {
        format!("{did}1")
    }

    fn assert_invalid_did(response: Response) {
        match response {
            Response::Error { code, .. } => assert_eq!(code, "invalid_did"),
            other => panic!("Expected invalid_did error, got {:?}", other),
        }
    }

    fn assert_invalid_recovery_proof(response: Response) {
        match response {
            Response::Error { code, .. } => assert_eq!(code, "invalid_recovery_proof"),
            other => panic!("Expected invalid_recovery_proof error, got {:?}", other),
        }
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
                assert!(
                    did.starts_with("did:key:z"),
                    "DID should start with did:key:z, got: {}",
                    did
                );
            }
            other => panic!("Expected ok response, got {:?}", other),
        }
    }

    #[test]
    fn test_did_key_encoding_roundtrip() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let vk = signing_key.verifying_key();
        let did = encode_did_key(&vk).unwrap();

        assert!(did.starts_with("did:key:z"));

        let decoded = decode_did_key(&did).unwrap();
        assert_eq!(vk.as_bytes(), decoded.as_bytes());
    }

    #[test]
    fn test_resolve_rejects_noncanonical_alias_did_via_shared_codec() {
        let provider = DidProvider::new();
        assert_invalid_did(provider.resolve(NONCANONICAL_ALIAS_DID));
    }

    #[test]
    fn test_resolve_rejects_overlong_did_via_shared_codec() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let provider = DidProvider::new();
        assert_invalid_did(provider.resolve(&overlong_did(&encode_signing_key_did(
            &signing_key,
        ))));
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
    fn test_sign_chat_message_and_verify_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        let did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        let ts = 1_714_000_000;
        let content = "hello world";
        let data_hex = elastos_common::chat_protocol::signing_payload_hex(&did, ts, content);
        let sig_hex = match provider.sign_chat_message(&did, ts, content) {
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

        let sig_hex = match provider.sign_chat_message(&did, 1_714_000_000, "hello") {
            Response::Ok { data: Some(d) } => d["signature"].as_str().unwrap().to_string(),
            _ => panic!("Expected signature"),
        };

        // Verify with different data
        let tampered =
            elastos_common::chat_protocol::signing_payload_hex(&did, 1_714_000_000, "tampered");
        match provider.verify(&did, &tampered, &sig_hex) {
            Response::Ok { data: Some(d) } => assert_eq!(d["valid"], false),
            other => panic!("Expected valid=false, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_rejects_noncanonical_alias_did_via_shared_codec() {
        let provider = DidProvider::new();
        assert_invalid_did(provider.verify(NONCANONICAL_ALIAS_DID, "00", "00"));
    }

    #[test]
    fn test_verify_rejects_overlong_did_via_shared_codec() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let provider = DidProvider::new();
        let did = overlong_did(&encode_signing_key_did(&signing_key));
        assert_invalid_did(provider.verify(&did, "00", "00"));
    }

    fn did_recovery_test_signature(
        signing_key: &SigningKey,
        recovery: DidRecoveryPayload<'_>,
    ) -> String {
        hex::encode(
            signing_key
                .sign(recovery.to_payload().as_bytes())
                .to_bytes(),
        )
    }

    #[test]
    fn test_did_provider_rejects_hidden_recovery_request_fields() {
        let request = serde_json::json!({
            "op": "verify_did_recovery",
            "did": "did:key:z6Mkh11111111111111111111111111111111111111111",
            "principal_id": "person:local:abc123",
            "localhost_root": "localhost://Users/person:local:abc123",
            "protector_id": "protector:did:abc123",
            "data_key_id": "pdek:abc123",
            "nonce": "nonce:abc123",
            "issued_at": now_ts(),
            "expires_at": now_ts() + 300,
            "signature": "00",
            "raw_data_key": "secret"
        });

        let err = serde_json::from_value::<Request>(request)
            .expect_err("DID recovery requests must reject hidden authority fields")
            .to_string();
        assert!(err.contains("raw_data_key"), "unexpected error: {err}");
    }

    #[test]
    fn test_did_provider_rejects_hidden_chat_signing_fields() {
        let request = serde_json::json!({
            "op": "sign_chat_message",
            "sender_id": "did:key:z6Mkh11111111111111111111111111111111111111111",
            "ts": 1_714_000_000u64,
            "content": "hello",
            "data": "arbitrary"
        });

        let err = serde_json::from_value::<Request>(request)
            .expect_err("DID signing intents must reject hidden generic data fields")
            .to_string();
        assert!(err.contains("data"), "unexpected error: {err}");
    }

    #[test]
    fn test_verify_did_recovery_accepts_typed_did_key_proof() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let did = encode_signing_key_did(&signing_key);
        let principal_id = "person:local:abc123";
        let localhost_root = "localhost://Users/person:local:abc123";
        let protector_id = "protector:did:abc123";
        let data_key_id = "pdek:abc123";
        let nonce = "nonce:abc123";
        let issued_at = now_ts();
        let expires_at = issued_at + 300;
        let recovery = DidRecoveryPayload {
            did: &did,
            principal_id,
            localhost_root,
            protector_id,
            data_key_id,
            nonce,
            issued_at,
            expires_at,
        };
        let signature = did_recovery_test_signature(&signing_key, recovery);
        let provider = DidProvider::new();

        match provider.verify_did_recovery(
            &did,
            principal_id,
            localhost_root,
            protector_id,
            data_key_id,
            nonce,
            issued_at,
            expires_at,
            &signature,
        ) {
            Response::Ok { data: Some(d) } => {
                assert_eq!(d["schema"], DID_RECOVERY_PROOF_SCHEMA);
                assert_eq!(d["valid"], true);
                assert_eq!(d["did"], did);
                assert_eq!(d["principal_id"], principal_id);
                assert!(d["payload_sha256"].as_str().unwrap().len() >= 64);
            }
            other => panic!("Expected valid DID recovery proof, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_did_recovery_rejects_noncanonical_root_binding() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let did = encode_signing_key_did(&signing_key);
        let provider = DidProvider::new();

        let err = match provider.verify_did_recovery(
            &did,
            "person:local:abc123",
            "localhost://Users/person:local:other",
            "protector:did:abc123",
            "pdek:abc123",
            "nonce:abc123",
            now_ts(),
            now_ts() + 300,
            "00",
        ) {
            Response::Error { message, .. } => message,
            other => panic!("Expected noncanonical root to fail closed, got {:?}", other),
        };
        assert!(err.contains("must match"));
    }

    #[test]
    fn test_verify_did_recovery_rejects_tampered_payload_binding() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let did = encode_signing_key_did(&signing_key);
        let principal_id = "person:local:abc123";
        let localhost_root = "localhost://Users/person:local:abc123";
        let protector_id = "protector:did:abc123";
        let data_key_id = "pdek:abc123";
        let nonce = "nonce:abc123";
        let issued_at = now_ts();
        let expires_at = issued_at + 300;
        let recovery = DidRecoveryPayload {
            did: &did,
            principal_id,
            localhost_root,
            protector_id,
            data_key_id,
            nonce,
            issued_at,
            expires_at,
        };
        let signature = did_recovery_test_signature(&signing_key, recovery);
        let provider = DidProvider::new();

        match provider.verify_did_recovery(
            &did,
            principal_id,
            localhost_root,
            protector_id,
            data_key_id,
            "nonce:other",
            issued_at,
            expires_at,
            &signature,
        ) {
            Response::Ok { data: Some(d) } => assert_eq!(d["valid"], false),
            other => panic!("Expected invalid signature response, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_did_recovery_rejects_did_elastos_until_resolver_exists() {
        let provider = DidProvider::new();
        let err = match provider.verify_did_recovery(
            "did:elastos:alice",
            "person:local:abc123",
            "localhost://Users/person:local:abc123",
            "protector:did:abc123",
            "pdek:abc123",
            "nonce:abc123",
            now_ts(),
            now_ts() + 300,
            "00",
        ) {
            Response::Error { message, .. } => message,
            other => panic!("Expected did:elastos to fail closed, got {:?}", other),
        };
        assert!(err.contains("did:key"));
    }

    #[test]
    fn test_verify_did_recovery_rejects_noncanonical_alias_did_via_shared_codec() {
        let provider = DidProvider::new();
        assert_invalid_recovery_proof(provider.verify_did_recovery(
            NONCANONICAL_ALIAS_DID,
            "person:local:abc123",
            "localhost://Users/person:local:abc123",
            "protector:did:abc123",
            "pdek:abc123",
            "nonce:abc123",
            now_ts(),
            now_ts() + 300,
            "00",
        ));
    }

    #[test]
    fn test_verify_did_recovery_rejects_overlong_did_via_shared_codec() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let did = overlong_did(&encode_signing_key_did(&signing_key));
        let provider = DidProvider::new();
        assert_invalid_recovery_proof(provider.verify_did_recovery(
            &did,
            "person:local:abc123",
            "localhost://Users/person:local:abc123",
            "protector:did:abc123",
            "pdek:abc123",
            "nonce:abc123",
            now_ts(),
            now_ts() + 300,
            "00",
        ));
    }

    #[test]
    fn test_verify_did_recovery_rejects_expired_proof() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let did = encode_signing_key_did(&signing_key);
        let issued_at = now_ts().saturating_sub(600);
        let expires_at = issued_at + 300;
        let signature = did_recovery_test_signature(
            &signing_key,
            DidRecoveryPayload {
                did: &did,
                principal_id: "person:local:abc123",
                localhost_root: "localhost://Users/person:local:abc123",
                protector_id: "protector:did:abc123",
                data_key_id: "pdek:abc123",
                nonce: "nonce:abc123",
                issued_at,
                expires_at,
            },
        );
        let provider = DidProvider::new();
        let err = match provider.verify_did_recovery(
            &did,
            "person:local:abc123",
            "localhost://Users/person:local:abc123",
            "protector:did:abc123",
            "pdek:abc123",
            "nonce:abc123",
            issued_at,
            expires_at,
            &signature,
        ) {
            Response::Error { message, .. } => message,
            other => panic!("Expected expired proof to fail closed, got {:?}", other),
        };
        assert!(err.contains("expired"));
    }

    #[test]
    fn test_sign_chat_message_rejects_missing_sender() {
        let dir = tempfile::tempdir().unwrap();
        let provider = init_provider(Some(dir.path().to_str().unwrap()));

        match provider.sign_chat_message("", 1_714_000_000, "hello") {
            Response::Error { code, .. } => assert_eq!(code, "invalid_intent"),
            other => panic!("Expected invalid intent, got {:?}", other),
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
    fn test_main_did_matches_runtime_derivation() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();
        let device_key = make_device_key();
        let expected_did = elastos_identity::derive_did(&device_key).1;

        let provider = init_provider(Some(bp));
        let actual_did = match provider.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            other => panic!("Expected DID, got {:?}", other),
        };

        assert_eq!(
            actual_did, expected_did,
            "did-provider must match the runtime's device-derived DID"
        );
    }

    #[test]
    fn test_different_device_key_cannot_read_did() {
        let dir = tempfile::tempdir().unwrap();
        let bp = dir.path().to_str().unwrap();

        // Create with one device key
        let provider1 = init_provider(Some(bp));
        let did1 = match provider1.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            _ => panic!("Expected DID"),
        };

        // Try to load with a different device key
        let config = serde_json::json!({
            "encryption_key": hex::encode([99u8; 32]),
            "base_path": bp,
        });
        let mut provider2 = DidProvider::new();
        let _ = provider2.handle(Request::Init { config });
        let did2 = match provider2.get_did() {
            Response::Ok { data: Some(d) } => d["did"].as_str().unwrap().to_string(),
            other => panic!("Expected DID, got {:?}", other),
        };

        assert_ne!(
            did1, did2,
            "changing the device key must change the main DID"
        );
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
    fn test_provider_set_nickname_is_visible_to_identity_library() {
        let dir = tempfile::tempdir().unwrap();
        let dk = make_device_key();
        write_runtime_device_key(dir.path(), dk);

        {
            let mut provider =
                init_provider_with_device_key(Some(dir.path().to_str().unwrap()), dk);
            provider.handle(Request::SetNickname {
                nickname: "alice".to_string(),
            });
        }

        assert_eq!(
            elastos_identity::load_nickname(dir.path())
                .unwrap()
                .as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn test_identity_library_nickname_is_visible_to_provider() {
        let dir = tempfile::tempdir().unwrap();
        let dk = make_device_key();
        write_runtime_device_key(dir.path(), dk);
        elastos_identity::save_nickname(dir.path(), "bob").unwrap();

        let provider = init_provider_with_device_key(Some(dir.path().to_str().unwrap()), dk);
        match provider.get_nickname() {
            Response::Ok { data: Some(d) } => assert_eq!(d["nickname"], "bob"),
            other => panic!("Expected nickname bob, got {:?}", other),
        }
    }

    #[test]
    fn test_sign_without_init() {
        let provider = DidProvider::new();
        match provider.sign_chat_message("did:key:zMissing", 1_714_000_000, "hello") {
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
