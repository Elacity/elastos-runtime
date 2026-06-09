//! `dkms-authority` — the EXTERNAL dKMS key-authority NODE (Day 87–88).
//!
//! This capsule is the SECRET-HOLDING half of the `dkms` backend. It owns the authority's
//! master key material (a durable, node-local key store) and exposes ONLY a `recover` op: given
//! a producer-escrowed CEK + the decrypt session's published key + the transcript binding, it
//! recovers the CEK INSIDE its own boundary and returns the suite-tagged `SealedDecryptMaterialV1`
//! re-sealed to that session — NEVER the raw CEK, NEVER the master.
//!
//! It is the runtime-core analogue of PC2's Lit/dKMS authority node
//! (`data/lit-actions/universal-decrypt-chipotle.js`): recover the CEK in the TEE
//! (`Lit.Actions.Decrypt`, `:572`) → rebind CEK↔KID↔authority (`:577`–`:590`) → seal-to-session
//! (`envelopeCEK`, `:602`–`:608`) → return ONLY the sealed envelope (`setResponse`, `:610`–`:613`).
//! The `key-provider` is the CLIENT that holds only this authority's PUBLIC identity and DELEGATES
//! recovery here (PC2's `recoverCEKEnvelope` RPCing the Lit network, holding only the public
//! `pkpId`/`authority`, `chipotle-client.ts:1438`). The master never crosses into the runtime.
//!
//! Protocol: newline-delimited JSON over stdin/stdout, one response object per request line.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = "0.1.0-dev";

/// The node's durable master-seed store schema. Node-local: the runtime never reads this file
/// (only the node does), and the master seed never leaves this process.
const NODE_KEYSTORE_SCHEMA: &str = "elastos.dkms_node.master_seed/v1";

/// Env var the node falls back to for its master-seed store path when `init` does not carry one.
/// The runtime/operator that PROVISIONS the node sets this; the `key-provider` CLIENT never sees
/// it (it only knows the node's endpoint + the node's PUBLIC identity).
const KEY_STORE_ENV: &str = "DKMS_AUTHORITY_KEY_STORE";

/// How long a session token the node mints at `hello` stays live (seconds). A long-lived node only
/// recovers for a caller whose handshake session is still within this window — a short, bounded
/// credential, the analogue of PC2's session TTL (`mediaSessionManager` lifetime).
const SESSION_TTL_SECONDS: u64 = 300;

/// A node-issued, node-signed SESSION TOKEN: it binds the client's handshake `challenge` to an
/// `expires_at` and the node SIGNS `(challenge, expires_at)` with its master-derived key. The node
/// REQUIRES one on every `recover` and verifies it under its OWN verifying key — so a long-lived
/// node recovers only for a caller that completed the handshake IN THIS (unexpired) session, and a
/// missing / expired / forged / tampered token is refused.
#[derive(Debug, Clone, Deserialize)]
struct SessionToken {
    challenge_b64: String,
    expires_at: u64,
    sig_b64: String,
}

