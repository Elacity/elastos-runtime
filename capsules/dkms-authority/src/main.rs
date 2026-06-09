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
//! Protocol: the same JSON request/response ops over two transports —
//!   * default: newline-delimited JSON over stdin/stdout (one-shot provisioning + tests);
//!   * `DKMS_AUTHORITY_LISTEN=<path>`: a length-prefixed FRAMED request/response transport over a
//!     Unix-domain socket the node BINDS + LISTENS on (a real remote-authority shape — the runtime
//!     connects but does not own the node's process). Many sequential connections, one handshake
//!     SESSION per connection; an oversized/torn/half-closed frame fails closed WITHOUT wedging the
//!     node (the connection is dropped, the listener keeps serving).

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const PROVIDER_VERSION: &str = "0.1.0-dev";

/// Env var selecting the SOCKET serve transport: when set to a path, the node binds + listens there
/// instead of running the stdin/stdout loop. `unix`-only (no Unix sockets on wasm32-wasip1).
#[cfg(unix)]
const LISTEN_ENV: &str = "DKMS_AUTHORITY_LISTEN";

/// The node's durable master-seed store schema. Node-local: the runtime never reads this file
/// (only the node does), and the master seed never leaves this process.
const NODE_KEYSTORE_SCHEMA: &str = "elastos.dkms_node.master_seed/v1";

/// Env var the node falls back to for its master-seed store path when `init` does not carry one.
/// The runtime/operator that PROVISIONS the node sets this; the `key-provider` CLIENT never sees
/// it (it only knows the node's endpoint + the node's PUBLIC identity).
const KEY_STORE_ENV: &str = "DKMS_AUTHORITY_KEY_STORE";

/// Env var carrying the node's ALLOW-LIST of KNOWN caller identities (Day 95–96): a comma-separated
/// list of base64 ML-DSA verifying keys. The OPERATOR/PROVISIONER who launches the daemon sets it;
/// the connecting CLIENT cannot override it (its `init {}` never clears it). When set + non-empty,
/// `hello` REFUSES a caller whose ephemeral pubkey is not on the list — the node serves only KNOWN
/// callers, the runtime-core analogue of PC2's session being OWNER-BOUND to a registered wallet
/// (`secureViewSession.ts:87`–`:100`). When unset/empty the node accepts any well-formed caller key
/// (anonymous enrollment — dev/test only; the production rail always provisions the allow-list).
#[cfg(unix)]
const ALLOWED_CALLERS_ENV: &str = "DKMS_AUTHORITY_ALLOWED_CALLERS";

/// How long a session token the node mints at `hello` stays live (seconds). A long-lived node only
/// recovers for a caller whose handshake session is still within this window — a short, bounded
/// credential, the analogue of PC2's session TTL (`mediaSessionManager` lifetime).
const SESSION_TTL_SECONDS: u64 = 300;

