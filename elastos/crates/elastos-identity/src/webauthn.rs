//! Minimal WebAuthn Relying Party implementation
//!
//! Implements the server side of WebAuthn passkey registration and authentication
//! without OpenSSL dependencies. Supports ES256 and RS256 passkeys.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aws_lc_rs::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::store::{IdentityStore, StoredCredential};

/// Challenge expiry duration
const CHALLENGE_EXPIRY: Duration = Duration::from_secs(300);
const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_ATTESTED_CREDENTIAL_DATA: u8 = 0x40;

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

#[derive(Debug, Clone)]
pub struct RegistrationOutcome {
    pub user_id: String,
    pub credential: StoredCredential,
    pub origin: String,
    pub user_verified: bool,
}

#[derive(Debug, Clone)]
pub struct AuthenticationOutcome {
    pub user_id: String,
    pub credential: StoredCredential,
    pub origin: String,
    pub user_verified: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticatorAttestationResponse {
    pub client_data_json: String,   // base64url
    pub attestation_object: String, // base64url
}

/// Browser → Server: authentication response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticationResponse {
    #[serde(rename = "id")]
    pub _id: String,
    pub raw_id: String, // base64url
    pub response: AuthenticatorAssertionResponse,
    #[serde(rename = "type")]
    pub _type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
        store.load()?;

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

    /// List stored passkey credentials without exposing private key material.
    pub fn credentials(&self) -> Vec<StoredCredential> {
        self.store.get_credentials()
    }

    /// Revoke one passkey credential from the local identity store.
    pub fn revoke_credential(&mut self, credential_id: &str) -> anyhow::Result<StoredCredential> {
        let credential = self
            .store
            .get_credentials()
            .into_iter()
            .find(|credential| credential.credential_id == credential_id)
            .ok_or_else(|| anyhow::anyhow!("passkey credential not found"))?;
        if !self.store.remove_credential(credential_id) {
            anyhow::bail!("passkey credential not found");
        }
        self.challenges.clear();
        self.store.save()?;
        Ok(credential)
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
        self.begin_registration_inner(session_token, rp_id, "ElastOS User", true)
    }

    /// Begin registration for a separate runtime principal.
    ///
    /// This intentionally omits `excludeCredentials`: the runtime model treats
    /// each passkey as its own principal, so the same platform authenticator may
    /// create an additional guest credential for the same RP.
    pub fn begin_principal_registration(
        &mut self,
        session_token: &str,
        rp_id: &str,
    ) -> anyhow::Result<CreationOptions> {
        self.begin_registration_inner(session_token, rp_id, "ElastOS Passkey", false)
    }

    fn begin_registration_inner(
        &mut self,
        session_token: &str,
        rp_id: &str,
        display_name: &str,
        exclude_existing: bool,
    ) -> anyhow::Result<CreationOptions> {
        self.cleanup_expired();

        let challenge = generate_challenge();
        let challenge_b64 = URL_SAFE_NO_PAD.encode(&challenge);

        // User ID is random for registration, real ID derived from credential after
        let user_id = URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes());