/// The node's effective wall clock: the caller-supplied `now_unix` when present (keeps issuance +
/// expiry deterministic for tests + lock-stepped with the client's clock), else the real clock.
fn effective_now(now_unix: Option<u64>) -> u64 {
    now_unix.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
// The `recover` variant is intentionally wide (it carries the full recover bundle); these are
// short-lived protocol messages, so the size asymmetry across variants is not worth boxing.
#[allow(clippy::large_enum_variant)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    /// IDENTITY HANDSHAKE (Day 89–90): a client pins the node's published verifying key, sends a
    /// fresh random challenge, and the node returns a signature over it proving it holds the
    /// master-derived signing key BEHIND that vk — so the client can refuse an impersonated node
    /// before delegating any recovery. The runtime-core analogue of pinning the Lit network's
    /// identity (the published `pkpId`/`authority`).
    Hello {
        challenge_b64: String,
        /// The caller's clock, so the node mints an `expires_at` lock-stepped with it (the client
        /// re-checks liveness against the same clock). Absent → the node uses its own wall clock.
        #[serde(default)]
        now_unix: Option<u64>,
    },
    /// DELEGATED recovery: recover a producer-escrowed CEK in-boundary and re-seal it to the
    /// decrypt session. The CEK source (escrow blob), KID, scheme and producer key authenticate
    /// the escrow; the session key + transcript AAD bind the re-seal. The rights receipt + the
    /// content/principal/session binding let the node RE-CHECK authorization in its OWN boundary
    /// (PC2's Lit action re-runs `hasAccessByContentId` in the TEE, `universal-decrypt-chipotle.js:560`–`:568`)
    /// — it refuses to recover without a valid, content-bound authorization, even if the caller is
    /// buggy/compromised. NO raw CEK on any wire.
    Recover {
        wrapped_cek_b64: String,
        scheme: String,
        kid_hex: String,
        producer_vk_b64: String,
        decrypt_session_pub_b64: String,
        #[serde(default)]
        aad_b64: String,
        ciphertext_b64: String,
        content_hash_b64: String,
        nonce_b64: String,
        #[serde(default)]
        init_segment_b64: Option<String>,
        /// The upstream rights decision the node RE-VALIDATES in its own boundary.
        rights_receipt: elastos_common::protected_content::RightsDecisionReceiptV1,
        /// The content/principal/session/right the receipt MUST bind — the node refuses a receipt
        /// that does not match this declared identity (a replayed/foreign receipt is rejected).
        content_id: String,
        principal_id: String,
        session_id: String,
        right: String,
        /// The live session token from `hello` — REQUIRED. The node verifies it under its own key +
        /// checks it is unexpired BEFORE re-authorizing or touching any key material; a missing token
        /// is a hard parse error (no recover without a session).
        session_token: SessionToken,
        /// The caller's clock for the expiry check (absent → the node's own wall clock).
        #[serde(default)]
        now_unix: Option<u64>,
    },
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

impl Response {
    fn ok(data: Value) -> Self {
        Response::Ok { data: Some(data) }
    }
    fn empty_ok() -> Self {
        Response::Ok { data: None }
    }
    fn error(code: &str, message: impl Into<String>) -> Self {
        Response::Error { code: code.to_string(), message: message.into() }
    }
}

/// The node's authority: the master-derived ML-DSA signer (signs the re-seal) + the PQ-hybrid KEM
/// recipient (recovers the producer-escrowed CEK). Both are derived deterministically from ONE
/// persisted master seed, so the published identity is STABLE across node launches (the producer
/// escrows to it at publish time; an open relaunch re-derives the same identity).
struct NodeAuthority {
    signer: ddrm_envelope::seal::MlDsaSealSigner,
    verifying_key: Vec<u8>,
    recipient_secret: ddrm_envelope::SessionKemSecret,
    recipient_public: Vec<u8>,
}

impl NodeAuthority {
    fn from_master(master: &[u8; 32]) -> Self {
        // Domain-separated sub-seeds keep the signing key and the encryption recipient independent;
        // the SAME master always yields byte-identical keys (stable published identity).
        let seal_seed = ddrm_envelope::derive_seed(master, b"key-authority/seal/v1");
        let (signer, verifying_key) = ddrm_envelope::seal::mldsa_seal_keypair(seal_seed);
        let recipient_seed = ddrm_envelope::derive_seed(master, b"key-authority/recipient/v1");
        let (recipient_secret, recipient_public) =
            ddrm_envelope::mint_session_from_seed(recipient_seed);
        Self {
            signer,
            verifying_key,
            recipient_secret,
            recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
        }
    }

    /// Recover a CEK the producer escrowed to THIS node's recipient key. Recomputes the IDENTICAL
    /// escrow AAD (shared encoder) and verifies the producer's published key, then hybrid-unwraps
    /// with the node's recipient secret. Fails closed on any mismatch. The CEK stays in `Zeroizing`.
    fn recover_escrowed_cek(
        &self,
        wrapped_cek: &[u8],
        scheme: &str,
        kid_bytes16: &[u8; 16],
        producer_vk: &[u8],
    ) -> Result<zeroize::Zeroizing<Vec<u8>>, String> {
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(wrapped_cek)
            .map_err(|e| format!("malformed escrow envelope: {e:?}"))?;
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(producer_vk)
            .ok_or_else(|| "malformed producer verifying key".to_string())?;
        let aad =
            ddrm_envelope::transcript::escrow_aad(scheme, kid_bytes16, &self.recipient_public);
        ddrm_envelope::hybrid_unwrap_bound(&self.recipient_secret, &env, &aad, &verifier)
            .map_err(|e| format!("escrow recover failed: {e:?}"))
    }
}

#[derive(Default)]
struct DkmsAuthorityNode {
    authority: Option<NodeAuthority>,
}

impl DkmsAuthorityNode {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Hello { challenge_b64, now_unix } => self.hello(&challenge_b64, now_unix),
            Request::Recover {
                wrapped_cek_b64,
                scheme,
                kid_hex,
                producer_vk_b64,
                decrypt_session_pub_b64,
                aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
                rights_receipt,
                content_id,
                principal_id,
                session_id,
                right,
                session_token,
                now_unix,
            } => self.recover(RecoverArgs {
                wrapped_cek_b64,
                scheme,
                kid_hex,
                producer_vk_b64,
                decrypt_session_pub_b64,
                aad_b64,
                ciphertext_b64,
                content_hash_b64,
                nonce_b64,
                init_segment_b64,
                rights_receipt,
                content_id,
                principal_id,
                session_id,
                right,
                session_token,
                now_unix,
            }),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    /// IDENTITY HANDSHAKE: sign the client's challenge with the node's master-derived signing key
    /// and return the attestation + the published verifying key. The client verifies the attestation
    /// against the vk it PINNED from the descriptor, proving it is talking to the authentic node
    /// (not an impersonator) before it delegates any recovery. Requires `init` (no key, no identity).
    fn hello(&self, challenge_b64: &str, now_unix: Option<u64>) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => {
                return Response::error(
                    "not_configured",
                    "dkms-authority node is not initialized (send `init` first)",
                )
            }
        };
        let challenge = match b64().decode(challenge_b64) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => return Response::error("invalid_request", "challenge_b64 must be non-empty"),
            Err(_) => return Response::error("invalid_request", "challenge_b64 is not valid base64"),
        };
        let attestation = ddrm_envelope::attest_challenge(&authority.signer, &challenge);
        // Mint a node-signed SESSION TOKEN binding this challenge to a bounded expiry. The node will
        // REQUIRE (and re-verify) it on every recover, so this handshake gates a whole session of
        // recovers rather than a single spawn — the PC2 per-view-session analogue.
        let expires_at = effective_now(now_unix) + SESSION_TTL_SECONDS;
        let token_sig = ddrm_envelope::sign_session_token(&authority.signer, &challenge, expires_at);
        Response::ok(json!({
            "verifying_key_b64": b64().encode(&authority.verifying_key),
            "attestation_b64": b64().encode(&attestation),
            "session_token": {
                "challenge_b64": challenge_b64,
                "expires_at": expires_at,
                "sig_b64": b64().encode(&token_sig),
            },
        }))
    }

    /// Stand the node up from its durable master-seed store (config `authority_key_store`, else the
    /// `DKMS_AUTHORITY_KEY_STORE` env the provisioner set). Publishes the node's PUBLIC identity —
    /// the verifying key (so the decrypt boundary trusts its seals) and the KEM recipient (so the
    /// producer escrows the CEK to it). Fail-closed: no store configured, or a corrupt store, is an
    /// error rather than a silent re-mint (which would strand every CEK escrowed to the prior recipient).
    fn init(&mut self, config: Value) -> Response {
        let store_path = match config.get("authority_key_store").and_then(|v| v.as_str()) {
            Some(path) => path.to_string(),
            None => match std::env::var(KEY_STORE_ENV) {
                Ok(path) if !path.trim().is_empty() => path,
                _ => {
                    return Response::error(
                        "not_configured",
                        format!(
                            "dkms-authority node requires a master-seed store (config.authority_key_store or ${KEY_STORE_ENV})"
                        ),
                    )
                }
            },
        };
        let master = match load_or_create_master_seed(&store_path) {
            Ok(master) => master,
            Err(err) => return Response::error("not_configured", err),
        };
        let authority = NodeAuthority::from_master(&master);
        let data = json!({
            "provider": "dkms-authority",
            "protocol_version": "1.0",
            "seal_verifying_key_b64": b64().encode(&authority.verifying_key),
            "seal_recipient_pub_b64": b64().encode(&authority.recipient_public),
        });
        self.authority = Some(authority);
        Response::ok(data)
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "dkms-authority",
            "version": PROVIDER_VERSION,
            "configured": self.authority.is_some(),
            "supported_operations": ["status", "init", "hello", "recover"],
            // The node NEVER returns these — the master + raw CEK stay inside this boundary.
            "blocked_authority": ["raw_cek", "master_seed", "recipient_secret"],
        }))
    }

    /// DELEGATED recovery (the `key-provider` client RPCs this): recover the escrowed CEK in this
    /// boundary, re-seal it to the decrypt session, and return ONLY the sealed material. The raw
    /// CEK is held in `Zeroizing` and never echoed back; the master never leaves this process.
    fn recover(&self, args: RecoverArgs) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => {
                return Response::error(
                    "not_configured",
                    "dkms-authority node is not initialized (send `init` first)",
                )
            }
        };
        // SESSION GATE FIRST — refuse to recover without a live, node-verified handshake session
        // (the channel gate), before re-authorizing or touching any key material.
        if let Err(err) = verify_session(authority, &args) {
            return Response::error("session_invalid", err);
        }
        // RE-AUTHORIZE in this boundary — refuse to recover for an unauthorized caller before
        // touching any key material (the node never trusts the client's claim).
        if let Err(err) = reauthorize(&args) {
            return Response::error("access_denied", err);
        }
        let wrapped = match b64().decode(&args.wrapped_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "wrapped_cek_b64 is not valid base64"),
        };
        let producer_vk = match b64().decode(&args.producer_vk_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "producer_vk_b64 is not valid base64"),
        };
        let kid16 = match decode_kid_bytes16(&args.kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        let public = match b64()
            .decode(&args.decrypt_session_pub_b64)
            .ok()
            .and_then(|bytes| ddrm_envelope::session_public_from_bytes(&bytes))
        {
            Some(public) => public,
            None => {
                return Response::error(
                    "invalid_request",
                    "decrypt_session_pub_b64 is not a valid session public key",
                )
            }
        };
        let aad = match b64().decode(&args.aad_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "aad_b64 is not valid base64"),
        };

        // Recover in-boundary (fail-closed on a foreign/tampered blob, KID-swap, scheme mismatch,
        // or forged producer), then re-seal to the session. The CEK never leaves unsealed.
        let cek = match authority.recover_escrowed_cek(&wrapped, &args.scheme, &kid16, &producer_vk) {
            Ok(cek) => cek,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "escrowed CEK could not be recovered (foreign/tampered escrow, wrong KID/scheme, or bad producer key)",
                )
            }
        };

        let envelope = ddrm_envelope::seal::seal_bound(&public, cek.as_slice(), &aad, &authority.signer);
        let mut material = json!({
            "suite": ddrm_envelope::SUITE_PQ_HYBRID,
            "sealed_cek_b64": b64().encode(envelope.to_bytes()),
            "ciphertext_b64": args.ciphertext_b64,
            "nonce_b64": args.nonce_b64,
            "content_hash_b64": args.content_hash_b64,
        });
        if let Some(init) = args.init_segment_b64 {
            material["init_segment_b64"] = json!(init);
        }
        Response::ok(json!({
            "suite": ddrm_envelope::SUITE_PQ_HYBRID,
            "material": material,
            "seal_verifying_key_b64": b64().encode(&authority.verifying_key),
        }))
    }
}

