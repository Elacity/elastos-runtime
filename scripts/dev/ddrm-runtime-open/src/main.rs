//! `ddrm-runtime-open` — the default-on runtime-core OPEN entrypoint (Phase A wiring).
//!
//! This is the runtime bootstrap a non-smoke caller runs: it reads a TYPED JSON CONFIG
//! (`OpenConfig`: provider binaries, work dir, viewer, content id, `mode`, `authority`) and
//! constructs the trusted `ddrm_plan_runner::DrmHost` from `ProviderLauncher`s + a
//! `DurableEventStore` via `DrmHost::launch`, then drives the open. NO caller assembles the host —
//! the binary owns the bootstrap (the analogue of PC2 booting `sessionService` ONCE from config,
//! `BackendSessionService.ts:495`). The producer's escrow is a publish-time fixture this binary
//! reads, not inline code. `mode:"open"` is the operator path (publish → launch → open →
//! persist → durable CEK-free readback); `mode:"verify"` (what `ddrm-consumer-smoke.sh` runs)
//! ALSO drives the two adversarial fail-closed gates.
//!
//! `authority.backend` selects the key authority — `reference` (a durable key store the runtime
//! generates) or `dkms` (an EXTERNAL, SECRET-HOLDING node). For `dkms` the publish phase PROVISIONS
//! the node (the master stays in the node's own store) and writes a PUBLIC-ONLY descriptor (the
//! node's pins + endpoint, NO secret); at open, the `key-provider` holds only that public identity
//! and DELEGATES recovery to the node — the master/CEK NEVER enter the runtime. The OPEN PATH is
//! BACKEND-AGNOSTIC: only the key provider's `init` config differs — the analogue of PC2's
//! `getSessionView(token)` dispatching on `stored.backend` (`BackendSessionService.ts:368`–`:377`)
//! while the downstream open is identical, and PC2's client holding only the public `pkpId` and
//! delegating recovery to the Lit network (`recoverCEKEnvelope`, `chipotle-client.ts:1438`).
//! It drives the REAL capsule binaries over their newline-delimited JSON stdin/stdout protocol,
//! proving the consumer half runs end to end:
//!
//!   drm/open  ->  rights  ->  key (reference | dkms authority)  ->  decrypt (OpenSessionV1)
//!
//! with NO Lit, NO dKMS and NO chain. The novel, previously-unproven step is the
//! cross-process `key -> decrypt` handoff:
//!
//!   1. the key authority publishes its ML-DSA-65 verifying key (`key init`);
//!   2. the decrypt boundary is configured to trust it, then MINTS an in-sandbox
//!      session keypair and PUBLISHES the public key (`decrypt init`, never the secret);
//!   3. the content CEK is ESCROWED to the authority's published recipient key, and the
//!      authority's CANONICAL `release` op RECOVERS it in-boundary (from the rights-bound
//!      key_envelope) and re-seals it to the published session key, bound to the canonical
//!      decrypt transcript — the SAME `to_aad` both sides share. No raw CEK is handed in;
//!   4. the decrypt boundary unwraps with its in-VM secret and decrypts a real CENC
//!      segment (`decrypt open_session_v1`), returning ONLY a scoped session — no CEK,
//!      no plaintext crosses the process boundary.
//!
//! The content `(CEK, ciphertext, plaintext)` is a committed golden
//! (`capsules/decrypt-provider/tests/vectors/classical_cenc.json`); the CEK reaches the
//! authority SEALED (escrowed, recovered in-boundary) via the canonical `release` op — no
//! dev raw-CEK shim — and the boundary recovers it and decrypts, proving the rail end to
//! end through the op drm-provider's plan actually names.
//!
//! The chain is no longer hand-walked here, and the smoke no longer drives the open
//! itself — nor does it pre-spawn the providers, nor escrow inline: it builds the trusted
//! runtime-core HOST `ddrm_plan_runner::DrmHost` via `DrmHost::launch` and the HOST LAUNCHES
//! the rail. The producer's escrow now happens at PUBLISH time against a STABLE recipient:
//! the key authority is backed by a DURABLE KEY STORE (`authority_key_store`), so its
//! verifying + escrow-recipient keys are persisted ONCE and re-derived identically on every
//! launch. The smoke first runs a `publish_escrow` phase (the producer role): bring the
//! authority up once, escrow the CEK to its stable recipient under the shared escrow AAD,
//! and write a durable publish fixture — the analogue of PC2 escrowing the CEK to the
//! stable `DEFAULT_AUTHORITY` at encode time (`dashPackager.ts` `encryptMediaCEK`, the
//! authority address baked into every video's PSSH, `dashPackager.ts:44`). Then it hands the
//! host three `ProviderLauncher`s (`RightsLauncher`/`KeyLauncher`/`DecryptLauncher`, each
//! owning one real capsule BINARY); `DrmHost::launch` (`RuntimeCapabilityTable::from_launchers`)
//! brings each up — spawn → init → PUBLISH material (the key authority RELAUNCHED from the
//! SAME durable store → the SAME recipient; the decrypt boundary its per-open in-sandbox
//! session key). This is the runtime-core analogue of PC2's
//! `BackendSessionService.createSession` launching a backend view (`WasmSessionView.createNew()`
//! mints + publishes the per-session key — `src/api/chipotle-client.ts:603`,
//! `BackendSessionService.ts:307`) against a long-lived authority identity. Once the rail is
//! up the runtime PROVES the authority recipient is STABLE across the relaunch, READS the
//! publish fixture (it never re-escrows), and binds only the per-open session transcript AAD
//! over the decrypt boundary's published session key. Then `host.open(content_id, viewer)`
//! (1) asks its `PlanSource` (a `SmokePlanSource`
//! wrapping the REAL `drm-provider`) for the canonical plan, (2) drives it through the
//! launched `RuntimeCapabilityTable` (`open_drm_plan`'s parse → resolve each required
//! provider → execute), and (3) PERSISTS the plan's runtime-OWNED post-steps
//! (`release_receipt` + the open audit) through a `RuntimeEventSink`. This mirrors PC2's
//! server-owned `/init` route owning fetch → recover → session → log in one place
//! (`pc2-node/src/api/media.ts:133`/`:481`/`:489`). The event sink is the lib's
//! `PersistingEventSink` over the production-shaped `DurableEventStore` — each runtime
//! event is written as a durable, CEK-FREE record (open identity + decision + artifact
//! NAMES, never key material) via atomic write into a stable on-disk layout, mirroring
//! PC2's `FileSessionStore` (`BackendSessionService.ts:107`). The smoke proves durability
//! by reading the records back through a FRESH `DurableEventStore::load` (a fresh reader,
//! as if a new process) and asserting no CEK/secret leaked. The HOST owns the transports,
//! so `host.shutdown()` tears the whole rail down — no manual per-capsule shutdown. The
//! host fails closed unless every required provider is registered and every runtime event
//! can be PERSISTED. Two more fail-closed gates ride along: a transcript-mismatched seal
//! must not open, and a TAMPERED plan FROM THE SOURCE (driven back through the SAME host)
//! must be rejected by the real key-provider.
//!
//! Usage: ddrm-consumer-smoke <key-provider-bin> <decrypt-provider-bin> <drm-bin> [rights-bin]

