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

/// The node's OWN pinned, read-only Base capability + the trustless `authorize_access`
/// gate (verify the wallet-signed grant here, evaluate `hasAccessByContentId` here).
mod node_chain;

// RELEASE-BUILD INVARIANT (pre-mainnet deploy gate — fix-pack ②).
// The legacy unsigned-receipt path (`legacy-receipt-authz`) and the broader `dev-modes` opt-in that
// pulls it in are migration/test scaffolds only (see Cargo.toml). A PRODUCTION node — a release
// build — MUST NOT compile them: with them on, a missing wallet grant can fall back to the legacy
// receipt path AND the node-set pin (`DKMS_AUTHORITY_NODE_SET_ID_B64`, see `authorize`) stops being
// mandatory — both cross-quorum-replay defenses. We detect "release" by the ABSENCE of
// `debug_assertions` (on for dev/test, off for `cargo build --release`); a production deploy builds
// with DEFAULT features, so this fails the build the instant a deploy command leaks
// `dev-modes`/`legacy-receipt-authz` into a release node. Dev/CI keep building DEBUG with
// `--features dev-modes`, which is unaffected. CI actively asserts both directions; see
// `.github/workflows/ci.yml` and `docs/DEPLOY_CHECKLIST.md`.
#[cfg(all(
    not(debug_assertions),
    not(test),
    any(feature = "legacy-receipt-authz", feature = "dev-modes")
))]
compile_error!(
    "release build must not enable `dev-modes`/`legacy-receipt-authz` (dev/migration-only): \
     build a production dkms-authority with default features"
);

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

/// Env var carrying the node's OPTIONAL allow-list of caller identities: a comma-separated list of
/// base64 ML-DSA verifying keys, set by the OPERATOR/PROVISIONER (the connecting CLIENT cannot
/// override it). Since W3/D4 this is a SOFT, OPTIONAL gate — NOT the security boundary. The real
/// authorization is the wallet-signed [`ddrm_envelope::access::AccessGrantV1`] the node verifies +
/// the on-chain token the node reads itself (see [`authorize`]). Roles of the allow-list now:
///   * a coarse DoS/handshake gate (refuse unknown callers a session) when an operator wants one;
///   * the TRUST scope for the LEGACY unsigned-receipt path — that path is honored ONLY for an
///     enrolled caller, so dropping the allow-list is SAFE: an anonymous caller can authorize only
///     with a grant and can never forge `allowed:true`.
/// When unset/empty the node serves ANONYMOUS callers — the millions-of-sovereign-runtimes posture
/// — relying on the trustless grant gate. (The pinned OPERATOR identity, separate, still governs
/// lifecycle/rotation/revocation.)
#[cfg(unix)]
const ALLOWED_CALLERS_ENV: &str = "DKMS_AUTHORITY_ALLOWED_CALLERS";

/// Env var carrying the node's pinned OPERATOR identity (Day 109–112): the base64 ML-DSA verifying
/// key whose signatures authorize the node's LIFECYCLE operations — a share-wise `rotate_share`
/// (re-escrow this node's share to a successor, refreshed) and a live `revoke_caller`. The
/// OPERATOR/PROVISIONER who launches the daemon sets it; the connecting client cannot override it.
/// When unset, BOTH lifecycle ops fail closed (`not_configured`) — a node with no pinned operator
/// can never be rotated or instructed to revoke by anyone.
#[cfg(unix)]
const OPERATOR_VK_ENV: &str = "DKMS_AUTHORITY_OPERATOR_VK";

/// How long a session token the node mints at `hello` stays live (seconds). A long-lived node only
/// recovers for a caller whose handshake session is still within this window — a short, bounded
/// credential, the analogue of PC2's session TTL (`mediaSessionManager` lifetime).
const SESSION_TTL_SECONDS: u64 = 300;

/// Per-connection read timeout (ELACITY-2282 Defect A insurance). Bounds an idle/stalled read so a
/// leaked or abandoned client connection can never camp its serving thread forever; the read then
/// errors and the connection thread ends. Applied on BOTH the Unix and TCP serve loops. A live
/// pooled client re-establishes on the next release if it idled past this window.
const CONNECTION_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Cap on concurrently-served connection threads (ELACITY-2282 hardening). Each accepted connection
/// is served on its OWN thread; without a bound, a hostile peer that opens many connections and
/// trickles a byte slower than [`CONNECTION_READ_TIMEOUT`] to keep each alive would spawn an
/// UNBOUNDED number of threads and exhaust the node's threads/memory — and taking 2-of-3 quorum
/// nodes down drops every release below threshold. Once this many connections are in flight, further
/// accepts are dropped (the socket is closed) until a slot frees, so a slow-loris peer can occupy at
/// most this many slots rather than crash the daemon. Sized generously so legitimate pooled clients
/// are never turned away in practice.
const MAX_ACTIVE_CONNECTIONS: usize = 512;

/// The daemon-lifetime REVOKED-caller set (Day 109–112), shared LIVE across every connection thread
/// via one `Arc`. A revocation performed on ANY connection is visible IMMEDIATELY to every other
/// connection's gates, so "revocation outranks a live session" holds across concurrency — not only
/// after the revoking connection closes (the ELACITY-2282 thread-per-connection follow-up: the old
/// per-connection snapshot merged additions back only on close, leaving a revoked caller with a warm
/// pooled connection served until the revoker disconnected). A node restart clears it, at which point
/// the operator's allow-list is the standing gate. A poisoned lock is recovered (`into_inner`): a
/// peer panic never corrupts the set, and the gate must still read it (fail-closed, never fail-open).
type RevokedSet = std::sync::Arc<std::sync::Mutex<std::collections::HashSet<Vec<u8>>>>;

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

/// The node's effective wall clock for ISSUANCE (minting a session token's expiry). A RELEASE build
/// CLAMPS the caller-supplied `now_unix` to the node's real clock (pre-audit #4): a caller may pass an
/// EARLIER `now` (which only SHRINKS its own session window — harmless), but never a LATER one, since a
/// future `now` would mint `expires_at = now + TTL` past the intended bound and keep the session alive
/// beyond its TTL as measured by the node's `security_now`. This mirrors `security_now`'s discipline
/// (never trust the caller's clock to push time forward). Tests and `dev-modes` honor the caller value
/// verbatim for deterministic windows.
fn effective_now(now_unix: Option<u64>) -> u64 {
    #[cfg(any(test, feature = "dev-modes"))]
    {
        return now_unix.unwrap_or_else(real_clock_secs);
    }
    #[cfg(not(any(test, feature = "dev-modes")))]
    {
        let node_now = real_clock_secs();
        // Upper-bound by the node clock; a caller can only ever shorten its window, never extend it.
        now_unix.map(|n| n.min(node_now)).unwrap_or(node_now)
    }
}

/// The node's clock for SECURITY-EXPIRY decisions (delegation/session windows, possession-token
/// expiry). A RELEASE build NEVER trusts the caller's `now_unix` here — otherwise a caller could
/// pass a `now` inside an already-expired window to keep a revoked delegation / expired token alive
/// indefinitely, defeating the bounded-window + revocation-via-expiry property the node enforces
/// trustlessly (DEV_MODE_GUARD_SPEC defense-in-depth; itself backstopped by the live on-chain check
/// + the strictly-advancing `recover_seq` possession proof). Tests and `dev-modes` builds honor the
/// caller value for deterministic windows.
fn security_now(now_unix: Option<u64>) -> u64 {
    #[cfg(any(test, feature = "dev-modes"))]
    {
        if let Some(n) = now_unix {
            return n;
        }
    }
    #[cfg(not(any(test, feature = "dev-modes")))]
    {
        let _ = now_unix;
    }
    real_clock_secs()
}

fn real_clock_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Process-held replay/revocation tracker (PC2's `revokedDelegations` + `seenRequestNonces`):
/// one per node process, consulted on every grant-authorized recover so a per-request nonce is
/// single-use and a revoked delegation is refused for its remaining lifetime. Each open assembles a
/// FRESH per-request nonce (gateway `access_grant.rs`), so legitimate re-opens are never rejected.
fn replay_guard() -> &'static std::sync::Mutex<ddrm_envelope::access::ReplayGuard> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<ddrm_envelope::access::ReplayGuard>> =
        std::sync::OnceLock::new();
    GUARD.get_or_init(|| std::sync::Mutex::new(ddrm_envelope::access::ReplayGuard::new()))
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
        /// QUORUM RELEASE ATTESTATION (Day 131–135): the node-set id this open is served by, base64.
        /// When present (with `attest_expiry`), the node CO-SIGNS a release attestation binding
        /// `(content_id, principal_id, right, node_set_id, decrypt_session_pub, kid, expiry)` and
        /// returns it as `release_attestation_b64`; the boundary aggregates the t co-signatures into
        /// a portable, offline-verifiable `QuorumReleaseProofV1`. Absent → no attestation (a single
        /// node-set open needs no quorum proof). The node binds the id it is HANDED; the offline
        /// verifier independently recomputes it from the member vks, so a lie does not help an attacker.
        #[serde(default)]
        attest_node_set_id_b64: Option<String>,
        /// The release attestation's expiry (unix seconds) — REQUIRED to co-sign. The offline verifier
        /// rejects a proof once `now > expiry`.
        #[serde(default)]
        attest_expiry: Option<u64>,
        /// TRUSTLESS AUTHORIZATION (W2): the wallet-signed access grant. When present, the node
        /// verifies the wallet (EIP-191/1271) + session-key signatures ITSELF and evaluates
        /// `hasAccessByContentId` per covered address ITSELF — no enrollment, no trusted receipt.
        /// When absent, the node falls back to the legacy receipt path iff `legacy-receipt-authz`
        /// is enabled (migration scaffold; off ⇒ a missing grant fails closed).
        #[serde(default)]
        access_grant: Option<ddrm_envelope::access::AccessGrantV1>,
    },
    /// SHARE-WISE ROTATION (Day 109–112): re-escrow THIS node's current share to a SUCCESSOR node,
    /// refreshed by an operator-sealed XOR delta — the whole CEK is NEVER reassembled anywhere
    /// during rotation (each node rotates only ITS OWN share). The delta envelope is sealed to this
    /// node's escrow recipient, SIGNED by the pinned operator identity, and AEAD-bound to
    /// `rotation_aad(kid16, this_node_recipient, successor_recipient)` — so a forged/tampered
    /// delta, a non-operator instruction, or a successor-redirect all fail the unwrap fail-closed.
    /// PC2 has no analogue: its "rotation" is a manual constant redeploy that can never migrate
    /// existing content (`chipotle-client.ts:125`/`:1043`/`:1064`).
    RotateShare {
        /// This node's CURRENT escrowed share (the producer's — or a prior rotation's — envelope).
        wrapped_cek_b64: String,
        scheme: String,
        kid_hex: String,
        /// The key that signed the CURRENT escrow (the producer at first publish; the PREVIOUS
        /// node after an earlier rotation).
        producer_vk_b64: String,
        /// The successor node's published escrow recipient — the rotated share is sealed to it.
        successor_recipient_pub_b64: String,
        /// The operator-sealed refresh delta (a `PqSealedEnvelope`, base64).
        delta_envelope_b64: String,
    },
    /// LIVE CALLER REVOCATION (Day 109–112): remove a caller from service at runtime, no restart.
    /// Requires an operator signature over the caller's verifying key (`sign_revocation`); once
    /// revoked, the caller's next `hello` AND any `recover` under a still-live session token are
    /// refused — revocation outranks a live session. The runtime-core analogue of PC2's revoked
    /// delegation nonce read back per request (`secureViewSession.ts:108`–`:112`), except the
    /// signed instruction reaches the KEY-HOLDING NODE itself, not just an HTTP middleware.
    RevokeCaller {
        caller_pub_b64: String,
        operator_sig_b64: String,
    },
    /// QUORUM RECONFIGURATION — CONTRIBUTE (Day 121–125): THIS node is an OLD quorum member asked to
    /// re-share its share into a NEW k-of-m set. It opens an operator authorization bound to
    /// `reshare_aad(kid, old_set, new_set, k, m)`, recovers its INDEXED share `x_i ‖ p(x_i)`, draws a
    /// fresh degree-(k−1) polynomial `q_i` with `q_i(0) = p(x_i)`, and returns the sub-share
    /// `q_i(y_j)` SEALED to each new node `j` (signed by this node, AEAD-bound to the
    /// contributor→target pair). The CEK is never reassembled — this node only ever touches ITS
    /// share. PC2 has no analogue (Lit's t, n and membership are fixed and uninspectable).
    ReshareContribute {
        wrapped_cek_b64: String,
        scheme: String,
        kid_hex: String,
        producer_vk_b64: String,
        operator_auth_b64: String,
        old_node_set_id_b64: String,
        new_node_set_id_b64: String,
        k: u8,
        m: u8,
        /// The m new-node escrow recipient public keys, in coordinate order (`x = 1..=m`).
        new_recipient_pubs_b64: Vec<String>,
    },
    /// QUORUM RECONFIGURATION — INSTALL (Day 121–125): THIS node is a NEW member assembling its share
    /// of the reconfigured set. It opens the same operator authorization (sealed to ITS recipient),
    /// unwraps the sub-shares an OLD quorum routed it (each verified under its contributor's identity,
    /// bound to this contributor→target pair), combines them via the OLD-contributor Lagrange
    /// (`P(y_j) = Σ λ_i · q_i(y_j)`, so `P(0) = CEK`), and RE-ESCROWS its new indexed share
    /// `y_j ‖ P(y_j)` to ITSELF — the share the k-of-m boundary later opens. The CEK never exists here.
    ReshareInstall {
        operator_auth_b64: String,
        old_node_set_id_b64: String,
        new_node_set_id_b64: String,
        k: u8,
        m: u8,
        /// This new node's coordinate `y_j` in the reconfigured set (`1..=m`).
        target_x: u8,
        scheme: String,
        kid_hex: String,
        /// The sub-shares this node received from an OLD quorum (≥ the old threshold, distinct).
        contributions: Vec<ReshareContribution>,
    },
    /// DISTRIBUTED KEY GENERATION — CONTRIBUTE (Day 126–130): THIS node is a DEALER in a fresh t-of-m
    /// ceremony. It opens an operator authorization bound to `dkg_aad(kid, dkg_id, node_set, t, m)`,
    /// draws a FRESH degree-(t−1) polynomial `f_i` whose CONSTANT term `c_i = f_i(0)` is a private,
    /// master-derived, ceremony-bound contribution (so the CEK `⊕_i c_i` is born distributed — this
    /// node knows ONLY its own addend), evaluates `f_i` at every member coordinate, and returns the
    /// sub-share `f_i(x_j)` SEALED to each member `j` (signed by this node, AEAD-bound to the
    /// dealer→target pair). The CEK is assembled NOWHERE. PC2 has no analogue (a Lit key is generated
    /// inside the Lit network with the dealer set, threshold, and policy opaque and immutable).
    DkgContribute {
        operator_auth_b64: String,
        dkg_id_b64: String,
        node_set_id_b64: String,
        t: u8,
        m: u8,
        /// THIS dealer's coordinate `x_i` in the node-set (`1..=m`) — tags its sub-shares.
        dealer_x: u8,
        kid_hex: String,
        /// The agreed CEK byte length (every dealer draws a polynomial of this length so the summed
        /// shares — and the reconstructed `F(0)` — are exactly this many bytes).
        cek_len: u32,
        /// The m member escrow recipient public keys, in coordinate order (`x = 1..=m`).
        member_recipient_pubs_b64: Vec<String>,
    },
    /// DISTRIBUTED KEY GENERATION — INSTALL (Day 126–130): THIS node is a MEMBER assembling its share
    /// of the DKG-born key. It opens the same operator authorization (sealed to ITS recipient),
    /// unwraps the sub-shares every dealer routed it (each verified under its dealer's identity, bound
    /// to the dealer→THIS pair — a tampered/forged/redirected sub-share is refused and the dealer
    /// NAMED), SUMS them over GF(256) into its final share `F(x_j) = ⊕_i f_i(x_j)`, and RE-ESCROWS its
    /// indexed share `x_j ‖ F(x_j)` to ITSELF (the share the t-of-m boundary later opens). The CEK
    /// never exists here — a single member share is one point of `F` and reveals nothing of `F(0)`.
    DkgInstall {
        operator_auth_b64: String,
        dkg_id_b64: String,
        node_set_id_b64: String,
        t: u8,
        m: u8,
        /// This member's coordinate `x_j` in the node-set (`1..=m`).
        target_x: u8,
        kid_hex: String,
        scheme: String,
        /// The sub-shares this member received from the dealers (one per dealer, distinct dealers).
        contributions: Vec<DkgContribution>,
    },
    Shutdown,
}