/// A node-issued, node-signed SESSION TOKEN: it binds the client's handshake `challenge` AND the
/// caller's ephemeral PUBLIC key (`caller_pub_b64`) to an `expires_at`, and the node SIGNS
/// `(challenge, caller_pub, expires_at)` with its master-derived key. The node REQUIRES one on every
/// `recover` and verifies it under its OWN verifying key — so a long-lived node recovers only for a
/// caller that completed the handshake IN THIS (unexpired) session. Binding `caller_pub` is what
/// makes the bearer token NON-REPLAYABLE: recover also requires a signature under the matching
/// private key (Day 93–94). A missing / expired / forged / tampered token is refused.
#[derive(Debug, Clone, Deserialize)]
struct SessionToken {
    challenge_b64: String,
    /// The caller's ephemeral verifying key (base64) the token is bound to — recover must prove
    /// possession of the matching private key.
    caller_pub_b64: String,
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
        /// The caller's EPHEMERAL verifying key (base64). The node binds the session token to it; the
        /// caller must prove possession of the matching private key on every recover, so a captured
        /// token replayed by a DIFFERENT caller (no private key) is refused.
        caller_pub_b64: String,
        /// The caller's clock, so the node mints an `expires_at` lock-stepped with it (the client
        /// re-checks liveness against the same clock). Absent → the node uses its own wall clock.
        #[serde(default)]
        now_unix: Option<u64>,
        /// ENCRYPTED CHANNEL (Day 105–108): the caller's EPHEMERAL channel KEM public key. When
        /// present, the node's hello response also carries the node's master-derived channel KEM key
        /// ATTESTED under its identity (`attest_channel_key` over `(challenge, channel_pub)`), and the
        /// connection's subsequent frames are sealed envelopes in BOTH directions (the transport layer
        /// enforces this). Required by the network (TCP) transport; optional on the Unix socket.
        #[serde(default)]
        channel_pub_b64: Option<String>,
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
        /// The caller's POSSESSION PROOF — REQUIRED (Day 93–94). A signature under the ephemeral
        /// private key whose public half the token is bound to, over the session challenge + this
        /// recover's content binding + the freshness counter. The node verifies it against the
        /// token-bound pubkey, so a captured token replayed without the private key (or signed by the
        /// wrong key) is refused.
        caller_sig_b64: String,
        /// The per-recover FRESHNESS counter — REQUIRED (Day 95–96). A strictly-increasing sequence
        /// number bound INTO the possession proof. The node tracks the highest it has consumed in
        /// THIS session and refuses any recover whose `recover_seq` does not advance — so a captured
        /// recover frame replayed verbatim (same seq) is refused even by the legitimate caller. The
        /// runtime-core analogue of PC2's revocable per-delegation `nonce` (`secureViewSession.ts:108`–`:112`).
        recover_seq: u64,
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
    /// The node's ENCRYPTED-CHANNEL KEM key (Day 105–108) — master-derived (stable across launches,
    /// domain-separated from the escrow recipient so channel traffic and escrow blobs can never be
    /// confused). Published at `hello` under a channel-key attestation; a network client encapsulates
    /// to it and the node proves possession by decapsulating every sealed frame. Only the socket
    /// serve loop decapsulates, so the secret half is unread on the (transport-less) wasm ladder.
    #[cfg_attr(not(unix), allow(dead_code))]
    channel_secret: ddrm_envelope::SessionKemSecret,
    channel_public: Vec<u8>,
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
        let channel_seed = ddrm_envelope::derive_seed(master, b"key-authority/channel/v1");
        let (channel_secret, channel_public) =
            ddrm_envelope::mint_session_from_seed(channel_seed);
        Self {
            signer,
            verifying_key,
            recipient_secret,
            recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
            channel_secret,
            channel_public: ddrm_envelope::session_public_bytes(&channel_public),
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
    /// Boxed: the authority carries several KB of PQ key material (ML-DSA signer + two KEM
    /// secrets); keeping it on the heap keeps the node struct cheap to move and keeps test-thread
    /// stacks (2 MiB) clear of the dev-profile PQ stack pressure.
    authority: Option<Box<NodeAuthority>>,
    /// The KNOWN-caller allow-list (Day 95–96): decoded ML-DSA verifying keys the node will serve.
    /// `None` = anonymous enrollment (any well-formed caller key); `Some(list)` = `hello` refuses a
    /// caller not on the list. Set by the OPERATOR (daemon env / direct construction), NOT by the
    /// connecting client — so a client cannot widen who the node serves.
    allowed_callers: Option<Vec<Vec<u8>>>,
    /// The highest per-recover FRESHNESS counter consumed in this connection's session (Day 95–96).
    /// `recover` requires a strictly-greater `recover_seq`, so a replayed recover frame is refused.
    /// Per-connection state (a fresh connection = a fresh session = counter resets, but a fresh
    /// session also requires a fresh `hello` + possession proof, so cross-connection replay is
    /// already blocked by the caller-bound token).
    last_recover_seq: u64,
}

impl DkmsAuthorityNode {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Hello { challenge_b64, caller_pub_b64, now_unix, channel_pub_b64 } => {
                self.hello(&challenge_b64, &caller_pub_b64, now_unix, channel_pub_b64.as_deref())
            }
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
                caller_sig_b64,
                recover_seq,
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
                caller_sig_b64,
                recover_seq,
                now_unix,
            }),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    /// IDENTITY HANDSHAKE: sign the client's challenge with the node's master-derived signing key
    /// and return the attestation + the published verifying key. The client verifies the attestation
    /// against the vk it PINNED from the descriptor, proving it is talking to the authentic node
    /// (not an impersonator) before it delegates any recovery. Requires `init` (no key, no identity).
    fn hello(
        &self,
        challenge_b64: &str,
        caller_pub_b64: &str,
        now_unix: Option<u64>,
        channel_pub_b64: Option<&str>,
    ) -> Response {
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
        // The caller's ephemeral PUBLIC key. We bind it into the session token; recover then requires
        // a signature under the matching private key, so a captured token is non-replayable by anyone
        // who does not hold that key. We require a non-empty, well-formed verifying key up front.
        let caller_pub = match b64().decode(caller_pub_b64) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => return Response::error("invalid_request", "caller_pub_b64 must be non-empty"),
            Err(_) => return Response::error("invalid_request", "caller_pub_b64 is not valid base64"),
        };
        if ddrm_envelope::MlDsa65Verifier::from_encoded(&caller_pub).is_none() {
            return Response::error("invalid_request", "caller_pub_b64 is not a valid verifying key");
        }
        // KNOWN-CALLER GATE (Day 95–96): when an allow-list is provisioned, the node serves ONLY a
        // caller whose ephemeral identity key it recognizes — an unknown caller is refused at the
        // handshake, BEFORE any session token is minted (the OWNER-BOUND analogue). When no allow-list
        // is configured the node accepts any well-formed key (anonymous enrollment, dev/test only).
        if let Some(allowed) = self.allowed_callers.as_ref() {
            if !allowed.iter().any(|vk| vk.as_slice() == caller_pub.as_slice()) {
                return Response::error(
                    "caller_not_authorized",
                    "caller identity is not on this node's allow-list (provision the caller's verifying key)",
                );
            }
        }
        // ENCRYPTED CHANNEL (Day 105–108): when the caller offers a channel KEM key, validate it
        // up-front (fail-closed: a malformed key never half-establishes a channel) and ATTEST the
        // node's own channel key for THIS handshake — the signature over `(challenge, channel_pub)`
        // under the node's pinned identity is what a MITM terminating the TCP connection cannot
        // forge for its own KEM key.
        let channel = match channel_pub_b64 {
            None => None,
            Some(pub_b64) => {
                let bytes = match b64().decode(pub_b64) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return Response::error(
                            "invalid_request",
                            "channel_pub_b64 is not valid base64",
                        )
                    }
                };
                if ddrm_envelope::session_public_from_bytes(&bytes).is_none() {
                    return Response::error(
                        "invalid_request",
                        "channel_pub_b64 is not a valid channel KEM key",
                    );
                }
                let sig = ddrm_envelope::attest_channel_key(
                    &authority.signer,
                    &challenge,
                    &authority.channel_public,
                );
                Some(json!({
                    "node_channel_pub_b64": b64().encode(&authority.channel_public),
                    "channel_sig_b64": b64().encode(&sig),
                }))
            }
        };
        let attestation = ddrm_envelope::attest_challenge(&authority.signer, &challenge);
        // Mint a node-signed SESSION TOKEN binding this challenge + the caller's pubkey to a bounded
        // expiry. The node will REQUIRE (and re-verify) it + a matching possession proof on every
        // recover, so this handshake gates a whole session of recovers for THIS caller only.
        let expires_at = effective_now(now_unix) + SESSION_TTL_SECONDS;
        let token_sig =
            ddrm_envelope::sign_session_token(&authority.signer, &challenge, &caller_pub, expires_at);
        let mut data = json!({
            "verifying_key_b64": b64().encode(&authority.verifying_key),
            "attestation_b64": b64().encode(&attestation),
            "session_token": {
                "challenge_b64": challenge_b64,
                "caller_pub_b64": caller_pub_b64,
                "expires_at": expires_at,
                "sig_b64": b64().encode(&token_sig),
            },
        });
        if let Some(channel) = channel {
            data["channel"] = channel;
        }
        Response::ok(data)
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
        let authority = Box::new(NodeAuthority::from_master(&master));
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

    /// DELEGATED recovery with the per-recover FRESHNESS gate (Day 95–96). A `recover` is accepted
    /// only when its `recover_seq` strictly advances this session's counter — so a captured recover
    /// frame replayed verbatim (same seq) is refused even by the legitimate caller (anti-replay). The
    /// counter is committed ONLY on a successful recover, so a transient failure does not burn a seq.
    /// The freshness gate runs FIRST (cheap, before any key work); the possession proof binds the seq
    /// (so a MITM cannot bump a stale frame's counter without invalidating the proof).
    fn recover(&mut self, args: RecoverArgs) -> Response {
        let recover_seq = args.recover_seq;
        if recover_seq <= self.last_recover_seq {
            return Response::error(
                "session_invalid",
                "stale or replayed recover_seq — the freshness counter must strictly advance (anti-replay)",
            );
        }
        let resp = self.recover_inner(&args);
        if matches!(resp, Response::Ok { .. }) {
            self.last_recover_seq = recover_seq;
        }
        resp
    }

    /// The recovery body: recover the escrowed CEK in this boundary, re-seal it to the decrypt
    /// session, and return ONLY the sealed material. The raw CEK is held in `Zeroizing` and never
    /// echoed back; the master never leaves this process. Borrows `&self` only (the freshness counter
    /// is committed by the `recover` wrapper after this returns Ok).
    fn recover_inner(&self, args: &RecoverArgs) -> Response {
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
        if let Err(err) = verify_session(authority, args) {
            return Response::error("session_invalid", err);
        }
        // RE-AUTHORIZE in this boundary — refuse to recover for an unauthorized caller before
        // touching any key material (the node never trusts the client's claim).
        if let Err(err) = reauthorize(args) {
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
        if let Some(init) = args.init_segment_b64.as_ref() {
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
    caller_sig_b64: String,
    recover_seq: u64,
    now_unix: Option<u64>,
}

/// VERIFY the caller's session token + POSSESSION PROOF in the node's OWN boundary. First, the token
/// must be one THIS node signed — a valid signature over `(challenge, caller_pub, expires_at)` under
/// the node's verifying key — and still unexpired. Second, the caller must PROVE possession of the
/// token-bound ephemeral private key by signing the session challenge + this recover's content
/// binding, which the node verifies against the token-bound pubkey.
/// A forged/tampered/expired token, OR a missing/wrong-key possession proof, is refused — so a
/// long-lived node recovers only for a caller with a live handshake session who STILL holds the key
/// the token was bound to. A captured bearer token replayed by a different caller (no private key)
/// fails the possession check. The runtime-core analogue of PC2's session being OWNER-BOUND,
/// re-checked in the TEE via `ecrecover(delegationSig)` (`secureViewSession.ts:87`–`:100`).
fn verify_session(authority: &NodeAuthority, args: &RecoverArgs) -> Result<(), String> {
    let token = &args.session_token;
    let challenge = b64()
        .decode(&token.challenge_b64)
        .map_err(|_| "session token challenge is not valid base64".to_string())?;
    let caller_pub = b64()
        .decode(&token.caller_pub_b64)
        .map_err(|_| "session token caller pubkey is not valid base64".to_string())?;
    let sig = b64()
        .decode(&token.sig_b64)
        .map_err(|_| "session token signature is not valid base64".to_string())?;
    let node_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&authority.verifying_key)
        .ok_or("node verifying key is malformed")?;
    if !ddrm_envelope::verify_session_token(&node_verifier, &challenge, &caller_pub, token.expires_at, &sig)
    {
        return Err("session token is forged or tampered (signature does not verify)".to_string());
    }
    if effective_now(args.now_unix) > token.expires_at {
        return Err("session token has expired — re-establish the handshake session".to_string());
    }

    // POSSESSION PROOF: the caller must sign the session challenge + this recover's content binding
    // under the private key whose public half the token committed to. This is what makes the bearer
    // token non-replayable: a third party who captured the token but lacks the key cannot forge it.
    let caller_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&caller_pub)
        .ok_or("session token caller pubkey is malformed")?;
    let caller_sig = b64()
        .decode(&args.caller_sig_b64)
        .map_err(|_| "caller possession proof is not valid base64".to_string())?;
    // Bind the proof to the SAME session-pub bytes the recover re-seals to (b64-decoded).
    let session_pub = b64()
        .decode(&args.decrypt_session_pub_b64)
        .map_err(|_| "decrypt_session_pub_b64 is not valid base64".to_string())?;
    if !ddrm_envelope::verify_recover_proof(
        &caller_verifier,
        &challenge,
        args.content_id.as_bytes(),
        args.kid_hex.as_bytes(),
        &session_pub,
        args.recover_seq,
        &caller_sig,
    ) {
        return Err(
            "caller possession proof is missing, forged, signed by the wrong key, or carries a swapped freshness counter (captured token replay refused)"
                .to_string(),
        );
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
    // A real remote authority is reached over a transport the runtime does NOT own the process of:
    // when a listen path is set, BIND + LISTEN on a Unix-domain socket and serve framed connections.
    // Otherwise keep the one-shot stdin/stdout loop (provisioning + tests). The socket transport is
    // `unix`-only (the wasm32-wasip1 ladder build has no Unix sockets), so it is conditionally compiled.
    #[cfg(unix)]
    match std::env::var(LISTEN_ENV) {
        // `tcp:HOST:PORT` → a REAL network listener (Day 105–108); anything else is a Unix path.
        Ok(ep) if ep.trim().starts_with("tcp:") => serve_tcp(ep.trim().trim_start_matches("tcp:")),
        Ok(path) if !path.trim().is_empty() => serve_socket(&path),
        _ => serve_stdio(),
    }
    #[cfg(not(unix))]
    serve_stdio();
    eprintln!("dkms-authority exiting");
}

/// One-shot, newline-delimited JSON over stdin/stdout (provisioning identity reads + the tests).
fn serve_stdio() {
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
}

/// SOCKET serve mode: bind a Unix-domain socket and serve framed connections sequentially. The node
/// is a long-lived daemon; each accepted connection gets its OWN fresh node state + handshake session
/// (one session per connection). A connection that sends a torn/oversized/half-closed frame is
/// dropped fail-closed — the listener keeps accepting, so a hostile/buggy client cannot wedge the
/// daemon for the next legitimate one.
#[cfg(unix)]
fn serve_socket(path: &str) {
    use std::os::unix::net::UnixListener;
    // Clear any stale socket from a prior run, then bind. A bind failure is fatal (nothing to serve).
    let _ = std::fs::remove_file(path);
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("dkms-authority: failed to bind {path}: {err}");
            std::process::exit(1);
        }
    };
    // The OPERATOR's KNOWN-caller allow-list (Day 95–96), resolved ONCE at daemon startup from the
    // provisioner-set env. The connecting client cannot influence it. `None`/empty = anonymous.
    let allowed_callers = allowed_callers_from_env();
    if let Some(list) = allowed_callers.as_ref() {
        eprintln!("dkms-authority: enforcing a {}-entry caller allow-list", list.len());
    }
    eprintln!("dkms-authority: listening on {path}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let reader = match stream.try_clone() {
                    Ok(s) => io::BufReader::new(s),
                    Err(err) => {
                        eprintln!("dkms-authority: connection clone failed: {err}");
                        continue;
                    }
                };
                // The Unix transport is host-local (filesystem-permissioned), so the encrypted
                // channel is OPTIONAL here — a client that offers a channel key still gets one.
                serve_connection_io(reader, stream, &allowed_callers, false);
            }
            Err(err) => {
                eprintln!("dkms-authority: accept error: {err}");
                continue;
            }
        }
    }
}