use base64::Engine as _;
use ddrm_envelope::transcript::{escrow_aad, release_receipt_hash, DecryptTranscriptV1};
use ddrm_envelope::SUITE_PQ_HYBRID;
use ddrm_plan_runner::{
    DrmHost, DurableEventStore, PersistingEventSink, PlanSource, ProviderHandle, ProviderLauncher,
    ProviderTransport, StepInputs,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// --- the shared identity used by BOTH the decrypt request and the sealed transcript -
const PRINCIPAL: &str = "person:local:smoke";
const SESSION: &str = "session:smoke";
const OBJECT_CID: &str = "bafybeigprotectedcontent";
const ACTION: &str = "view";
const VIEWER: &str = "elastos.viewer/document@1";
const OUTPUT_KIND: &str = "rendered";
const EXPIRES_AT: u64 = 1_900_000_000;
const NOW_UNIX: u64 = 1_850_000_000;

// Release receipt identity (hashed into the transcript by both sides).
const RR_SCHEMA: &str = "elastos.release.receipt/v1";
const RR_REQUEST_ID: &str = "key-release:smoke";
const RR_PROVIDER: &str = "key-provider";
const RR_STATUS: &str = "released";
const RR_ISSUED_AT: u64 = 1_800_000_000;

// Committed golden (classical_cenc.json): a 16-byte CEK + an AES-128-CTR CENC segment
// whose plaintext is "the quick brown fox jumps over!!" — a single protected sample.
const GOLDEN_CEK_B64: &str = "EREREREREREREREREREREQ==";
const GOLDEN_CIPHERTEXT_B64: &str = "AAAAPG1vb2YAAAA0dHJhZgAAABR0cnVuAAACAAAAAAEAAAAgAAAAGHNlbmMAAAAAAAAAASIiIiIiIiIiAAAAKG1kYXScNDPiT64BF0MfL13dprDn+6eX7LyGcmlu1lMPPiQQpA==";
const EXPECTED_SAMPLE_COUNT: u64 = 1;

/// The content identity carried across the WHOLE chain (the chain ownership query,
/// the rights binding, and the decrypt transcript). Defaults to the golden's CID for
/// the offline smoke; a live run overrides it with the on-chain contentId/KID the
/// AuthorityGateway actually answers for (`DDRM_SMOKE_CONTENT_ID`).
fn cid() -> String {
    std::env::var("DDRM_SMOKE_CONTENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| OBJECT_CID.to_string())
}

/// A live capsule process driven over its stdin/stdout JSON line protocol.
struct Capsule {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Capsule {
    fn spawn(name: &str, bin: &str) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {name} ({bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(Self { name: name.to_string(), child, stdin, stdout })
    }

    /// Send one request line, read one response line, parse it. The capsule emits
    /// exactly one JSON object per input line.
    fn call(&mut self, req: &Value) -> Result<Value, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}").map_err(|e| format!("write to {}: {e}", self.name))?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let mut resp = String::new();
        let n = self
            .stdout
            .read_line(&mut resp)
            .map_err(|e| format!("read from {}: {e}", self.name))?;
        if n == 0 {
            return Err(format!("{} closed its output unexpectedly", self.name));
        }
        serde_json::from_str(resp.trim()).map_err(|e| format!("{} sent non-JSON: {e}: {resp}", self.name))
    }

    fn shutdown(&mut self) {
        let _ = self.call(&json!({ "op": "shutdown" }));
        let _ = self.child.wait();
    }
}

fn ok_data(resp: &Value, ctx: &str) -> Result<Value, String> {
    if resp.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!("{ctx}: expected ok, got {resp}"))
    }
}

/// The key step's release receipt, in the shape the decrypt boundary consumes.
fn release_receipt_json() -> Value {
    json!({
        "schema": RR_SCHEMA,
        "request_id": RR_REQUEST_ID,
        "object_cid": cid(),
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "action": ACTION,
        "provider": RR_PROVIDER,
        "status": RR_STATUS,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

/// Build the decrypt-session request WITHOUT the plan-threaded fields. The runtime
/// executor threads `release_receipt`, `object_cid` and `viewer_interface` in from the
/// plan's binding edges (see [`SmokeRunner::run_step`]) — so a wrong edge places an
/// artifact under an unknown field (or omits it) and the real decrypt-provider fails
/// closed, rather than this literal hard-coding where each value lands.
fn decrypt_request_base() -> Value {
    json!({
        "schema": "elastos.decrypt.session.request/v1",
        "request_id": "decrypt:consumer-smoke",
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "action": ACTION,
        "output_kind": OUTPUT_KIND,
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    })
}

fn rights_access_request() -> Value {
    json!({
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "content_id": cid(),
        "right": ACTION,
        "reason": "open protected document",
    })
}

/// A mocked on-chain ownership answer (stands in for `chain-provider::has_access_by_
/// content_id`; used on the offline path). `has_access: true` => owned.
fn owned_chain_attestation() -> Value {
    json!({
        "network": "base",
        "contract": "0x00000000000000000000000000000000000000aa",
        "content_id": cid(),
        "subject": "0x00000000000000000000000000000000000000bb",
        "right": ACTION,
        "has_access": true,
    })
}

/// A hardcoded fallback receipt (used only when no rights binary is supplied).
fn fallback_rights_receipt() -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "rights:smoke",
        "content_id": cid(),
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": ACTION,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": RR_ISSUED_AT,
        "expires_at": EXPIRES_AT,
    })
}

/// Build the key-release request WITHOUT the plan-threaded rights receipt. The
/// runtime executor threads the `RightsDecisionReceiptV1` in under the field the
/// plan's `rights_check -> key_release` edge declares (default `rights_receipt`); a
/// wrong edge omits it and the real key-provider fails closed.
fn key_release_request_base(kid_hex: &str, wrapped_cek_b64: &str) -> Value {
    json!({
        "schema": "elastos.key_release.request/v1",
        "request_id": RR_REQUEST_ID,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "object_cid": cid(),
        "action": ACTION,
        "key_envelope": {
            "scheme": "elastos-pq-hybrid-threshold-v0",
            // The bytes16 KID the escrow is bound to, and the producer's escrow blob —
            // the wrapped CEK rides INSIDE the rights-bound request (canonical `release`).
            "kid": kid_hex,
            "wrapped_cek": wrapped_cek_b64,
            "policy_hash": "sha256:smoke",
            "algorithms": {
                "cipher": "aes-256-gcm",
                "signature": ["ed25519", "ml-dsa-65"],
                "kem": ["x25519", "ml-kem-768"],
                "share_scheme": "shamir-t-of-n",
            },
        },
        "reason": "open protected document",
        "expires_at": EXPIRES_AT,
    })
}

/// The `drm/open` request: a sealed object whose KID is the content identity the
/// chain keys on. The drm-provider validates it and returns the canonical open plan.
fn drm_open_request() -> Value {
    json!({
        "op": "open",
        "request": {
            "object": {
                "schema": "elastos.sealed.object/v1",
                "payload_cid": "bafybeigpayload",
                "rights_policy_cid": "bafybeigpolicy",
                "availability_receipt_cid": "bafybeigreceipt",
                "key_envelope": {
                    "scheme": "elastos-pq-hybrid-threshold-v0",
                    "kid": cid(),
                    "wrapped_cek": "wrapped",
                    "policy_hash": "sha256:smoke",
                    "algorithms": {
                        "cipher": "aes-256-gcm",
                        "signature": ["ed25519", "ml-dsa-65"],
                        "kem": ["x25519", "ml-kem-768"],
                        "share_scheme": "shamir-t-of-n",
                    },
                },
                "viewer": { "required_interface": VIEWER },
            },
            "principal_id": PRINCIPAL,
            "session_id": SESSION,
            "action": ACTION,
            "reason": "open protected document",
        }
    })
}

/// Rebuild the canonical decrypt-transcript AAD exactly as the decrypt boundary will,
/// using the SHARED `ddrm-envelope` encoder (no parallel definition).
fn transcript_aad(session_pub: &[u8], content_hash: &[u8], nonce: &[u8]) -> Vec<u8> {
    let c = cid();
    let receipt_hash = release_receipt_hash(
        RR_SCHEMA,
        RR_REQUEST_ID,
        &c,
        PRINCIPAL,
        SESSION,
        ACTION,
        RR_PROVIDER,
        RR_STATUS,
        RR_ISSUED_AT,
        EXPIRES_AT,
    );
    DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt-provider",
        principal_id: PRINCIPAL,
        session_id: SESSION,
        object_cid: &c,
        content_hash,
        action: ACTION,
        viewer_interface: VIEWER,
        output_kind: OUTPUT_KIND,
        expires_at: EXPIRES_AT,
        release_receipt_hash: receipt_hash,
        decrypt_session_pub: session_pub,
        nonce,
    }
    .to_aad()
}

fn step(n: u32, msg: &str) {
    println!("  [{n}] {msg}");
}

/// Resolve the on-chain ownership answer. Live (`DDRM_SMOKE_CHAIN_RPC` set + a
/// chain-provider binary supplied) drives the REAL `chain-provider` against Base;
/// otherwise a mocked-owned attestation keeps the offline smoke deterministic.
/// Returns `(attestation, mode_label)`.
fn chain_attestation(chain_bin: Option<&String>) -> Result<(Value, String), String> {
    let rpc = std::env::var("DDRM_SMOKE_CHAIN_RPC").ok().filter(|s| !s.is_empty());
    match (rpc, chain_bin) {
        (Some(rpc_url), Some(bin)) => Ok((live_chain_attestation(bin, &rpc_url)?, "live chain".to_string())),
        (Some(_), None) => Err("DDRM_SMOKE_CHAIN_RPC is set but no chain-provider binary was supplied".to_string()),
        _ => Ok((owned_chain_attestation(), "mocked owned".to_string())),
    }
}

