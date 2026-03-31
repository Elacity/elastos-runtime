//! Minimal WebAuthn Relying Party implementation
//!
//! Implements the server side of WebAuthn passkey registration and authentication
//! without OpenSSL dependencies. Uses p256 for ES256 signature verification.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{IdentityStore, StoredCredential};

/// Challenge expiry duration
const CHALLENGE_EXPIRY: Duration = Duration::from_secs(300);

/// Challenge type
enum ChallengeType {
    Registration,
    Authentication,
}

struct PendingChallenge {
    challenge: Vec<u8>,
    challenge_type: ChallengeType,
    created: Instant,
}

/// Identity status returned to clients
#[derive(Debug, Clone, Serialize)]
pub struct IdentityStatus {
    pub registered: bool,
    pub authenticated: bool,
    pub user_id: Option<String>,
}

// === WebAuthn Protocol Types ===
// These match the WebAuthn spec JSON format that browsers produce/consume.

/// Server → Browser: options for navigator.credentials.create()
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreationOptions {
    pub public_key: PublicKeyCredentialCreationOptions,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyCredentialCreationOptions {
    pub rp: RelyingParty,
    pub user: UserEntity,
    pub challenge: String, // base64url
    pub pub_key_cred_params: Vec<PubKeyCredParam>,
    pub timeout: u64,
    pub authenticator_selection: AuthenticatorSelection,
    pub attestation: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_credentials: Vec<CredentialDescriptor>,
}

#[derive(Debug, Serialize)]
pub struct RelyingParty {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserEntity {
    pub id: String, // base64url
    pub name: String,
    pub display_name: String,
}

#[derive(Debug, Serialize)]
pub struct PubKeyCredParam {
    #[serde(rename = "type")]
    pub type_: String,
    pub alg: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatorSelection {
    pub authenticator_attachment: Option<String>,
    pub resident_key: String,
    pub require_resident_key: bool,
    pub user_verification: String,
}

#[derive(Debug, Serialize)]
pub struct CredentialDescriptor {
    #[serde(rename = "type")]
    pub type_: String,
    pub id: String, // base64url
}

/// Server → Browser: options for navigator.credentials.get()
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestOptions {
    pub public_key: PublicKeyCredentialRequestOptions,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKeyCredentialRequestOptions {
    pub challenge: String, // base64url
    pub timeout: u64,
    pub rp_id: String,
    pub allow_credentials: Vec<CredentialDescriptor>,
    pub user_verification: String,
}

/// Browser → Server: registration response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponse {
    #[serde(rename = "id")]
    pub _id: String,
    #[serde(rename = "rawId")]
    pub _raw_id: String, // base64url
    pub response: AuthenticatorAttestationResponse,
    #[serde(rename = "type")]
    pub _type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatorAttestationResponse {
    pub client_data_json: String,   // base64url
    pub attestation_object: String, // base64url
}

/// Browser → Server: authentication response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationResponse {
    #[serde(rename = "id")]
    pub _id: String,
    pub raw_id: String, // base64url
    pub response: AuthenticatorAssertionResponse,
    #[serde(rename = "type")]
    pub _type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatorAssertionResponse {
    pub client_data_json: String,   // base64url
    pub authenticator_data: String, // base64url
    pub signature: String,          // base64url
    #[serde(rename = "userHandle")]
    pub _user_handle: Option<String>, // base64url
}

/// Parsed client data
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CollectedClientData {
    #[serde(rename = "type")]
    type_: String,
    challenge: String,
    origin: String,
}

/// Manages WebAuthn registration and authentication
pub struct IdentityManager {
    store: IdentityStore,
    challenges: HashMap<String, PendingChallenge>,
    /// When false, a sign_count regression returns an error instead of a warning.
    /// Set to true during development to tolerate virtual authenticators that
    /// reset their counters.
    pub allow_clone: bool,
}

impl IdentityManager {
    /// Create a new identity manager
    ///
    /// RP ID and origin are provided per-request (derived from Host header)
    /// so passkeys work from any transport (localhost, LAN, Tailscale, etc.)
    pub fn new(data_dir: PathBuf) -> anyhow::Result<Self> {
        let mut store = IdentityStore::new(&data_dir)?;
        store.load().unwrap_or_else(|e| {
            tracing::warn!("Failed to load identity store: {}", e);
        });

        Ok(Self {
            store,
            challenges: HashMap::new(),
            allow_clone: false,
        })
    }

    /// Get current identity status
    pub fn status(&self) -> IdentityStatus {
        IdentityStatus {
            registered: self.store.is_registered(),
            authenticated: false,
            user_id: self.store.user_id().map(String::from),
        }
    }

    /// Begin registration flow
    /// Begin registration of an additional passkey.
    ///
    /// The first call creates the user identity. Subsequent calls add backup
    /// credentials to the same identity. Previously-registered credential IDs
    /// are sent in `excludeCredentials` so the browser won't re-register them.
    pub fn begin_registration(
        &mut self,
        session_token: &str,
        rp_id: &str,
    ) -> anyhow::Result<CreationOptions> {
        self.cleanup_expired();

        let challenge = generate_challenge();
        let challenge_b64 = URL_SAFE_NO_PAD.encode(&challenge);

        // User ID is random for registration, real ID derived from credential after
        let user_id = URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes());

        let exclude = self
            .store
            .get_credentials()
            .iter()
            .map(|c| CredentialDescriptor {
                type_: "public-key".to_string(),
                id: c.credential_id.clone(),
            })
            .collect();

        let options = CreationOptions {
            public_key: PublicKeyCredentialCreationOptions {
                rp: RelyingParty {
                    name: "ElastOS".to_string(),
                    id: rp_id.to_string(),
                },
                user: UserEntity {
                    id: user_id,
                    name: "elastos-user".to_string(),
                    display_name: "ElastOS User".to_string(),
                },
                challenge: challenge_b64,
                pub_key_cred_params: vec![PubKeyCredParam {
                    type_: "public-key".to_string(),
                    alg: -7, // ES256
                }],
                timeout: 300000,
                authenticator_selection: AuthenticatorSelection {
                    authenticator_attachment: None, // platform or cross-platform
                    resident_key: "preferred".to_string(),
                    require_resident_key: false,
                    user_verification: "preferred".to_string(),
                },
                attestation: "none".to_string(),
                exclude_credentials: exclude,
            },
        };

        self.challenges.insert(
            session_token.to_string(),
            PendingChallenge {
                challenge,
                challenge_type: ChallengeType::Registration,
                created: Instant::now(),
            },
        );

        Ok(options)
    }

    /// Complete registration flow
    pub fn complete_registration(
        &mut self,
        session_token: &str,
        response: &RegistrationResponse,
        rp_id: &str,
        rp_origin: &str,
    ) -> anyhow::Result<String> {
        let pending = self
            .challenges
            .remove(session_token)
            .ok_or_else(|| anyhow::anyhow!("No pending registration challenge"))?;

        if !matches!(pending.challenge_type, ChallengeType::Registration) {
            anyhow::bail!("Pending challenge is not a registration");
        }
        if pending.created.elapsed() > CHALLENGE_EXPIRY {
            anyhow::bail!("Registration challenge expired");
        }

        // Decode and verify client data
        let client_data_bytes = URL_SAFE_NO_PAD.decode(&response.response.client_data_json)?;
        let client_data: CollectedClientData = serde_json::from_slice(&client_data_bytes)?;

        if client_data.type_ != "webauthn.create" {
            anyhow::bail!("Invalid client data type: {}", client_data.type_);
        }

        // Verify challenge matches
        let received_challenge = URL_SAFE_NO_PAD.decode(&client_data.challenge)?;
        if received_challenge != pending.challenge {
            anyhow::bail!("Challenge mismatch");
        }

        // Verify origin
        let expected_origin = rp_origin.trim_end_matches('/');
        if client_data.origin.trim_end_matches('/') != expected_origin {
            anyhow::bail!(
                "Origin mismatch: expected {}, got {}",
                expected_origin,
                client_data.origin
            );
        }

        // Decode attestation object (CBOR)
        let att_obj_bytes = URL_SAFE_NO_PAD.decode(&response.response.attestation_object)?;
        let att_obj: ciborium::Value = ciborium::from_reader(&att_obj_bytes[..])
            .map_err(|e| anyhow::anyhow!("CBOR: {}", e))?;

        // Extract authData from attestation object
        let auth_data_bytes = extract_cbor_bytes(&att_obj, "authData")?;

        // Parse authenticator data
        if auth_data_bytes.len() < 37 {
            anyhow::bail!("AuthData too short");
        }

        // Verify RP ID hash (first 32 bytes)
        let expected_rp_hash = Sha256::digest(rp_id.as_bytes());
        if auth_data_bytes[..32] != expected_rp_hash[..] {
            anyhow::bail!("RP ID hash mismatch");
        }

        let flags = auth_data_bytes[32];
        // Bit 0: UP (user present)
        if flags & 0x01 == 0 {
            anyhow::bail!("User presence flag not set");
        }
        // Bit 6: AT (attested credential data included)
        if flags & 0x40 == 0 {
            anyhow::bail!("No attested credential data");
        }

        // Parse attested credential data (after 37 bytes of rpIdHash + flags + signCount)
        let sign_count = u32::from_be_bytes([
            auth_data_bytes[33],
            auth_data_bytes[34],
            auth_data_bytes[35],
            auth_data_bytes[36],
        ]);

        // AAGUID (16 bytes) + credential ID length (2 bytes) + credential ID + COSE key
        let _aaguid = &auth_data_bytes[37..53];
        let cred_id_len = u16::from_be_bytes([auth_data_bytes[53], auth_data_bytes[54]]) as usize;
        let cred_id = &auth_data_bytes[55..55 + cred_id_len];
        let cose_key_bytes = &auth_data_bytes[55 + cred_id_len..];

        let credential_id = URL_SAFE_NO_PAD.encode(cred_id);
        let public_key = URL_SAFE_NO_PAD.encode(cose_key_bytes);

        // Verify the COSE key is valid ES256 by trying to parse it
        parse_cose_es256_key(cose_key_bytes)?;

        let stored = StoredCredential {
            credential_id,
            public_key,
            sign_count,
            rp_id: rp_id.to_string(),
        };

        let user_id = self.store.add_credential(stored);
        self.store.save()?;

        Ok(user_id)
    }

    /// Begin authentication flow
    pub fn begin_authentication(
        &mut self,
        session_token: &str,
        rp_id: &str,
    ) -> anyhow::Result<RequestOptions> {
        self.cleanup_expired();

        let credentials = self.store.get_credentials();
        if credentials.is_empty() {
            anyhow::bail!("No registered credentials. Register first.");
        }

        let challenge = generate_challenge();
        let challenge_b64 = URL_SAFE_NO_PAD.encode(&challenge);

        let allow = credentials
            .iter()
            .map(|c| CredentialDescriptor {
                type_: "public-key".to_string(),
                id: c.credential_id.clone(),
            })
            .collect();

        let options = RequestOptions {
            public_key: PublicKeyCredentialRequestOptions {
                challenge: challenge_b64,
                timeout: 300000,
                rp_id: rp_id.to_string(),
                allow_credentials: allow,
                user_verification: "preferred".to_string(),
            },
        };

        self.challenges.insert(
            session_token.to_string(),
            PendingChallenge {
                challenge,
                challenge_type: ChallengeType::Authentication,
                created: Instant::now(),
            },
        );

        Ok(options)
    }

    /// Complete authentication flow
    pub fn complete_authentication(
        &mut self,
        session_token: &str,
        response: &AuthenticationResponse,
        rp_id: &str,
        rp_origin: &str,
    ) -> anyhow::Result<String> {
        let pending = self
            .challenges
            .remove(session_token)
            .ok_or_else(|| anyhow::anyhow!("No pending authentication challenge"))?;

        if !matches!(pending.challenge_type, ChallengeType::Authentication) {
            anyhow::bail!("Pending challenge is not an authentication");
        }
        if pending.created.elapsed() > CHALLENGE_EXPIRY {
            anyhow::bail!("Authentication challenge expired");
        }

        // Find the matching credential
        let credential_id = &response.raw_id;
        let stored = self
            .store
            .get_credentials()
            .into_iter()
            .find(|c| c.credential_id == *credential_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown credential"))?;

        // Decode and verify client data
        let client_data_bytes = URL_SAFE_NO_PAD.decode(&response.response.client_data_json)?;
        let client_data: CollectedClientData = serde_json::from_slice(&client_data_bytes)?;

        if client_data.type_ != "webauthn.get" {
            anyhow::bail!("Invalid client data type: {}", client_data.type_);
        }

        let received_challenge = URL_SAFE_NO_PAD.decode(&client_data.challenge)?;
        if received_challenge != pending.challenge {
            anyhow::bail!("Challenge mismatch");
        }

        if client_data.origin.trim_end_matches('/') != rp_origin.trim_end_matches('/') {
            anyhow::bail!("Origin mismatch");
        }

        // Decode authenticator data
        let auth_data_bytes = URL_SAFE_NO_PAD.decode(&response.response.authenticator_data)?;

        if auth_data_bytes.len() < 37 {
            anyhow::bail!("AuthData too short");
        }

        // Verify RP ID hash
        let expected_rp_hash = Sha256::digest(rp_id.as_bytes());
        if auth_data_bytes[..32] != expected_rp_hash[..] {
            anyhow::bail!("RP ID hash mismatch");
        }

        let flags = auth_data_bytes[32];
        if flags & 0x01 == 0 {
            anyhow::bail!("User presence flag not set");
        }

        let sign_count = u32::from_be_bytes([
            auth_data_bytes[33],
            auth_data_bytes[34],
            auth_data_bytes[35],
            auth_data_bytes[36],
        ]);

        // Clone detection: sign count should increase
        if stored.sign_count > 0 && sign_count <= stored.sign_count {
            if self.allow_clone {
                tracing::warn!(
                    "Possible credential clone detected (dev mode, allowing): stored={}, received={}",
                    stored.sign_count,
                    sign_count
                );
            } else {
                anyhow::bail!(
                    "Credential clone detected: sign_count went from {} to {} (expected increase). \
                     This passkey may have been copied. Set allow_clone=true in dev mode to override.",
                    stored.sign_count,
                    sign_count
                );
            }
        }

        // Verify signature: sign(authData || SHA256(clientDataJSON))
        let client_data_hash = Sha256::digest(&client_data_bytes);
        let mut signed_data = auth_data_bytes.clone();
        signed_data.extend_from_slice(&client_data_hash);

        let public_key_bytes = URL_SAFE_NO_PAD.decode(&stored.public_key)?;
        let verifying_key = parse_cose_es256_key(&public_key_bytes)?;

        let sig_bytes = URL_SAFE_NO_PAD.decode(&response.response.signature)?;
        let signature = Signature::from_der(&sig_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid signature format: {}", e))?;

        verifying_key
            .verify(&signed_data, &signature)
            .map_err(|e| anyhow::anyhow!("Signature verification failed: {}", e))?;

        // Update sign count
        self.store
            .update_sign_count(&stored.credential_id, sign_count);
        self.store.save()?;

        Ok(self
            .store
            .user_id()
            .ok_or_else(|| {
                anyhow::anyhow!("Identity store has no user ID after successful authentication")
            })?
            .to_string())
    }

    fn cleanup_expired(&mut self) {
        self.challenges
            .retain(|_, c| c.created.elapsed() < CHALLENGE_EXPIRY);
    }
}

/// Generate a random 32-byte challenge
fn generate_challenge() -> Vec<u8> {
    use rand::RngCore;
    let mut challenge = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut challenge);
    challenge
}

/// Parse a COSE ES256 public key and return a p256 VerifyingKey
fn parse_cose_es256_key(cose_bytes: &[u8]) -> anyhow::Result<VerifyingKey> {
    let cose_key: ciborium::Value =
        ciborium::from_reader(cose_bytes).map_err(|e| anyhow::anyhow!("COSE CBOR: {}", e))?;

    let map = match &cose_key {
        ciborium::Value::Map(m) => m,
        _ => anyhow::bail!("COSE key is not a map"),
    };

    // kty (1) must be EC2 (2)
    let kty = find_cbor_int(map, 1)?;
    if kty != 2 {
        anyhow::bail!("Unsupported key type: {} (expected EC2=2)", kty);
    }

    // alg (3) must be ES256 (-7)
    let alg = find_cbor_int(map, 3)?;
    if alg != -7 {
        anyhow::bail!("Unsupported algorithm: {} (expected ES256=-7)", alg);
    }

    // x coordinate (-2)
    let x = find_cbor_bytes(map, -2)?;
    // y coordinate (-3)
    let y = find_cbor_bytes(map, -3)?;

    if x.len() != 32 || y.len() != 32 {
        anyhow::bail!("Invalid EC point size: x={}, y={}", x.len(), y.len());
    }

    // Construct uncompressed point: 0x04 || x || y
    let mut point = Vec::with_capacity(65);
    point.push(0x04);
    point.extend_from_slice(&x);
    point.extend_from_slice(&y);

    VerifyingKey::from_sec1_bytes(&point)
        .map_err(|e| anyhow::anyhow!("Invalid EC public key: {}", e))
}

/// Find an integer value in a CBOR map by integer key
fn find_cbor_int(map: &[(ciborium::Value, ciborium::Value)], key: i128) -> anyhow::Result<i128> {
    for (k, v) in map {
        if let ciborium::Value::Integer(i) = k {
            if i128::from(*i) == key {
                if let ciborium::Value::Integer(val) = v {
                    return Ok(i128::from(*val));
                }
            }
        }
    }
    anyhow::bail!("COSE key missing field {}", key)
}

/// Find bytes value in a CBOR map by integer key
fn find_cbor_bytes(
    map: &[(ciborium::Value, ciborium::Value)],
    key: i128,
) -> anyhow::Result<Vec<u8>> {
    for (k, v) in map {
        if let ciborium::Value::Integer(i) = k {
            if i128::from(*i) == key {
                if let ciborium::Value::Bytes(bytes) = v {
                    return Ok(bytes.clone());
                }
            }
        }
    }
    anyhow::bail!("COSE key missing bytes field {}", key)
}

/// Extract a byte string from a CBOR map by string key
fn extract_cbor_bytes(value: &ciborium::Value, key: &str) -> anyhow::Result<Vec<u8>> {
    let map = match value {
        ciborium::Value::Map(m) => m,
        _ => anyhow::bail!("Expected CBOR map"),
    };

    for (k, v) in map {
        if let ciborium::Value::Text(s) = k {
            if s == key {
                if let ciborium::Value::Bytes(bytes) = v {
                    return Ok(bytes.clone());
                }
            }
        }
    }
    anyhow::bail!("Missing CBOR field: {}", key)
}