#[derive(Clone)]
struct RecoverArgs {
    wrapped_cek_b64: String,
    scheme: String,
    kid_hex: String,
    producer_vk_b64: String,
    decrypt_session_pub_b64: String,
    aad_b64: String,
    ciphertext_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
    init_segment_b64: Option<String>,
    rights_receipt: elastos_common::protected_content::RightsDecisionReceiptV1,
    content_id: String,
    principal_id: String,
    session_id: String,
    right: String,
    session_token: SessionToken,
    now_unix: Option<u64>,
}

/// VERIFY the caller's session token in the node's OWN boundary: it must be a token THIS node
/// signed (valid signature over `(challenge, expires_at)` under the node's verifying key) and still
/// unexpired against the caller's clock. A forged/tampered token (bad signature) or an expired one
/// is refused — so a long-lived node recovers only for a caller with a live handshake session, and
/// a captured handshake cannot be replayed past its window. The runtime-core analogue of PC2
/// resurrecting + validating the per-view session before recovery (`secureViewSession.ts:81`–`:128`).
fn verify_session(authority: &NodeAuthority, args: &RecoverArgs) -> Result<(), String> {
    let challenge = b64()
        .decode(&args.session_token.challenge_b64)
        .map_err(|_| "session token challenge is not valid base64".to_string())?;
    let sig = b64()
        .decode(&args.session_token.sig_b64)
        .map_err(|_| "session token signature is not valid base64".to_string())?;
    let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&authority.verifying_key)
        .ok_or("node verifying key is malformed")?;
    if !ddrm_envelope::verify_session_token(&verifier, &challenge, args.session_token.expires_at, &sig) {
        return Err("session token is forged or tampered (signature does not verify)".to_string());
    }
    if effective_now(args.now_unix) > args.session_token.expires_at {
        return Err("session token has expired — re-establish the handshake session".to_string());
    }
    Ok(())
}