/// Drive the real `chain-provider` for a live `has_access_by_content_id` query, and
/// return its response shaped as the rights attestation (1:1 by field name). All the
/// network/contract/subject inputs come from the environment so nothing is hard-coded.
fn live_chain_attestation(bin: &str, rpc_url: &str) -> Result<Value, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let network = env("DDRM_SMOKE_CHAIN_NETWORK").unwrap_or_else(|| "base".to_string());
    let contract = env("DDRM_SMOKE_CHAIN_CONTRACT")
        .ok_or("DDRM_SMOKE_CHAIN_CONTRACT (AuthorityGateway address) is required for a live chain check")?;
    let selector = env("DDRM_SMOKE_CHAIN_SELECTOR")
        .ok_or("DDRM_SMOKE_CHAIN_SELECTOR (has_access selector, e.g. 0x........) is required for a live chain check")?;
    let subject = env("DDRM_SMOKE_CHAIN_SUBJECT")
        .ok_or("DDRM_SMOKE_CHAIN_SUBJECT (your wallet address) is required for a live chain check")?;
    let chain_id: i64 = env("DDRM_SMOKE_CHAIN_ID").and_then(|s| s.parse().ok()).unwrap_or(8453);

    let mut chain = Capsule::spawn("chain-provider", bin)?;
    ok_data(
        &chain.call(&json!({
            "op": "init",
            "config": { "networks": [{
                "id": network,
                "display_name": network,
                "kind": "evm_json_rpc",
                "chain_id": chain_id,
                "native_symbol": "ETH",
                "provider": "ddrm-consumer-smoke",
                "mainnet": true,
                "explorer_url": null,
                "rpc_url": rpc_url,
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": contract,
                    "abi": "has_access_by_content_id_string_address_string",
                    "selector": selector,
                }]
            }]}
        }))?,
        "chain init",
    )?;
    let resp = ok_data(
        &chain.call(&json!({
            "op": "has_access_by_content_id",
            "network": network,
            "contract": contract,
            "content_id": cid(),
            "subject": subject,
            "right": ACTION,
        }))?,
        "chain has_access_by_content_id",
    )?;
    chain.shutdown();

    // chain-provider's response IS the attestation shape rights-provider consumes.
    Ok(json!({
        "network": resp["network"],
        "contract": resp["contract"],
        "content_id": resp["content_id"],
        "subject": resp["subject"],
        "right": resp["right"],
        "has_access": resp["has_access"],
    }))
}

/// Thread the plan's binding inputs into a base request: each artifact lands under the
/// field name the PLAN's edge declared. A wrong edge therefore places it under an
/// unknown field (or omits it) and the real provider — `deny_unknown_fields` over a
/// required field — fails closed.
fn thread_into(base: &mut Value, inputs: &StepInputs) {
    if let Some(obj) = base.as_object_mut() {
        for (field, artifact) in inputs.threaded_fields() {
            obj.insert(field.clone(), artifact.clone());
        }
    }
}

/// The material the providers PUBLISH as the host LAUNCHES the rail: the key authority's
/// verifying + escrow-recipient keys, the decrypt boundary's in-sandbox session key. The
/// launchers fill this during `from_launchers`; once the rail is up the runtime reads it
/// to perform the producer escrow + compute the canonical transcript AAD. (No secret lands
/// here — only PUBLISHED public material, the analogue of `createNew()`'s `publicKeyHex`.)
#[derive(Default)]
struct RailMaterial {
    vk_b64: Option<String>,
    recipient_pub_b64: Option<String>,
    session_pub_b64: Option<String>,
}

/// The key transport's per-open material, BOUND by the runtime after the rail is launched
/// and the producer escrow is done. The `KeyHandle` reads it at open and fails closed if
/// the runtime opened the key transport before binding it.
#[derive(Clone)]
struct KeyOpenMaterial {
    kid_hex: String,
    wrapped_cek_b64: String,
    producer_vk_b64: String,
    session_pub_b64: String,
    aad_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
}

/// The durable PUBLISH-TIME escrow fixture: what the producer wrote when it escrowed the
/// content CEK to the authority's STABLE recipient (no session binding — that's per-open).
/// The runtime open path READS this instead of escrowing inline. The analogue of PC2's
/// encode-time `encryptMediaCEK(cek, kid) -> authority: DEFAULT_AUTHORITY` artifact baked
/// alongside the content (`dashPackager.ts`), not recomputed at play time.
struct PublishEscrow {
    kid_hex: String,
    wrapped_cek_b64: String,
    producer_vk_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
    /// The recipient the CEK was escrowed to — checked against the relaunched authority's
    /// published recipient to prove the authority identity is STABLE across processes.
    recipient_pub_b64: String,
}

impl PublishEscrow {
    fn to_json(&self) -> Value {
        json!({
            "schema": "elastos.publish.escrow.fixture/v1",
            "kid_hex": self.kid_hex,
            "wrapped_cek_b64": self.wrapped_cek_b64,
            "producer_vk_b64": self.producer_vk_b64,
            "content_hash_b64": self.content_hash_b64,
            "nonce_b64": self.nonce_b64,
            "recipient_pub_b64": self.recipient_pub_b64,
        })
    }

    fn from_json(v: &Value) -> Result<Self, String> {
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("publish escrow fixture is missing `{k}`"))
        };
        Ok(Self {
            kid_hex: field("kid_hex")?,
            wrapped_cek_b64: field("wrapped_cek_b64")?,
            producer_vk_b64: field("producer_vk_b64")?,
            content_hash_b64: field("content_hash_b64")?,
            nonce_b64: field("nonce_b64")?,
            recipient_pub_b64: field("recipient_pub_b64")?,
        })
    }
}

/// PUBLISH-TIME escrow (the producer role, run ONCE before any open): bring the
/// durable-key-store authority up, read its STABLE published recipient, escrow the content
/// CEK to it under the shared escrow AAD, and write a durable publish fixture. Mirrors PC2
/// escrowing the CEK to the stable `DEFAULT_AUTHORITY` at encode time (`dashPackager.ts`
/// `encryptMediaCEK`). After this, the open path NEVER escrows — it reads the fixture.
fn publish_escrow(
    key_bin: &str,
    key_store_path: &str,
    fixture_path: &std::path::Path,
    backend: AuthorityBackend,
    descriptor_path: &std::path::Path,
    dkms_node_bin: Option<&str>,
    node_store_path: &str,
) -> Result<PublishEscrow, String> {
    // PROVISION the selected authority and read its PUBLISHED identity (stable vk + recipient).
    //
    // `reference`: the in-runtime authority generates + persists its own master on a durable store.
    //
    // `dkms`: the SECRET-HOLDING NODE is provisioned — it generates + persists its master in its OWN
    // node-local store (`node_store_path`), and the runtime only reads back its PUBLIC identity. We
    // then publish a PUBLIC-ONLY descriptor (the node's pins + endpoint, NO master) the runtime later
    // RESOLVES. The master NEVER enters the runtime. The analogue of provisioning a dKMS node + its
    // published authority pubkey (PC2 holds only the public `pkpId`/`authority`, `chipotle-client.ts`).
    let (_verifying_key_b64, recipient_pub_b64) = match backend {
        AuthorityBackend::Reference => {
            let mut key = Capsule::spawn("key-provider(publish)", key_bin)?;
            let init = ok_data(
                &key.call(&json!({
                    "op": "init",
                    "config": { "backend": "reference", "authority_key_store": key_store_path }
                }))?,
                "key init (publish)",
            )?;
            let vk = init["seal_verifying_key_b64"].as_str()
                .ok_or("key-provider did not publish a seal verifying key (build with --features key-authority-ref)")?
                .to_string();
            let recipient = init["seal_recipient_pub_b64"].as_str()
                .ok_or("key-provider did not publish an escrow recipient key (build with --features key-authority-ref)")?
                .to_string();
            key.shutdown();
            (vk, recipient)
        }
        AuthorityBackend::Dkms => {
            let node_bin = dkms_node_bin.ok_or("dkms backend requires a dkms_authority_bin in the config")?;
            let mut node = Capsule::spawn("dkms-authority(provision)", node_bin)?;
            let init = ok_data(
                &node.call(&json!({
                    "op": "init",
                    "config": { "authority_key_store": node_store_path }
                }))?,
                "dkms-authority init (provision)",
            )?;
            let vk = init["seal_verifying_key_b64"].as_str()
                .ok_or("dkms-authority node did not publish a verifying key")?
                .to_string();
            let recipient = init["seal_recipient_pub_b64"].as_str()
                .ok_or("dkms-authority node did not publish a recipient key")?
                .to_string();
            node.shutdown();
            // Publish the PUBLIC-ONLY descriptor — pins + endpoint, NOTHING secret. The master lives
            // ONLY in `node_store_path` (which this runtime created via the node but never reads).
            let descriptor = json!({
                "schema": "elastos.dkms.authority/v2",
                "verifying_key_b64": vk,
                "recipient_pub_b64": recipient,
                "authority_endpoint": node_bin,
            });
            let bytes = serde_json::to_vec_pretty(&descriptor).map_err(|e| e.to_string())?;
            std::fs::write(descriptor_path, bytes)
                .map_err(|e| format!("write dkms authority descriptor: {e}"))?;
            (vk, recipient)
        }
    };

    let recipient_pub_bytes = B64.decode(&recipient_pub_b64).map_err(|e| e.to_string())?;
    let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_pub_bytes)
        .ok_or("key-provider published a malformed escrow recipient key")?;
    let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
    let cek_bytes = B64.decode(GOLDEN_CEK_B64).map_err(|e| e.to_string())?;
    let kid16 = [0xC5u8; 16];
    let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();
    let escrow = escrow_aad(SUITE_PQ_HYBRID, &kid16, &recipient_pub_bytes);
    let wrapped_cek_b64 = B64.encode(
        ddrm_envelope::seal::seal_bound(&recipient_public, &cek_bytes, &escrow, &producer_signer)
            .to_bytes(),
    );
    let content_hash = b"consumer-smoke-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"consumer-smoke-nonce-1".to_vec();
    let fixture = PublishEscrow {
        kid_hex,
        wrapped_cek_b64,
        producer_vk_b64: B64.encode(&producer_vk),
        content_hash_b64: B64.encode(&content_hash),
        nonce_b64: B64.encode(&nonce),
        recipient_pub_b64,
    };
    let bytes = serde_json::to_vec_pretty(&fixture.to_json()).map_err(|e| e.to_string())?;
    std::fs::write(fixture_path, bytes).map_err(|e| format!("write publish fixture: {e}"))?;
    Ok(fixture)
}