/// One sub-share a DEALER routed to a MEMBER during a DKG ceremony: the dealer's coordinate `x_i`
/// (only used to authenticate + dedupe — DKG members SUM, they do not Lagrange-weight), its verifying
/// key, and the sealed `dealer_x ‖ f_i(x_j)` payload.
#[derive(Debug, Deserialize)]
struct DkgContribution {
    dealer_x: u8,
    dealer_vk_b64: String,
    sealed_subshare_b64: String,
}

/// One sub-share an OLD quorum member routed to a NEW node during reconfiguration: the
/// contributor's coordinate `x_i` (its Lagrange weight), its verifying key (to authenticate the
/// sub-share), and the sealed `contributor_x ‖ q_i(y_j)` payload.
#[derive(Debug, Deserialize)]
struct ReshareContribution {
    contributor_x: u8,
    contributor_vk_b64: String,
    sealed_subshare_b64: String,
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
    /// Master-derived seed for the node's RE-SHARING sub-share polynomials (Day 121–125). When this
    /// node contributes to a quorum reconfiguration it draws its fresh degree-(k−1) coefficients by
    /// expanding this seed over the target new-set id — secret (master-derived, never published),
    /// unpredictable to any adversary, and deterministic so a re-driven contribution is stable. Kept
    /// out of the envelope crate (the RNG/PRF-input policy) and domain-separated from every key seed.
    reshare_seed: [u8; 32],
    /// Master-derived seed for this node's DKG dealer polynomials (Day 126–130). When this node acts
    /// as a dealer in a key-generation ceremony it draws its FRESH degree-(t−1) polynomial — the
    /// private contribution `c_i = f_i(0)` AND the higher coefficients — by expanding this seed over
    /// the ceremony id. Secret (master-derived, never published), unpredictable to any adversary, and
    /// deterministic so a re-driven contribution is byte-stable. Domain-separated from `reshare_seed`
    /// and every key seed (a re-share delta and a DKG contribution can never collide).
    dkg_seed: [u8; 32],
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
        let reshare_seed = ddrm_envelope::derive_seed(master, b"key-authority/reshare/v1");
        let dkg_seed = ddrm_envelope::derive_seed(master, b"key-authority/dkg/v1");
        Self {
            signer,
            verifying_key,
            recipient_secret,
            recipient_public: ddrm_envelope::session_public_bytes(&recipient_public),
            channel_secret,
            channel_public: ddrm_envelope::session_public_bytes(&channel_public),
            reshare_seed,
            dkg_seed,
        }
    }

    /// Expand the master-derived [`Self::reshare_seed`] into `count` fresh degree-coefficient vectors
    /// of `len` bytes each, bound to `new_set_id` and the coefficient index — the secret higher
    /// coefficients of THIS node's re-sharing polynomial `q_i`. Domain-separated counter expansion
    /// over `derive_seed` (no RNG, deterministic, so a re-driven contribution reproduces the SAME
    /// sub-shares). Each vector is independent of the others and of every key seed.
    fn reshare_coefficients(&self, new_set_id: &[u8], count: usize, len: usize) -> Vec<Vec<u8>> {
        (0..count)
            .map(|d| {
                let mut info = b"reshare/coeff/v1".to_vec();
                info.extend_from_slice(new_set_id);
                info.extend_from_slice(&(d as u32).to_be_bytes());
                let mut out = Vec::with_capacity(len);
                let mut block = 0u32;
                while out.len() < len {
                    let mut blk_info = info.clone();
                    blk_info.extend_from_slice(&block.to_be_bytes());
                    out.extend_from_slice(&ddrm_envelope::derive_seed(&self.reshare_seed, &blk_info));
                    block += 1;
                }
                out.truncate(len);
                out
            })
            .collect()
    }

    /// Expand the master-derived [`Self::dkg_seed`] into THIS dealer's fresh degree-(t−1) polynomial
    /// for a ceremony: `t` coefficient vectors of `len` bytes each (index 0 = the private constant
    /// term `c_i = f_i(0)`, indices 1..t-1 = the higher coefficients), bound to `dkg_id` and the
    /// coefficient index. Domain-separated counter expansion over `derive_seed` (no RNG,
    /// deterministic, so a re-driven contribution reproduces the SAME sub-shares). Each vector is
    /// independent of the others, of the re-sharing coefficients, and of every key seed.
    fn dkg_polynomial(&self, dkg_id: &[u8], t: usize, len: usize) -> Vec<Vec<u8>> {
        (0..t)
            .map(|d| {
                let mut info = b"dkg/coeff/v1".to_vec();
                info.extend_from_slice(dkg_id);
                info.extend_from_slice(&(d as u32).to_be_bytes());
                let mut out = Vec::with_capacity(len);
                let mut block = 0u32;
                while out.len() < len {
                    let mut blk_info = info.clone();
                    blk_info.extend_from_slice(&block.to_be_bytes());
                    out.extend_from_slice(&ddrm_envelope::derive_seed(&self.dkg_seed, &blk_info));
                    block += 1;
                }
                out.truncate(len);
                out
            })
            .collect()
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
    /// The pinned OPERATOR identity (Day 109–112): the decoded ML-DSA verifying key whose
    /// signatures authorize `rotate_share` + `revoke_caller`. Set by the OPERATOR at daemon start
    /// (env), never by the connecting client. `None` = lifecycle ops fail closed.
    operator_vk: Option<Vec<u8>>,
    /// Callers REVOKED at runtime (Day 109–112) — see [`RevokedSet`]. Their `hello` is refused and a
    /// `recover` under a still-live session token is refused (revocation outranks a live session).
    /// Shared LIVE across all connection threads (the same `Arc` every thread holds), so a revocation
    /// is enforced the instant the operator's `revoke_caller` lands — on every already-open connection,
    /// not just future ones. In-memory like PC2's `revokedDelegations` map
    /// (`utils/secureViewSession.ts:374`) — a node restart clears it, at which point the operator's
    /// allow-list is the standing gate. `#[derive(Default)]` gives a freshly-constructed node its OWN
    /// empty set; the serve loop wires every connection to ONE shared set instead.
    revoked_callers: RevokedSet,
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
                attest_node_set_id_b64,
                attest_expiry,
                access_grant,
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
                attest_node_set_id_b64,
                attest_expiry,
                access_grant,
            }),
            Request::RotateShare {
                wrapped_cek_b64,
                scheme,
                kid_hex,
                producer_vk_b64,
                successor_recipient_pub_b64,
                delta_envelope_b64,
            } => self.rotate_share(
                &wrapped_cek_b64,
                &scheme,
                &kid_hex,
                &producer_vk_b64,
                &successor_recipient_pub_b64,
                &delta_envelope_b64,
            ),
            Request::RevokeCaller { caller_pub_b64, operator_sig_b64 } => {
                self.revoke_caller(&caller_pub_b64, &operator_sig_b64)
            }
            Request::ReshareContribute {
                wrapped_cek_b64,
                scheme,
                kid_hex,
                producer_vk_b64,
                operator_auth_b64,
                old_node_set_id_b64,
                new_node_set_id_b64,
                k,
                m,
                new_recipient_pubs_b64,
            } => self.reshare_contribute(ReshareContributeArgs {
                wrapped_cek_b64,
                scheme,
                kid_hex,
                producer_vk_b64,
                operator_auth_b64,
                old_node_set_id_b64,
                new_node_set_id_b64,
                k,
                m,
                new_recipient_pubs_b64,
            }),
            Request::ReshareInstall {
                operator_auth_b64,
                old_node_set_id_b64,
                new_node_set_id_b64,
                k,
                m,
                target_x,
                scheme,
                kid_hex,
                contributions,
            } => self.reshare_install(ReshareInstallArgs {
                operator_auth_b64,
                old_node_set_id_b64,
                new_node_set_id_b64,
                k,
                m,
                target_x,
                scheme,
                kid_hex,
                contributions,
            }),
            Request::DkgContribute {
                operator_auth_b64,
                dkg_id_b64,
                node_set_id_b64,
                t,
                m,
                dealer_x,
                kid_hex,
                cek_len,
                member_recipient_pubs_b64,
            } => self.dkg_contribute(DkgContributeArgs {
                operator_auth_b64,
                dkg_id_b64,
                node_set_id_b64,
                t,
                m,
                dealer_x,
                kid_hex,
                cek_len,
                member_recipient_pubs_b64,
            }),
            Request::DkgInstall {
                operator_auth_b64,
                dkg_id_b64,
                node_set_id_b64,
                t,
                m,
                target_x,
                kid_hex,
                scheme,
                contributions,
            } => self.dkg_install(DkgInstallArgs {
                operator_auth_b64,
                dkg_id_b64,
                node_set_id_b64,
                t,
                m,
                target_x,
                kid_hex,
                scheme,
                contributions,
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
        // OPTIONAL DoS GATE (W3/D4): when an allow-list is provisioned, the node serves only a caller
        // whose ephemeral identity key it recognizes — refused at the handshake, BEFORE a token is
        // minted. This is now a SOFT gate, not the security boundary (which is the trustless grant
        // check in `authorize`). When no allow-list is configured the node accepts any well-formed
        // key (the anonymous, millions-of-runtimes posture — still safe: recover requires a grant).
        if let Some(allowed) = self.allowed_callers.as_ref() {
            if !allowed.iter().any(|vk| vk.as_slice() == caller_pub.as_slice()) {
                return Response::error(
                    "caller_not_authorized",
                    "caller identity is not on this node's allow-list (provision the caller's verifying key)",
                );
            }
        }
        // REVOCATION GATE (Day 109–112): a caller the operator revoked at runtime is refused at the
        // handshake even though it is still on the allow-list — no new session is ever minted for it.
        if self.is_caller_revoked(caller_pub.as_slice()) {
            return Response::error(
                "caller_revoked",
                "caller identity has been revoked by the operator — this node no longer serves it",
            );
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
            "supported_operations": ["status", "init", "hello", "recover", "rotate_share", "revoke_caller", "reshare_contribute", "reshare_install", "dkg_contribute", "dkg_install"],
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
        // REVOCATION OUTRANKS A LIVE SESSION (Day 109–112): a caller the operator revoked is refused
        // even when its session token is valid + unexpired — the cutoff is immediate, not "at the
        // next handshake". Checked FIRST, before any signature work. (PC2's analogue: the revoked
        // delegation nonce is read back per request BEFORE the session view is resurrected,
        // `secureViewSession.ts:104`–`:112`.)
        if let Ok(token_caller) = b64().decode(&args.session_token.caller_pub_b64) {
            if self.is_caller_revoked(token_caller.as_slice()) {
                return Response::error(
                    "caller_revoked",
                    "caller identity has been revoked by the operator — a live session does not outrank a revocation",
                );
            }
        }
        // SESSION GATE — refuse to recover without a live, node-verified handshake session
        // (the channel gate), before re-authorizing or touching any key material.
        if let Err(err) = verify_session(authority, args) {
            return Response::error("session_invalid", err);
        }
        // AUTHORIZE in this boundary — refuse to recover for an unauthorized caller before
        // touching any key material (the node never trusts the client's claim). Trustless path:
        // a wallet-signed AccessGrant the node verifies itself + a node-side on-chain
        // hasAccessByContentId check. Legacy path (feature `legacy-receipt-authz`): the unsigned
        // RightsDecisionReceiptV1, used ONLY when no grant is supplied (migration scaffold).
        // The legacy unsigned-receipt path is honored ONLY for an ENROLLED (allow-listed) caller —
        // an anonymous caller (no allow-list, the millions-of-runtimes posture) can authorize ONLY
        // with a wallet-signed grant the node verifies + a chain token the node reads. This is what
        // makes dropping the allow-list as the security boundary SAFE (W3/D4).
        let legacy_receipt_allowed = self.allowed_callers.is_some();
        if let Err(err) = authorize(args, legacy_receipt_allowed) {
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

        // SECURITY INVARIANT (pre-mainnet, scoped with the external auditor): `aad` here is the
        // CALLER-SUPPLIED `args.aad_b64` (decoded above), and it is NOT bound into the recover
        // possession-proof — the node verifies the escrow (recover_escrowed_cek) and the producer,
        // but does NOT independently verify that this re-seal AAD matches the segment-bound
        // transcript / node-set the open claims. Therefore the node's re-seal AAD is NOT
        // independently trustworthy. This is safe TODAY only because the single consumer — the
        // decrypt boundary — rebuilds the segment-bound AAD itself and fails closed on a mismatch;
        // it does not trust this value. DO NOT add a consumer that trusts this re-seal AAD without
        // first binding aad_b64 / segment_digests / node_set_id into the recover possession-proof
        // (so a tampered aad_b64 fails the proof closed here). See docs/THREAT_MODEL.md.
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
        let mut ok = json!({
            "suite": ddrm_envelope::SUITE_PQ_HYBRID,
            "material": material,
            "seal_verifying_key_b64": b64().encode(&authority.verifying_key),
        });
        // QUORUM RELEASE ATTESTATION (Day 131–135): when the boundary declares the node-set this open
        // is served by + an expiry, the node CO-SIGNS a portable proof that IT authorized THIS grant
        // for THIS principal under THIS decrypt session — signed by the secret-holder itself, so a
        // relying party need not trust the runtime's self-authored record. The decrypt session pubkey
        // is the per-open freshness; the binding is over the EXACT (content, principal, right,
        // node_set, session, kid, expiry). Only emitted when both fields are present (fail-closed:
        // missing/invalid attestation inputs simply omit the attestation rather than fabricate one).
        if let (Some(node_set_b64), Some(expiry)) =
            (args.attest_node_set_id_b64.as_ref(), args.attest_expiry)
        {
            if let Ok(node_set_id) = b64().decode(node_set_b64) {
                if let Ok(session_pub) = b64().decode(&args.decrypt_session_pub_b64) {
                    let sig = ddrm_envelope::sign_release_attestation(
                        &authority.signer,
                        args.content_id.as_bytes(),
                        args.principal_id.as_bytes(),
                        args.right.as_bytes(),
                        &node_set_id,
                        &session_pub,
                        &kid16,
                        expiry,
                    );
                    ok["release_attestation_b64"] = json!(b64().encode(&sig));
                    ok["release_attestation_expiry"] = json!(expiry);
                }
            }
        }
        Response::ok(ok)
    }

    /// SHARE-WISE ROTATION (Day 109–112): unwrap THIS node's escrowed share, XOR it with the
    /// operator-sealed refresh delta, and re-escrow the REFRESHED share to the successor node —
    /// all inside this boundary. The share, the delta and the refreshed share live only in
    /// `Zeroizing`; the response carries ONLY the new sealed envelope (+ this node's vk, which is
    /// the rotated escrow's producer identity the successor will verify at recover time).
    ///
    /// Authorization is the OPERATOR SEAL, checked FIRST: the delta envelope must open under this
    /// node's recipient secret, VERIFY under the pinned operator identity, and be AEAD-bound to
    /// `rotation_aad(kid16, this_node_recipient, successor_recipient)` — so a non-operator
    /// instruction, a tampered delta, a kid-swap, or a successor-redirect all fail closed BEFORE
    /// any share material is touched. No pinned operator → rotation is impossible on this node.
    fn rotate_share(
        &self,
        wrapped_cek_b64: &str,
        scheme: &str,
        kid_hex: &str,
        producer_vk_b64: &str,
        successor_recipient_pub_b64: &str,
        delta_envelope_b64: &str,
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
        let operator_vk = match self.operator_vk.as_ref() {
            Some(vk) => vk,
            None => {
                return Response::error(
                    "not_configured",
                    "this node has no pinned operator identity — rotation is refused (provision DKMS_AUTHORITY_OPERATOR_VK)",
                )
            }
        };
        let operator_verifier = match ddrm_envelope::MlDsa65Verifier::from_encoded(operator_vk) {
            Some(v) => v,
            None => return Response::error("not_configured", "pinned operator identity is malformed"),
        };
        let kid16 = match decode_kid_bytes16(kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        let successor_bytes = match b64().decode(successor_recipient_pub_b64) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "successor_recipient_pub_b64 is not valid base64",
                )
            }
        };
        let successor_public = match ddrm_envelope::session_public_from_bytes(&successor_bytes) {
            Some(public) => public,
            None => {
                return Response::error(
                    "invalid_request",
                    "successor_recipient_pub_b64 is not a valid escrow recipient key",
                )
            }
        };
        // OPERATOR AUTHORIZATION FIRST: open the delta under this node's recipient secret, verified
        // under the pinned operator identity, AEAD-bound to the full rotation context. Any mismatch
        // (forged signer / tampered bytes / wrong kid / wrong source node / redirected successor)
        // fails here, before the share is unwrapped.
        let delta = {
            let env = match b64()
                .decode(delta_envelope_b64)
                .ok()
                .and_then(|bytes| ddrm_envelope::PqSealedEnvelope::from_bytes(&bytes).ok())
            {
                Some(env) => env,
                None => {
                    return Response::error(
                        "invalid_request",
                        "delta_envelope_b64 is not a valid sealed envelope",
                    )
                }
            };
            let aad =
                ddrm_envelope::rotation_aad(&kid16, &authority.recipient_public, &successor_bytes);
            match ddrm_envelope::hybrid_unwrap_bound(
                &authority.recipient_secret,
                &env,
                &aad,
                &operator_verifier,
            ) {
                Ok(delta) => delta,
                Err(_) => {
                    return Response::error(
                        "access_denied",
                        "rotation refused: the refresh delta does not open under the pinned operator identity for THIS (kid, node, successor) — forged, tampered, or redirected",
                    )
                }
            }
        };
        // Unwrap this node's CURRENT share (the same authenticated path `recover` uses).
        let wrapped = match b64().decode(wrapped_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "wrapped_cek_b64 is not valid base64"),
        };
        let producer_vk = match b64().decode(producer_vk_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "producer_vk_b64 is not valid base64"),
        };
        let share = match authority.recover_escrowed_cek(&wrapped, scheme, &kid16, &producer_vk) {
            Ok(share) => share,
            Err(_) => {
                return Response::error(
                    "invalid_request",
                    "escrowed share could not be recovered (foreign/tampered escrow, wrong KID/scheme, or bad producer key)",
                )
            }
        };
        if share.len() != delta.len() {
            return Response::error(
                "invalid_request",
                "refresh delta length does not match the escrowed share — rotation refused",
            );
        }
        // REFRESH: share' = share ⊕ delta. Both nodes of a 2-of-2 rotate with the SAME delta, so the
        // CEK is invariant (share1' ⊕ share2' = share1 ⊕ share2) while an OLD captured share is now
        // USELESS next to a NEW share (old ⊕ new' = delta-masked garbage). The whole CEK never
        // exists here — this node only ever sees ITS share.
        let refreshed = zeroize::Zeroizing::new(
            share.iter().zip(delta.iter()).map(|(a, b)| a ^ b).collect::<Vec<u8>>(),
        );
        // Re-escrow to the SUCCESSOR under the shared escrow AAD, signed by THIS node — the rotated
        // escrow's producer identity. The successor verifies it at recover exactly as it would a
        // producer's.
        let new_aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &successor_bytes);
        let rotated = ddrm_envelope::seal::seal_bound(
            &successor_public,
            refreshed.as_slice(),
            &new_aad,
            &authority.signer,
        );
        Response::ok(json!({
            "rotated_wrapped_cek_b64": b64().encode(rotated.to_bytes()),
            "escrow_producer_vk_b64": b64().encode(&authority.verifying_key),
        }))
    }

    /// LIVE CALLER REVOCATION (Day 109–112): verify the operator's signature over the caller key
    /// and add it to the revoked set — the caller's next `hello` and any `recover` under a
    /// still-live session token are refused from this moment. Idempotent. Requires the pinned
    /// operator identity; a node with no operator can never be instructed to revoke (fail-closed:
    /// the allow-list remains the standing gate).
    fn revoke_caller(&mut self, caller_pub_b64: &str, operator_sig_b64: &str) -> Response {
        let operator_vk = match self.operator_vk.as_ref() {
            Some(vk) => vk,
            None => {
                return Response::error(
                    "not_configured",
                    "this node has no pinned operator identity — revocation is refused (provision DKMS_AUTHORITY_OPERATOR_VK)",
                )
            }
        };
        let operator_verifier = match ddrm_envelope::MlDsa65Verifier::from_encoded(operator_vk) {
            Some(v) => v,
            None => return Response::error("not_configured", "pinned operator identity is malformed"),
        };
        let caller_pub = match b64().decode(caller_pub_b64) {
            Ok(bytes) if !bytes.is_empty() => bytes,
            _ => return Response::error("invalid_request", "caller_pub_b64 is not valid non-empty base64"),
        };
        let sig = match b64().decode(operator_sig_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "operator_sig_b64 is not valid base64"),
        };
        if !ddrm_envelope::verify_revocation(&operator_verifier, &caller_pub, &sig) {
            return Response::error(
                "access_denied",
                "revocation refused: the signature does not verify under the pinned operator identity",
            );
        }
        // Insert into the SHARED live set: the revocation binds every other open connection's gates
        // at once (HashSet insertion is idempotent, so a repeat revoke is a no-op).
        self.revoked_callers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(caller_pub);
        Response::ok(json!({ "revoked": true }))
    }

    /// True iff `vk` is in the shared daemon-lifetime revocation set, read LIVE (a revocation on any
    /// connection is visible here immediately). Recovers the guard if a peer thread poisoned the lock
    /// — the set's data survives a panic and the gate must never fail open.
    fn is_caller_revoked(&self, vk: &[u8]) -> bool {
        self.revoked_callers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(vk)
    }

    /// QUORUM RECONFIGURATION — CONTRIBUTE (Day 121–125). This OLD quorum member re-shares its share
    /// into the new k-of-m set. Authorization is the OPERATOR SEAL, checked FIRST: the auth envelope
    /// must open under this node's recipient secret, verify under the pinned operator identity, and
    /// be AEAD-bound to `reshare_aad(kid, old_set, new_set, k, m)` — so a non-operator instruction, a
    /// tampered context, a kid/threshold/membership swap all fail BEFORE any share is touched. The
    /// node then recovers its INDEXED share `x_i ‖ p(x_i)`, draws a fresh degree-(k−1) polynomial with
    /// `q_i(0) = p(x_i)`, and seals `x_i ‖ q_i(y_j)` to each new node (signed by this node, bound to
    /// the contributor→target pair). The full share, the coefficients and the sub-shares live only in
    /// `Zeroizing`-class scope; the CEK is never reassembled — this node only ever holds ITS point.
    fn reshare_contribute(&self, args: ReshareContributeArgs) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => return Response::error("not_configured", "dkms-authority node is not initialized (send `init` first)"),
        };
        let operator_verifier = match self.pinned_operator_verifier() {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let kid16 = match decode_kid_bytes16(&args.kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        if args.k < 2 {
            return Response::error("invalid_request", "reconfiguration threshold k must be at least 2");
        }
        if args.new_recipient_pubs_b64.len() != args.m as usize || args.m < args.k {
            return Response::error("invalid_request", "new_recipient_pubs_b64 must list exactly m recipients with m ≥ k");
        }
        let old_set_id = match b64().decode(&args.old_node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "old_node_set_id_b64 is not valid base64"),
        };
        let new_set_id = match b64().decode(&args.new_node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "new_node_set_id_b64 is not valid base64"),
        };
        // OPERATOR AUTHORIZATION FIRST — bind the whole reconfiguration context into the AEAD.
        let reshare_aad = ddrm_envelope::reshare_aad(&kid16, &old_set_id, &new_set_id, args.k, args.m);
        if let Err(resp) = self.open_operator_auth(&args.operator_auth_b64, &reshare_aad, &operator_verifier) {
            return resp;
        }
        // Recover THIS node's current INDEXED share (the same authenticated path `recover` uses).
        let wrapped = match b64().decode(&args.wrapped_cek_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "wrapped_cek_b64 is not valid base64"),
        };
        let producer_vk = match b64().decode(&args.producer_vk_b64) {
            Ok(bytes) => bytes,
            Err(_) => return Response::error("invalid_request", "producer_vk_b64 is not valid base64"),
        };
        let indexed = match authority.recover_escrowed_cek(&wrapped, &args.scheme, &kid16, &producer_vk) {
            Ok(share) => share,
            Err(_) => return Response::error("invalid_request", "escrowed share could not be recovered (foreign/tampered escrow, wrong KID/scheme, or bad producer key)"),
        };
        let (contributor_x, body) = match ddrm_envelope::parse_indexed_share(&indexed) {
            Some((x, body)) => (x, body),
            None => return Response::error("invalid_request", "this node's escrow is not a valid indexed quorum share"),
        };
        // Fresh degree-(k−1) polynomial: k−1 higher coefficients, master-derived + new-set-bound.
        let higher = authority.reshare_coefficients(&new_set_id, (args.k - 1) as usize, body.len());
        let higher_refs: Vec<&[u8]> = higher.iter().map(|c| c.as_slice()).collect();

        let mut subshares = Vec::with_capacity(args.m as usize);
        for (j, recipient_b64) in args.new_recipient_pubs_b64.iter().enumerate() {
            let target_x = (j + 1) as u8;
            let recipient_bytes = match b64().decode(recipient_b64) {
                Ok(b) => b,
                Err(_) => return Response::error("invalid_request", "a new recipient key is not valid base64"),
            };
            let recipient_public = match ddrm_envelope::session_public_from_bytes(&recipient_bytes) {
                Some(p) => p,
                None => return Response::error("invalid_request", "a new recipient key is not a valid escrow recipient"),
            };
            let sub_body = match ddrm_envelope::reshare_eval(body, &higher_refs, target_x) {
                Ok(s) => zeroize::Zeroizing::new(s),
                Err(e) => return Response::error("invalid_request", e),
            };
            // The sub-share carries the CONTRIBUTOR's coordinate (its Lagrange weight at the new node).
            let payload = zeroize::Zeroizing::new(ddrm_envelope::indexed_share(contributor_x, &sub_body));
            let aad = ddrm_envelope::reshare_subshare_aad(&kid16, &new_set_id, contributor_x, target_x);
            let sealed = ddrm_envelope::seal::seal_bound(&recipient_public, payload.as_slice(), &aad, &authority.signer);
            subshares.push(json!({
                "target_x": target_x,
                "sealed_subshare_b64": b64().encode(sealed.to_bytes()),
            }));
        }
        Response::ok(json!({
            "contributor_x": contributor_x,
            "contributor_vk_b64": b64().encode(&authority.verifying_key),
            "subshares": subshares,
        }))
    }

    /// QUORUM RECONFIGURATION — INSTALL (Day 121–125). This NEW member assembles its share of the
    /// reconfigured set. Authorization is the OPERATOR SEAL (sealed to THIS node's recipient), bound
    /// to `reshare_aad(kid, old_set, new_set, k, m)` — checked first. It then unwraps each sub-share
    /// (verified under its contributor's identity, AEAD-bound to the contributor→THIS-node pair, the
    /// inner coordinate matching the declared contributor), and combines them via the OLD-contributor
    /// Lagrange — `P(y_j) = Σ λ_i · q_i(y_j)`, whose constant term is the original CEK. The node
    /// RE-ESCROWS its new indexed share `y_j ‖ P(y_j)` to ITSELF (signed by this node, the new
    /// escrow's producer identity the k-of-m boundary verifies). The CEK never exists here; a single
    /// new share is one point of the new polynomial and reveals nothing.
    fn reshare_install(&self, args: ReshareInstallArgs) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => return Response::error("not_configured", "dkms-authority node is not initialized (send `init` first)"),
        };
        let operator_verifier = match self.pinned_operator_verifier() {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let kid16 = match decode_kid_bytes16(&args.kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        if args.k < 2 || args.target_x == 0 {
            return Response::error("invalid_request", "reconfiguration needs k ≥ 2 and a non-zero target coordinate");
        }
        if args.contributions.len() < 2 {
            return Response::error("invalid_request", "reconfiguration install needs at least an old-quorum's worth of sub-shares");
        }
        let old_set_id = match b64().decode(&args.old_node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "old_node_set_id_b64 is not valid base64"),
        };
        let new_set_id = match b64().decode(&args.new_node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "new_node_set_id_b64 is not valid base64"),
        };
        let reshare_aad = ddrm_envelope::reshare_aad(&kid16, &old_set_id, &new_set_id, args.k, args.m);
        if let Err(resp) = self.open_operator_auth(&args.operator_auth_b64, &reshare_aad, &operator_verifier) {
            return resp;
        }
        // Unwrap + authenticate each sub-share, then combine over the OLD contributors' coordinates.
        let mut points: Vec<(u8, zeroize::Zeroizing<Vec<u8>>)> = Vec::with_capacity(args.contributions.len());
        for c in &args.contributions {
            if c.contributor_x == 0 {
                return Response::error("invalid_request", "a contributor coordinate is zero (the secret position is never a node)");
            }
            if points.iter().any(|(x, _)| *x == c.contributor_x) {
                return Response::error("invalid_request", "duplicate contributor coordinate — not a real old quorum");
            }
            let vk = match b64().decode(&c.contributor_vk_b64) {
                Ok(b) => b,
                Err(_) => return Response::error("invalid_request", "a contributor verifying key is not valid base64"),
            };
            let verifier = match ddrm_envelope::MlDsa65Verifier::from_encoded(&vk) {
                Some(v) => v,
                None => return Response::error("invalid_request", "a contributor verifying key is malformed"),
            };
            let env = match b64().decode(&c.sealed_subshare_b64).ok().and_then(|b| ddrm_envelope::PqSealedEnvelope::from_bytes(&b).ok()) {
                Some(env) => env,
                None => return Response::error("invalid_request", "a sealed sub-share is not a valid envelope"),
            };
            let aad = ddrm_envelope::reshare_subshare_aad(&kid16, &new_set_id, c.contributor_x, args.target_x);
            let payload = match ddrm_envelope::hybrid_unwrap_bound(&authority.recipient_secret, &env, &aad, &verifier) {
                Ok(p) => p,
                Err(_) => return Response::error("access_denied", "a sub-share did not open under THIS node for its declared contributor→target pair (forged, tampered, or redirected)"),
            };
            let (inner_x, body) = match ddrm_envelope::parse_indexed_share(&payload) {
                Some((x, body)) => (x, body),
                None => return Response::error("invalid_request", "a sub-share carries no valid contributor coordinate"),
            };
            if inner_x != c.contributor_x {
                return Response::error("invalid_request", "a sub-share's sealed coordinate disagrees with its declared contributor");
            }
            points.push((c.contributor_x, zeroize::Zeroizing::new(body.to_vec())));
        }
        let point_refs: Vec<(u8, &[u8])> = points.iter().map(|(x, b)| (*x, b.as_slice())).collect();
        let new_share = match ddrm_envelope::lagrange_combine_at_zero(&point_refs) {
            Ok(s) => s,
            Err(e) => return Response::error("invalid_request", e),
        };
        // Re-escrow `target_x ‖ P(target_x)` to THIS node under the shared escrow AAD, signed by it.
        let indexed = zeroize::Zeroizing::new(ddrm_envelope::indexed_share(args.target_x, new_share.as_slice()));
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(&args.scheme, &kid16, &authority.recipient_public);
        let escrow = ddrm_envelope::seal::seal_bound(
            &ddrm_envelope::session_public_from_bytes(&authority.recipient_public).expect("node recipient is a valid escrow key"),
            indexed.as_slice(),
            &escrow_aad,
            &authority.signer,
        );
        Response::ok(json!({
            "target_x": args.target_x,
            "wrapped_cek_b64": b64().encode(escrow.to_bytes()),
            "escrow_producer_vk_b64": b64().encode(&authority.verifying_key),
        }))
    }

    /// DISTRIBUTED KEY GENERATION — CONTRIBUTE (Day 126–130). THIS node is a DEALER in a fresh t-of-m
    /// ceremony. Authorization is the OPERATOR SEAL (sealed to THIS node's recipient), bound to
    /// `dkg_aad(kid, dkg_id, node_set, t, m)` — checked first. It then draws its FRESH degree-(t−1)
    /// polynomial `f_i` (constant term `c_i` + (t−1) higher coefficients, all master-derived +
    /// ceremony-bound via [`NodeAuthority::dkg_polynomial`]), evaluates `f_i` at each member
    /// coordinate `x_j = 1..=m`, and seals the sub-share `dealer_x ‖ f_i(x_j)` to member `j` (signed
    /// by this node, AEAD-bound to the dealer→target pair). The private contribution `c_i` never
    /// leaves the node; the CEK `⊕_i c_i` is assembled nowhere.
    fn dkg_contribute(&self, args: DkgContributeArgs) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => return Response::error("not_configured", "dkms-authority node is not initialized (send `init` first)"),
        };
        let operator_verifier = match self.pinned_operator_verifier() {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let kid16 = match decode_kid_bytes16(&args.kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        if args.t < 2 {
            return Response::error("invalid_request", "DKG threshold t must be at least 2");
        }
        if args.dealer_x == 0 {
            return Response::error("invalid_request", "dealer coordinate must be non-zero (x=0 is the secret position)");
        }
        if args.cek_len == 0 {
            return Response::error("invalid_request", "cek_len must be non-zero");
        }
        if args.member_recipient_pubs_b64.len() != args.m as usize || args.m < args.t {
            return Response::error("invalid_request", "member_recipient_pubs_b64 must list exactly m recipients with m ≥ t");
        }
        let dkg_id = match b64().decode(&args.dkg_id_b64) {
            Ok(b) if !b.is_empty() => b,
            _ => return Response::error("invalid_request", "dkg_id_b64 must be non-empty base64"),
        };
        let node_set_id = match b64().decode(&args.node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "node_set_id_b64 is not valid base64"),
        };
        let dkg_aad = ddrm_envelope::dkg_aad(&kid16, &dkg_id, &node_set_id, args.t, args.m);
        if let Err(resp) = self.open_operator_auth(&args.operator_auth_b64, &dkg_aad, &operator_verifier) {
            return resp;
        }
        // Fresh degree-(t−1) polynomial: coeff[0] = the private contribution c_i, coeff[1..t] higher.
        let len = args.cek_len as usize;
        let coeffs = authority.dkg_polynomial(&dkg_id, args.t as usize, len);
        let contribution = zeroize::Zeroizing::new(coeffs[0].clone());
        let higher_refs: Vec<&[u8]> = coeffs[1..].iter().map(|c| c.as_slice()).collect();

        let mut subshares = Vec::with_capacity(args.m as usize);
        for (j, recipient_b64) in args.member_recipient_pubs_b64.iter().enumerate() {
            let target_x = (j + 1) as u8;
            let recipient_bytes = match b64().decode(recipient_b64) {
                Ok(b) => b,
                Err(_) => return Response::error("invalid_request", "a member recipient key is not valid base64"),
            };
            let recipient_public = match ddrm_envelope::session_public_from_bytes(&recipient_bytes) {
                Some(p) => p,
                None => return Response::error("invalid_request", "a member recipient key is not a valid escrow recipient"),
            };
            // f_i(x_j) = c_i ⊕ Σ_{d≥1} coeff[d]·x_j^d — reshare_eval evaluates exactly this polynomial.
            let sub_body = match ddrm_envelope::reshare_eval(contribution.as_slice(), &higher_refs, target_x) {
                Ok(s) => zeroize::Zeroizing::new(s),
                Err(e) => return Response::error("invalid_request", e),
            };
            // The sub-share carries this DEALER's coordinate (used to authenticate + dedupe).
            let payload = zeroize::Zeroizing::new(ddrm_envelope::indexed_share(args.dealer_x, &sub_body));
            let aad = ddrm_envelope::dkg_subshare_aad(&kid16, &dkg_id, &node_set_id, args.dealer_x, target_x);
            let sealed = ddrm_envelope::seal::seal_bound(&recipient_public, payload.as_slice(), &aad, &authority.signer);
            subshares.push(json!({
                "target_x": target_x,
                "sealed_subshare_b64": b64().encode(sealed.to_bytes()),
            }));
        }
        Response::ok(json!({
            "dealer_x": args.dealer_x,
            "dealer_vk_b64": b64().encode(&authority.verifying_key),
            "subshares": subshares,
        }))
    }

    /// DISTRIBUTED KEY GENERATION — INSTALL (Day 126–130). THIS node is a MEMBER assembling its share
    /// of the DKG-born key. Authorization is the OPERATOR SEAL (sealed to THIS node's recipient),
    /// bound to `dkg_aad(kid, dkg_id, node_set, t, m)` — checked first. It then unwraps each dealer's
    /// sub-share (verified under the dealer's identity, AEAD-bound to the dealer→THIS-node pair, the
    /// inner coordinate matching the declared dealer — a tampered/forged/redirected sub-share is
    /// refused and the dealer NAMED), and SUMS them over GF(256) into its final share
    /// `F(x_j) = ⊕_i f_i(x_j)` ([`ddrm_envelope::dkg_sum_subshares`]). It RE-ESCROWS its indexed share
    /// `x_j ‖ F(x_j)` to ITSELF (signed by this node, the new escrow's producer identity the t-of-m
    /// boundary verifies). The CEK never exists here; a single member share reveals nothing of `F(0)`.
    fn dkg_install(&self, args: DkgInstallArgs) -> Response {
        let authority = match self.authority.as_ref() {
            Some(authority) => authority,
            None => return Response::error("not_configured", "dkms-authority node is not initialized (send `init` first)"),
        };
        let operator_verifier = match self.pinned_operator_verifier() {
            Ok(v) => v,
            Err(resp) => return resp,
        };
        let kid16 = match decode_kid_bytes16(&args.kid_hex) {
            Ok(k) => k,
            Err(e) => return Response::error("invalid_request", e),
        };
        if args.t < 2 || args.target_x == 0 {
            return Response::error("invalid_request", "DKG install needs t ≥ 2 and a non-zero target coordinate");
        }
        // A member's share is the sum of EVERY dealer's contribution; fewer than m dealers means the
        // CEK is missing addends. Require the full declared dealer set (m distinct dealers).
        if args.contributions.len() != args.m as usize {
            return Response::error("invalid_request", "DKG install needs exactly one sub-share from each of the m dealers");
        }
        let dkg_id = match b64().decode(&args.dkg_id_b64) {
            Ok(b) if !b.is_empty() => b,
            _ => return Response::error("invalid_request", "dkg_id_b64 must be non-empty base64"),
        };
        let node_set_id = match b64().decode(&args.node_set_id_b64) {
            Ok(b) => b,
            Err(_) => return Response::error("invalid_request", "node_set_id_b64 is not valid base64"),
        };
        let dkg_aad = ddrm_envelope::dkg_aad(&kid16, &dkg_id, &node_set_id, args.t, args.m);
        if let Err(resp) = self.open_operator_auth(&args.operator_auth_b64, &dkg_aad, &operator_verifier) {
            return resp;
        }
        // Unwrap + authenticate each dealer's sub-share, then SUM (not Lagrange — DKG members sum the
        // dealers' polynomials evaluated at THIS coordinate).
        let mut seen_dealers: Vec<u8> = Vec::with_capacity(args.contributions.len());
        let mut bodies: Vec<zeroize::Zeroizing<Vec<u8>>> = Vec::with_capacity(args.contributions.len());
        for c in &args.contributions {
            if c.dealer_x == 0 {
                return Response::error("invalid_request", "a dealer coordinate is zero (the secret position is never a dealer)");
            }
            if seen_dealers.contains(&c.dealer_x) {
                return Response::error("invalid_request", "duplicate dealer coordinate — a dealer cannot contribute twice");
            }
            let vk = match b64().decode(&c.dealer_vk_b64) {
                Ok(b) => b,
                Err(_) => return Response::error("invalid_request", "a dealer verifying key is not valid base64"),
            };
            let verifier = match ddrm_envelope::MlDsa65Verifier::from_encoded(&vk) {
                Some(v) => v,
                None => return Response::error("invalid_request", "a dealer verifying key is malformed"),
            };
            let env = match b64().decode(&c.sealed_subshare_b64).ok().and_then(|b| ddrm_envelope::PqSealedEnvelope::from_bytes(&b).ok()) {
                Some(env) => env,
                None => return Response::error("invalid_request", "a sealed sub-share is not a valid envelope"),
            };
            let aad = ddrm_envelope::dkg_subshare_aad(&kid16, &dkg_id, &node_set_id, c.dealer_x, args.target_x);
            let payload = match ddrm_envelope::hybrid_unwrap_bound(&authority.recipient_secret, &env, &aad, &verifier) {
                Ok(p) => p,
                Err(_) => return Response::error("access_denied", "a DKG sub-share did not open under THIS node for its declared dealer→target pair (forged, tampered, or redirected — the dealer is named by its coordinate/key)"),
            };
            let (inner_x, body) = match ddrm_envelope::parse_indexed_share(&payload) {
                Some((x, body)) => (x, body),
                None => return Response::error("invalid_request", "a sub-share carries no valid dealer coordinate"),
            };
            if inner_x != c.dealer_x {
                return Response::error("invalid_request", "a sub-share's sealed coordinate disagrees with its declared dealer");
            }
            seen_dealers.push(c.dealer_x);
            bodies.push(zeroize::Zeroizing::new(body.to_vec()));
        }
        let body_refs: Vec<&[u8]> = bodies.iter().map(|b| b.as_slice()).collect();
        let new_share = match ddrm_envelope::dkg_sum_subshares(&body_refs) {
            Ok(s) => s,
            Err(e) => return Response::error("invalid_request", e),
        };
        // Re-escrow `target_x ‖ F(target_x)` to THIS node under the shared escrow AAD, signed by it.
        let indexed = zeroize::Zeroizing::new(ddrm_envelope::indexed_share(args.target_x, new_share.as_slice()));
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(&args.scheme, &kid16, &authority.recipient_public);
        let escrow = ddrm_envelope::seal::seal_bound(
            &ddrm_envelope::session_public_from_bytes(&authority.recipient_public).expect("node recipient is a valid escrow key"),
            indexed.as_slice(),
            &escrow_aad,
            &authority.signer,
        );
        Response::ok(json!({
            "target_x": args.target_x,
            "wrapped_cek_b64": b64().encode(escrow.to_bytes()),
            "escrow_producer_vk_b64": b64().encode(&authority.verifying_key),
        }))
    }

    /// The pinned operator verifier or a fail-closed response (shared by every lifecycle op).
    fn pinned_operator_verifier(&self) -> Result<ddrm_envelope::MlDsa65Verifier, Response> {
        let operator_vk = self.operator_vk.as_ref().ok_or_else(|| {
            Response::error("not_configured", "this node has no pinned operator identity — lifecycle ops are refused (provision DKMS_AUTHORITY_OPERATOR_VK)")
        })?;
        ddrm_envelope::MlDsa65Verifier::from_encoded(operator_vk)
            .ok_or_else(|| Response::error("not_configured", "pinned operator identity is malformed"))
    }

    /// Open an operator-sealed authorization envelope under this node's recipient secret, verified
    /// under the pinned operator identity and AEAD-bound to `expected_aad`. Fail-closed on any
    /// mismatch (forged signer, tampered bytes, wrong reconfiguration context). The payload is
    /// discarded — the AUTHORIZATION is the successful, identity-verified, context-bound open itself.
    fn open_operator_auth(
        &self,
        auth_b64: &str,
        expected_aad: &[u8],
        operator_verifier: &ddrm_envelope::MlDsa65Verifier,
    ) -> Result<(), Response> {
        let authority = self.authority.as_ref().expect("authority checked by caller");
        let env = b64()
            .decode(auth_b64)
            .ok()
            .and_then(|b| ddrm_envelope::PqSealedEnvelope::from_bytes(&b).ok())
            .ok_or_else(|| Response::error("invalid_request", "operator_auth_b64 is not a valid sealed envelope"))?;
        ddrm_envelope::hybrid_unwrap_bound(&authority.recipient_secret, &env, expected_aad, operator_verifier)
            .map(|_| ())
            .map_err(|_| {
                Response::error(
                    "access_denied",
                    "reconfiguration refused: the authorization does not open under the pinned operator identity for THIS (kid, old set, new set, k, m)",
                )
            })
    }
}