/// TCP serve mode (Day 105–108): the node taken OFF localhost — a REAL network listener with the
/// same framed protocol. Because the network is HOSTILE (no filesystem permission boundary), every
/// `recover` on this transport REQUIRES the app-layer encrypted, mutually-authenticated channel:
/// a plaintext recover is refused (`channel_required`), and once a channel is established every
/// frame in BOTH directions is a sealed envelope (a plaintext/tampered/replayed frame drops the
/// connection fail-closed). Contrast PC2's dDRM network boundary: HTTPS with
/// `rejectUnauthorized: false` (`chipotle-client.ts:840`) — its channel authenticates nothing.
#[cfg(unix)]
fn serve_tcp(addr: &str) {
    use std::net::TcpListener;
    let listener = match TcpListener::bind(addr) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("dkms-authority: failed to bind tcp:{addr}: {err}");
            std::process::exit(1);
        }
    };
    let allowed_callers = allowed_callers_from_env();
    if let Some(list) = allowed_callers.as_ref() {
        eprintln!("dkms-authority: enforcing a {}-entry caller allow-list", list.len());
    }
    eprintln!("dkms-authority: listening on tcp:{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // NETWORK-FAULT SEMANTICS: a stalled remote peer must not wedge the sequential
                // daemon — bound every read so the listener always gets back to `accept`.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
                let reader = match stream.try_clone() {
                    Ok(s) => io::BufReader::new(s),
                    Err(err) => {
                        eprintln!("dkms-authority: connection clone failed: {err}");
                        continue;
                    }
                };
                serve_connection_io(reader, stream, &allowed_callers, true);
            }
            Err(err) => {
                eprintln!("dkms-authority: accept error: {err}");
                continue;
            }
        }
    }
}