        let exclude = if exclude_existing {
            self.store
                .get_credentials()
                .iter()
                .map(|c| CredentialDescriptor {
                    type_: "public-key".to_string(),
                    id: c.credential_id.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        let options = CreationOptions {
            public_key: PublicKeyCredentialCreationOptions {
                rp: RelyingParty {
                    name: "ElastOS".to_string(),
                    id: rp_id.to_string(),
                },
                user: UserEntity {
                    id: user_id,
                    name: "elastos-user".to_string(),
                    display_name: display_name.to_string(),
                },
                challenge: challenge_b64,
                pub_key_cred_params: vec![
                    PubKeyCredParam {
                        type_: "public-key".to_string(),
                        alg: -7, // ES256
                    },
                    PubKeyCredParam {
                        type_: "public-key".to_string(),
                        alg: -257, // RS256
                    },
                ],
                timeout: 300000,
                authenticator_selection: AuthenticatorSelection {
                    authenticator_attachment: None, // platform or cross-platform
                    resident_key: "preferred".to_string(),
                    require_resident_key: false,
                    user_verification: "required".to_string(),
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
    ) -> anyhow::Result<RegistrationOutcome> {
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
        require_user_present_and_verified(flags)?;
        // Bit 6: AT (attested credential data included)
        if flags & FLAG_ATTESTED_CREDENTIAL_DATA == 0 {
            anyhow::bail!("No attested credential data");
        }

        // Parse attested credential data (after 37 bytes of rpIdHash + flags + signCount)
        let sign_count = u32::from_be_bytes([
            auth_data_bytes[33],
            auth_data_bytes[34],
            auth_data_bytes[35],
            auth_data_bytes[36],
        ]);

        // AAGUID (16 bytes) + credential ID length (2 bytes) + credential ID + COSE key.
        // authData is attacker-controlled (it is lifted from the supplied attestation
        // object), so every slice below must be bounds-checked first: an out-of-range
        // index here would panic the auth thread (fail-open-by-crash DoS) instead of
        // failing closed. Attested credential data needs at least 55 bytes
        // (37-byte header + 16-byte AAGUID + 2-byte credential-id length).
        if auth_data_bytes.len() < 55 {
            anyhow::bail!("AuthData too short for attested credential data");
        }
        let _aaguid = &auth_data_bytes[37..53];
        let cred_id_len = u16::from_be_bytes([auth_data_bytes[53], auth_data_bytes[54]]) as usize;
        let cred_id_end = 55usize
            .checked_add(cred_id_len)
            .filter(|end| *end <= auth_data_bytes.len())
            .ok_or_else(|| anyhow::anyhow!("AuthData credential ID length out of range"))?;
        let cred_id = &auth_data_bytes[55..cred_id_end];
        let cose_key_bytes = &auth_data_bytes[cred_id_end..];

        let credential_id = URL_SAFE_NO_PAD.encode(cred_id);
        let public_key = URL_SAFE_NO_PAD.encode(cose_key_bytes);

        // Verify the COSE key uses an algorithm this runtime can validate.
        parse_cose_public_key(cose_key_bytes)?;

        let stored = StoredCredential {
            credential_id,
            public_key,
            sign_count,
            rp_id: rp_id.to_string(),
        };

        let user_id = self.store.add_credential(stored.clone());
        self.store.save()?;

        Ok(RegistrationOutcome {
            user_id,
            credential: stored,
            origin: client_data.origin,
            user_verified: true,
        })
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
                user_verification: "required".to_string(),
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
    ) -> anyhow::Result<AuthenticationOutcome> {
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
        require_user_present_and_verified(flags)?;

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

        let sig_bytes = URL_SAFE_NO_PAD.decode(&response.response.signature)?;
        let public_key_bytes = URL_SAFE_NO_PAD.decode(&stored.public_key)?;
        parse_cose_public_key(&public_key_bytes)?
            .verify(&signed_data, &sig_bytes)
            .map_err(|e| anyhow::anyhow!("Signature verification failed: {}", e))?;

        // Update sign count
        self.store
            .update_sign_count(&stored.credential_id, sign_count);
        self.store.save()?;

        let mut credential = stored.clone();
        credential.sign_count = sign_count;
        let user_id = self
            .store
            .user_id()
            .ok_or_else(|| {
                anyhow::anyhow!("Identity store has no user ID after successful authentication")
            })?
            .to_string();

        Ok(AuthenticationOutcome {
            user_id,
            credential,
            origin: client_data.origin,
            user_verified: true,
        })
    }

    fn cleanup_expired(&mut self) {
        self.challenges
            .retain(|_, c| c.created.elapsed() < CHALLENGE_EXPIRY);
    }

    #[cfg(test)]
    fn expire_challenge_for_test(&mut self, session_token: &str) {
        if let Some(challenge) = self.challenges.get_mut(session_token) {
            challenge.created = Instant::now() - CHALLENGE_EXPIRY - Duration::from_secs(1);
        }
    }
}

/// Generate a random 32-byte challenge
fn generate_challenge() -> Vec<u8> {
    use rand::RngCore;
    let mut challenge = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut challenge);
    challenge
}

fn require_user_present_and_verified(flags: u8) -> anyhow::Result<()> {
    if flags & FLAG_USER_PRESENT == 0 {
        anyhow::bail!("User presence flag not set");
    }
    if flags & FLAG_USER_VERIFIED == 0 {
        anyhow::bail!("User verification flag not set");
    }
    Ok(())
}

enum CosePublicKey {
    Es256(VerifyingKey),
    Rs256(Vec<u8>),
}

impl CosePublicKey {
    fn verify(&self, signed_data: &[u8], sig_bytes: &[u8]) -> anyhow::Result<()> {
        match self {
            CosePublicKey::Es256(verifying_key) => {
                let signature = Signature::from_der(sig_bytes)
                    .map_err(|e| anyhow::anyhow!("Invalid ES256 signature format: {}", e))?;
                verifying_key
                    .verify(signed_data, &signature)
                    .map_err(|e| anyhow::anyhow!("ES256 verification failed: {}", e))
            }
            CosePublicKey::Rs256(public_key_spki) => {
                UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key_spki)
                    .verify(signed_data, sig_bytes)
                    .map_err(|e| anyhow::anyhow!("RS256 verification failed: {:?}", e))
            }
        }
    }
}

fn parse_cose_public_key(cose_bytes: &[u8]) -> anyhow::Result<CosePublicKey> {
    let cose_key: ciborium::Value =
        ciborium::from_reader(cose_bytes).map_err(|e| anyhow::anyhow!("COSE CBOR: {}", e))?;

    let map = match &cose_key {
        ciborium::Value::Map(m) => m,
        _ => anyhow::bail!("COSE key is not a map"),
    };

    let alg = find_cbor_int(map, 3)?;
    match alg {
        -7 => parse_cose_es256_key_map(map).map(CosePublicKey::Es256),
        -257 => parse_cose_rs256_key_map(map).map(CosePublicKey::Rs256),
        _ => anyhow::bail!(
            "Unsupported algorithm: {} (expected ES256=-7 or RS256=-257)",
            alg
        ),
    }
}

fn parse_cose_es256_key_map(
    map: &[(ciborium::Value, ciborium::Value)],
) -> anyhow::Result<VerifyingKey> {
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

fn parse_cose_rs256_key_map(map: &[(ciborium::Value, ciborium::Value)]) -> anyhow::Result<Vec<u8>> {
    // kty (1) must be RSA (3)
    let kty = find_cbor_int(map, 1)?;
    if kty != 3 {
        anyhow::bail!("Unsupported key type: {} (expected RSA=3)", kty);
    }

    // alg (3) must be RS256 (-257)
    let alg = find_cbor_int(map, 3)?;
    if alg != -257 {
        anyhow::bail!("Unsupported algorithm: {} (expected RS256=-257)", alg);
    }

    let n = find_cbor_bytes(map, -1)?;
    let e = find_cbor_bytes(map, -2)?;
    rsa_spki_der(&n, &e)
}

fn rsa_spki_der(modulus: &[u8], exponent: &[u8]) -> anyhow::Result<Vec<u8>> {
    let modulus = der_positive_integer_bytes(modulus, "RSA modulus")?;
    let exponent = der_positive_integer_bytes(exponent, "RSA exponent")?;

    let rsa_public_key = der_sequence(&[der_tlv(0x02, &modulus), der_tlv(0x02, &exponent)]);
    let algorithm = der_sequence(&[
        vec![
            0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
        ],
        vec![0x05, 0x00],
    ]);
    let mut subject_public_key = Vec::with_capacity(rsa_public_key.len() + 1);
    subject_public_key.push(0);
    subject_public_key.extend_from_slice(&rsa_public_key);

    Ok(der_sequence(&[
        algorithm,
        der_tlv(0x03, &subject_public_key),
    ]))
}

fn der_positive_integer_bytes(bytes: &[u8], label: &str) -> anyhow::Result<Vec<u8>> {
    let first_non_zero = bytes.iter().position(|byte| *byte != 0);
    let Some(offset) = first_non_zero else {
        anyhow::bail!("{} must be non-zero", label);
    };
    let bytes = &bytes[offset..];

    let mut out = Vec::with_capacity(bytes.len() + 1);
    if bytes[0] & 0x80 != 0 {
        out.push(0);
    }
    out.extend_from_slice(bytes);
    Ok(out)
}

fn der_sequence(parts: &[Vec<u8>]) -> Vec<u8> {
    let len = parts.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(1 + der_len_size(len) + len);
    out.push(0x30);
    der_push_len(&mut out, len);
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

fn der_tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + der_len_size(value.len()) + value.len());
    out.push(tag);
    der_push_len(&mut out, value.len());
    out.extend_from_slice(value);
    out
}

fn der_len_size(len: usize) -> usize {
    if len < 128 {
        1
    } else {
        1 + (usize::BITS - len.leading_zeros()).div_ceil(8) as usize
    }
}

fn der_push_len(out: &mut Vec<u8>, len: usize) {
    if len < 128 {
        out.push(len as u8);
        return;
    }

    let bytes = len.to_be_bytes();
    let offset = bytes
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(bytes.len() - 1);
    let significant = &bytes[offset..];
    out.push(0x80 | significant.len() as u8);
    out.extend_from_slice(significant);
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::{Digest, Sha256};

    #[test]
    fn registration_options_require_user_verification() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = IdentityManager::new(temp.path().to_path_buf()).unwrap();

        let options = manager
            .begin_registration("session-token", "localhost")
            .unwrap();

        assert_eq!(
            options.public_key.authenticator_selection.user_verification,
            "required"
        );
    }

    #[test]
    fn registration_options_offer_default_algorithms_without_null_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = IdentityManager::new(temp.path().to_path_buf()).unwrap();

        let options = manager
            .begin_principal_registration("session-token", "localhost")
            .unwrap();
        let algorithms: Vec<i64> = options
            .public_key
            .pub_key_cred_params
            .iter()
            .map(|param| param.alg)
            .collect();
        assert_eq!(algorithms, vec![-7, -257]);

        let json = serde_json::to_value(&options).unwrap();
        assert!(json["publicKey"]["authenticatorSelection"]
            .get("authenticatorAttachment")
            .is_none());
    }

    #[test]
    fn rs256_cose_key_is_encoded_as_positive_spki_der() {
        let spki = rsa_spki_der(&[0x00, 0x80, 0x01], &[0x01, 0x00, 0x01]).unwrap();

        assert_eq!(spki[0], 0x30);
        assert!(spki
            .windows(11)
            .any(|window| window
                == [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]));
        assert!(spki
            .windows(5)
            .any(|window| window == [0x02, 0x03, 0x00, 0x80, 0x01]));
    }

    #[test]
    fn rs256_cose_key_rejects_zero_integers() {
        assert!(rsa_spki_der(&[0], &[0x01, 0x00, 0x01]).is_err());
        assert!(rsa_spki_der(&[0x01], &[0]).is_err());
    }

    #[test]
    fn rs256_verification_accepts_pkcs1_sha256_vector() {
        let modulus = hex::decode(concat!(
            "00b46062899be0b9f25018e81494c3889573c1acdd27db80f03609ebd92f",
            "95419036edc31052283ae0367602e40e710621b02e4dbf1d44954966eb86",
            "208c114f2cd885629cbe2a89598469e12ded100969065156e2de32ea3f72",
            "041c6e36a8e7b9b77d58ec7ff7ad5d4e9dff37d3c0ef976ab358ee64f1b3",
            "1af35d65a01362bc64c8b6aec7e4959f192c3263a38f4f6c012797d6041f",
            "870ffed7ab8a4653b89d75b3997a7cb13cc08775ba779652c9c4316cb03a",
            "797244d257b4571b8bf928eb6d735b15f7a8d20239867844891664500a2c",
            "0d8b416c402d931f2664701a8d024a7f9d2911283f1ee487e8e43798a394",
            "dc2165e448003c8fb61aad773e109ceed3"
        ))
        .unwrap();
        let signature = hex::decode(concat!(
            "631411a560c6e1fa0277775ed0d44e4a3450a47c668f361cad2e925036d92445",
            "f18d9ecf4f2219d37315db11656e0794c76c5205420b6def7beb18cf75a2a88",
            "11889a8b9d1af52e11a0599852fe5ef3ab23182f7068215acd967a568e6f",
            "3dc9c3ca4284185b595ad3401937c96a1de0373c12eed680c9f4c7576ba47",
            "8aff03ba520460383103dcf0d6d66e0b65897bcbb8896d3f75c73561607",
            "8bbc3d8a623f31521a4c2f887a1595ede72728d6443b1b72bc07a3843ba",
            "200aa6701900d1eb296ed4dfdbc6cd2522519aabef7db0ff777e88b4ecd",
            "57a8b49237c37f05a8f4b3448d60f6e01847a67e38163b53f1b67cf109",
            "b6ccbb852569bb0e7bd780a7951df"
        ))
        .unwrap();
        let key = CosePublicKey::Rs256(rsa_spki_der(&modulus, &[0x01, 0x00, 0x01]).unwrap());

        key.verify(b"elastos-rs256-test", &signature).unwrap();
        assert!(key.verify(b"elastos-rs256-tampered", &signature).is_err());
    }

    #[test]
    fn principal_registration_does_not_exclude_existing_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = IdentityManager::new(temp.path().to_path_buf()).unwrap();
        manager.store.add_credential(StoredCredential {
            credential_id: "credential-id".to_string(),
            public_key: "public-key".to_string(),
            sign_count: 0,
            rp_id: "localhost".to_string(),
        });

        let backup_options = manager
            .begin_registration("backup-session", "localhost")
            .unwrap();
        assert_eq!(backup_options.public_key.exclude_credentials.len(), 1);

        let principal_options = manager
            .begin_principal_registration("guest-session", "localhost")
            .unwrap();
        assert!(principal_options.public_key.exclude_credentials.is_empty());
    }