#[derive(Clone)]
struct ReshareContributeArgs {
    wrapped_cek_b64: String,
    scheme: String,
    kid_hex: String,
    producer_vk_b64: String,
    operator_auth_b64: String,
    old_node_set_id_b64: String,
    new_node_set_id_b64: String,
    k: u8,
    m: u8,
    new_recipient_pubs_b64: Vec<String>,
}

struct ReshareInstallArgs {
    operator_auth_b64: String,
    old_node_set_id_b64: String,
    new_node_set_id_b64: String,
    k: u8,
    m: u8,
    target_x: u8,
    scheme: String,
    kid_hex: String,
    contributions: Vec<ReshareContribution>,
}

struct DkgContributeArgs {
    operator_auth_b64: String,
    dkg_id_b64: String,
    node_set_id_b64: String,
    t: u8,
    m: u8,
    dealer_x: u8,
    kid_hex: String,
    cek_len: u32,
    member_recipient_pubs_b64: Vec<String>,
}

struct DkgInstallArgs {
    operator_auth_b64: String,
    dkg_id_b64: String,
    node_set_id_b64: String,
    t: u8,
    m: u8,
    target_x: u8,
    kid_hex: String,
    scheme: String,
    contributions: Vec<DkgContribution>,
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
    attest_node_set_id_b64: Option<String>,
    attest_expiry: Option<u64>,
    access_grant: Option<ddrm_envelope::access::AccessGrantV1>,
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
    // SECURITY-EXPIRY: validate against the node's own clock, not the caller's `now_unix`, so a
    // captured token can't be kept alive past its window by a backdated caller clock.
    if security_now(args.now_unix) > token.expires_at {
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
/// THE AUTHORIZATION GATE. When the caller supplies a wallet-signed [`AccessGrantV1`],
/// the node authorizes TRUSTLESSLY: it verifies the wallet signature (EIP-191/1271) +
/// the session-key request signature + the window/nonce/node-set/kid binding ITSELF, then
/// evaluates `hasAccessByContentId` per covered address against its OWN pinned Base RPC
/// pool — no enrollment, no trusted gateway. When no grant is supplied it falls back to the
/// legacy unsigned-receipt path iff `legacy-receipt-authz` is enabled (a migration scaffold;
/// disabled ⇒ a missing grant fails closed). Every path FAILS CLOSED.
fn authorize(args: &RecoverArgs, legacy_receipt_allowed: bool) -> Result<(), String> {
    if let Some(grant) = args.access_grant.as_ref() {
        let chain = node_chain::NodeChain::from_env().ok_or(
            "trustless authorization requires a configured node chain capability (DKMS_CHAIN_RPC_POOL)",
        )?;
        // The node enforces ITS OWN quorum identity (anti cross-quorum replay): a grant minted for a
        // DIFFERENT node-set must not authorize a recover here. In RELEASE builds the pin is MANDATORY
        // (pre-audit #4) — absent it, the node would have to trust the grant's caller-declared node-set,
        // which the attacker controls, so we FAIL CLOSED. Tests/dev-modes fall back to the grant's
        // declared id for deterministic single-set fixtures (the grant is still wallet- + chain-bound).
        let pinned_ns = std::env::var("DKMS_AUTHORITY_NODE_SET_ID_B64")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let expected_ns = match pinned_ns {
            Some(ns) => ns,
            None => {
                #[cfg(not(any(test, feature = "dev-modes")))]
                {
                    return Err(
                        "DKMS_AUTHORITY_NODE_SET_ID_B64 must be set in release builds: the node refuses to authorize against a caller-declared node-set (cross-quorum replay defense)"
                            .to_string(),
                    );
                }
                #[cfg(any(test, feature = "dev-modes"))]
                {
                    grant.delegation.node_set_id_b64.clone()
                }
            }
        };
        // SECURITY-EXPIRY uses the node's own clock (not the caller's) so an expired/revoked
        // delegation can't be kept alive by a backdated `now_unix`.
        let now = security_now(args.now_unix);
        // Wire the process-held ReplayGuard so the per-request nonce is single-use and a
        // revoked delegation is refused (PC2 parity). Recover from a poisoned lock rather than
        // panicking — a single bad recover must not wedge the node.
        let mut guard = replay_guard().lock().unwrap_or_else(|e| e.into_inner());
        return node_chain::authorize_access(
            grant,
            &expected_ns,
            chain.chain_id(),
            &args.kid_hex,
            now,
            Some(&mut guard),
            &chain,
        );
    }
    // No wallet-signed grant. The legacy receipt is trusted ONLY for an enrolled (allow-listed)
    // caller; an anonymous caller MUST present a grant (it cannot forge `allowed:true`).
    if !legacy_receipt_allowed {
        return Err(
            "anonymous caller must present a wallet-signed access grant — the node has no allow-list, so the legacy unsigned-receipt path is closed (W3/D4)"
                .to_string(),
        );
    }
    #[cfg(feature = "legacy-receipt-authz")]
    {
        reauthorize(args)
    }
    #[cfg(not(feature = "legacy-receipt-authz"))]
    {
        Err("no wallet-signed access grant supplied and legacy receipt authorization is disabled".to_string())
    }
}

/// LEGACY (pre-trustless) authorization: re-check the unsigned `RightsDecisionReceiptV1` in the
/// node's own boundary. Retained behind `legacy-receipt-authz` for the migration window; a present
/// [`AccessGrantV1`] always supersedes this. See [`authorize`].
#[cfg_attr(not(feature = "legacy-receipt-authz"), allow(dead_code))]
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
    // The pinned OPERATOR identity (Day 109–112), resolved ONCE at daemon startup. Lifecycle ops
    // (rotate_share / revoke_caller) are refused without it. REVOCATIONS are daemon-lifetime state
    // shared across connections — a caller revoked on one connection stays revoked on the next.
    let operator_vk = operator_vk_from_env();
    if operator_vk.is_some() {
        eprintln!("dkms-authority: operator identity pinned (lifecycle ops enabled)");
    }
    eprintln!("dkms-authority: listening on {path}");
    serve_unix_listener(listener, allowed_callers, operator_vk);
}

/// A transport an accepted connection can be served over (Unix or TCP). Abstracts the two
/// otherwise-identical accept loops behind one generic [`serve_accept_loop`], so the per-connection
/// lifecycle — read-timeout arming, reader split, concurrency cap, thread spawn — lives in EXACTLY
/// one place and can never diverge between the host-local and hostile-network transports.
#[cfg(unix)]
trait AcceptedConn: io::Write + Send + Sized + 'static {
    /// An independent, buffered read handle over the same connection.
    type Reader: io::Read + Send + 'static;
    /// Bound every read on this connection (idle/stall insurance, ELACITY-2282 Defect A).
    fn arm_read_timeout(&self);
    /// A buffered reader over a clone of this connection (the write half stays on `self`).
    fn split_reader(&self) -> io::Result<Self::Reader>;
}