/// RE-CHECK the rights authorization in the node's OWN boundary before recovering anything. The
/// node does NOT trust the caller: the receipt must be a valid, ALLOWED, protected-content
/// authorization that binds the SAME content/principal/session/right the recover declares — so a
/// buggy/compromised client that forwards a denied, foreign, or incoherent receipt is refused. The
/// runtime-core analogue of PC2's Lit action re-running `hasAccessByContentId` in the TEE
/// (`universal-decrypt-chipotle.js:560`–`:568`) rather than trusting the caller's claim.
fn reauthorize(args: &RecoverArgs) -> Result<(), String> {
    use elastos_common::protected_content::{PROTECTED_CONTENT_ACTIONS, RIGHTS_DECISION_RECEIPT_SCHEMA};
    let r = &args.rights_receipt;
    if r.schema != RIGHTS_DECISION_RECEIPT_SCHEMA {
        return Err("rights receipt schema is unsupported".to_string());
    }
    if !r.allowed {
        return Err("rights receipt does not authorize this recovery".to_string());
    }
    if !PROTECTED_CONTENT_ACTIONS.contains(&r.right.as_str()) {
        return Err(format!("rights receipt right is not a protected-content action: {}", r.right));
    }
    if r.content_id != args.content_id {
        return Err("rights receipt content does not match the recover request".to_string());
    }
    if r.principal_id != args.principal_id {
        return Err("rights receipt principal does not match the recover request".to_string());
    }
    if r.session_id != args.session_id {
        return Err("rights receipt session does not match the recover request".to_string());
    }
    if r.right != args.right {
        return Err("rights receipt right does not match the recover request".to_string());
    }
    Ok(())
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Load the node's master seed from its durable store, or create + persist one on first launch.
/// Atomic write (`*.tmp` → `rename`, mode 0600). Fail-closed on a present-but-corrupt store.
fn load_or_create_master_seed(path: &str) -> Result<[u8; 32], String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let record: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("dkms node key store {path} is corrupt: {e}"))?;
            if record.get("schema").and_then(|v| v.as_str()) != Some(NODE_KEYSTORE_SCHEMA) {
                return Err(format!("dkms node key store {path} has an unexpected schema"));
            }
            let seed_b64 = record
                .get("master_seed_b64")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("dkms node key store {path} is missing master_seed_b64"))?;
            let seed_bytes = b64()
                .decode(seed_b64)
                .map_err(|e| format!("dkms node key store {path} seed is not base64: {e}"))?;
            if seed_bytes.len() != 32 {
                return Err(format!(
                    "dkms node key store {path} seed is {} bytes, expected 32",
                    seed_bytes.len()
                ));
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&seed_bytes);
            Ok(seed)
        }
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
            let seed = ddrm_envelope::random_seed();
            let record = json!({
                "schema": NODE_KEYSTORE_SCHEMA,
                "master_seed_b64": b64().encode(seed),
            });
            persist_atomic(path, &serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?)?;
            Ok(seed)
        }
        Err(e) => Err(format!("dkms node key store {path}: {e}")),
    }
}