    #[test]
    fn authentication_options_require_user_verification() {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = IdentityManager::new(temp.path().to_path_buf()).unwrap();
        manager.store.add_credential(StoredCredential {
            credential_id: "credential-id".to_string(),
            public_key: "public-key".to_string(),
            sign_count: 0,
            rp_id: "localhost".to_string(),
        });

        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();

        assert_eq!(options.public_key.user_verification, "required");
    }

    #[test]
    fn registration_response_rejects_extension_payloads() {
        let response = serde_json::json!({
            "id": "credential-id",
            "rawId": "credential-id",
            "type": "public-key",
            "clientExtensionResults": {
                "prf": {
                    "results": { "first": "raw-key-material" }
                }
            },
            "response": {
                "clientDataJson": "AA",
                "attestationObject": "AA"
            }
        });

        let err = serde_json::from_value::<RegistrationResponse>(response)
            .unwrap_err()
            .to_string();

        assert!(err.contains("clientExtensionResults"));
    }

    #[test]
    fn authentication_response_rejects_extension_payloads() {
        let response = serde_json::json!({
            "id": "credential-id",
            "rawId": "credential-id",
            "type": "public-key",
            "response": {
                "clientDataJson": "AA",
                "authenticatorData": "AA",
                "signature": "AA",
                "userHandle": null,
                "clientExtensionResults": {
                    "prf": {
                        "results": { "first": "raw-key-material" }
                    }
                }
            }
        });

        let err = serde_json::from_value::<AuthenticationResponse>(response)
            .unwrap_err()
            .to_string();

        assert!(err.contains("clientExtensionResults"));
    }