#[cfg(unix)]
impl AcceptedConn for std::os::unix::net::UnixStream {
    type Reader = io::BufReader<std::os::unix::net::UnixStream>;
    fn arm_read_timeout(&self) {
        let _ = self.set_read_timeout(Some(CONNECTION_READ_TIMEOUT));
    }
    fn split_reader(&self) -> io::Result<Self::Reader> {
        Ok(io::BufReader::new(self.try_clone()?))
    }
}

#[cfg(unix)]
impl AcceptedConn for std::net::TcpStream {
    type Reader = io::BufReader<std::net::TcpStream>;
    fn arm_read_timeout(&self) {
        let _ = self.set_read_timeout(Some(CONNECTION_READ_TIMEOUT));
    }
    fn split_reader(&self) -> io::Result<Self::Reader> {
        Ok(io::BufReader::new(self.try_clone()?))
    }
}

/// RAII guard for one slot in the [`MAX_ACTIVE_CONNECTIONS`] budget: decrements the active-connection
/// counter when a served connection ends — normal return OR panic — so the cap can never leak slots.
#[cfg(unix)]
struct ActiveSlot(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[cfg(unix)]
impl Drop for ActiveSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The shared accept loop for BOTH transports (ELACITY-2282). Each accepted connection is served on
/// its OWN thread so a single idle/slow/leaked client can never head-of-line-block the others — the
/// daemon always returns to `accept`. The daemon-lifetime revoked-caller set is shared LIVE across
/// every connection thread (one `Arc`, see [`RevokedSet`]), so a revocation binds every open
/// connection immediately. Concurrency is bounded by [`MAX_ACTIVE_CONNECTIONS`]: past the cap, a new
/// connection is dropped rather than spawning an unbounded thread, so a slow-loris peer cannot
/// exhaust the node. `require_channel` distinguishes the hostile-network TCP transport (a plaintext
/// recover is refused) from the host-local Unix transport.
#[cfg(unix)]
fn serve_accept_loop<S, I>(
    incoming: I,
    allowed_callers: Option<Vec<Vec<u8>>>,
    operator_vk: Option<Vec<u8>>,
    require_channel: bool,
) where
    S: AcceptedConn,
    I: IntoIterator<Item = io::Result<S>>,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let allowed_callers = Arc::new(allowed_callers);
    let operator_vk = Arc::new(operator_vk);
    let revoked_callers: RevokedSet = RevokedSet::default();
    let active = Arc::new(AtomicUsize::new(0));
    for stream in incoming {
        let stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("dkms-authority: accept error: {err}");
                continue;
            }
        };
        stream.arm_read_timeout();
        let reader = match stream.split_reader() {
            Ok(reader) => reader,
            Err(err) => {
                eprintln!("dkms-authority: connection clone failed: {err}");
                continue;
            }
        };
        // CONCURRENCY CAP (ELACITY-2282 hardening): claim a slot atomically. At the cap, roll the
        // claim back and DROP this connection (the reader + stream close on scope exit) rather than
        // spawn an unbounded thread — a slow-loris peer can hold at most the cap, not exhaust us.
        if active.fetch_add(1, Ordering::AcqRel) >= MAX_ACTIVE_CONNECTIONS {
            active.fetch_sub(1, Ordering::AcqRel);
            eprintln!(
                "dkms-authority: connection cap reached ({MAX_ACTIVE_CONNECTIONS}) — dropping connection"
            );
            continue;
        }
        let slot = ActiveSlot(Arc::clone(&active));
        let allowed = Arc::clone(&allowed_callers);
        let operator = Arc::clone(&operator_vk);
        let revoked = revoked_callers.clone();
        std::thread::spawn(move || {
            let _slot = slot; // released (counter decremented) when this connection thread ends
            serve_connection_io(reader, stream, &allowed, &operator, &revoked, require_channel);
        });
    }
}