// The consumer half is now driven by the runtime-core `RuntimeStepRunner` over three
// INJECTED per-provider capability handles — the runtime-core analogue of PC2's
// per-request `BackendSessionView` (resurrected in middleware, threaded into the
// downstream stage). Each handle wraps one real capsule binary (the runtime would
// inject real provider handles instead); the core routes each plan step to the handle
// for that step's provider, and fails closed if a required handle is missing. The
// capsules are shared (`Rc<RefCell<_>>`) so the post-walk fail-closed checks + shutdown
// can still reach them after the runner has borrowed the handles.

/// Injected `rights` capability: the REAL rights-provider rendering the (live or
/// mocked) on-chain ownership answer into a typed `RightsDecisionReceiptV1` (falls back
/// to a hardcoded receipt only when no rights binary was supplied).
struct RightsHandle {
    rights: Rc<RefCell<Option<Capsule>>>,
    chain_attestation: Value,
    chain_mode: String,
}

impl ProviderHandle for RightsHandle {
    fn provider(&self) -> &str {
        "rights"
    }
    fn run(&mut self, _inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        let mut guard = self.rights.borrow_mut();
        let receipt = if let Some(rights) = guard.as_mut() {
            ok_data(&rights.call(&json!({ "op": "status" }))?, "rights status")?;
            let decision = ok_data(
                &rights.call(&json!({
                    "op": "decide_access_from_chain",
                    "request_id": RR_REQUEST_ID,
                    "request": rights_access_request(),
                    "chain_access": self.chain_attestation.clone(),
                    "now_unix": RR_ISSUED_AT,
                    "ttl_secs": EXPIRES_AT - RR_ISSUED_AT,
                }))?,
                "rights decide_access_from_chain",
            )?;
            if decision["decision"].as_str() != Some("allowed") {
                return Err(format!(
                    "rights did not allow this content ({}); the chain says you do not own it: {decision}",
                    self.chain_mode
                ));
            }
            decision["receipt"].clone()
        } else {
            fallback_rights_receipt()
        };
        Ok(BTreeMap::from([(
            "RightsDecisionReceiptV1".to_string(),
            receipt,
        )]))
    }
}

/// Injected `key` capability: the canonical `release` op (the one drm-provider's plan
/// names). The rights receipt is threaded in by the executor; the authority RECOVERS
/// the escrowed CEK from the rights-bound `key_envelope` and re-seals it to the
/// published session key — no raw CEK ever handed in. Produces the release receipt
/// (threaded onward into decrypt) and the sealed material (carried in the context).
struct KeyHandle {
    key: Rc<RefCell<Option<Capsule>>>,
    material: Rc<RefCell<Option<KeyOpenMaterial>>>,
}

impl ProviderHandle for KeyHandle {
    fn provider(&self) -> &str {
        "key"
    }
    fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        // Fail closed if the runtime opened the key transport before binding the session
        // material it provisions once the rail is up (escrow + transcript AAD).
        let m = self
            .material
            .borrow()
            .clone()
            .ok_or("key transport opened before the runtime bound its session material")?;
        let mut request = key_release_request_base(&m.kid_hex, &m.wrapped_cek_b64);
        thread_into(&mut request, inputs);
        let mut guard = self.key.borrow_mut();
        let key = guard.as_mut().ok_or("key capsule was already torn down")?;
        let release = ok_data(
            &key.call(&json!({
                "op": "release",
                "request": request,
                "session": {
                    "decrypt_session_pub_b64": m.session_pub_b64,
                    "producer_vk_b64": m.producer_vk_b64,
                    "aad_b64": m.aad_b64,
                    "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                    "content_hash_b64": m.content_hash_b64,
                    "nonce_b64": m.nonce_b64,
                    "now_unix": NOW_UNIX,
                }
            }))?,
            "key release",
        )?;
        let material = release["material"].clone();
        if material["sealed_cek_b64"].as_str().unwrap_or_default().is_empty() {
            return Err(format!("key-provider returned no sealed material: {release}"));
        }
        // Containment on the key->decrypt wire: neither the raw CEK nor the escrow blob is echoed.
        let release_str = serde_json::to_string(&release).map_err(|e| e.to_string())?;
        if release_str.contains(GOLDEN_CEK_B64) {
            return Err("raw CEK leaked in the key-provider response".to_string());
        }
        if release_str.contains(&m.wrapped_cek_b64) {
            return Err("the producer escrow blob was echoed by the key authority".to_string());
        }
        Ok(BTreeMap::from([
            ("ReleaseReceiptV1".to_string(), release_receipt_json()),
            ("material".to_string(), material),
        ]))
    }
}

/// Injected `decrypt` capability: `open_session_v1` pushes the executor-threaded
/// release receipt + sealed material into the boundary, which unwraps in-VM and
/// decrypts a real CENC segment, returning ONLY a scoped session — no CEK, no plaintext
/// crosses the boundary. The `render` step is also a decrypt-provider step but is a
/// no-op here (the smoke drives the open, not playback).
struct DecryptHandle {
    decrypt: Rc<RefCell<Option<Capsule>>>,
}

impl ProviderHandle for DecryptHandle {
    fn provider(&self) -> &str {
        "decrypt"
    }
    fn run(&mut self, inputs: &StepInputs) -> Result<BTreeMap<String, Value>, String> {
        if inputs.step.name != "decrypt_session" {
            return Ok(BTreeMap::new());
        }
        // The sealed material rides the context alongside the release receipt (it is
        // not a plan binding edge — only the receipt is — so read it from the context).
        let material = inputs
            .artifact("material")
            .ok_or("decrypt_session lost the sealed material produced by key_release")?
            .clone();
        let mut request = decrypt_request_base();
        thread_into(&mut request, inputs);
        let mut guard = self.decrypt.borrow_mut();
        let decrypt = guard.as_mut().ok_or("decrypt capsule was already torn down")?;
        let open = decrypt.call(&json!({
            "op": "open_session_v1",
            "request": request,
            "material": material,
            "now_unix": NOW_UNIX,
        }))?;
        let open_data = ok_data(&open, "decrypt open_session_v1")?;
        if open_data["decision"].as_str() != Some("opened") {
            return Err(format!("decrypt did not open the session: {open}"));
        }
        let session = &open_data["session"];
        if session["is_protected"].as_bool() != Some(true) {
            return Err(format!("decrypt did not report a protected segment: {open}"));
        }
        if session["sample_count"].as_u64() != Some(EXPECTED_SAMPLE_COUNT) {
            return Err(format!(
                "decrypt sample_count mismatch: expected {EXPECTED_SAMPLE_COUNT}, got {open}"
            ));
        }
        // Containment at the consumer edge: neither CEK nor plaintext crosses the boundary.
        let open_str = serde_json::to_string(&open).map_err(|e| e.to_string())?;
        if open_str.contains(GOLDEN_CEK_B64) || open_str.contains("the quick brown fox") {
            return Err("CEK or plaintext leaked from the decrypt boundary".to_string());
        }
        Ok(BTreeMap::from([(
            "decrypt_session".to_string(),
            session.clone(),
        )]))
    }
}

// The host LAUNCHES three runtime-OWNED transports through three `ProviderLauncher`s. Each
// launcher owns a real capsule BINARY; `from_launchers` brings the provider up (spawn →
// init → the provider PUBLISHES its material) and hands back a transport that owns the
// spawned connection — the same registry type the trusted core uses. Each transport holds
// a shared capsule cell (so `host.shutdown()` tears it down, and the raw transcript-mismatch
// gate can reach the live capsules), and OPENS a fresh per-provider `ProviderHandle` on each
// open — mirroring PC2's `BackendSessionService` launching a backend view per session and
// minting a fresh view per request.