fn persist_atomic(path: &str, bytes: &[u8]) -> Result<(), String> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {tmp}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {tmp} -> {path}: {e}"))
}

/// Decode a 32-hex KID into the on-chain `bytes16` contentId the escrow AAD binds.
fn decode_kid_bytes16(kid_hex: &str) -> Result<[u8; 16], String> {
    if kid_hex.len() != 32 || !kid_hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("kid_hex must be 32 lowercase-hex chars (bytes16 contentId)".to_string());
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&kid_hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("kid hex: {e}"))?;
    }
    Ok(out)
}

fn main() {
    eprintln!("dkms-authority: starting v{PROVIDER_VERSION} (external key authority node)");
    let mut node = DkmsAuthorityNode::default();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("dkms-authority read error: {err}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = Response::error("invalid_request", err.to_string());
                writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
                stdout.flush().unwrap();
                continue;
            }
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = node.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }
    eprintln!("dkms-authority exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_data(resp: Response) -> Value {
        match resp {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected ok data, got {other:?}"),
        }
    }
    fn error_code(resp: &Response) -> &str {
        match resp {
            Response::Error { code, .. } => code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    fn unique_store(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("dkms-node-{tag}-{}-{nanos}.json", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    const CONTENT: &str = "bafybeigdyrcontent";
    const PRINCIPAL: &str = "did:key:zViewer";
    const SESSION: &str = "session:abc";
    const RIGHT: &str = "view";
    const NOW: u64 = 1_700_000_000;

    /// Drive the node's real `hello` to mint a live session token at clock `NOW`.
    fn live_token(node: &DkmsAuthorityNode) -> SessionToken {
        let challenge = b64().encode([0xA1u8; 32]);
        let hello = ok_data(node.hello(&challenge, Some(NOW)));
        let t = &hello["session_token"];
        SessionToken {
            challenge_b64: t["challenge_b64"].as_str().unwrap().to_string(),
            expires_at: t["expires_at"].as_u64().unwrap(),
            sig_b64: t["sig_b64"].as_str().unwrap().to_string(),
        }
    }

    /// A structurally-valid but never-verified token (for cases that fail BEFORE the session gate).
    fn dummy_token() -> SessionToken {
        SessionToken {
            challenge_b64: b64().encode([0u8; 32]),
            expires_at: NOW + 1,
            sig_b64: b64().encode([0u8; 8]),
        }
    }

    /// A valid, ALLOWED rights receipt bound to the canonical test content/principal/session/right.
    fn good_receipt() -> elastos_common::protected_content::RightsDecisionReceiptV1 {
        elastos_common::protected_content::RightsDecisionReceiptV1 {
            schema: elastos_common::protected_content::RIGHTS_DECISION_RECEIPT_SCHEMA.to_string(),
            request_id: "rights:test".to_string(),
            content_id: CONTENT.to_string(),
            principal_id: PRINCIPAL.to_string(),
            session_id: SESSION.to_string(),
            right: RIGHT.to_string(),
            provider: "rights-provider".to_string(),
            allowed: true,
            issued_at: 1,
            expires_at: u64::MAX,
        }
    }

    /// Escrow a CEK to the node's published recipient exactly as the producer does, then drive a
    /// transcript-bound `recover`; the sealed material the node returns opens to the SAME CEK.
    #[test]
    fn recover_round_trips_an_escrowed_cek_and_re_seals_to_the_session() {
        let store = unique_store("roundtrip");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let recipient_pub_b64 = init["seal_recipient_pub_b64"].as_str().unwrap().to_string();
        let recipient_pub = b64().decode(&recipient_pub_b64).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();

        // Producer escrows a CEK to the node's recipient under the shared escrow AAD.
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let cek: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &escrow_aad, &producer_signer);

        // The decrypt boundary's session key + a transcript AAD bind the re-seal.
        let (session_secret, session_public) = ddrm_envelope::mint_session();
        let session_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&session_public));
        let transcript_aad = b"day87-88-transcript".to_vec();

        let token = live_token(&node);
        let resp = node.recover(RecoverArgs {
            wrapped_cek_b64: b64().encode(wrapped.to_bytes()),
            scheme: scheme.to_string(),
            kid_hex,
            producer_vk_b64: b64().encode(&producer_vk),
            decrypt_session_pub_b64: session_pub_b64,
            aad_b64: b64().encode(&transcript_aad),
            ciphertext_b64: b64().encode(b"ct"),
            content_hash_b64: b64().encode(b"hash"),
            nonce_b64: b64().encode(b"nonce"),
            init_segment_b64: None,
            rights_receipt: good_receipt(),
            content_id: CONTENT.to_string(),
            principal_id: PRINCIPAL.to_string(),
            session_id: SESSION.to_string(),
            right: RIGHT.to_string(),
            session_token: token,
            now_unix: Some(NOW),
        });
        let data = ok_data(resp);
        // The response carries SEALED material only — never a raw CEK.
        assert!(data["material"].get("sealed_cek_b64").is_some());
        let sealed_str = serde_json::to_string(&data).unwrap();
        assert!(!sealed_str.contains(&b64().encode(&cek)), "the raw CEK must never appear in the response");

        // The decrypt boundary opens the sealed material to the original CEK.
        let sealed = b64().decode(data["material"]["sealed_cek_b64"].as_str().unwrap()).unwrap();
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed).unwrap();
        let node_vk = b64().decode(data["seal_verifying_key_b64"].as_str().unwrap()).unwrap();
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&node_vk).unwrap();
        let opened = ddrm_envelope::hybrid_unwrap_bound(&session_secret, &env, &transcript_aad, &verifier).unwrap();
        assert_eq!(opened.as_slice(), cek.as_slice());

        let _ = std::fs::remove_file(&store);
    }

    /// The published identity is STABLE across node relaunches (escrow-at-publish works): the same
    /// store yields the same verifying key + recipient.
    #[test]
    fn published_identity_is_stable_across_relaunches() {
        let store = unique_store("stable");
        let mut a = DkmsAuthorityNode::default();
        let da = ok_data(a.init(json!({ "authority_key_store": store })));
        let mut b = DkmsAuthorityNode::default();
        let db = ok_data(b.init(json!({ "authority_key_store": store })));
        assert_eq!(da["seal_verifying_key_b64"], db["seal_verifying_key_b64"]);
        assert_eq!(da["seal_recipient_pub_b64"], db["seal_recipient_pub_b64"]);
        let _ = std::fs::remove_file(&store);
    }

    /// Recover fails closed on a forged producer key (the escrow authenticates the producer), and
    /// before `init` (no master, no recovery).
    #[test]
    fn recover_fails_closed_on_forged_producer_and_before_init() {
        let store = unique_store("forged");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let recipient_pub = b64().decode(init["seal_recipient_pub_b64"].as_str().unwrap()).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        let (producer_signer, _producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([1u8; 32]);
        let (_forged_signer, forged_vk) = ddrm_envelope::seal::mldsa_seal_keypair([2u8; 32]);
        let cek: Vec<u8> = (0u8..16).collect();
        let kid16 = [0x11u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &aad, &producer_signer);
        let (_s, session_public) = ddrm_envelope::mint_session();
        let session_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&session_public));

        // Forged producer vk → recover fails closed (a live session passes the gate, so the producer
        // check is what we're exercising).
        let token = live_token(&node);
        let forged = node.recover(RecoverArgs {
            wrapped_cek_b64: b64().encode(wrapped.to_bytes()),
            scheme: scheme.to_string(),
            kid_hex: kid_hex.clone(),
            producer_vk_b64: b64().encode(&forged_vk),
            decrypt_session_pub_b64: session_pub_b64.clone(),
            aad_b64: b64().encode(b"t"),
            ciphertext_b64: b64().encode(b"ct"),
            content_hash_b64: b64().encode(b"h"),
            nonce_b64: b64().encode(b"n"),
            init_segment_b64: None,
            rights_receipt: good_receipt(),
            content_id: CONTENT.to_string(),
            principal_id: PRINCIPAL.to_string(),
            session_id: SESSION.to_string(),
            right: RIGHT.to_string(),
            session_token: token,
            now_unix: Some(NOW),
        });
        assert_eq!(error_code(&forged), "invalid_request");

        // Before init → not_configured (the authority check precedes the session gate, so a dummy
        // token is never reached).
        let fresh = DkmsAuthorityNode::default();
        let pre = fresh.recover(RecoverArgs {
            wrapped_cek_b64: b64().encode(wrapped.to_bytes()),
            scheme: scheme.to_string(),
            kid_hex,
            producer_vk_b64: b64().encode(&forged_vk),
            decrypt_session_pub_b64: session_pub_b64,
            aad_b64: b64().encode(b"t"),
            ciphertext_b64: b64().encode(b"ct"),
            content_hash_b64: b64().encode(b"h"),
            nonce_b64: b64().encode(b"n"),
            init_segment_b64: None,
            rights_receipt: good_receipt(),
            content_id: CONTENT.to_string(),
            principal_id: PRINCIPAL.to_string(),
            session_id: SESSION.to_string(),
            right: RIGHT.to_string(),
            session_token: dummy_token(),
            now_unix: Some(NOW),
        });
        assert_eq!(error_code(&pre), "not_configured");
        let _ = std::fs::remove_file(&store);
    }

    /// Fail-closed when no master store is configured (neither config nor env).
    #[test]
    fn init_fails_closed_without_a_store() {
        std::env::remove_var(KEY_STORE_ENV);
        let mut node = DkmsAuthorityNode::default();
        assert_eq!(error_code(&node.init(json!({}))), "not_configured");
    }

    /// Build an initialized node plus a recover request whose escrow + transcript are valid, so a
    /// re-auth test can vary ONLY the receipt/binding and observe the node's independent decision.
    fn setup_recover(store: &str) -> (DkmsAuthorityNode, RecoverArgs) {
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let recipient_pub = b64().decode(init["seal_recipient_pub_b64"].as_str().unwrap()).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let cek: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &escrow_aad, &producer_signer);
        let (_session_secret, session_public) = ddrm_envelope::mint_session();
        let session_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&session_public));
        let token = live_token(&node);
        let args = RecoverArgs {
            wrapped_cek_b64: b64().encode(wrapped.to_bytes()),
            scheme: scheme.to_string(),
            kid_hex,
            producer_vk_b64: b64().encode(&producer_vk),
            decrypt_session_pub_b64: session_pub_b64,
            aad_b64: b64().encode(b"transcript"),
            ciphertext_b64: b64().encode(b"ct"),
            content_hash_b64: b64().encode(b"hash"),
            nonce_b64: b64().encode(b"nonce"),
            init_segment_b64: None,
            rights_receipt: good_receipt(),
            content_id: CONTENT.to_string(),
            principal_id: PRINCIPAL.to_string(),
            session_id: SESSION.to_string(),
            right: RIGHT.to_string(),
            session_token: token,
            now_unix: Some(NOW),
        };
        (node, args)
    }

    /// The node's IDENTITY handshake: a `hello` returns the published vk + an attestation that
    /// verifies under that pinned vk for the supplied challenge — and refuses before `init`.
    #[test]
    fn hello_attests_node_identity_and_requires_init() {
        let store = unique_store("hello");
        let mut node = DkmsAuthorityNode::default();

        // Before init there is no key material → fail closed.
        assert_eq!(error_code(&node.hello(&b64().encode([1u8; 32]), Some(NOW))), "not_configured");

        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let pinned_vk_b64 = init["seal_verifying_key_b64"].as_str().unwrap().to_string();

        let challenge = ddrm_envelope::random_seed();
        let resp = ok_data(node.hello(&b64().encode(challenge), Some(NOW)));
        // The node advertises the SAME vk it published at init (the pin).
        assert_eq!(resp["verifying_key_b64"].as_str().unwrap(), pinned_vk_b64);

        // The attestation verifies under the PINNED vk for THIS challenge.
        let pinned = b64().decode(&pinned_vk_b64).unwrap();
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned).unwrap();
        let attestation = b64().decode(resp["attestation_b64"].as_str().unwrap()).unwrap();
        assert!(ddrm_envelope::verify_attestation(&verifier, &challenge, &attestation));

        // hello ALSO mints a node-signed SESSION TOKEN bound to this challenge + a bounded expiry,
        // and the token verifies under the node's own vk (the node will require it on recover).
        let token = &resp["session_token"];
        assert_eq!(token["expires_at"].as_u64().unwrap(), NOW + SESSION_TTL_SECONDS);
        let token_sig = b64().decode(token["sig_b64"].as_str().unwrap()).unwrap();
        assert!(ddrm_envelope::verify_session_token(
            &verifier,
            &challenge,
            NOW + SESSION_TTL_SECONDS,
            &token_sig
        ));

        // An impersonating node's vk would NOT verify this attestation (client pins + rejects).
        let (_other, other_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0xEEu8; 32]);
        let other_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&other_vk).unwrap();
        assert!(!ddrm_envelope::verify_attestation(&other_verifier, &challenge, &attestation));

        // A different challenge (replay) does not verify under the genuine vk either.
        let mut replay = challenge;
        replay[0] ^= 1;
        assert!(!ddrm_envelope::verify_attestation(&verifier, &replay, &attestation));

        let _ = std::fs::remove_file(&store);
    }

    /// The node RE-AUTHORIZES in its own boundary: it refuses to recover when the receipt is denied,
    /// or binds different content/principal/session/right than the recover declares — even though
    /// the escrow + transcript are otherwise perfectly valid (a buggy/compromised caller is caught).
    #[test]
    fn recover_fails_closed_on_unauthorized_or_mismatched_receipt() {
        let store = unique_store("reauth");
        // One node + one valid escrow; each case clones the base args and varies ONLY the receipt.
        let (node, base) = setup_recover(&store);

        // Denied receipt → access_denied.
        let mut denied = base.clone();
        denied.rights_receipt.allowed = false;
        assert_eq!(error_code(&node.recover(denied)), "access_denied");

        // Receipt binds DIFFERENT content than the recover declares → access_denied.
        let mut wrong_content = base.clone();
        wrong_content.rights_receipt.content_id = "bafybeigOTHER".to_string();
        assert_eq!(error_code(&node.recover(wrong_content)), "access_denied");

        // Receipt binds DIFFERENT principal → access_denied.
        let mut wrong_principal = base.clone();
        wrong_principal.rights_receipt.principal_id = "did:key:zAttacker".to_string();
        assert_eq!(error_code(&node.recover(wrong_principal)), "access_denied");

        // Receipt right is not a protected-content action → access_denied.
        let mut bad_right = base.clone();
        bad_right.rights_receipt.right = "delete".to_string();
        bad_right.right = "delete".to_string();
        assert_eq!(error_code(&node.recover(bad_right)), "access_denied");

        // Sanity: the SAME setup with a coherent allowed receipt recovers (re-auth is the only gate
        // we varied above), proving the failures are the re-auth, not a broken fixture.
        assert!(matches!(node.recover(base), Response::Ok { .. }));

        let _ = std::fs::remove_file(&store);
    }

    /// A `recover` with NO session token is a hard parse error — the node never recovers without a
    /// handshake session (the `session_token` field is required on the wire).
    #[test]
    fn recover_without_a_session_token_is_rejected_at_the_protocol() {
        // A recover line that omits session_token must fail to deserialize into a Request.
        let line = json!({
            "op": "recover",
            "wrapped_cek_b64": "AA==", "scheme": "s", "kid_hex": "00",
            "producer_vk_b64": "AA==", "decrypt_session_pub_b64": "AA==",
            "ciphertext_b64": "AA==", "content_hash_b64": "AA==", "nonce_b64": "AA==",
            "rights_receipt": good_receipt(),
            "content_id": CONTENT, "principal_id": PRINCIPAL, "session_id": SESSION, "right": RIGHT,
        });
        let parsed = serde_json::from_value::<Request>(line);
        assert!(parsed.is_err(), "recover without session_token must not deserialize");
    }

    /// The node refuses a recover whose session token is EXPIRED, FORGED, or TAMPERED — even with a
    /// perfectly valid escrow + receipt — so a long-lived node only recovers within a live handshake
    /// session and a captured/forged token cannot drive recovery.
    #[test]
    fn recover_fails_closed_on_an_expired_or_forged_session() {
        let store = unique_store("session");
        let (node, base) = setup_recover(&store);

        // Expired: a clock past the token's expiry → session_invalid.
        let mut expired = base.clone();
        expired.now_unix = Some(base.session_token.expires_at + 1);
        assert_eq!(error_code(&node.recover(expired)), "session_invalid");

        // Forged signature: tamper the token sig → it no longer verifies under the node's vk.
        let mut forged = base.clone();
        forged.session_token.sig_b64 = b64().encode([0u8; 8]);
        assert_eq!(error_code(&node.recover(forged)), "session_invalid");

        // Tampered binding: change the token's challenge → signature no longer matches.
        let mut tampered = base.clone();
        tampered.session_token.challenge_b64 = b64().encode([0x99u8; 32]);
        assert_eq!(error_code(&node.recover(tampered)), "session_invalid");

        // Tampered expiry: extend the window → signature (over challenge+expiry) no longer matches.
        let mut extended = base.clone();
        extended.session_token.expires_at += 10_000;
        assert_eq!(error_code(&node.recover(extended)), "session_invalid");

        // Sanity: the unmodified live-session base recovers (the failures above are the session gate).
        assert!(matches!(node.recover(base), Response::Ok { .. }));

        let _ = std::fs::remove_file(&store);
    }

    /// ONE handshake session authorizes MANY recovers — the node accepts the same live token across
    /// repeated recovers (the persistent-session shape: hello once, recover many).
    #[test]
    fn one_session_authorizes_many_recovers() {
        let store = unique_store("many");
        let (node, base) = setup_recover(&store);
        // Reuse the SAME session token across three recovers — all succeed.
        for _ in 0..3 {
            assert!(matches!(node.recover(base.clone()), Response::Ok { .. }));
        }
        let _ = std::fs::remove_file(&store);
    }
}