/// The Unix accept loop, factored out of [`serve_socket`] so it can be driven over a test-owned
/// listener. The Unix transport is host-local (filesystem-permissioned), so the encrypted channel is
/// OPTIONAL (`require_channel = false`): a client that offers a channel key still gets one.
#[cfg(unix)]
fn serve_unix_listener(
    listener: std::os::unix::net::UnixListener,
    allowed_callers: Option<Vec<Vec<u8>>>,
    operator_vk: Option<Vec<u8>>,
) {
    serve_accept_loop(listener.incoming(), allowed_callers, operator_vk, false);
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
    let operator_vk = operator_vk_from_env();
    if operator_vk.is_some() {
        eprintln!("dkms-authority: operator identity pinned (lifecycle ops enabled)");
    }
    eprintln!("dkms-authority: listening on tcp:{addr}");
    serve_tcp_listener(listener, allowed_callers, operator_vk);
}

/// The TCP accept loop, factored out of [`serve_tcp`]. Like the Unix loop it serves each connection
/// on its OWN thread (ELACITY-2282); every recover on this hostile transport still REQUIRES the
/// encrypted, mutually-authenticated channel (`require_channel = true`).
#[cfg(unix)]
fn serve_tcp_listener(
    listener: std::net::TcpListener,
    allowed_callers: Option<Vec<Vec<u8>>>,
    operator_vk: Option<Vec<u8>>,
) {
    serve_accept_loop(listener.incoming(), allowed_callers, operator_vk, true);
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

/// Parse the pinned OPERATOR identity from `OPERATOR_VK_ENV` (base64 ML-DSA verifying key).
/// `None` when unset/empty/malformed — the lifecycle ops then fail closed (`not_configured`).
#[cfg(unix)]
fn operator_vk_from_env() -> Option<Vec<u8>> {
    let raw = std::env::var(OPERATOR_VK_ENV).ok()?;
    let vk = b64().decode(raw.trim()).ok()?;
    ddrm_envelope::MlDsa65Verifier::from_encoded(&vk)?;
    Some(vk)
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
    operator_vk: &Option<Vec<u8>>,
    revoked_callers: &RevokedSet,
    require_channel: bool,
) {
    use ddrm_envelope::frame::{read_frame, write_frame};
    let mut node = DkmsAuthorityNode {
        allowed_callers: allowed_callers.clone(),
        operator_vk: operator_vk.clone(),
        // Share the ONE daemon-lifetime revoked set (an `Arc` clone, not a snapshot): revocations
        // this connection performs are visible to every other open connection immediately, and
        // revocations they perform are visible here immediately.
        revoked_callers: revoked_callers.clone(),
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
                    // Tolerant unpad: accept a padded (padding-aware client) OR an un-padded
                    // (legacy client) request, so the node interoperates regardless of rollout
                    // order. Integrity is the seal above; padding is metadata-hiding only.
                    Ok(opened) => ddrm_envelope::channel_pad::unpad_incoming(&opened),
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
        // FAIL-CLOSED TRANSPORT GATE: on a network transport, a recover — and the lifecycle ops,
        // which move rotated share escrows + operator instructions (Day 109–112) — NEVER travel in
        // plaintext: no channel, no service. (init/hello/status are public-protocol messages; the
        // secrets these ops move get channel confidentiality.)
        if require_channel
            && channel.is_none()
            && matches!(
                request,
                Request::Recover { .. }
                    | Request::RotateShare { .. }
                    | Request::RevokeCaller { .. }
                    | Request::ReshareContribute { .. }
                    | Request::ReshareInstall { .. }
                    | Request::DkgContribute { .. }
                    | Request::DkgInstall { .. }
            )
        {
            let resp = Response::error(
                "channel_required",
                "this transport requires the encrypted channel: re-run `hello` with a channel_pub_b64 first",
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
    // No write-back needed: `node.revoked_callers` IS the shared daemon-lifetime set (an `Arc`
    // clone), so every revocation was already published to all connections the instant it landed.
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
            // Optionally pad the plaintext to a size bucket BEFORE sealing so the on-wire frame
            // length reveals only the bucket (pre-audit #5 metadata minimization). OFF by default
            // and emitted only when ELASTOS_DKMS_CHANNEL_PAD is set across a padding-aware quorum —
            // a legacy client cannot strip a padded response.
            let padded = ddrm_envelope::channel_pad::pad_outgoing(&bytes);
            let env = ddrm_envelope::seal::seal_bound(&ch.client_pub, &padded, &aad, &authority.signer);
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

    /// A SHORT temp Unix-socket path — macOS `sun_path` is only 104 bytes and `temp_dir()` is long,
    /// so bind under `/tmp` to stay well under the limit.
    #[cfg(unix)]
    fn unique_socket_path(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("/tmp/dkms-{tag}-{}-{nanos}.sock", std::process::id())
    }

    /// ELACITY-2282 (regression): a single idle client must NOT head-of-line-block the accept loop.
    /// Drive the real Unix accept loop (`serve_unix_listener`) with connection A held idle — it never
    /// sends a frame, so its serving read blocks — and assert a SECOND client B is still served
    /// promptly. Before the thread-per-connection fix, A wedged the sequential loop and B starved in
    /// the kernel backlog until the client's hard cap → fail-closed quorum.
    #[test]
    #[cfg(unix)]
    fn idle_connection_does_not_block_other_clients() {
        use ddrm_envelope::frame::{read_frame, write_frame};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::time::Duration;

        let path = unique_socket_path("hol");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        // Anonymous (no allow-list), no operator — `status` is a public message that answers here.
        std::thread::spawn(move || serve_unix_listener(listener, None, None));

        // Client A connects and stays IDLE (sends nothing): its serving thread blocks in read_frame.
        let idle = UnixStream::connect(&path).unwrap();
        std::thread::sleep(Duration::from_millis(150)); // let A be accepted + block

        // Client B must still be served promptly while A is idle.
        let mut b = UnixStream::connect(&path).unwrap();
        b.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        write_frame(&mut b, &serde_json::to_vec(&json!({ "op": "status" })).unwrap()).unwrap();
        let resp = read_frame(&mut b)
            .expect("client B must be served while A is idle (no head-of-line block)")
            .expect("a framed response");
        let resp: Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(resp["status"].as_str().unwrap(), "ok");
        assert_eq!(resp["data"]["provider"].as_str().unwrap(), "dkms-authority");

        drop(idle);
        let _ = std::fs::remove_file(&path);
    }

    /// ELACITY-2282 (regression, shared-live revocation): a revoke performed on ONE connection must
    /// bind every OTHER already-open connection IMMEDIATELY — not only after the revoking connection
    /// closes. Two `DkmsAuthorityNode`s sharing the daemon-lifetime revoked set model two concurrent
    /// connections exactly as `serve_accept_loop` wires them (one shared `Arc`). Before the fix each
    /// connection held a private snapshot merged back only on close, so a revoked caller holding a
    /// warm pooled connection kept being served until the operator's connection disconnected.
    #[test]
    #[cfg(unix)]
    fn revocation_binds_other_open_connections_immediately() {
        let shared: RevokedSet = RevokedSet::default();
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        let (_caller_signer, caller_vk) = caller_keypair();
        let caller_vk_b64 = b64().encode(&caller_vk);

        // Connection B: the caller's own connection — initialized and serving its `hello`.
        let store = unique_store("revoke-shared-b");
        let mut conn_b = DkmsAuthorityNode { revoked_callers: shared.clone(), ..Default::default() };
        conn_b.init(json!({ "authority_key_store": store.clone() }));
        let challenge = b64().encode([0xA1u8; 32]);
        assert!(
            matches!(conn_b.hello(&challenge, &caller_vk_b64, Some(NOW), None), Response::Ok { .. }),
            "before revocation the caller is served",
        );

        // Connection A: the operator's admin connection — revokes the caller while B stays OPEN.
        let mut conn_a = DkmsAuthorityNode {
            operator_vk: Some(operator_vk),
            revoked_callers: shared.clone(),
            ..Default::default()
        };
        let sig = b64().encode(ddrm_envelope::sign_revocation(&operator, &caller_vk));
        assert!(
            matches!(conn_a.revoke_caller(&caller_vk_b64, &sig), Response::Ok { .. }),
            "the pinned operator's genuine revocation is accepted",
        );

        // Connection B — never closed — must refuse the caller's next `hello` at once.
        assert_eq!(
            error_code(&conn_b.hello(&challenge, &caller_vk_b64, Some(NOW), None)),
            "caller_revoked",
            "a revoke on connection A must bind the still-open connection B immediately",
        );

        let _ = std::fs::remove_file(&store);
    }

    /// ELACITY-2282 (regression, connection-cap leak-safety): the [`ActiveSlot`] guard that bounds
    /// concurrent connection threads via [`MAX_ACTIVE_CONNECTIONS`] must decrement the counter when a
    /// connection ends — on a normal return AND when the serving thread panics — or the cap would
    /// leak slots and eventually refuse every client (a self-inflicted outage). Without the RAII
    /// guard a panicking connection would permanently consume a slot.
    #[test]
    #[cfg(unix)]
    fn active_slot_releases_on_drop_and_on_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let active = Arc::new(AtomicUsize::new(0));

        active.fetch_add(1, Ordering::AcqRel);
        {
            let _slot = ActiveSlot(Arc::clone(&active));
            assert_eq!(active.load(Ordering::Acquire), 1);
        }
        assert_eq!(active.load(Ordering::Acquire), 0, "slot released on normal drop");

        active.fetch_add(1, Ordering::AcqRel);
        let claimed = Arc::clone(&active);
        let joined = std::thread::spawn(move || {
            let _slot = ActiveSlot(claimed);
            panic!("connection thread panicked mid-serve");
        })
        .join();
        assert!(joined.is_err(), "the worker thread panicked as set up");
        assert_eq!(
            active.load(Ordering::Acquire),
            0,
            "slot must release even when the connection thread panics",
        );
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
    #[cfg(feature = "legacy-receipt-authz")]
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
    /// DEV_MODE_GUARD_SPEC: the secure default build does NOT compile the unsigned legacy-receipt
    /// path, so a missing wallet-signed grant fails closed regardless of any allow-list — the
    /// audit's MED→closed assertion for the node. The positive legacy-path tests above are gated
    /// behind `legacy-receipt-authz` (enabled by `--features dev-modes`) and prove migration parity.
    #[test]
    #[cfg(not(feature = "legacy-receipt-authz"))]
    fn release_build_fences_out_the_legacy_receipt_path() {
        assert!(
            !cfg!(feature = "legacy-receipt-authz"),
            "a release build must not compile the legacy unsigned-receipt authorization path"
        );
    }

    #[test]
    #[cfg(feature = "legacy-receipt-authz")]
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
        node.allowed_callers = Some(vec![caller_vk.clone()]);
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
            attest_node_set_id_b64: None,
            attest_expiry: None,
            access_grant: None,
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

    /// Day 131–135 — a real node `recover` co-signs a release attestation bound to THIS grant +
    /// session + node-set, and the standalone offline verifier accepts it as a (1-of-1) quorum proof
    /// while rejecting a wrong-principal check. Proves the node-side half of the portable proof.
    #[test]
    #[cfg(feature = "legacy-receipt-authz")]
    fn recover_co_signs_a_release_attestation_the_offline_verifier_accepts() {
        let store = unique_store("attest");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let recipient_pub_b64 = init["seal_recipient_pub_b64"].as_str().unwrap().to_string();
        let recipient_pub = b64().decode(&recipient_pub_b64).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        let node_vk = b64().decode(init["seal_verifying_key_b64"].as_str().unwrap()).unwrap();

        // The node-set this open is served by (a 1-of-1 set keyed on the node's own vk).
        let t = 1u8;
        let members: Vec<&[u8]> = vec![&node_vk];
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(t, &members);
        let expiry = NOW + 3_600;

        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
        let cek: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC6u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow_aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &cek, &escrow_aad, &producer_signer);

        let (_session_secret, session_public) = ddrm_envelope::mint_session();
        let session_pub_b64 = b64().encode(ddrm_envelope::session_public_bytes(&session_public));
        let session_pub = b64().decode(&session_pub_b64).unwrap();

        let (caller, caller_vk) = caller_keypair();
        node.allowed_callers = Some(vec![caller_vk.clone()]);
        let token = live_token(&node, &b64().encode(&caller_vk));
        let caller_sig_b64 = proof_for(&caller, &token, CONTENT, &kid_hex, &session_pub_b64, 1);
        let data = ok_data(node.recover(RecoverArgs {
            wrapped_cek_b64: b64().encode(wrapped.to_bytes()),
            scheme: scheme.to_string(),
            kid_hex,
            producer_vk_b64: b64().encode(&producer_vk),
            decrypt_session_pub_b64: session_pub_b64.clone(),
            aad_b64: b64().encode(b"attest-transcript"),
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
            attest_node_set_id_b64: Some(b64().encode(node_set_id)),
            attest_expiry: Some(expiry),
            access_grant: None,
        }));

        // The node emitted a co-signed attestation; the offline verifier accepts the 1-of-1 quorum.
        let att = b64().decode(data["release_attestation_b64"].as_str().expect("attestation present")).unwrap();
        assert_eq!(data["release_attestation_expiry"].as_u64(), Some(expiry));
        assert_eq!(
            ddrm_envelope::verify_quorum_release_proof(
                t, &members, &node_set_id, CONTENT.as_bytes(), PRINCIPAL.as_bytes(), RIGHT.as_bytes(),
                &session_pub, &kid16, expiry, NOW, &[(0, &att)],
            ),
            Ok(1),
            "the node's co-signed attestation verifies offline as a quorum proof"
        );
        // Wrong-principal check fails closed and names the node.
        assert_eq!(
            ddrm_envelope::verify_quorum_release_proof(
                t, &members, &node_set_id, CONTENT.as_bytes(), b"principal:not-alice", RIGHT.as_bytes(),
                &session_pub, &kid16, expiry, NOW, &[(0, &att)],
            ),
            Err(ddrm_envelope::QuorumProofError::BadSignature { member_index: 0 }),
            "the attestation does not authorize a different principal"
        );

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
    #[cfg(feature = "legacy-receipt-authz")]
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
        node.allowed_callers = Some(vec![caller_vk.clone()]);
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
            attest_node_set_id_b64: None,
            attest_expiry: None,
            access_grant: None,
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
            attest_node_set_id_b64: None,
            attest_expiry: None,
            access_grant: None,
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
        node.allowed_callers = Some(vec![caller_vk.clone()]);
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
            attest_node_set_id_b64: None,
            attest_expiry: None,
            access_grant: None,
        };
        (node, args, caller)
    }

    /// W3/D4 — an ANONYMOUS caller (the node has no allow-list) presenting a valid live session +
    /// a perfectly-formed `allowed:true` receipt but NO wallet-signed grant is REFUSED. This is the
    /// safety property that lets the allow-list be dropped as the security boundary: an unenrolled
    /// runtime cannot forge authorization — it must present a grant the node verifies + a chain
    /// token the node reads. (The "WITH a valid grant succeeds" half is `node_chain`'s
    /// `authorize_allows_when_grant_valid_and_chain_true`, which consults no allow-list at all.)
    #[test]
    fn anonymous_caller_without_a_grant_is_refused() {
        let store = unique_store("anon-no-grant");
        let (mut node, args, _caller) = setup_recover(&store);
        node.allowed_callers = None; // drop enrollment → caller is anonymous
        let resp = node.recover(args);
        assert_eq!(
            error_code(&resp),
            "access_denied",
            "an anonymous caller cannot authorize with an unsigned receipt — a grant is required"
        );
        let _ = std::fs::remove_file(&store);
    }

    /// W3/D4 — an ENROLLED (allow-listed) caller may still use the legacy unsigned-receipt path
    /// during the migration window (feature `legacy-receipt-authz`). The allow-list is now a TRUST
    /// scope for the legacy path, not the system-wide security boundary.
    #[test]
    #[cfg(feature = "legacy-receipt-authz")]
    fn enrolled_caller_legacy_receipt_path_still_authorizes() {
        let store = unique_store("anon-enrolled");
        let (mut node, args, _caller) = setup_recover(&store); // setup_recover enrolls the caller
        let resp = node.recover(args);
        assert!(
            matches!(resp, Response::Ok { .. }),
            "an enrolled caller + a valid receipt recovers via the legacy path: {resp:?}"
        );
        let _ = std::fs::remove_file(&store);
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
    #[cfg(feature = "legacy-receipt-authz")]
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
    #[cfg(feature = "legacy-receipt-authz")]
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
    #[cfg(feature = "legacy-receipt-authz")]
    fn framed_connection_serves_a_full_session_and_drops_a_torn_frame() {
        use ddrm_envelope::frame::{read_frame, write_frame};
        use std::os::unix::net::UnixStream;

        let store = unique_store("framed");

        // ---- Happy path: a full framed session over one connection. The caller is ENROLLED on the
        // node's allow-list so the legacy receipt path authorizes (the TRANSPORT, not authZ, is the
        // subject of this test). ----
        let (caller, caller_vk) = caller_keypair();
        let allowed = Some(vec![caller_vk.clone()]);
        let (mut client, server) = UnixStream::pair().unwrap();
        let handle = std::thread::spawn(move || {
            let reader = io::BufReader::new(server.try_clone().unwrap());
            serve_connection_io(reader, server, &allowed, &None, &RevokedSet::default(), false)
        });

        let call = |client: &mut UnixStream, req: Value| -> Value {
            write_frame(client, &serde_json::to_vec(&req).unwrap()).unwrap();
            let payload = read_frame(client).unwrap().expect("a framed response");
            serde_json::from_slice(&payload).unwrap()
        };

        // Pass the store in config (no process-wide env — keeps parallel tests independent).
        let init = call(&mut client, json!({ "op": "init", "config": { "authority_key_store": store } }));
        assert_eq!(init["status"].as_str().unwrap(), "ok");

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
            serve_connection_io(reader, server2, &None, &None, &RevokedSet::default(), false)
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
            serve_connection_io(reader, server, &None, &None, &RevokedSet::default(), true)
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
        // Channel frames are length-bucket padded before sealing (pre-audit #5); the node strips it.
        let padded_req = ddrm_envelope::channel_pad::pad(&req_bytes);
        let sealed_req = ddrm_envelope::seal::seal_bound(&node_channel, &padded_req, &aad_out, &caller);
        write_frame(&mut client, &sealed_req.to_bytes()).unwrap();
        let sealed_resp = read_frame(&mut client).unwrap().expect("a sealed response frame");
        assert!(
            serde_json::from_slice::<Value>(&sealed_resp).is_err(),
            "the response must NOT be plaintext JSON after channel establishment"
        );
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed_resp).unwrap();
        let aad_in = ddrm_envelope::channel_frame_aad(&challenge, 1, 1);
        let opened = ddrm_envelope::hybrid_unwrap_bound(&client_secret, &env, &aad_in, &verifier).unwrap();
        // Tolerant unpad: the response is un-padded by default (padding is off unless negotiated),
        // and would be stripped here too if a padding-aware quorum had it enabled.
        let unpadded = ddrm_envelope::channel_pad::unpad_incoming(&opened);
        let resp: Value = serde_json::from_slice(&unpadded).unwrap();
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

    /// SHARE-WISE ROTATION (Day 109–112), happy path: with the operator identity pinned, the node
    /// re-escrows its share to a SUCCESSOR node, refreshed by the operator-sealed XOR delta — the
    /// successor (and only the successor) recovers `share ⊕ delta` under the ROTATING node's
    /// signature, and the whole CEK never existed anywhere in the exchange.
    #[test]
    fn rotate_share_re_escrows_a_refreshed_share_to_the_successor() {
        let store = unique_store("rotate");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let node_vk_b64 = init["seal_verifying_key_b64"].as_str().unwrap().to_string();
        let recipient_pub = b64().decode(init["seal_recipient_pub_b64"].as_str().unwrap()).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        // The OPERATOR identity, pinned exactly as the daemon pins it (env at start, never client-set).
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        node.operator_vk = Some(operator_vk);
        // The SUCCESSOR node — its own master, so a genuinely distinct identity + recipient.
        let successor = NodeAuthority::from_master(&[0xA2u8; 32]);

        // The producer's CURRENT escrow of this node's share.
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let share: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped =
            ddrm_envelope::seal::seal_bound(&recipient_public, &share, &escrow, &producer_signer);

        // The operator seals the refresh delta TO THE ROTATING NODE, bound to (kid, source, successor).
        let delta: Vec<u8> = (100u8..132).collect();
        let rot_aad = ddrm_envelope::rotation_aad(&kid16, &recipient_pub, &successor.recipient_public);
        let delta_env =
            ddrm_envelope::seal::seal_bound(&recipient_public, &delta, &rot_aad, &operator);

        let resp = node.rotate_share(
            &b64().encode(wrapped.to_bytes()),
            scheme,
            &kid_hex,
            &b64().encode(&producer_vk),
            &b64().encode(&successor.recipient_public),
            &b64().encode(delta_env.to_bytes()),
        );
        let data = ok_data(resp);
        // The rotated escrow names THIS node as its producer (the successor verifies it at recover).
        assert_eq!(data["escrow_producer_vk_b64"].as_str().unwrap(), node_vk_b64);
        // The SUCCESSOR recovers exactly the REFRESHED share — share ⊕ delta — under the rotating
        // node's identity, through the SAME authenticated escrow path a recover uses.
        let rotated = b64().decode(data["rotated_wrapped_cek_b64"].as_str().unwrap()).unwrap();
        let node_vk = b64().decode(&node_vk_b64).unwrap();
        let recovered = successor
            .recover_escrowed_cek(&rotated, scheme, &kid16, &node_vk)
            .expect("the successor recovers the rotated share");
        let refreshed: Vec<u8> = share.iter().zip(delta.iter()).map(|(a, b)| a ^ b).collect();
        assert_eq!(recovered.as_slice(), refreshed.as_slice());
        assert_ne!(recovered.as_slice(), share.as_slice(), "the rotated share must be REFRESHED, not copied");
        // And the OLD node itself can NOT recover the rotated escrow (it is sealed to the successor).
        assert!(node
            .authority
            .as_ref()
            .unwrap()
            .recover_escrowed_cek(&rotated, scheme, &kid16, &node_vk)
            .is_err());
        let _ = std::fs::remove_file(&store);
    }

    /// SHARE-WISE ROTATION (Day 109–112), fail-closed edges: no pinned operator, a non-operator
    /// delta, a tampered delta, a successor-REDIRECTED delta, and a length-mismatched delta are all
    /// refused — BEFORE any share material is re-escrowed anywhere.
    #[test]
    fn rotate_share_fails_closed_on_missing_operator_forged_or_redirected_delta() {
        let store = unique_store("rotate-adversarial");
        let mut node = DkmsAuthorityNode::default();
        let init = ok_data(node.init(json!({ "authority_key_store": store })));
        let recipient_pub = b64().decode(init["seal_recipient_pub_b64"].as_str().unwrap()).unwrap();
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub).unwrap();
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        let successor = NodeAuthority::from_master(&[0xA2u8; 32]);
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let share: Vec<u8> = (0u8..32).collect();
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let escrow = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_pub);
        let wrapped =
            ddrm_envelope::seal::seal_bound(&recipient_public, &share, &escrow, &producer_signer);
        let wrapped_b64 = b64().encode(wrapped.to_bytes());
        let delta: Vec<u8> = (100u8..132).collect();
        let rot_aad = ddrm_envelope::rotation_aad(&kid16, &recipient_pub, &successor.recipient_public);
        let good_delta = b64().encode(
            ddrm_envelope::seal::seal_bound(&recipient_public, &delta, &rot_aad, &operator).to_bytes(),
        );
        let successor_b64 = b64().encode(&successor.recipient_public);
        let producer_vk_b64 = b64().encode(&producer_vk);

        // NO PINNED OPERATOR: rotation is impossible on this node, even with a well-formed delta.
        let resp = node.rotate_share(&wrapped_b64, scheme, &kid_hex, &producer_vk_b64, &successor_b64, &good_delta);
        assert_eq!(error_code(&resp), "not_configured");

        node.operator_vk = Some(operator_vk);
        // A NON-OPERATOR delta (an impostor's signature) fails the operator verification.
        let (impostor, _impostor_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
        let impostor_delta = b64().encode(
            ddrm_envelope::seal::seal_bound(&recipient_public, &delta, &rot_aad, &impostor).to_bytes(),
        );
        let resp = node.rotate_share(&wrapped_b64, scheme, &kid_hex, &producer_vk_b64, &successor_b64, &impostor_delta);
        assert_eq!(error_code(&resp), "access_denied");

        // A TAMPERED delta envelope (one flipped byte) fails the AEAD/signature open.
        let mut torn = b64().decode(&good_delta).unwrap();
        let last = torn.len() - 1;
        torn[last] ^= 0x01;
        let resp = node.rotate_share(&wrapped_b64, scheme, &kid_hex, &producer_vk_b64, &successor_b64, &b64().encode(torn));
        assert_eq!(error_code(&resp), "access_denied");

        // A REDIRECTED rotation: the delta was authorized for THIS successor, but the request names
        // the ATTACKER's recipient — the AAD no longer matches, so the share cannot be re-routed.
        let attacker = NodeAuthority::from_master(&[0xEEu8; 32]);
        let resp = node.rotate_share(
            &wrapped_b64,
            scheme,
            &kid_hex,
            &producer_vk_b64,
            &b64().encode(&attacker.recipient_public),
            &good_delta,
        );
        assert_eq!(error_code(&resp), "access_denied");

        // A LENGTH-MISMATCHED delta (16 bytes vs the 32-byte share) is refused — never a partial XOR.
        let short_aad = ddrm_envelope::rotation_aad(&kid16, &recipient_pub, &successor.recipient_public);
        let short_delta = b64().encode(
            ddrm_envelope::seal::seal_bound(&recipient_public, &delta[..16], &short_aad, &operator).to_bytes(),
        );
        let resp = node.rotate_share(&wrapped_b64, scheme, &kid_hex, &producer_vk_b64, &successor_b64, &short_delta);
        assert_eq!(error_code(&resp), "invalid_request");

        let _ = std::fs::remove_file(&store);
    }

    /// QUORUM RECONFIGURATION (Day 121–125), full live protocol across real nodes: a 2-of-3 set is
    /// RE-SHARED into a 3-of-5 set. Two OLD members CONTRIBUTE sub-shares of their shares under a
    /// fresh polynomial each; five NEW members INSTALL their shares by combining the sub-shares over
    /// the OLD-contributor Lagrange. ANY THREE of the five reconstruct the EXACT original CEK, any
    /// TWO do not, and the CEK is never reassembled on any node. A non-operator authorization and a
    /// redirected sub-share are both refused.
    #[test]
    fn reshare_2of3_to_3of5_across_real_nodes_reconstructs_and_lifts_the_threshold() {
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let kid16 = [0xC5u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
        let producer_vk_b64 = b64().encode(&producer_vk);

        // OLD 2-of-3 sharing of the CEK.
        let cek: Vec<u8> = (0u8..16).collect();
        let coeff: Vec<u8> = (200u8..216).collect();
        let old_shares = ddrm_envelope::split_cek_shamir2(&cek, &coeff).unwrap();

        // Provision 3 OLD nodes, each holding its INDEXED escrowed share.
        let mut stores: Vec<String> = Vec::new();
        let mut old_nodes: Vec<DkmsAuthorityNode> = Vec::new();
        let mut old_vk: Vec<String> = Vec::new();
        let mut old_recipient: Vec<String> = Vec::new();
        let mut old_escrow: Vec<String> = Vec::new();
        for (i, old_share) in old_shares.iter().enumerate() {
            let store = unique_store(&format!("reshare-old-{i}"));
            let mut node = DkmsAuthorityNode::default();
            let init = ok_data(node.init(json!({ "authority_key_store": store })));
            node.operator_vk = Some(operator_vk.clone());
            let recipient_b64 = init["seal_recipient_pub_b64"].as_str().unwrap().to_string();
            let recipient_bytes = b64().decode(&recipient_b64).unwrap();
            let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_bytes).unwrap();
            let payload = ddrm_envelope::indexed_share((i + 1) as u8, old_share);
            let aad = ddrm_envelope::transcript::escrow_aad(scheme, &kid16, &recipient_bytes);
            let wrapped = ddrm_envelope::seal::seal_bound(&recipient_public, &payload, &aad, &producer_signer);
            old_vk.push(init["seal_verifying_key_b64"].as_str().unwrap().to_string());
            old_recipient.push(recipient_b64);
            old_escrow.push(b64().encode(wrapped.to_bytes()));
            old_nodes.push(node);
            stores.push(store);
        }

        // Provision 5 NEW nodes (fresh masters → genuinely new identities).
        let mut new_nodes: Vec<DkmsAuthorityNode> = Vec::new();
        let mut new_vk: Vec<String> = Vec::new();
        let mut new_recipient: Vec<String> = Vec::new();
        for j in 0..5usize {
            let store = unique_store(&format!("reshare-new-{j}"));
            let mut node = DkmsAuthorityNode::default();
            let init = ok_data(node.init(json!({ "authority_key_store": store })));
            node.operator_vk = Some(operator_vk.clone());
            new_vk.push(init["seal_verifying_key_b64"].as_str().unwrap().to_string());
            new_recipient.push(init["seal_recipient_pub_b64"].as_str().unwrap().to_string());
            new_nodes.push(node);
            stores.push(store);
        }

        let (k, m) = (3u8, 5u8);
        let decode_vks = |vks: &[String]| -> Vec<Vec<u8>> {
            vks.iter().map(|v| b64().decode(v).unwrap()).collect()
        };
        let old_vk_bytes = decode_vks(&old_vk);
        let old_refs: Vec<&[u8]> = old_vk_bytes.iter().map(|v| v.as_slice()).collect();
        let old_set_id = ddrm_envelope::threshold_node_set_id_n(2, &old_refs);
        let new_vk_bytes = decode_vks(&new_vk);
        let new_refs: Vec<&[u8]> = new_vk_bytes.iter().map(|v| v.as_slice()).collect();
        let new_set_id = ddrm_envelope::threshold_node_set_id_n(k, &new_refs);
        let reshare_aad = ddrm_envelope::reshare_aad(&kid16, &old_set_id, &new_set_id, k, m);
        let old_set_b64 = b64().encode(old_set_id);
        let new_set_b64 = b64().encode(new_set_id);

        // The operator seals an authorization to a recipient, bound to the WHOLE reconfiguration.
        let seal_auth_with = |recipient_b64: &str, signer: &ddrm_envelope::seal::MlDsaSealSigner| -> String {
            let bytes = b64().decode(recipient_b64).unwrap();
            let public = ddrm_envelope::session_public_from_bytes(&bytes).unwrap();
            b64().encode(ddrm_envelope::seal::seal_bound(&public, b"reconfigure", &reshare_aad, signer).to_bytes())
        };

        // CONTRIBUTE: the OLD quorum {x=1, x=2} re-shares; collect each sub-share by target new node.
        let contributors = [0usize, 1usize];
        let mut by_target: Vec<Vec<(u8, String, String)>> = vec![Vec::new(); 5];
        for &ci in &contributors {
            let data = ok_data(old_nodes[ci].reshare_contribute(ReshareContributeArgs {
                wrapped_cek_b64: old_escrow[ci].clone(),
                scheme: scheme.to_string(),
                kid_hex: kid_hex.clone(),
                producer_vk_b64: producer_vk_b64.clone(),
                operator_auth_b64: seal_auth_with(&old_recipient[ci], &operator),
                old_node_set_id_b64: old_set_b64.clone(),
                new_node_set_id_b64: new_set_b64.clone(),
                k,
                m,
                new_recipient_pubs_b64: new_recipient.clone(),
            }));
            let cx = data["contributor_x"].as_u64().unwrap() as u8;
            assert_eq!(cx, (ci + 1) as u8, "contributor reports its own coordinate");
            let cvk = data["contributor_vk_b64"].as_str().unwrap().to_string();
            for sub in data["subshares"].as_array().unwrap() {
                let tx = sub["target_x"].as_u64().unwrap() as usize;
                by_target[tx - 1].push((cx, cvk.clone(), sub["sealed_subshare_b64"].as_str().unwrap().to_string()));
            }
        }

        // INSTALL: each NEW node combines its two sub-shares into its share of the reconfigured set.
        let install_contributions = |j: usize| -> Vec<ReshareContribution> {
            by_target[j]
                .iter()
                .map(|(cx, vk, sealed)| ReshareContribution {
                    contributor_x: *cx,
                    contributor_vk_b64: vk.clone(),
                    sealed_subshare_b64: sealed.clone(),
                })
                .collect()
        };
        let mut new_escrow: Vec<String> = Vec::new();
        for j in 0..5usize {
            let data = ok_data(new_nodes[j].reshare_install(ReshareInstallArgs {
                operator_auth_b64: seal_auth_with(&new_recipient[j], &operator),
                old_node_set_id_b64: old_set_b64.clone(),
                new_node_set_id_b64: new_set_b64.clone(),
                k,
                m,
                target_x: (j + 1) as u8,
                scheme: scheme.to_string(),
                kid_hex: kid_hex.clone(),
                contributions: install_contributions(j),
            }));
            assert_eq!(data["escrow_producer_vk_b64"].as_str().unwrap(), new_vk[j], "new escrow names the new node");
            new_escrow.push(data["wrapped_cek_b64"].as_str().unwrap().to_string());
        }

        // Recover each new node's INDEXED share in its OWN boundary (the path the k-of-m open uses).
        let new_share = |j: usize| -> (u8, Vec<u8>) {
            let wrapped = b64().decode(&new_escrow[j]).unwrap();
            let vk = b64().decode(&new_vk[j]).unwrap();
            let payload = new_nodes[j]
                .authority
                .as_ref()
                .unwrap()
                .recover_escrowed_cek(&wrapped, scheme, &kid16, &vk)
                .expect("the new node recovers its own reconfigured share");
            let (x, body) = ddrm_envelope::parse_indexed_share(&payload).unwrap();
            assert_eq!(x, (j + 1) as u8, "the new share is indexed by the new node's coordinate");
            (x, body.to_vec())
        };

        // ANY THREE of the five reconstruct the EXACT original CEK.
        for triple in [[0usize, 2, 4], [1, 2, 3], [0, 1, 4]] {
            let a = new_share(triple[0]);
            let b = new_share(triple[1]);
            let c = new_share(triple[2]);
            let cek_out = ddrm_envelope::lagrange_combine_at_zero(&[
                (a.0, &a.1),
                (b.0, &b.1),
                (c.0, &c.1),
            ])
            .unwrap();
            assert_eq!(cek_out.to_vec(), cek, "any 3 of the reconfigured 3-of-5 reconstruct the CEK");
        }
        // TWO of the five do NOT (the threshold genuinely lifted to 3).
        let a = new_share(0);
        let b = new_share(1);
        let two = ddrm_envelope::lagrange_combine_at_zero(&[(a.0, &a.1), (b.0, &b.1)]).unwrap();
        assert_ne!(two.to_vec(), cek, "two reconfigured shares are below the NEW threshold");

        // FAIL-CLOSED: a NON-operator authorization is refused.
        let (impostor, _ivk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
        let bad = new_nodes[0].reshare_install(ReshareInstallArgs {
            operator_auth_b64: seal_auth_with(&new_recipient[0], &impostor),
            old_node_set_id_b64: old_set_b64.clone(),
            new_node_set_id_b64: new_set_b64.clone(),
            k,
            m,
            target_x: 1,
            scheme: scheme.to_string(),
            kid_hex: kid_hex.clone(),
            contributions: install_contributions(0),
        });
        assert_eq!(error_code(&bad), "access_denied", "a non-operator reconfiguration is refused");

        // FAIL-CLOSED: a sub-share minted for new node 1 (target_x=1) routed to new node 2 (target_x=2)
        // is refused — the sub-share is bound to its (contributor → target) pair AND its recipient.
        let redirected = new_nodes[1].reshare_install(ReshareInstallArgs {
            operator_auth_b64: seal_auth_with(&new_recipient[1], &operator),
            old_node_set_id_b64: old_set_b64.clone(),
            new_node_set_id_b64: new_set_b64.clone(),
            k,
            m,
            target_x: 2,
            scheme: scheme.to_string(),
            kid_hex: kid_hex.clone(),
            contributions: install_contributions(0),
        });
        assert_eq!(error_code(&redirected), "access_denied", "a redirected sub-share is refused");

        for store in &stores {
            let _ = std::fs::remove_file(store);
        }
    }

    /// DISTRIBUTED KEY GENERATION across REAL nodes (Day 126–130): a 2-of-3 CEK is BORN distributed —
    /// three nodes each act as a DEALER (`dkg_contribute`) routing sub-shares to all three members,
    /// each node then INSTALLS its share (`dkg_install`, summing the dealers' sub-shares). Any TWO
    /// installed shares reconstruct the SAME CEK; the CEK is assembled nowhere (each dealer knows only
    /// its own contribution); a tampered/redirected sub-share is refused and the dealer named; and a
    /// non-operator authorization is refused.
    #[test]
    fn dkg_2of3_across_real_nodes_is_born_distributed_and_reconstructs() {
        let scheme = ddrm_envelope::SUITE_PQ_HYBRID;
        let kid16 = [0xD7u8; 16];
        let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        let (t, m) = (2u8, 3u8);
        let cek_len = 16u32;
        let dkg_id = [0x33u8; 16];
        let dkg_id_b64 = b64().encode(dkg_id);

        // Provision 3 nodes (fresh masters → distinct identities); each is both a dealer and a member.
        let mut stores: Vec<String> = Vec::new();
        let mut nodes: Vec<DkmsAuthorityNode> = Vec::new();
        let mut vk: Vec<String> = Vec::new();
        let mut recipient: Vec<String> = Vec::new();
        for i in 0..m as usize {
            let store = unique_store(&format!("dkg-{i}"));
            let mut node = DkmsAuthorityNode::default();
            let init = ok_data(node.init(json!({ "authority_key_store": store })));
            node.operator_vk = Some(operator_vk.clone());
            vk.push(init["seal_verifying_key_b64"].as_str().unwrap().to_string());
            recipient.push(init["seal_recipient_pub_b64"].as_str().unwrap().to_string());
            nodes.push(node);
            stores.push(store);
        }

        let vk_bytes: Vec<Vec<u8>> = vk.iter().map(|v| b64().decode(v).unwrap()).collect();
        let refs: Vec<&[u8]> = vk_bytes.iter().map(|v| v.as_slice()).collect();
        let node_set_id = ddrm_envelope::threshold_node_set_id_n(t, &refs);
        let node_set_b64 = b64().encode(node_set_id);
        let dkg_aad = ddrm_envelope::dkg_aad(&kid16, &dkg_id, &node_set_id, t, m);
        let seal_auth_with = |recipient_b64: &str, signer: &ddrm_envelope::seal::MlDsaSealSigner| -> String {
            let bytes = b64().decode(recipient_b64).unwrap();
            let public = ddrm_envelope::session_public_from_bytes(&bytes).unwrap();
            b64().encode(ddrm_envelope::seal::seal_bound(&public, b"dkg", &dkg_aad, signer).to_bytes())
        };

        // CONTRIBUTE: each node deals to all three members; collect each sub-share by target member.
        let mut by_target: Vec<Vec<(u8, String, String)>> = vec![Vec::new(); m as usize];
        for di in 0..m as usize {
            let data = ok_data(nodes[di].dkg_contribute(DkgContributeArgs {
                operator_auth_b64: seal_auth_with(&recipient[di], &operator),
                dkg_id_b64: dkg_id_b64.clone(),
                node_set_id_b64: node_set_b64.clone(),
                t,
                m,
                dealer_x: (di + 1) as u8,
                kid_hex: kid_hex.clone(),
                cek_len,
                member_recipient_pubs_b64: recipient.clone(),
            }));
            let dx = data["dealer_x"].as_u64().unwrap() as u8;
            assert_eq!(dx, (di + 1) as u8, "dealer reports its own coordinate");
            let dvk = data["dealer_vk_b64"].as_str().unwrap().to_string();
            for sub in data["subshares"].as_array().unwrap() {
                let tx = sub["target_x"].as_u64().unwrap() as usize;
                by_target[tx - 1].push((dx, dvk.clone(), sub["sealed_subshare_b64"].as_str().unwrap().to_string()));
            }
        }

        let install_contributions = |j: usize| -> Vec<DkgContribution> {
            by_target[j]
                .iter()
                .map(|(dx, dvk, sealed)| DkgContribution {
                    dealer_x: *dx,
                    dealer_vk_b64: dvk.clone(),
                    sealed_subshare_b64: sealed.clone(),
                })
                .collect()
        };

        // INSTALL: each member sums the three sub-shares routed to it into its final share.
        let mut escrow: Vec<String> = Vec::new();
        for j in 0..m as usize {
            let data = ok_data(nodes[j].dkg_install(DkgInstallArgs {
                operator_auth_b64: seal_auth_with(&recipient[j], &operator),
                dkg_id_b64: dkg_id_b64.clone(),
                node_set_id_b64: node_set_b64.clone(),
                t,
                m,
                target_x: (j + 1) as u8,
                kid_hex: kid_hex.clone(),
                scheme: scheme.to_string(),
                contributions: install_contributions(j),
            }));
            assert_eq!(data["escrow_producer_vk_b64"].as_str().unwrap(), vk[j], "DKG escrow names the member");
            escrow.push(data["wrapped_cek_b64"].as_str().unwrap().to_string());
        }

        // Recover each member's INDEXED share in its OWN boundary (the path the t-of-m open uses).
        let share = |j: usize| -> (u8, Vec<u8>) {
            let wrapped = b64().decode(&escrow[j]).unwrap();
            let vkj = b64().decode(&vk[j]).unwrap();
            let payload = nodes[j]
                .authority
                .as_ref()
                .unwrap()
                .recover_escrowed_cek(&wrapped, scheme, &kid16, &vkj)
                .expect("the member recovers its own DKG share");
            let (x, body) = ddrm_envelope::parse_indexed_share(&payload).unwrap();
            assert_eq!(x, (j + 1) as u8, "the DKG share is indexed by the member's coordinate");
            (x, body.to_vec())
        };

        // ANY TWO of the three reconstruct the SAME CEK; one share is below quorum.
        let s1 = share(0);
        let s2 = share(1);
        let s3 = share(2);
        let cek12 = ddrm_envelope::lagrange_combine_at_zero(&[(s1.0, &s1.1), (s2.0, &s2.1)]).unwrap().to_vec();
        let cek13 = ddrm_envelope::lagrange_combine_at_zero(&[(s1.0, &s1.1), (s3.0, &s3.1)]).unwrap().to_vec();
        let cek23 = ddrm_envelope::lagrange_combine_at_zero(&[(s2.0, &s2.1), (s3.0, &s3.1)]).unwrap().to_vec();
        assert_eq!(cek12, cek13, "distinct quorums reconstruct the SAME DKG-born CEK (dealers are consistent)");
        assert_eq!(cek12, cek23, "distinct quorums reconstruct the SAME DKG-born CEK (dealers are consistent)");
        assert_eq!(cek12.len(), cek_len as usize, "the DKG-born CEK is the agreed length");
        // BORN DISTRIBUTED: no single installed share equals the CEK.
        assert_ne!(s1.1, cek12, "a single DKG member share is not the CEK");

        // FAIL-CLOSED: a NON-operator authorization is refused.
        let (impostor, _ivk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
        let bad = nodes[0].dkg_install(DkgInstallArgs {
            operator_auth_b64: seal_auth_with(&recipient[0], &impostor),
            dkg_id_b64: dkg_id_b64.clone(),
            node_set_id_b64: node_set_b64.clone(),
            t,
            m,
            target_x: 1,
            kid_hex: kid_hex.clone(),
            scheme: scheme.to_string(),
            contributions: install_contributions(0),
        });
        assert_eq!(error_code(&bad), "access_denied", "a non-operator DKG install is refused");

        // FAIL-CLOSED: a sub-share routed to member 1 (target_x=1) installed at member 2 (target_x=2)
        // is refused — bound to its (dealer → target) pair AND its recipient; the dealer is named.
        let redirected = nodes[1].dkg_install(DkgInstallArgs {
            operator_auth_b64: seal_auth_with(&recipient[1], &operator),
            dkg_id_b64: dkg_id_b64.clone(),
            node_set_id_b64: node_set_b64.clone(),
            t,
            m,
            target_x: 2,
            kid_hex: kid_hex.clone(),
            scheme: scheme.to_string(),
            contributions: install_contributions(0),
        });
        assert_eq!(error_code(&redirected), "access_denied", "a redirected DKG sub-share is refused");

        for store in &stores {
            let _ = std::fs::remove_file(store);
        }
    }

    /// LIVE CALLER REVOCATION (Day 109–112): only the pinned operator can revoke; once revoked, the
    /// caller's `hello` is refused AND a recover under its STILL-LIVE session token is refused —
    /// revocation outranks a live session (the immediate-cutoff property, the node-side analogue of
    /// PC2 reading the revoked delegation nonce back per request, `secureViewSession.ts:108`–`:112`).
    #[test]
    #[cfg(feature = "legacy-receipt-authz")]
    fn revocation_outranks_a_live_session_and_requires_the_operator() {
        let store = unique_store("revoke");
        let (mut node, base, caller) = setup_recover(&store);
        let caller_vk_b64 = base.session_token.caller_pub_b64.clone();
        let (operator, operator_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x6Fu8; 32]);
        let caller_pub = b64().decode(&caller_vk_b64).unwrap();
        let genuine_sig = b64().encode(ddrm_envelope::sign_revocation(&operator, &caller_pub));

        // NO PINNED OPERATOR: revocation is refused outright.
        assert_eq!(error_code(&node.revoke_caller(&caller_vk_b64, &genuine_sig)), "not_configured");
        node.operator_vk = Some(operator_vk);

        // A FORGED revocation (impostor signature) is refused — and the caller is still served.
        let (impostor, _vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
        let forged = b64().encode(ddrm_envelope::sign_revocation(&impostor, &caller_pub));
        assert_eq!(error_code(&node.revoke_caller(&caller_vk_b64, &forged)), "access_denied");
        assert!(matches!(node.recover(base.clone()), Response::Ok { .. }), "not revoked: still served");

        // The GENUINE operator revocation lands…
        assert!(matches!(node.revoke_caller(&caller_vk_b64, &genuine_sig), Response::Ok { .. }));
        // …the caller's next hello is refused (no new session is ever minted)…
        let hello = node.hello(&b64().encode([0xB9u8; 32]), &caller_vk_b64, Some(NOW), None);
        assert_eq!(error_code(&hello), "caller_revoked");
        // …and a recover under the STILL-LIVE token (valid signature, unexpired, fresh seq, valid
        // possession proof) is refused too — a live session does not outrank a revocation.
        let mut live = base.clone();
        live.recover_seq = 2;
        live.caller_sig_b64 = proof_for(
            &caller,
            &base.session_token,
            CONTENT,
            &base.kid_hex,
            &base.decrypt_session_pub_b64,
            2,
        );
        assert_eq!(error_code(&node.recover(live)), "caller_revoked");

        let _ = std::fs::remove_file(&store);
    }

    /// Pull the challenge string out of a session-token JSON value.
    #[cfg(feature = "legacy-receipt-authz")]
    fn challenge_str(token: &Value) -> &str {
        token["challenge_b64"].as_str().unwrap()
    }

    /// The node refuses a recover whose session token is EXPIRED, FORGED, or TAMPERED — even with a
    /// perfectly valid escrow + receipt — so a long-lived node only recovers within a live handshake
    /// session and a captured/forged token cannot drive recovery.
    #[test]
    #[cfg(feature = "legacy-receipt-authz")]
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
    #[cfg(feature = "legacy-receipt-authz")]
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
    #[cfg(feature = "legacy-receipt-authz")]
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