/// Runtime-owned `rights` transport: opens a `RightsHandle` over the rights capsule.
struct RightsTransport {
    rights: Rc<RefCell<Option<Capsule>>>,
    chain_attestation: Value,
    chain_mode: String,
}

impl ProviderTransport for RightsTransport {
    fn provider(&self) -> &str {
        "rights"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(RightsHandle {
            rights: self.rights.clone(),
            chain_attestation: self.chain_attestation.clone(),
            chain_mode: self.chain_mode.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        // The host owns this transport, so it owns the capsule's teardown.
        if let Some(rights) = self.rights.borrow_mut().as_mut() {
            rights.shutdown();
        }
        Ok(())
    }
}

/// Runtime-owned `key` transport: opens a `KeyHandle` over the key-provider capsule. The
/// per-open escrow + session material is BOUND by the runtime (`material`) once the rail is
/// up, so a clone of that shared cell reaches the handle.
struct KeyTransport {
    key: Rc<RefCell<Option<Capsule>>>,
    material: Rc<RefCell<Option<KeyOpenMaterial>>>,
}

impl ProviderTransport for KeyTransport {
    fn provider(&self) -> &str {
        "key"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(KeyHandle {
            key: self.key.clone(),
            material: self.material.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        if let Some(key) = self.key.borrow_mut().as_mut() {
            key.shutdown();
        }
        Ok(())
    }
}

/// Runtime-owned `decrypt` transport: opens a `DecryptHandle` over the decrypt boundary.
struct DecryptTransport {
    decrypt: Rc<RefCell<Option<Capsule>>>,
}

impl ProviderTransport for DecryptTransport {
    fn provider(&self) -> &str {
        "decrypt"
    }
    fn open(&self) -> Box<dyn ProviderHandle> {
        Box::new(DecryptHandle {
            decrypt: self.decrypt.clone(),
        })
    }
    fn shutdown(&mut self) -> Result<(), String> {
        if let Some(decrypt) = self.decrypt.borrow_mut().as_mut() {
            decrypt.shutdown();
        }
        Ok(())
    }
}

// ── the launchers the host brings the rail up through ──
//
// Each launcher owns a capsule BINARY (not a pre-spawned process). `launch()` spawns the
// capsule, drives its init, and captures the material the provider PUBLISHES into the shared
// `RailMaterial` — the runtime-core analogue of `createSession` launching a backend view
// (`BackendSessionService.ts:307`) whose `createNew()` mints + publishes its session key.
// The spawned capsule lands in a shared cell the launcher's transport (and the raw gate)
// reads, so the HOST owns the live process from launch through `shutdown`.

/// Launches the `rights` provider: spawns the rights capsule (if a binary was supplied) and
/// returns a transport that renders the (live or mocked) on-chain ownership answer.
struct RightsLauncher {
    rights_bin: Option<String>,
    cell: Rc<RefCell<Option<Capsule>>>,
    chain_attestation: Value,
    chain_mode: String,
}

impl ProviderLauncher for RightsLauncher {
    fn provider(&self) -> &str {
        "rights"
    }
    fn launch(self: Box<Self>) -> Result<Box<dyn ProviderTransport>, String> {
        if let Some(bin) = &self.rights_bin {
            *self.cell.borrow_mut() = Some(Capsule::spawn("rights-provider", bin)?);
        }
        Ok(Box::new(RightsTransport {
            rights: self.cell.clone(),
            chain_attestation: self.chain_attestation,
            chain_mode: self.chain_mode,
        }))
    }
}

/// Launches the `key` provider with the runtime-selected authority `init_config` (a durable
/// key-store path for `reference`, or an external descriptor path for `dkms`), drives `init`, and
/// PUBLISHES the authority's verifying + escrow-recipient keys into the shared rail material.
/// Whichever backend, those keys are STABLE across launches (reference re-derives from its persisted
/// master; dkms re-derives from the provisioned descriptor), so the CEK escrowed at PUBLISH time
/// recovers; this launch only re-resolves the same recipient. The OPEN PATH is backend-agnostic —
/// only `init_config` differs (PC2's `getSessionView` dispatch, `BackendSessionService.ts:368`).
struct KeyLauncher {
    key_bin: String,
    init_config: Value,
    cell: Rc<RefCell<Option<Capsule>>>,
    rail: Rc<RefCell<RailMaterial>>,
    material: Rc<RefCell<Option<KeyOpenMaterial>>>,
}

impl ProviderLauncher for KeyLauncher {
    fn provider(&self) -> &str {
        "key"
    }
    fn launch(self: Box<Self>) -> Result<Box<dyn ProviderTransport>, String> {
        let mut key = Capsule::spawn("key-provider", &self.key_bin)?;
        let init = ok_data(
            &key.call(&json!({ "op": "init", "config": self.init_config }))?,
            "key init",
        )?;
        let vk_b64 = init["seal_verifying_key_b64"]
            .as_str()
            .ok_or("key-provider did not publish a seal verifying key (build with --features key-authority-ref)")?
            .to_string();
        let recipient_pub_b64 = init["seal_recipient_pub_b64"]
            .as_str()
            .ok_or("key-provider did not publish an escrow recipient key (build with --features key-authority-ref)")?
            .to_string();
        {
            let mut rail = self.rail.borrow_mut();
            rail.vk_b64 = Some(vk_b64);
            rail.recipient_pub_b64 = Some(recipient_pub_b64);
        }
        *self.cell.borrow_mut() = Some(key);
        Ok(Box::new(KeyTransport {
            key: self.cell.clone(),
            material: self.material.clone(),
        }))
    }
}

/// Launches the `decrypt` provider: spawns the boundary, configures it to TRUST the
/// authority's published verifying key (read from the rail — so `key` must launch first),
/// and PUBLISHES the boundary's in-sandbox session key into the rail.
struct DecryptLauncher {
    decrypt_bin: String,
    cell: Rc<RefCell<Option<Capsule>>>,
    rail: Rc<RefCell<RailMaterial>>,
}

impl ProviderLauncher for DecryptLauncher {
    fn provider(&self) -> &str {
        "decrypt"
    }
    fn launch(self: Box<Self>) -> Result<Box<dyn ProviderTransport>, String> {
        let vk_b64 = self
            .rail
            .borrow()
            .vk_b64
            .clone()
            .ok_or("decrypt launched before the key authority published its verifying key")?;
        let mut decrypt = Capsule::spawn("decrypt-provider", &self.decrypt_bin)?;
        let init = ok_data(
            &decrypt.call(&json!({ "op": "init", "config": { "authority_vk_b64": vk_b64 } }))?,
            "decrypt init",
        )?;
        let session_pub_b64 = init["decrypt_session_public_key_b64"]
            .as_str()
            .ok_or("decrypt-provider did not publish a session key (build with --features rail-material)")?
            .to_string();
        self.rail.borrow_mut().session_pub_b64 = Some(session_pub_b64);
        *self.cell.borrow_mut() = Some(decrypt);
        Ok(Box::new(DecryptTransport {
            decrypt: self.cell.clone(),
        }))
    }
}

/// The host's runtime-owned PLAN SOURCE: asks the REAL drm-provider for the canonical
/// open plan (the drm-provider holds no authority and decrypts nothing — it only emits
/// the `planned` plan). Spawns + shuts down the drm capsule per fetch and validates the
/// plan carries the content identity flowing through the chain. `tamper` (shared with
/// `run`) models a source that yields a corrupted plan so the host must fail closed.
struct SmokePlanSource {
    drm_bin: String,
    tamper: Rc<std::cell::Cell<bool>>,
}

impl PlanSource for SmokePlanSource {
    fn fetch(&mut self, _content_id: &str, _viewer_interface: &str) -> Result<Value, String> {
        let mut drm = Capsule::spawn("drm-provider", &self.drm_bin)?;
        ok_data(&drm.call(&json!({ "op": "status" }))?, "drm status")?;
        let mut plan = ok_data(&drm.call(&drm_open_request())?, "drm open")?;
        drm.shutdown();
        if plan["content_id"].as_str() != Some(cid().as_str()) {
            return Err(format!(
                "plan content_id {} != the content identity flowing through the chain {}",
                plan["content_id"],
                cid()
            ));
        }
        if plan["action"].as_str() != Some(ACTION) {
            return Err(format!("plan action {} != {ACTION}", plan["action"]));
        }
        if self.tamper.get() {
            // Relabel the key_release input edge — the real key-provider
            // (deny_unknown_fields over a required `rights_receipt`) must reject it.
            for b in plan["bindings"].as_array_mut().ok_or("plan has no bindings")? {
                if b["into_step"] == json!("key_release") {
                    b["into_field"] = json!("bogus_edge");
                }
            }
        }
        Ok(plan)
    }
}

/// How far this run drives the open. An operator runs `open` (publish → launch → open →
/// persist → durable CEK-free readback); the consumer smoke runs `verify`, which ALSO drives
/// the two adversarial fail-closed gates (a transcript-mismatched seal; a tampered plan edge).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OpenMode {
    Open,
    Verify,
}

/// Which `key-provider` authority backend the open binds to. The OPEN PATH is backend-agnostic:
/// only the backend's `init` config differs (a durable key-store path vs an external descriptor
/// path); the publish → launch → open → recover/re-seal flow is byte-identical. Mirrors PC2's
/// `getSessionView(token)` dispatching on `stored.backend` to construct the per-backend view
/// (`BackendSessionService.ts:368`–`:377`) while the downstream open is the same.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AuthorityBackend {
    /// In-runtime dev authority: the runtime GENERATES + persists its own master seed (durable key store).
    Reference,
    /// External authority: a SECRET-HOLDING node owns the master; the runtime holds only its
    /// PUBLIC identity (a public-only descriptor) and DELEGATES recovery to the node — never the
    /// master, never the raw CEK.
    Dkms,
}

impl AuthorityBackend {
    fn tag(self) -> &'static str {
        match self {
            AuthorityBackend::Reference => "reference",
            AuthorityBackend::Dkms => "dkms",
        }
    }
}

/// The TYPED runtime-open config — the SINGLE input to this binary, read from a JSON file
/// (argv[1]) or `DDRM_OPEN_CONFIG`. The runtime bootstrap is config-driven: NO caller assembles
/// the host. Mirrors PC2 booting `sessionService` ONCE from config (`BackendSessionService.ts:495`),
/// not per request.
#[derive(Debug)]
struct OpenConfig {
    key_bin: String,
    decrypt_bin: String,
    drm_bin: String,
    rights_bin: Option<String>,
    chain_bin: Option<String>,
    /// Durable working dir (key store + publish fixture + receipts). Default: a per-pid temp dir.
    work_dir: Option<String>,
    /// Keep the durable artifacts after a successful run (default: clean up).
    keep_work_dir: bool,
    mode: OpenMode,
    /// Which key authority backend the open binds to (`authority.backend`, default `reference`).
    authority: AuthorityBackend,
    /// The EXTERNAL dKMS authority NODE binary (`authority.dkms_authority_bin`). Required when
    /// `authority.backend == dkms`: the publish phase provisions it (master stays in the node) and
    /// the runtime delegates recovery to it. Absent for `reference`.
    dkms_authority_bin: Option<String>,
}

impl OpenConfig {
    fn from_json(v: &Value) -> Result<Self, String> {
        let obj = v.as_object().ok_or("config must be a JSON object")?;
        let req = |k: &str| {
            obj.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("config is missing required string `{k}`"))
        };
        let opt = |k: &str| obj.get(k).and_then(Value::as_str).map(str::to_string);
        let mode = match obj.get("mode").and_then(Value::as_str).unwrap_or("verify") {
            "open" => OpenMode::Open,
            "verify" => OpenMode::Verify,
            other => return Err(format!("config `mode` must be \"open\" or \"verify\", got {other:?}")),
        };
        // `authority` is an OBJECT (room to carry per-backend descriptors later); today only its
        // `backend` tag is read. Absent → reference (back-compat). Fail-closed on an unknown tag or
        // a non-object `authority`.
        let (authority, dkms_authority_bin) = match obj.get("authority") {
            None => (AuthorityBackend::Reference, None),
            Some(Value::Object(auth)) => {
                let backend = match auth.get("backend").and_then(Value::as_str).unwrap_or("reference") {
                    "reference" => AuthorityBackend::Reference,
                    "dkms" => AuthorityBackend::Dkms,
                    other => return Err(format!("config `authority.backend` must be \"reference\" or \"dkms\", got {other:?}")),
                };
                let node_bin = auth.get("dkms_authority_bin").and_then(Value::as_str).map(str::to_string);
                if backend == AuthorityBackend::Dkms && node_bin.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    return Err("config `authority.dkms_authority_bin` is required when authority.backend is \"dkms\"".to_string());
                }
                (backend, node_bin)
            }
            Some(_) => return Err("config `authority` must be an object".to_string()),
        };
        Ok(Self {
            key_bin: req("key_bin")?,
            decrypt_bin: req("decrypt_bin")?,
            drm_bin: req("drm_bin")?,
            rights_bin: opt("rights_bin"),
            chain_bin: opt("chain_bin"),
            work_dir: opt("work_dir"),
            keep_work_dir: obj.get("keep_work_dir").and_then(Value::as_bool).unwrap_or(false),
            mode,
            authority,
            dkms_authority_bin,
        })
    }
}