/// Parse the OPERATOR's KNOWN-caller allow-list from `ALLOWED_CALLERS_ENV`: a comma-separated list
/// of base64 ML-DSA verifying keys. `None` when unset/empty (anonymous enrollment); a malformed or
/// non-key entry is dropped (fail-closed: it simply will not match any caller). Provisioner-only.
#[cfg(unix)]
fn allowed_callers_from_env() -> Option<Vec<Vec<u8>>> {
    let raw = std::env::var(ALLOWED_CALLERS_ENV).ok()?;
    let list: Vec<Vec<u8>> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| b64().decode(s).ok())
        .filter(|vk| ddrm_envelope::MlDsa65Verifier::from_encoded(vk).is_some())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

/// The per-connection state of an ESTABLISHED encrypted channel (Day 105–108): the handshake
/// challenge scopes it (`channel_id`), the client's ephemeral KEM key receives the node's sealed
/// responses, the token-bound caller identity verifies the client's sealed frames, and the two
/// strictly-advancing counters make every frame non-replayable + non-reflectable (they are bound
/// into each frame's AAD with a direction tag).
#[cfg(unix)]
struct ServerChannel {
    channel_id: Vec<u8>,
    client_pub: ddrm_envelope::SessionKemPublic,
    caller_verifier: ddrm_envelope::MlDsa65Verifier,
    send_seq: u64,
    recv_seq: u64,
}

/// Serve ONE connection: a fresh node + session, framed request/response until the client hangs up,
/// sends `shutdown`, or trips a transport error. Any framing/parse error ends THIS connection only
/// (fail-closed) — it never propagates to the listener.
///
/// ENCRYPTED CHANNEL (Day 105–108): a `hello` carrying a client channel KEM key ESTABLISHES the
/// channel — after the (plaintext, but signed) hello response, every frame in BOTH directions is a
/// sealed envelope bound to `(channel_id, direction, seq)`. A plaintext frame after establishment
/// (downgrade), a tampered envelope (MITM), or a replayed envelope (stale seq) all DROP the
/// connection without serving anything. When `require_channel` (the TCP transport), a plaintext
/// `recover` is refused outright — the hostile network never carries an unencrypted recover.
#[cfg(unix)]
fn serve_connection_io<R: io::Read, W: io::Write>(
    mut reader: R,
    mut writer: W,
    allowed_callers: &Option<Vec<Vec<u8>>>,
    require_channel: bool,
) {
    use ddrm_envelope::frame::{read_frame, write_frame};
    let mut node = DkmsAuthorityNode {
        allowed_callers: allowed_callers.clone(),
        ..DkmsAuthorityNode::default()
    };
    let mut channel: Option<ServerChannel> = None;
    loop {
        let raw = match read_frame(&mut reader) {
            Ok(Some(payload)) => payload,
            Ok(None) => break, // clean half-close at a frame boundary
            Err(err) => {
                // Torn/oversized/hostile frame: refuse + drop the connection (the listener serves on).
                eprintln!("dkms-authority: framing error, dropping connection: {err}");
                if channel.is_none() {
                    let _ = write_frame(
                        &mut writer,
                        &serde_json::to_vec(&Response::error("invalid_frame", err.to_string()))
                            .unwrap_or_default(),
                    );
                }
                break;
            }
        };
        // CHANNEL-ESTABLISHED PATH: every incoming frame MUST be a sealed envelope from the
        // token-bound caller — open it (signature + AEAD, bound to this channel/direction/seq) or
        // drop the connection. A plaintext frame here is a DOWNGRADE attempt; a flipped byte is a
        // MITM; a replayed frame carries a stale seq. All fail closed with NO response (a sealed
        // error to an unauthenticated peer would itself be an oracle).
        let payload: Vec<u8> = match channel.as_mut() {
            None => raw,
            Some(ch) => {
                let env = match ddrm_envelope::PqSealedEnvelope::from_bytes(&raw) {
                    Ok(env) => env,
                    Err(_) => {
                        eprintln!("dkms-authority: PLAINTEXT/garbled frame on an established channel — dropping connection (no downgrade)");
                        break;
                    }
                };
                ch.recv_seq += 1;
                let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 0, ch.recv_seq);
                let channel_secret = match node.authority.as_ref() {
                    Some(authority) => &authority.channel_secret,
                    None => break, // unreachable: a channel implies a completed hello (post-init)
                };
                match ddrm_envelope::hybrid_unwrap_bound(channel_secret, &env, &aad, &ch.caller_verifier) {
                    Ok(opened) => opened.to_vec(),
                    Err(_) => {
                        eprintln!("dkms-authority: sealed frame failed to open (tampered/replayed/wrong key) — dropping connection");
                        break;
                    }
                }
            }
        };
        let request = match serde_json::from_slice::<Request>(&payload) {
            Ok(request) => request,
            Err(err) => {
                let resp = Response::error("invalid_request", err.to_string());
                if respond(&mut writer, &mut channel, &node, &resp).is_err() {
                    break;
                }
                continue;
            }
        };
        // FAIL-CLOSED TRANSPORT GATE: on a network transport, a recover NEVER travels in plaintext —
        // no channel, no recover. (init/hello/status are public-protocol messages; the secrets a
        // recover moves — the sealed result, the rights context — get channel confidentiality.)
        if require_channel && channel.is_none() && matches!(request, Request::Recover { .. }) {
            let resp = Response::error(
                "channel_required",
                "this transport requires the encrypted channel: re-run `hello` with a channel_pub_b64 before `recover`",
            );
            if respond(&mut writer, &mut channel, &node, &resp).is_err() {
                break;
            }
            continue;
        }
        // A hello OFFERING a channel key arms channel establishment — committed only if the node
        // accepts the hello (allow-list + validation), i.e. the response is `ok`.
        let pending_channel = match &request {
            Request::Hello { challenge_b64, caller_pub_b64, channel_pub_b64: Some(client_pub), .. } => {
                Some((challenge_b64.clone(), caller_pub_b64.clone(), client_pub.clone()))
            }
            _ => None,
        };
        let is_shutdown = matches!(request, Request::Shutdown);
        let response = node.handle(request);
        let accepted = matches!(response, Response::Ok { .. });
        if respond(&mut writer, &mut channel, &node, &response).is_err() {
            break;
        }
        if let (Some((chal_b64, caller_b64, client_pub_b64)), true) = (pending_channel, accepted) {
            // The hello validated all three fields (it returned ok); re-derive the channel state
            // from them. Any inconsistency here is a protocol bug — drop the connection fail-closed.
            let establish = || -> Option<ServerChannel> {
                let channel_id = b64().decode(&chal_b64).ok()?;
                let caller_vk = b64().decode(&caller_b64).ok()?;
                let caller_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&caller_vk)?;
                let client_pub_bytes = b64().decode(&client_pub_b64).ok()?;
                let client_pub = ddrm_envelope::session_public_from_bytes(&client_pub_bytes)?;
                Some(ServerChannel { channel_id, client_pub, caller_verifier, send_seq: 0, recv_seq: 0 })
            };
            match establish() {
                Some(state) => channel = Some(state),
                None => {
                    eprintln!("dkms-authority: channel establishment failed after an accepted hello — dropping connection");
                    break;
                }
            }
        }
        if is_shutdown {
            break;
        }
    }
}