    #[test]
    fn auth_data_flags_must_include_user_presence_and_verification() {
        require_user_present_and_verified(FLAG_USER_PRESENT | FLAG_USER_VERIFIED).unwrap();

        let no_presence = require_user_present_and_verified(FLAG_USER_VERIFIED).unwrap_err();
        assert!(no_presence.to_string().contains("User presence"));

        let no_verification = require_user_present_and_verified(FLAG_USER_PRESENT).unwrap_err();
        assert!(no_verification.to_string().contains("User verification"));
    }

    #[test]
    fn registration_rejects_malformed_attested_credential_data_without_panicking() {
        // Regression: authData is lifted from the attacker-supplied attestation
        // object, so a truncated header or an oversized credential-id length must
        // fail closed with an error, not panic the auth thread (a DoS in an
        // authority primitive).
        let attested_flags = FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_ATTESTED_CREDENTIAL_DATA;

        // Case 1: AT flag set, but authData carries only the 37-byte header.
        {
            let mut manager = registration_manager();
            let challenge = manager
                .begin_registration("session-token", "localhost")
                .unwrap()
                .public_key
                .challenge;
            let mut auth_data_bytes = Vec::new();
            auth_data_bytes.extend_from_slice(&Sha256::digest("localhost".as_bytes()));
            auth_data_bytes.push(attested_flags);
            auth_data_bytes.extend_from_slice(&0u32.to_be_bytes()); // 37 bytes, no attested data
            let response = registration_response(&challenge, auth_data_bytes);
            let err = manager
                .complete_registration("session-token", &response, "localhost", "http://localhost")
                .unwrap_err()
                .to_string();
            assert!(err.contains("too short"), "unexpected error: {err}");
        }

        // Case 2: credential-id length claims more bytes than authData contains.
        {
            let mut manager = registration_manager();
            let challenge = manager
                .begin_registration("session-token", "localhost")
                .unwrap()
                .public_key
                .challenge;
            let mut auth_data_bytes = Vec::new();
            auth_data_bytes.extend_from_slice(&Sha256::digest("localhost".as_bytes()));
            auth_data_bytes.push(attested_flags);
            auth_data_bytes.extend_from_slice(&0u32.to_be_bytes());
            auth_data_bytes.extend_from_slice(&[0u8; 16]); // AAGUID
            auth_data_bytes.extend_from_slice(&u16::MAX.to_be_bytes()); // cred_id_len = 65535
            let response = registration_response(&challenge, auth_data_bytes);
            let err = manager
                .complete_registration("session-token", &response, "localhost", "http://localhost")
                .unwrap_err()
                .to_string();
            assert!(err.contains("out of range"), "unexpected error: {err}");
        }
    }