fn run(cfg: &OpenConfig) -> Result<(), String> {
    let key_bin = &cfg.key_bin;
    let decrypt_bin = &cfg.decrypt_bin;
    let drm_bin = &cfg.drm_bin;
    let rights_bin = cfg.rights_bin.as_ref();
    let chain_bin = cfg.chain_bin.as_ref();

    println!(
        "== dDRM runtime-core open (config-driven DrmHost::launch -> open; authority={}; drm -> rights -> key -> decrypt) ==",
        cfg.authority.tag()
    );

    // --- PUBLISH PHASE (the producer, run ONCE before any open). The authority is now
    // backed by a DURABLE KEY STORE, so its escrow recipient is STABLE across launches. The
    // producer brings it up once, escrows the content CEK to that recipient, and writes a
    // durable publish fixture — exactly as PC2 escrows the CEK to the stable DEFAULT_AUTHORITY
    // at encode time (`dashPackager.ts` `encryptMediaCEK`). The open path below NEVER escrows;
    // it READS this fixture, collapsing the Day-79/80 "launch → publish → escrow → bind" dance
    // into "escrow at publish; launch resolves the same recipient." For `dkms`, the publish
    // phase ALSO provisions the immutable external-authority descriptor the open then resolves.
    let work_dir = match &cfg.work_dir {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::env::temp_dir().join(format!("ddrm-open-{}", std::process::id())),
    };
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("create work dir: {e}"))?;
    let key_store_path = work_dir.join("authority-key-store.json").to_string_lossy().into_owned();
    let descriptor_path = work_dir.join("dkms-authority.json");
    let fixture_path = work_dir.join("publish-escrow.json");
    // The dKMS NODE's master store — node-local, the SECRET stays here. The runtime creates it via
    // the node at provision time + names it in the env the node reads, but NEVER reads it itself.
    let node_store_path = work_dir.join("dkms-node-master.json").to_string_lossy().into_owned();
    if cfg.authority == AuthorityBackend::Dkms {
        // Grant the (later) node child its store via the env it resolves — the key-provider client
        // that spawns the node never passes or sees this path; it's the node's own concern.
        std::env::set_var("DKMS_AUTHORITY_KEY_STORE", &node_store_path);
    }
    let escrow = publish_escrow(
        key_bin,
        &key_store_path,
        &fixture_path,
        cfg.authority,
        &descriptor_path,
        cfg.dkms_authority_bin.as_deref(),
        &node_store_path,
    )?;
    // The open path binds to the selected backend via its `init` config ONLY — the publish →
    // launch → open → recover/re-seal flow below is byte-identical across backends.
    let key_init_config = match cfg.authority {
        AuthorityBackend::Reference => json!({ "backend": "reference", "authority_key_store": key_store_path }),
        AuthorityBackend::Dkms => json!({
            "backend": "dkms",
            "dkms_authority_descriptor": descriptor_path.to_string_lossy(),
        }),
    };
    // For `dkms`, snapshot the descriptor so we can PROVE the runtime treated it as immutable
    // published data (read-only) across the whole open — the key-provider only ever READS it.
    // ALSO assert the descriptor handed to the runtime is PUBLIC-ONLY: it must carry NO master seed
    // (the secret stays in the node), proving the master never crosses into the runtime.
    let descriptor_before = match cfg.authority {
        AuthorityBackend::Dkms => {
            let bytes = std::fs::read(&descriptor_path).map_err(|e| format!("read dkms descriptor: {e}"))?;
            let desc: Value = serde_json::from_slice(&bytes).map_err(|e| format!("parse dkms descriptor: {e}"))?;
            if desc.get("authority_master_seed_b64").is_some() {
                return Err("the dkms descriptor handed to the runtime carries a master seed — the secret must stay in the node".to_string());
            }
            if desc.get("verifying_key_b64").and_then(Value::as_str).is_none()
                || desc.get("recipient_pub_b64").and_then(Value::as_str).is_none()
                || desc.get("authority_endpoint").and_then(Value::as_str).is_none()
            {
                return Err("the dkms descriptor is not a complete PUBLIC-ONLY descriptor (need vk + recipient + endpoint)".to_string());
            }
            Some(bytes)
        }
        AuthorityBackend::Reference => None,
    };
    step(1, &format!(
        "producer (publish-time): escrowed the CEK to the {} authority's STABLE recipient + wrote a durable publish fixture{}",
        cfg.authority.tag(),
        if cfg.authority == AuthorityBackend::Dkms {
            " + provisioned the EXTERNAL dkms NODE (master stays in the node) + a PUBLIC-ONLY descriptor"
        } else { "" },
    ));

    // --- the HOST LAUNCHES the rail. The smoke hands the host three launchers (each owning a
    // capsule BINARY); `DrmHost::launch` (the trusted core's composition helper) brings each
    // provider up in dependency order: `key` first (RELAUNCHED from the SAME durable key store
    // → the SAME stable recipient), then `decrypt` (trusts the published vk; mints + publishes
    // its per-open session key), then `rights`. The shared cells receive the spawned capsules
    // so the host owns them through `shutdown` (and the raw transcript gate can still reach
    // them). The runtime-core analogue of `BackendSessionService.createSession` launching a
    // backend view (`:307`).
    let (attestation, chain_mode) = chain_attestation(chain_bin)?;
    let rail = Rc::new(RefCell::new(RailMaterial::default()));
    let key_material: Rc<RefCell<Option<KeyOpenMaterial>>> = Rc::new(RefCell::new(None));
    let rights_cell: Rc<RefCell<Option<Capsule>>> = Rc::new(RefCell::new(None));
    let key_cell: Rc<RefCell<Option<Capsule>>> = Rc::new(RefCell::new(None));
    let decrypt_cell: Rc<RefCell<Option<Capsule>>> = Rc::new(RefCell::new(None));

    let receipts_dir = work_dir.join("receipts");
    let store = DurableEventStore::open(&receipts_dir)?;
    let tamper = Rc::new(std::cell::Cell::new(false));
    let mut host = DrmHost::launch(
        Box::new(SmokePlanSource {
            drm_bin: drm_bin.clone(),
            tamper: tamper.clone(),
        }),
        vec![
            Box::new(KeyLauncher {
                key_bin: key_bin.clone(),
                init_config: key_init_config.clone(),
                cell: key_cell.clone(),
                rail: rail.clone(),
                material: key_material.clone(),
            }),
            Box::new(DecryptLauncher {
                decrypt_bin: decrypt_bin.clone(),
                cell: decrypt_cell.clone(),
                rail: rail.clone(),
            }),
            Box::new(RightsLauncher {
                rights_bin: rights_bin.cloned(),
                cell: rights_cell.clone(),
                chain_attestation: attestation,
                chain_mode: chain_mode.clone(),
            }),
        ],
        Box::new(PersistingEventSink::new(store)),
    )?;
    step(2, &format!(
        "runtime-core host: LAUNCHED the rail via DrmHost::launch — key ({}), decrypt (per-open session key), rights",
        match cfg.authority {
            AuthorityBackend::Reference => "reference, relaunched from the durable store",
            AuthorityBackend::Dkms => "dkms, PUBLIC-ONLY client (delegates recovery to the external node)",
        }
    ));

    // --- the rail is up: the runtime binds the key transport's per-open material from the
    // PUBLISH FIXTURE (no inline escrow). First prove the authority identity is STABLE: the
    // recipient the relaunched authority published MUST equal the one the producer escrowed to.
    let (recipient_pub_b64, session_pub_b64) = {
        let rail = rail.borrow();
        (
            rail.recipient_pub_b64.clone().ok_or("the key authority published no escrow recipient")?,
            rail.session_pub_b64.clone().ok_or("the decrypt boundary published no session key")?,
        )
    };
    if recipient_pub_b64 != escrow.recipient_pub_b64 {
        return Err(
            "the relaunched authority's recipient differs from the publish-time recipient — the durable key store is not stable".to_string(),
        );
    }
    // Re-read the fixture from disk (the open path consumes the producer's durable artifact,
    // it does not trust the in-memory value) and verify it matches what was written.
    let fixture = PublishEscrow::from_json(
        &serde_json::from_slice(&std::fs::read(&fixture_path).map_err(|e| format!("read publish fixture: {e}"))?)
            .map_err(|e| format!("parse publish fixture: {e}"))?,
    )?;
    if fixture.recipient_pub_b64 != recipient_pub_b64 {
        return Err("publish fixture recipient does not match the launched authority".to_string());
    }
    // The session binding is the ONLY per-open part: compute the transcript AAD over the
    // decrypt boundary's freshly-minted session key, then bind the key transport's material
    // (wrapped CEK + producer vk + content hash + nonce come from the publish fixture).
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;
    let content_hash = B64.decode(&fixture.content_hash_b64).map_err(|e| e.to_string())?;
    let nonce = B64.decode(&fixture.nonce_b64).map_err(|e| e.to_string())?;
    let aad = transcript_aad(&session_pub, &content_hash, &nonce);
    let kid_hex = fixture.kid_hex.clone();
    let wrapped_cek_b64 = fixture.wrapped_cek_b64.clone();
    let producer_vk_b64 = fixture.producer_vk_b64.clone();
    *key_material.borrow_mut() = Some(KeyOpenMaterial {
        kid_hex: kid_hex.clone(),
        wrapped_cek_b64: wrapped_cek_b64.clone(),
        producer_vk_b64: producer_vk_b64.clone(),
        session_pub_b64: session_pub_b64.clone(),
        aad_b64: B64.encode(&aad),
        content_hash_b64: fixture.content_hash_b64.clone(),
        nonce_b64: fixture.nonce_b64.clone(),
    });
    step(3, "runtime-core host: authority recipient STABLE across relaunch; bound key material from the publish fixture + the per-open session key");

    // --- the trusted host owns the open end to end ---
    let report = host.open(&cid(), VIEWER)?;
    if report.artifact("decrypt_session").is_none() {
        return Err("the host finished without opening a decrypt session".to_string());
    }
    step(4, "drm-provider (host plan source): emitted the canonical plan (planned); host parsed + validated its order + edges");
    step(5, &format!("rights-provider: on-chain ownership ({chain_mode}) -> allowed; typed receipt issued"));
    step(6, "key-provider: canonical `release` recovered the publish-escrowed CEK + re-sealed it to the session (no raw CEK, no shim)");
    step(7, "decrypt-provider: unwrapped in-VM + decrypted the segment; only a scoped session returned");
    // The host emitted the plan's runtime-OWNED post-steps (no provider performs these).
    if report.events_emitted != ["release_receipt", "protected_content.open.audit"] {
        return Err(format!(
            "host did not emit the plan's runtime events in order: {:?}",
            report.events_emitted
        ));
    }
    // The host PERSISTED a durable record per runtime event. Read them back through a FRESH
    // `DurableEventStore::load` (a brand-new reader, as if a separate process had opened the
    // store) — proving the receipt + audit are durable across process boundaries, not just
    // live in memory. The analogue of `FileSessionStore::loadAll` restoring on startup.
    let receipt_files = DurableEventStore::load(&receipts_dir)?;
    if receipt_files.len() != 2 {
        return Err(format!("expected 2 durable open records, found {}", receipt_files.len()));
    }
    // The record may carry artifact NAMES (safe — PC2 logs step names) but NEVER any secret
    // VALUE. Forbid the concrete secret bytes that flowed through this open: the CEK, the
    // sealed/escrowed material, the ciphertext, and the session/producer keys.
    for (name, record) in &receipt_files {
        let blob = serde_json::to_string(record).map_err(|e| e.to_string())?;
        let secrets = [
            ("CEK", GOLDEN_CEK_B64),
            ("ciphertext", GOLDEN_CIPHERTEXT_B64),
            ("wrapped/escrowed CEK", wrapped_cek_b64.as_str()),
            ("producer vk", producer_vk_b64.as_str()),
            ("decrypt session key", session_pub_b64.as_str()),
        ];
        for (what, secret) in secrets {
            if blob.contains(secret) {
                return Err(format!("durable open record {name} leaks the {what}"));
            }
        }
        // It records artifact NAMES, never the artifact map (values).
        if record.get("artifacts").is_some() {
            return Err(format!("durable record {name} embeds artifact values"));
        }
        if record["content_id"].as_str() != Some(cid().as_str()) {
            return Err(format!("durable record {name} has wrong content_id"));
        }
    }
    step(8, "runtime-core host: PERSISTED the runtime-owned post-steps (release_receipt + audit) as durable CEK-free records; read back through a fresh DurableEventStore");

    // In `open` mode (the operator path) we are done: a real open ran and a durable, CEK-free
    // record persisted. `verify` mode (the consumer smoke) additionally drives the two
    // adversarial fail-closed gates below before tearing the rail down.
    if cfg.mode == OpenMode::Verify {
    // --- fail-closed #1 (crypto binding): a replayed/altered transcript must not open.
    // Re-seal to a DIFFERENT nonce while the material still names the original — the
    // boundary rebuilds the original transcript and the seal cannot open.
    let bad_nonce = b"consumer-smoke-nonce-9".to_vec();
    let bad_aad = transcript_aad(&session_pub, &content_hash, &bad_nonce);
    let mut bad_req = key_release_request_base(&kid_hex, &wrapped_cek_b64);
    bad_req
        .as_object_mut()
        .expect("key release request is an object")
        .insert("rights_receipt".to_string(), fallback_rights_receipt());
    let bad_release = ok_data(
        &key_cell
            .borrow_mut()
            .as_mut()
            .ok_or("key capsule torn down before the raw transcript gate")?
            .call(&json!({
                "op": "release",
                "request": bad_req,
                "session": {
                    "decrypt_session_pub_b64": session_pub_b64,
                    "producer_vk_b64": producer_vk_b64,
                    "aad_b64": B64.encode(&bad_aad),
                    "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                    "content_hash_b64": B64.encode(&content_hash),
                    "nonce_b64": B64.encode(&nonce),
                    "now_unix": NOW_UNIX,
                }
            }))?,
        "key release (mismatch)",
    )?;
    let mut bad_open_req = decrypt_request_base();
    bad_open_req
        .as_object_mut()
        .expect("decrypt request is an object")
        .insert("release_receipt".to_string(), release_receipt_json());
    bad_open_req["object_cid"] = json!(cid());
    bad_open_req["viewer_interface"] = json!(VIEWER);
    let bad_open = decrypt_cell
        .borrow_mut()
        .as_mut()
        .ok_or("decrypt capsule torn down before the raw transcript gate")?
        .call(&json!({
            "op": "open_session_v1",
            "request": bad_open_req,
            "material": bad_release["material"].clone(),
            "now_unix": NOW_UNIX,
        }))?;
    if bad_open.get("data").and_then(|d| d.get("decision")).and_then(Value::as_str) == Some("opened") {
        return Err(format!("a transcript-mismatched seal must NOT open: {bad_open}"));
    }
    step(9, "decrypt-provider: a transcript-mismatched seal failed closed");

    // --- fail-closed #2 (plan integrity): flip the host's plan source into TAMPER mode
    // (it relabels the key_release input edge) and re-open through the SAME host. The
    // host fetches the corrupted plan, threads the rights receipt into the wrong field,
    // and the real key-provider (deny_unknown_fields over a required `rights_receipt`)
    // rejects it — proving the host only proceeds when the plan's edges are intact, and
    // that a bad plan FROM THE SOURCE fails closed, cross-binary, with no event emitted.
    let persisted_before = DurableEventStore::load(&receipts_dir)?.len();
    tamper.set(true);
    match host.open(&cid(), VIEWER) {
        Ok(_) => return Err("a tampered plan edge must NOT drive a successful open".to_string()),
        Err(_) => step(10, "runtime-core host: a tampered binding edge from the plan source failed closed at the real key-provider"),
    }
    if DurableEventStore::load(&receipts_dir)?.len() != persisted_before {
        return Err("a failed open must persist no runtime-event record".to_string());
    }
    } // end verify-only adversarial gates

    // The HOST owns the rail it was handed: shutting it down tears down every runtime-owned
    // transport (each shuts down the capsule it owns) — no manual per-capsule shutdown here.
    host.shutdown()?;
    step(11, "runtime-core host: shut down — every runtime-owned transport tore down its capsule");

    // --- dkms: PROVE the external-authority descriptor was IMMUTABLE published data — the
    // key-provider RESOLVED its identity from it and NEVER wrote it back (PC2 caches the
    // provisioned descriptor once + only reads it, `chipotle-client.ts:935`/`:950`).
    if let Some(before) = descriptor_before {
        let after = std::fs::read(&descriptor_path).map_err(|e| format!("re-read dkms descriptor: {e}"))?;
        if after != before {
            return Err("the dkms authority descriptor was mutated across the open — it must be read-only published data".to_string());
        }
        step(12, "runtime-core host: the EXTERNAL dkms identity was PUBLIC-ONLY + read-only across the open (master stayed in the node; recovery was DELEGATED — never the secret)");
    }
    // The cells kept for the raw gate are now stale (the host tore the processes down); drop
    // them and clean up the durable artifacts (key store + fixture + receipts) unless the
    // config asks to keep them.
    drop(rights_cell);
    drop(key_cell);
    drop(decrypt_cell);
    if !cfg.keep_work_dir {
        let _ = std::fs::remove_dir_all(&work_dir);
    }
    Ok(())
}