/// Write one response frame: SEALED to the client's channel key (+ signed by the node, AAD-bound to
/// this channel/direction/seq) when a channel is established, plaintext otherwise. The hello
/// response that ESTABLISHES the channel is the last plaintext frame of a connection — it is
/// integrity-protected by the node's signatures (attestation + channel-key attestation + token
/// signature), and nothing secret has flowed yet.
#[cfg(unix)]
fn respond<W: io::Write>(
    writer: &mut W,
    channel: &mut Option<ServerChannel>,
    node: &DkmsAuthorityNode,
    response: &Response,
) -> io::Result<()> {
    use ddrm_envelope::frame::write_frame;
    let bytes = serde_json::to_vec(response).unwrap_or_default();
    match channel.as_mut() {
        None => write_frame(writer, &bytes),
        Some(ch) => {
            let authority = match node.authority.as_ref() {
                Some(authority) => authority,
                None => return Err(io::Error::new(io::ErrorKind::InvalidData, "channel without authority")),
            };
            ch.send_seq += 1;
            let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 1, ch.send_seq);
            let env = ddrm_envelope::seal::seal_bound(&ch.client_pub, &bytes, &aad, &authority.signer);
            write_frame(writer, &env.to_bytes())
        }
    }
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

    /// The caller's EPHEMERAL keypair: the client mints this per connection, sends the public half at
    /// `hello`, and signs every recover with the private half (the possession proof the node checks).
    fn caller_keypair() -> (ddrm_envelope::seal::MlDsaSealSigner, Vec<u8>) {
        ddrm_envelope::seal::mldsa_seal_keypair([0x33u8; 32])
    }

    /// Drive the node's real `hello` (bound to `caller_pub_b64`) to mint a live session token at `NOW`.
    fn live_token(node: &DkmsAuthorityNode, caller_pub_b64: &str) -> SessionToken {
        let challenge = b64().encode([0xA1u8; 32]);
        let hello = ok_data(node.hello(&challenge, caller_pub_b64, Some(NOW), None));
        let t = &hello["session_token"];
        SessionToken {
            challenge_b64: t["challenge_b64"].as_str().unwrap().to_string(),
            caller_pub_b64: t["caller_pub_b64"].as_str().unwrap().to_string(),
            expires_at: t["expires_at"].as_u64().unwrap(),
            sig_b64: t["sig_b64"].as_str().unwrap().to_string(),
        }
    }

    /// The caller's possession proof over a token's challenge + a recover's content binding + the
    /// per-recover freshness counter (Day 95–96).
    fn proof_for(
        signer: &ddrm_envelope::seal::MlDsaSealSigner,
        token: &SessionToken,
        content_id: &str,
        kid_hex: &str,
        session_pub_b64: &str,
        recover_seq: u64,
    ) -> String {
        let challenge = b64().decode(&token.challenge_b64).unwrap();
        let session_pub = b64().decode(session_pub_b64).unwrap();
        b64().encode(ddrm_envelope::sign_recover_proof(
            signer,
            &challenge,
            content_id.as_bytes(),
            kid_hex.as_bytes(),
            &session_pub,
            recover_seq,
        ))
    }

    /// A structurally-valid but never-verified token (for cases that fail BEFORE the session gate).
    fn dummy_token() -> SessionToken {
        SessionToken {
            challenge_b64: b64().encode([0u8; 32]),
            caller_pub_b64: b64().encode(caller_keypair().1),
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

        let (caller, caller_vk) = caller_keypair();
        let token = live_token(&node, &b64().encode(&caller_vk));
        let caller_sig_b64 = proof_for(&caller, &token, CONTENT, &kid_hex, &session_pub_b64, 1);
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
            caller_sig_b64,
            recover_seq: 1,
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

        // Forged producer vk → recover fails closed (a live session + valid possession proof pass the
        // gate, so the producer check is what we're exercising).
        let (caller, caller_vk) = caller_keypair();
        let token = live_token(&node, &b64().encode(&caller_vk));
        let caller_sig_b64 = proof_for(&caller, &token, CONTENT, &kid_hex, &session_pub_b64, 1);
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
            caller_sig_b64,
            recover_seq: 1,
            now_unix: Some(NOW),
        });
        assert_eq!(error_code(&forged), "invalid_request");

        // Before init → not_configured (the authority check precedes the session gate, so a dummy
        // token is never reached).
        let mut fresh = DkmsAuthorityNode::default();
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
            caller_sig_b64: b64().encode([0u8; 8]),
            recover_seq: 1,
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
    fn setup_recover(store: &str) -> (DkmsAuthorityNode, RecoverArgs, ddrm_envelope::seal::MlDsaSealSigner) {
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
        let (caller, caller_vk) = caller_keypair();
        let token = live_token(&node, &b64().encode(&caller_vk));
        // Base case uses freshness seq 1; tests that drive multiple recovers re-sign with the
        // returned caller signer at a higher seq.
        let caller_sig_b64 = proof_for(&caller, &token, CONTENT, &kid_hex, &session_pub_b64, 1);
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
            caller_sig_b64,
            recover_seq: 1,
            now_unix: Some(NOW),
        };
        (node, args, caller)
    }

    /// The node's IDENTITY handshake: a `hello` returns the published vk + an attestation that
    /// verifies under that pinned vk for the supplied challenge — and refuses before `init`.
    #[test]
    fn hello_attests_node_identity_and_requires_init() {
        let store = unique_store("hello");
        let mut node = DkmsAuthorityNode::default();
        let (_caller, caller_vk) = caller_keypair();
        let caller_pub_b64 = b64().encode(&caller_vk);

        // Before init there is no key material → fail closed.
        assert_eq!(
            error_code(&node.hello(&b64().encode([1u8; 32]), &caller_pub_b64, Some(NOW), None)),
            "not_configured"
        );

        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let pinned_vk_b64 = init["seal_verifying_key_b64"].as_str().unwrap().to_string();

        // A malformed caller pubkey is refused up front (the token must bind a real verifying key).
        assert_eq!(
            error_code(&node.hello(&b64().encode([1u8; 32]), "not-a-key", Some(NOW), None)),
            "invalid_request"
        );

        let challenge = ddrm_envelope::random_seed();
        let resp = ok_data(node.hello(&b64().encode(challenge), &caller_pub_b64, Some(NOW), None));
        // The node advertises the SAME vk it published at init (the pin).
        assert_eq!(resp["verifying_key_b64"].as_str().unwrap(), pinned_vk_b64);

        // The attestation verifies under the PINNED vk for THIS challenge.
        let pinned = b64().decode(&pinned_vk_b64).unwrap();
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned).unwrap();
        let attestation = b64().decode(resp["attestation_b64"].as_str().unwrap()).unwrap();
        assert!(ddrm_envelope::verify_attestation(&verifier, &challenge, &attestation));

        // hello ALSO mints a node-signed SESSION TOKEN bound to this challenge + the CALLER's pubkey
        // + a bounded expiry, and the token verifies under the node's own vk (recover will require it).
        let token = &resp["session_token"];
        assert_eq!(token["expires_at"].as_u64().unwrap(), NOW + SESSION_TTL_SECONDS);
        assert_eq!(token["caller_pub_b64"].as_str().unwrap(), caller_pub_b64);
        let token_sig = b64().decode(token["sig_b64"].as_str().unwrap()).unwrap();
        assert!(ddrm_envelope::verify_session_token(
            &verifier,
            &challenge,
            &caller_vk,
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
        let (mut node, base, _caller) = setup_recover(&store);

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

    /// A `recover` with NO session token (or NO possession proof) is a hard parse error — the node
    /// never recovers without a handshake session AND a caller signature (both fields are required).
    #[test]
    fn recover_without_a_session_token_is_rejected_at_the_protocol() {
        // A recover line that omits session_token must fail to deserialize into a Request.
        let no_token = json!({
            "op": "recover",
            "wrapped_cek_b64": "AA==", "scheme": "s", "kid_hex": "00",
            "producer_vk_b64": "AA==", "decrypt_session_pub_b64": "AA==",
            "ciphertext_b64": "AA==", "content_hash_b64": "AA==", "nonce_b64": "AA==",
            "rights_receipt": good_receipt(),
            "content_id": CONTENT, "principal_id": PRINCIPAL, "session_id": SESSION, "right": RIGHT,
            "caller_sig_b64": "AA==",
        });
        assert!(
            serde_json::from_value::<Request>(no_token).is_err(),
            "recover without session_token must not deserialize"
        );

        // A recover line that omits the possession proof (caller_sig_b64) must also fail to parse.
        let no_proof = json!({
            "op": "recover",
            "wrapped_cek_b64": "AA==", "scheme": "s", "kid_hex": "00",
            "producer_vk_b64": "AA==", "decrypt_session_pub_b64": "AA==",
            "ciphertext_b64": "AA==", "content_hash_b64": "AA==", "nonce_b64": "AA==",
            "rights_receipt": good_receipt(),
            "content_id": CONTENT, "principal_id": PRINCIPAL, "session_id": SESSION, "right": RIGHT,
            "session_token": { "challenge_b64": "AA==", "caller_pub_b64": "AA==", "expires_at": 1, "sig_b64": "AA==" },
        });
        assert!(
            serde_json::from_value::<Request>(no_proof).is_err(),
            "recover without caller_sig_b64 must not deserialize"
        );
    }

    /// The POSSESSION GATE (Day 93–94): even with a valid, live, node-signed token + escrow + receipt,
    /// a recover whose possession proof is MISSING/garbage, signed by the WRONG key, or over a
    /// TAMPERED binding is refused — so a captured BEARER token replayed by a caller who does not hold
    /// the token-bound private key cannot drive recovery (the OWNER-BOUND analogue,
    /// `secureViewSession.ts:87`–`:100`).
    #[test]
    fn recover_fails_closed_without_or_with_wrong_possession_proof() {
        let store = unique_store("possession");
        let (mut node, base, _caller) = setup_recover(&store);

        // Garbage proof (a token-replayer who cannot sign) → session_invalid.
        let mut garbage = base.clone();
        garbage.caller_sig_b64 = b64().encode([0u8; 8]);
        assert_eq!(error_code(&node.recover(garbage)), "session_invalid");

        // Proof from a DIFFERENT key (right binding, wrong signer) → session_invalid.
        let (other, _ovk) = ddrm_envelope::seal::mldsa_seal_keypair([0x99u8; 32]);
        let mut wrong_key = base.clone();
        wrong_key.caller_sig_b64 = proof_for(
            &other,
            &base.session_token,
            CONTENT,
            &base.kid_hex,
            &base.decrypt_session_pub_b64,
            base.recover_seq,
        );
        assert_eq!(error_code(&node.recover(wrong_key)), "session_invalid");

        // Sanity: the unmodified base (correct possession proof) recovers.
        assert!(matches!(node.recover(base), Response::Ok { .. }));
        let _ = std::fs::remove_file(&store);
    }

    /// The FRAMED Unix-socket transport (Day 93–94): a full session — init → hello → recover →
    /// shutdown — round-trips over length-prefixed frames on one connection, and a torn frame on a
    /// FRESH connection is refused fail-closed (the handler returns an `invalid_frame` error + drops
    /// the connection) without the served node panicking.
    #[test]
    fn framed_connection_serves_a_full_session_and_drops_a_torn_frame() {
        use ddrm_envelope::frame::{read_frame, write_frame};
        use std::os::unix::net::UnixStream;

        let store = unique_store("framed");

        // ---- Happy path: a full framed session over one connection. ----
        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let reader = io::BufReader::new(server.try_clone().unwrap());
            serve_connection_io(reader, server, &None, false)
        });

        let call = |client: &mut UnixStream, req: Value| -> Value {
            write_frame(client, &serde_json::to_vec(&req).unwrap()).unwrap();
            let payload = read_frame(client).unwrap().expect("a framed response");
            serde_json::from_slice(&payload).unwrap()
        };

        // Pass the store in config (no process-wide env — keeps parallel tests independent).
        let init = call(&mut client, json!({ "op": "init", "config": { "authority_key_store": store } }));
        assert_eq!(init["status"].as_str().unwrap(), "ok");

        let (caller, caller_vk) = caller_keypair();
        let challenge = b64().encode([0xB2u8; 32]);
        let hello = call(
            &mut client,
            json!({ "op": "hello", "challenge_b64": challenge, "caller_pub_b64": b64().encode(&caller_vk), "now_unix": NOW }),
        );
        let token = &hello["data"]["session_token"];
        let session_pub_b64 = init["data"]["seal_recipient_pub_b64"].as_str().unwrap();
        // Escrow a CEK to the node's recipient so the recover has something real to recover.
        let recipient_pub = b64().decode(session_pub_b64).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let cek: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &escrow_aad, &producer_signer);
        let (_ds, dsp) = ddrm_envelope::mint_session();
        let decrypt_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&dsp));
        let proof = {
            let chal = b64().decode(challenge_str(token)).unwrap();
            let dp = b64().decode(&decrypt_pub_b64).unwrap();
            b64().encode(ddrm_envelope::sign_recover_proof(&caller, &chal, CONTENT.as_bytes(), kid_hex.as_bytes(), &dp, 1))
        };
        let recover = call(
            &mut client,
            json!({
                "op": "recover",
                "wrapped_cek_b64": b64().encode(wrapped.to_bytes()),
                "scheme": scheme, "kid_hex": kid_hex,
                "producer_vk_b64": b64().encode(&producer_vk),
                "decrypt_session_pub_b64": decrypt_pub_b64,
                "aad_b64": b64().encode(b"transcript"),
                "ciphertext_b64": b64().encode(b"ct"),
                "content_hash_b64": b64().encode(b"hash"),
                "nonce_b64": b64().encode(b"nonce"),
                "rights_receipt": good_receipt(),
                "content_id": CONTENT, "principal_id": PRINCIPAL, "session_id": SESSION, "right": RIGHT,
                "session_token": token,
                "caller_sig_b64": proof,
                "recover_seq": 1,
                "now_unix": NOW,
            }),
        );
        assert_eq!(recover["status"].as_str().unwrap(), "ok", "framed recover over the socket succeeds");
        assert!(recover["data"]["material"].get("sealed_cek_b64").is_some());
        // Clean shutdown ends the connection; the served thread returns without panicking.
        write_frame(&mut client, &serde_json::to_vec(&json!({ "op": "shutdown" })).unwrap()).unwrap();
        let _ = read_frame(&mut client);
        handle.join().unwrap();

        // ---- Torn frame on a fresh connection: refused fail-closed, no panic. ----
        let (mut bad, server2) = UnixStream::pair().unwrap();
        let handle2 = std::thread::spawn(move || {
            let reader = io::BufReader::new(server2.try_clone().unwrap());
            serve_connection_io(reader, server2, &None, false)
        });
        // A header promising 64 bytes followed by only 3, then half-close.
        use std::io::Write as _;
        bad.write_all(&64u32.to_be_bytes()).unwrap();
        bad.write_all(b"abc").unwrap();
        bad.shutdown(std::net::Shutdown::Write).unwrap();
        let resp = read_frame(&mut bad).unwrap().expect("an error frame");
        let resp: Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(resp["code"].as_str().unwrap(), "invalid_frame");
        handle2.join().unwrap();

        let _ = std::fs::remove_file(&store);
    }

    /// ENCRYPTED CHANNEL (Day 105–108), node side of the handshake: a `hello` offering a client
    /// channel KEM key gets back the node's master-derived channel key ATTESTED under the node's
    /// identity — and that attestation pins BOTH the challenge and the key, so a MITM terminating
    /// the connection cannot substitute its own KEM key. A malformed offered key is refused.
    #[test]
    fn hello_with_a_channel_key_returns_an_attested_node_channel_key() {
        let store = unique_store("channel-hello");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.handle(Request::Init {
            config: json!({ "authority_key_store": store }),
        }));
        let node_vk = b64().decode(init["seal_verifying_key_b64"].as_str().unwrap()).unwrap();
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&node_vk).unwrap();

        let (_caller, caller_vk) = caller_keypair();
        let (_client_secret, client_pub) = ddrm_envelope::mint_session();
        let client_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&client_pub));
        let challenge = [0xC7u8; 32];

        let hello = ok_data(node.hello(
            &b64().encode(challenge),
            &b64().encode(&caller_vk),
            Some(NOW),
            Some(&client_pub_b64),
        ));
        let channel = &hello["channel"];
        let node_channel_pub =
            b64().decode(channel["node_channel_pub_b64"].as_str().unwrap()).unwrap();
        let sig = b64().decode(channel["channel_sig_b64"].as_str().unwrap()).unwrap();
        // The node's channel key is a REAL KEM key, distinct from the escrow recipient (domain-
        // separated derivations from the same master).
        assert!(ddrm_envelope::session_public_from_bytes(&node_channel_pub).is_some());
        assert_ne!(
            channel["node_channel_pub_b64"].as_str().unwrap(),
            init["seal_recipient_pub_b64"].as_str().unwrap(),
            "the channel key must be domain-separated from the escrow recipient"
        );
        // The attestation verifies for the GENUINE (challenge, key) pair under the pinned identity…
        assert!(ddrm_envelope::verify_channel_key(&verifier, &challenge, &node_channel_pub, &sig));
        // …and fails for a SUBSTITUTED key (the MITM's own KEM key under a relayed hello).
        let (_mitm_secret, mitm_pub) = ddrm_envelope::mint_session();
        let mitm_pub = ddrm_envelope::session_public_bytes(&mitm_pub);
        assert!(!ddrm_envelope::verify_channel_key(&verifier, &challenge, &mitm_pub, &sig));
        // A hello WITHOUT a channel offer carries no channel block (single-frame back-compat)…
        let plain = ok_data(node.hello(&b64().encode(challenge), &b64().encode(&caller_vk), Some(NOW), None));
        assert!(plain.get("channel").is_none());
        // …and a MALFORMED offered key is refused outright (fail-closed, no half-channel).
        let bad = node.hello(
            &b64().encode(challenge),
            &b64().encode(&caller_vk),
            Some(NOW),
            Some("AAAA"),
        );
        assert_eq!(error_code(&bad), "invalid_request");
        let _ = std::fs::remove_file(&store);
    }

    /// ENCRYPTED CHANNEL (Day 105–108), transport enforcement on a NETWORK-shaped connection
    /// (`require_channel = true`, the TCP serve mode): a PLAINTEXT recover is refused
    /// (`channel_required`) before any key material is touched; after the channel is established the
    /// request/response round-trip is SEALED in both directions; and a PLAINTEXT frame after
    /// establishment (a downgrade) drops the connection without an answer.
    #[test]
    fn network_connection_requires_the_channel_and_refuses_a_downgrade() {
        use ddrm_envelope::frame::{read_frame, write_frame};
        use std::os::unix::net::UnixStream;

        let store = unique_store("channel-transport");
        // A socketpair stands in for the TCP stream — `serve_connection_io` is transport-generic;
        // `require_channel = true` is exactly what the TCP accept loop passes.
        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let reader = io::BufReader::new(server.try_clone().unwrap());
            serve_connection_io(reader, server, &None, true)
        });
        let call_plain = |client: &mut UnixStream, req: Value| -> Value {
            write_frame(client, &serde_json::to_vec(&req).unwrap()).unwrap();
            let payload = read_frame(client).unwrap().expect("a framed response");
            serde_json::from_slice(&payload).unwrap()
        };

        let init = call_plain(&mut client, json!({ "op": "init", "config": { "authority_key_store": store } }));
        assert_eq!(init["status"].as_str().unwrap(), "ok");

        // A plaintext RECOVER on the network transport is refused before anything else runs (the
        // recover here is shaped well enough to parse — the refusal is the transport gate).
        let (caller, caller_vk) = caller_keypair();
        let refused = call_plain(
            &mut client,
            json!({
                "op": "recover",
                "wrapped_cek_b64": "AAAA", "scheme": ddrm_envelope::SUITE_PQ_HYBRID,
                "kid_hex": "00", "producer_vk_b64": "AAAA", "decrypt_session_pub_b64": "AAAA",
                "ciphertext_b64": "AAAA", "content_hash_b64": "AAAA", "nonce_b64": "AAAA",
                "rights_receipt": good_receipt(),
                "content_id": CONTENT, "principal_id": PRINCIPAL, "session_id": SESSION, "right": RIGHT,
                "session_token": { "challenge_b64": "AAAA", "caller_pub_b64": "AAAA", "expires_at": 1, "sig_b64": "AAAA" },
                "caller_sig_b64": "AAAA", "recover_seq": 1, "now_unix": NOW,
            }),
        );
        assert_eq!(refused["code"].as_str().unwrap(), "channel_required");

        // ESTABLISH the channel: hello with a client channel key; verify the node's attested key.
        let node_vk = b64().decode(init["data"]["seal_verifying_key_b64"].as_str().unwrap()).unwrap();
        let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&node_vk).unwrap();
        let (client_secret, client_pub) = ddrm_envelope::mint_session();
        let client_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&client_pub));
        let challenge = [0xD4u8; 32];
        let hello = call_plain(
            &mut client,
            json!({
                "op": "hello",
                "challenge_b64": b64().encode(challenge),
                "caller_pub_b64": b64().encode(&caller_vk),
                "now_unix": NOW,
                "channel_pub_b64": client_pub_b64,
            }),
        );
        assert_eq!(hello["status"].as_str().unwrap(), "ok");
        let node_channel_pub =
            b64().decode(hello["data"]["channel"]["node_channel_pub_b64"].as_str().unwrap()).unwrap();
        let sig = b64().decode(hello["data"]["channel"]["channel_sig_b64"].as_str().unwrap()).unwrap();
        assert!(ddrm_envelope::verify_channel_key(&verifier, &challenge, &node_channel_pub, &sig));
        let node_channel = ddrm_envelope::session_public_from_bytes(&node_channel_pub).unwrap();

        // SEALED round-trip: a status request sealed to the node's channel key (signed by the
        // caller, AAD-bound to channel/direction/seq) gets a response sealed BACK to the client's
        // key (signed by the node) — nothing plaintext crosses after establishment.
        let req_bytes = serde_json::to_vec(&json!({ "op": "status" })).unwrap();
        let aad_out = ddrm_envelope::channel_frame_aad(&challenge, 0, 1);
        let sealed_req = ddrm_envelope::seal::seal_bound(&node_channel, &req_bytes, &aad_out, &caller);
        write_frame(&mut client, &sealed_req.to_bytes()).unwrap();
        let sealed_resp = read_frame(&mut client).unwrap().expect("a sealed response frame");
        assert!(
            serde_json::from_slice::<Value>(&sealed_resp).is_err(),
            "the response must NOT be plaintext JSON after channel establishment"
        );
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed_resp).unwrap();
        let aad_in = ddrm_envelope::channel_frame_aad(&challenge, 1, 1);
        let opened = ddrm_envelope::hybrid_unwrap_bound(&client_secret, &env, &aad_in, &verifier).unwrap();
        let resp: Value = serde_json::from_slice(&opened).unwrap();
        assert_eq!(resp["status"].as_str().unwrap(), "ok");

        // DOWNGRADE: a plaintext frame after establishment DROPS the connection (no response).
        write_frame(&mut client, &serde_json::to_vec(&json!({ "op": "status" })).unwrap()).unwrap();
        match read_frame(&mut client) {
            Ok(None) | Err(_) => {} // dropped — fail-closed, no plaintext service after the channel
            Ok(Some(bytes)) => panic!(
                "the node answered a plaintext frame on an established channel: {}",
                String::from_utf8_lossy(&bytes)
            ),
        }
        handle.join().unwrap();
        let _ = std::fs::remove_file(&store);
    }

    /// Pull the challenge string out of a session-token JSON value.
    fn challenge_str(token: &Value) -> &str {
        token["challenge_b64"].as_str().unwrap()
    }

    /// The node refuses a recover whose session token is EXPIRED, FORGED, or TAMPERED — even with a
    /// perfectly valid escrow + receipt — so a long-lived node only recovers within a live handshake
    /// session and a captured/forged token cannot drive recovery.
    #[test]
    fn recover_fails_closed_on_an_expired_or_forged_session() {
        let store = unique_store("session");
        let (mut node, base, _caller) = setup_recover(&store);

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
        let (mut node, base, caller) = setup_recover(&store);
        // Reuse the SAME session token across three recovers — each carries a STRICTLY-ADVANCING
        // freshness counter (re-signed under the caller key), and all succeed.
        for seq in 1u64..=3 {
            let mut args = base.clone();
            args.recover_seq = seq;
            args.caller_sig_b64 = proof_for(
                &caller,
                &base.session_token,
                CONTENT,
                &base.kid_hex,
                &base.decrypt_session_pub_b64,
                seq,
            );
            assert!(matches!(node.recover(args), Response::Ok { .. }), "recover seq {seq} should succeed");
        }
        let _ = std::fs::remove_file(&store);
    }

    /// The per-recover FRESHNESS gate (Day 95–96): a captured recover replayed VERBATIM (same
    /// `recover_seq`, same proof) is refused, and a recover whose `recover_seq` does NOT strictly
    /// advance the session counter is refused — so a replayed recover frame cannot re-drive recovery
    /// even with an otherwise-valid token + possession proof. The OWNER-bound, anti-replay analogue
    /// of PC2's revocable per-delegation nonce (`secureViewSession.ts:108`–`:112`).
    #[test]
    fn recover_fails_closed_on_a_replayed_or_stale_recover_seq() {
        let store = unique_store("freshness");
        let (mut node, base, caller) = setup_recover(&store);

        // First recover at seq 1 succeeds and consumes the counter.
        assert!(matches!(node.recover(base.clone()), Response::Ok { .. }));

        // Replaying the SAME frame (seq 1) verbatim is refused (the counter no longer advances).
        assert_eq!(error_code(&node.recover(base.clone())), "session_invalid");

        // A NEW, correctly-signed recover at a HIGHER seq is accepted (the session is still live).
        let mut next = base.clone();
        next.recover_seq = 2;
        next.caller_sig_b64 =
            proof_for(&caller, &base.session_token, CONTENT, &base.kid_hex, &base.decrypt_session_pub_b64, 2);
        assert!(matches!(node.recover(next), Response::Ok { .. }));

        // After consuming seq 2, a recover that regresses to seq 1 (or repeats 2) is refused.
        let mut stale = base.clone();
        stale.recover_seq = 1;
        assert_eq!(error_code(&node.recover(stale)), "session_invalid");

        let _ = std::fs::remove_file(&store);
    }

    /// The KNOWN-caller allow-list (Day 95–96): when the node is provisioned with an allow-list, a
    /// `hello` from a caller whose ephemeral identity key is NOT on the list is refused at the
    /// handshake (`caller_not_authorized`), BEFORE any session token is minted — while a caller ON
    /// the list completes the handshake. The OWNER-BOUND analogue of PC2's session being tied to a
    /// registered wallet (`secureViewSession.ts:87`–`:100`). With no allow-list, any well-formed
    /// caller is accepted (anonymous enrollment).
    #[test]
    fn hello_enforces_a_known_caller_allow_list() {
        let store = unique_store("allowlist");
        let (known, known_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x41u8; 32]);
        let _ = &known;
        let (_unknown, unknown_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x42u8; 32]);

        // A node provisioned with ONLY `known` on its allow-list.
        let mut node = DkmsAuthorityNode {
            allowed_callers: Some(vec![known_vk.clone()]),
            ..DkmsAuthorityNode::default()
        };
        ok_data(node.init(json!({ "authority_key_store": store })));

        let challenge = b64().encode([0xA1u8; 32]);
        // The KNOWN caller completes the handshake (and gets a session token).
        let ok = ok_data(node.hello(&challenge, &b64().encode(&known_vk), Some(NOW), None));
        assert!(ok["session_token"].is_object());
        // An UNKNOWN caller is refused at hello, before any token is minted.
        assert_eq!(
            error_code(&node.hello(&challenge, &b64().encode(&unknown_vk), Some(NOW), None)),
            "caller_not_authorized"
        );

        // Sanity: a node with NO allow-list accepts the same unknown caller (anonymous enrollment).
        let store2 = unique_store("allowlist-anon");
        let mut anon = DkmsAuthorityNode::default();
        ok_data(anon.init(json!({ "authority_key_store": store2 })));
        assert!(ok_data(anon.hello(&challenge, &b64().encode(&unknown_vk), Some(NOW), None))["session_token"].is_object());

        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(&store2);
    }
}