    fn registration_manager() -> IdentityManager {
        let temp = tempfile::tempdir().unwrap();
        IdentityManager::new(temp.path().to_path_buf()).unwrap()
    }

    fn registration_response(challenge: &str, auth_data_bytes: Vec<u8>) -> RegistrationResponse {
        let client_data = serde_json::json!({
            "type": "webauthn.create",
            "challenge": challenge,
            "origin": "http://localhost"
        });
        let att_obj = ciborium::Value::Map(vec![(
            ciborium::Value::Text("authData".to_string()),
            ciborium::Value::Bytes(auth_data_bytes),
        )]);
        let mut att_bytes = Vec::new();
        ciborium::into_writer(&att_obj, &mut att_bytes).unwrap();
        RegistrationResponse {
            _id: "credential-id".to_string(),
            _raw_id: "credential-id".to_string(),
            response: AuthenticatorAttestationResponse {
                client_data_json: URL_SAFE_NO_PAD.encode(client_data.to_string()),
                attestation_object: URL_SAFE_NO_PAD.encode(att_bytes),
            },
            _type: "public-key".to_string(),
        }
    }

    #[test]
    fn complete_registration_never_panics_on_fuzzed_attestation() {
        // Fuzz the attestation/authData parse path where a real DoS lived (an unchecked slice on
        // attacker-controlled authData). A trust-boundary parser must return Err, never panic, on
        // any input. Builds a valid rp-hash + attested flags so it reaches the credential-data
        // slicing, then fuzzes the tail length (straddling the 16-byte AAGUID + 2-byte length +
        // credential-id boundaries). Deterministic seed ⇒ any failure reproduces exactly.
        let mut m = registration_manager();
        let rp_hash = Sha256::digest("localhost".as_bytes());
        let attested = FLAG_USER_PRESENT | FLAG_USER_VERIFIED | FLAG_ATTESTED_CREDENTIAL_DATA;
        let mut state: u64 = 0xDEAD_BEEF_CAFE_F00D;
        macro_rules! rnd {
            () => {{
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            }};
        }
        for i in 0..6000u32 {
            let challenge = m
                .begin_registration("fuzz", "localhost")
                .unwrap()
                .public_key
                .challenge;
            let tail_len = (rnd!() as usize) % 48;
            let mut authdata = Vec::with_capacity(37 + tail_len);
            authdata.extend_from_slice(&rp_hash);
            authdata.push(attested);
            authdata.extend_from_slice(&i.to_be_bytes()); // sign_count → 37-byte header
            for _ in 0..tail_len {
                authdata.push((rnd!() & 0xff) as u8);
            }
            let resp = registration_response(&challenge, authdata.clone());
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = m.complete_registration("fuzz", &resp, "localhost", "http://localhost");
            }));
            assert!(
                r.is_ok(),
                "complete_registration PANICKED on authData ({} bytes): {:02x?}",
                authdata.len(),
                authdata
            );
        }
    }

    #[test]
    fn complete_registration_never_panics_on_random_blobs() {
        // Shallow fuzz: fully random client_data + attestation_object bytes exercise the early
        // base64 / JSON / CBOR reject path. Must fail closed (Err), never panic.
        let mut m = registration_manager();
        let mut state: u64 = 0x0123_4567_89AB_CDEF;
        macro_rules! rnd {
            () => {{
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            }};
        }
        for _ in 0..6000 {
            let _ = m.begin_registration("fuzz", "localhost");
            let blen = (rnd!() as usize) % 256;
            let bytes: Vec<u8> = (0..blen).map(|_| (rnd!() & 0xff) as u8).collect();
            let resp = RegistrationResponse {
                _id: "x".to_string(),
                _raw_id: "x".to_string(),
                response: AuthenticatorAttestationResponse {
                    client_data_json: URL_SAFE_NO_PAD.encode(&bytes),
                    attestation_object: URL_SAFE_NO_PAD.encode(&bytes),
                },
                _type: "public-key".to_string(),
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = m.complete_registration("fuzz", &resp, "localhost", "http://localhost");
            }));
            assert!(r.is_ok(), "complete_registration PANICKED on a random blob");
        }
    }

    fn manager_with_credential(sign_count: u32) -> IdentityManager {
        let temp = tempfile::tempdir().unwrap();
        let mut manager = IdentityManager::new(temp.path().to_path_buf()).unwrap();
        manager.store.add_credential(StoredCredential {
            credential_id: "credential-id".to_string(),
            public_key: "invalid-public-key".to_string(),
            sign_count,
            rp_id: "localhost".to_string(),
        });
        manager
    }

    fn auth_data(rp_id: &str, flags: u8, sign_count: u32) -> String {
        let mut data = Vec::new();
        data.extend_from_slice(&Sha256::digest(rp_id.as_bytes()));
        data.push(flags);
        data.extend_from_slice(&sign_count.to_be_bytes());
        URL_SAFE_NO_PAD.encode(data)
    }

    fn assertion_response(
        challenge: &str,
        origin: &str,
        rp_id: &str,
        flags: u8,
        sign_count: u32,
    ) -> AuthenticationResponse {
        let client_data = serde_json::json!({
            "type": "webauthn.get",
            "challenge": challenge,
            "origin": origin
        });
        AuthenticationResponse {
            _id: "credential-id".to_string(),
            raw_id: "credential-id".to_string(),
            response: AuthenticatorAssertionResponse {
                client_data_json: URL_SAFE_NO_PAD.encode(client_data.to_string()),
                authenticator_data: auth_data(rp_id, flags, sign_count),
                signature: URL_SAFE_NO_PAD.encode(b"not-a-der-signature"),
                _user_handle: None,
            },
            _type: "public-key".to_string(),
        }
    }

    #[test]
    fn authentication_rejects_wrong_origin_and_consumes_challenge() {
        let mut manager = manager_with_credential(0);
        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();
        let response = assertion_response(
            &options.public_key.challenge,
            "https://evil.example",
            "localhost",
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED,
            1,
        );

        let err = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Origin mismatch"));

        let replay = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();
        assert!(replay.contains("No pending authentication challenge"));
    }

    #[test]
    fn authentication_rejects_expired_challenge() {
        let mut manager = manager_with_credential(0);
        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();
        manager.expire_challenge_for_test("session-token");
        let response = assertion_response(
            &options.public_key.challenge,
            "http://localhost",
            "localhost",
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED,
            1,
        );

        let err = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();

        assert!(err.contains("expired"));
    }

    #[test]
    fn authentication_rejects_wrong_rp_hash() {
        let mut manager = manager_with_credential(0);
        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();
        let response = assertion_response(
            &options.public_key.challenge,
            "http://localhost",
            "evil.example",
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED,
            1,
        );

        let err = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();

        assert!(err.contains("RP ID hash mismatch"));
    }

    #[test]
    fn authentication_rejects_missing_user_verification() {
        let mut manager = manager_with_credential(0);
        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();
        let response = assertion_response(
            &options.public_key.challenge,
            "http://localhost",
            "localhost",
            FLAG_USER_PRESENT,
            1,
        );

        let err = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();

        assert!(err.contains("User verification"));
    }

    #[test]
    fn authentication_rejects_counter_regression_before_signature_check() {
        let mut manager = manager_with_credential(7);
        let options = manager
            .begin_authentication("session-token", "localhost")
            .unwrap();
        let response = assertion_response(
            &options.public_key.challenge,
            "http://localhost",
            "localhost",
            FLAG_USER_PRESENT | FLAG_USER_VERIFIED,
            7,
        );

        let err = manager
            .complete_authentication("session-token", &response, "localhost", "http://localhost")
            .unwrap_err()
            .to_string();

        assert!(err.contains("Credential clone detected"));
    }
}