/// Read the typed JSON config from argv[1] (a path) or `$DDRM_OPEN_CONFIG`. Fail-closed:
/// no config path, an unreadable file, or malformed JSON is an error — the runtime never
/// guesses its providers/dirs.
fn load_config() -> Result<OpenConfig, String> {
    let path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("DDRM_OPEN_CONFIG").ok())
        .ok_or("usage: ddrm-runtime-open <config.json>  (or set DDRM_OPEN_CONFIG)")?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read config {path}: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("parse config {path}: {e}"))?;
    OpenConfig::from_json(&value)
}

fn main() {
    let result = load_config().and_then(|cfg| run(&cfg));
    match result {
        Ok(()) => {
            println!("\nddrm-runtime-open: PASS — the runtime-core host opened end to end (key -> decrypt), fail-closed, no key/plaintext leak.");
        }
        Err(e) => {
            eprintln!("\nddrm-runtime-open: FAIL — {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_full_config_and_defaults_to_verify() {
        let cfg = OpenConfig::from_json(&json!({
            "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m",
            "rights_bin": "/r", "work_dir": "/w"
        }))
        .expect("a config with the required provider binaries parses");
        assert_eq!(cfg.key_bin, "/k");
        assert_eq!(cfg.rights_bin.as_deref(), Some("/r"));
        assert_eq!(cfg.chain_bin, None);
        assert_eq!(cfg.work_dir.as_deref(), Some("/w"));
        assert!(!cfg.keep_work_dir);
        assert!(cfg.mode == OpenMode::Verify, "mode defaults to verify");
    }

    #[test]
    fn open_mode_is_explicit() {
        let cfg = OpenConfig::from_json(&json!({
            "mode": "open", "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m"
        }))
        .expect("open mode parses");
        assert!(cfg.mode == OpenMode::Open);
    }

    #[test]
    fn fails_closed_on_a_missing_required_binary() {
        let err = OpenConfig::from_json(&json!({ "decrypt_bin": "/d", "drm_bin": "/m" }))
            .expect_err("a config without key_bin must fail closed");
        assert!(err.contains("key_bin"), "the error names the missing field: {err}");
    }

    #[test]
    fn fails_closed_on_an_unknown_mode() {
        let err = OpenConfig::from_json(&json!({
            "mode": "wide-open", "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m"
        }))
        .expect_err("an unknown mode must fail closed");
        assert!(err.contains("mode"), "the error names the bad field: {err}");
    }

    #[test]
    fn fails_closed_when_config_is_not_an_object() {
        assert!(OpenConfig::from_json(&json!(["not", "an", "object"])).is_err());
    }

    #[test]
    fn authority_defaults_to_reference_and_parses_dkms() {
        let base = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        assert!(OpenConfig::from_json(&base).unwrap().authority == AuthorityBackend::Reference);

        let mut dkms = base.clone();
        dkms.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node" }),
        );
        let parsed = OpenConfig::from_json(&dkms).unwrap();
        assert!(parsed.authority == AuthorityBackend::Dkms);
        assert_eq!(parsed.dkms_authority_bin.as_deref(), Some("/node"));

        let mut explicit_ref = base.clone();
        explicit_ref.as_object_mut().unwrap().insert("authority".into(), json!({ "backend": "reference" }));
        let parsed_ref = OpenConfig::from_json(&explicit_ref).unwrap();
        assert!(parsed_ref.authority == AuthorityBackend::Reference);
        assert_eq!(parsed_ref.dkms_authority_bin, None);
    }

    #[test]
    fn dkms_fails_closed_without_a_node_binary() {
        let mut dkms = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        dkms.as_object_mut().unwrap().insert("authority".into(), json!({ "backend": "dkms" }));
        let err = OpenConfig::from_json(&dkms)
            .expect_err("dkms without a node binary must fail closed");
        assert!(err.contains("dkms_authority_bin"), "the error names the missing field: {err}");
    }

    #[test]
    fn fails_closed_on_an_unknown_or_malformed_authority() {
        let mut bad_backend = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        bad_backend.as_object_mut().unwrap().insert("authority".into(), json!({ "backend": "lit-please" }));
        assert!(OpenConfig::from_json(&bad_backend)
            .expect_err("an unknown authority backend must fail closed")
            .contains("authority.backend"));

        let mut not_object = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        not_object.as_object_mut().unwrap().insert("authority".into(), json!("dkms"));
        assert!(OpenConfig::from_json(&not_object).is_err());
    }
}
