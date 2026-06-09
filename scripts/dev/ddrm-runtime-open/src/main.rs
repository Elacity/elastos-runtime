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
    open_event_record, DrmHost, DurableEventStore, EventStore, ExecutionReport, OpenContext,
    PlanSource, ProviderHandle, ProviderLauncher, ProviderTransport, RuntimeEventSink, StepInputs,
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

/// The probe's side of an ESTABLISHED encrypted channel to a network dKMS node (Day 105–108):
/// requests sealed to the node's ATTESTED channel KEM key under the probe's caller identity,
/// responses opened with the probe's ephemeral secret and verified under the PINNED node identity —
/// the SAME channel discipline the production `key-provider` client enforces.
struct ProbeChannel {
    channel_id: Vec<u8>,
    node_pub: ddrm_envelope::SessionKemPublic,
    secret: ddrm_envelope::SessionKemSecret,
    node_verifier: ddrm_envelope::MlDsa65Verifier,
    signer: ddrm_envelope::seal::MlDsaSealSigner,
    send_seq: u64,
    recv_seq: u64,
}

/// A FRAMED client to the dKMS node's listening endpoint (Day 93–94) — the transport the real
/// `key-provider` uses: a Unix-domain socket path, or `tcp:HOST:PORT` for a node off localhost
/// (Day 105–108). Length-prefixed request/response; the probe connects to the running daemon rather
/// than spawning a child, exercising the SAME wire as production. On TCP, `establish_channel`
/// upgrades the connection to sealed frames (REQUIRED there for any recover).
struct NodeSocket {
    writer: Box<dyn Write>,
    reader: Box<dyn std::io::Read>,
    channel: Option<ProbeChannel>,
}

impl NodeSocket {
    fn connect(endpoint: &str) -> Result<Self, String> {
        let (writer, reader): (Box<dyn Write>, Box<dyn std::io::Read>) =
            match endpoint.strip_prefix("tcp:") {
                Some(addr) => {
                    let stream = std::net::TcpStream::connect(addr)
                        .map_err(|e| format!("connect dkms tcp endpoint {addr}: {e}"))?;
                    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
                    (Box::new(stream), Box::new(reader))
                }
                None => {
                    let stream = std::os::unix::net::UnixStream::connect(endpoint)
                        .map_err(|e| format!("connect dkms socket {endpoint}: {e}"))?;
                    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
                    (Box::new(stream), Box::new(reader))
                }
            };
        Ok(Self { writer, reader, channel: None })
    }

    /// One framed request, one framed response — sealed in both directions once a channel is
    /// established (a tampered/plaintext-downgraded response fails the open here, fail-closed).
    fn call(&mut self, req: &Value) -> Result<Value, String> {
        let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        let wire = match self.channel.as_mut() {
            None => payload,
            Some(ch) => {
                ch.send_seq += 1;
                let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 0, ch.send_seq);
                ddrm_envelope::seal::seal_bound(&ch.node_pub, &payload, &aad, &ch.signer).to_bytes()
            }
        };
        ddrm_envelope::frame::write_frame(&mut self.writer, &wire)
            .map_err(|e| format!("write framed request: {e}"))?;
        let bytes = match ddrm_envelope::frame::read_frame(&mut self.reader) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err("dkms node closed the connection".to_string()),
            Err(e) => return Err(format!("read framed response: {e}")),
        };
        let plain = match self.channel.as_mut() {
            None => bytes,
            Some(ch) => {
                let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&bytes)
                    .map_err(|_| "node sent a non-sealed frame on the channel".to_string())?;
                ch.recv_seq += 1;
                let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 1, ch.recv_seq);
                ddrm_envelope::hybrid_unwrap_bound(&ch.secret, &env, &aad, &ch.node_verifier)
                    .map_err(|_| "node response failed to authenticate on the channel".to_string())?
                    .to_vec()
            }
        };
        serde_json::from_slice(&plain).map_err(|e| format!("non-JSON frame: {e}"))
    }

    /// ESTABLISH the encrypted channel over this connection: drive a `hello` that offers a fresh
    /// client channel KEM key, verify the node's attestation + its CHANNEL-KEY attestation under the
    /// PINNED vk (a substituted key fails closed), and switch the connection to sealed frames.
    /// Returns the hello response data (token etc.) for the caller's own assertions.
    fn establish_channel(
        &mut self,
        pinned_vk_b64: &str,
        caller_seed: [u8; 32],
        challenge: [u8; 32],
        now_unix: u64,
    ) -> Result<Value, String> {
        let (signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);
        let (secret, client_pub) = ddrm_envelope::mint_session();
        let hello = self.call(&json!({
            "op": "hello",
            "challenge_b64": B64.encode(challenge),
            "caller_pub_b64": B64.encode(&caller_vk),
            "now_unix": now_unix,
            "channel_pub_b64": B64.encode(ddrm_envelope::session_public_bytes(&client_pub)),
        }))?;
        let data = ok_data(&hello, "dkms hello (channel establishment)")?;
        let pinned = B64.decode(pinned_vk_b64).map_err(|e| e.to_string())?;
        let node_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned)
            .ok_or("pinned dkms vk is malformed")?;
        let attestation = B64
            .decode(data["attestation_b64"].as_str().unwrap_or(""))
            .map_err(|e| format!("node attestation is not base64: {e}"))?;
        if !ddrm_envelope::verify_attestation(&node_verifier, &challenge, &attestation) {
            return Err("node identity attestation failed under the pinned vk".to_string());
        }
        let node_channel_pub = B64
            .decode(data["channel"]["node_channel_pub_b64"].as_str().unwrap_or(""))
            .map_err(|_| "node returned no/invalid channel key".to_string())?;
        let channel_sig = B64
            .decode(data["channel"]["channel_sig_b64"].as_str().unwrap_or(""))
            .map_err(|_| "node returned no/invalid channel attestation".to_string())?;
        if !ddrm_envelope::verify_channel_key(&node_verifier, &challenge, &node_channel_pub, &channel_sig) {
            return Err("node channel key failed to verify under the pinned identity".to_string());
        }
        let node_pub = ddrm_envelope::session_public_from_bytes(&node_channel_pub)
            .ok_or("node channel key is malformed")?;
        self.channel = Some(ProbeChannel {
            channel_id: challenge.to_vec(),
            node_pub,
            secret,
            node_verifier,
            signer,
            send_seq: 0,
            recv_seq: 0,
        });
        Ok(data)
    }

    /// ADVERSARIAL: write one RAW frame (bypassing the channel sealing) and try to read a response.
    /// Used by the downgrade/tamper gates — a fail-closed node DROPS the connection (`Ok(None)`/err).
    fn raw_round_trip(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, String> {
        ddrm_envelope::frame::write_frame(&mut self.writer, frame)
            .map_err(|e| format!("write raw frame: {e}"))?;
        ddrm_envelope::frame::read_frame(&mut self.reader).map_err(|e| format!("read raw: {e}"))
    }

    /// ADVERSARIAL: seal `req` correctly for the established channel, then FLIP one ciphertext byte
    /// before framing — the MITM-tamper shape. Returns the raw wire bytes to send.
    fn tampered_sealed_frame(&mut self, req: &Value) -> Result<Vec<u8>, String> {
        let ch = self.channel.as_mut().ok_or("tamper gate needs an established channel")?;
        let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        ch.send_seq += 1;
        let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 0, ch.send_seq);
        let mut wire =
            ddrm_envelope::seal::seal_bound(&ch.node_pub, &payload, &aad, &ch.signer).to_bytes();
        let last = wire.len() - 1;
        wire[last] ^= 0x01;
        Ok(wire)
    }
}

/// A running dKMS NODE DAEMON: the node binary launched in LISTEN mode, bound to a Unix-domain
/// socket OR a TCP address (Day 105–108). The runtime CONNECTS to it (it does not own the process
/// the way it owns a child pipe), and this guard KILLS + reaps it on drop so the smoke leaves no
/// orphan. The real-remote-authority shape.
struct DaemonGuard {
    child: Child,
    sock: String,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Only a Unix endpoint leaves a filesystem artifact to clean up.
        if !self.sock.starts_with("tcp:") {
            let _ = std::fs::remove_file(&self.sock);
        }
    }
}

/// Pick a fresh loopback TCP endpoint for a node daemon: bind port 0 (the OS assigns a free port),
/// read it back, release it. (The tiny bind race against the daemon is acceptable for a smoke.)
fn pick_tcp_endpoint() -> Result<String, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("pick tcp endpoint: {e}"))?;
    let port = listener.local_addr().map_err(|e| format!("read picked port: {e}"))?.port();
    drop(listener);
    Ok(format!("tcp:127.0.0.1:{port}"))
}

/// Can `endpoint` (a Unix path or `tcp:HOST:PORT`) be connected to right now?
fn endpoint_accepts(endpoint: &str) -> bool {
    match endpoint.strip_prefix("tcp:") {
        Some(addr) => std::net::TcpStream::connect(addr).is_ok(),
        None => {
            std::path::Path::new(endpoint).exists()
                && std::os::unix::net::UnixStream::connect(endpoint).is_ok()
        }
    }
}

/// Start the dKMS node DAEMON listening on `sock` (a Unix path or a `tcp:HOST:PORT` endpoint) with
/// its node-local master store + its KNOWN-caller allow-list (Day 95–96: the OPERATOR provisions the
/// comma-separated b64 verifying keys the node will serve; an unknown caller's `hello` is refused),
/// and wait for the listener to ACCEPT (fail-closed if it never binds). The daemon serves many
/// sequential connections.
fn start_dkms_daemon(
    node_bin: &str,
    sock: &str,
    node_store_path: &str,
    allowed_callers: &str,
) -> Result<DaemonGuard, String> {
    if !sock.starts_with("tcp:") {
        let _ = std::fs::remove_file(sock);
    }
    let child = Command::new(node_bin)
        .env("DKMS_AUTHORITY_LISTEN", sock)
        .env("DKMS_AUTHORITY_KEY_STORE", node_store_path)
        .env("DKMS_AUTHORITY_ALLOWED_CALLERS", allowed_callers)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn dkms daemon ({node_bin}): {e}"))?;
    let guard = DaemonGuard { child, sock: sock.to_string() };
    for _ in 0..400 {
        // Confirm it actually ACCEPTS (bound + listening), whichever transport.
        if endpoint_accepts(sock) {
            return Ok(guard);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err(format!("dkms daemon did not start listening on {sock} within timeout"))
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
///
/// `node_set_id` (Day 103–104): on the 2-of-2 threshold rail, the node-set identity the
/// boundary derives from its own pinned vks — welded into the AAD so the release is
/// cryptographically bound to the exact secret-holders. `None` on the single-node rail
/// (the encoding stays byte-identical).
fn transcript_aad(
    session_pub: &[u8],
    content_hash: &[u8],
    nonce: &[u8],
    node_set_id: Option<&[u8]>,
) -> Vec<u8> {
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
        node_set_id,
    }
    .to_aad()
}

/// Day 103–104: the runtime's persisting event sink — writes each runtime event as the SAME
/// durable, CEK-free record the lib's `PersistingEventSink` would (`open_event_record`, one
/// canonical shape) and, on the 2-of-2 threshold rail, STAMPS the NODE-SET IDENTITY into it
/// (`node_set_id_b64`). An auditor reading the durable record can prove WHICH set of
/// secret-holders served a given open after the fact — a public hash over public vks, never
/// key material, so the CEK-free invariant is untouched. `None` (single-node) persists the
/// record byte-identically to the lib sink. PC2 cannot record this: its node-set lives inside
/// Lit's opaque network, so its audit trail can never say which nodes served a decrypt.
struct NodeSetStampingSink {
    store: DurableEventStore,
    node_set_id_b64: Option<String>,
}

impl RuntimeEventSink for NodeSetStampingSink {
    fn emit(&mut self, event: &str, ctx: &OpenContext, report: &ExecutionReport) -> Result<(), String> {
        let mut record = open_event_record(event, ctx, report);
        if let Some(id) = &self.node_set_id_b64 {
            record["node_set_id_b64"] = json!(id);
        }
        let key = format!("{}/{}", ctx.content_id, event);
        self.store.persist(&key, &record)
    }
}

/// Re-derive the 2-of-2 NODE-SET IDENTITY from a published dkms descriptor's `threshold`
/// block (`threshold_node_set_id` over both listed nodes' vks + t=2). The ONE code path both
/// the run() pin check and the rotation gate use — so "which node-set does this descriptor
/// name?" can never be answered two different ways. Fail-closed on a malformed descriptor.
fn derive_node_set_from_descriptor(descriptor_path: &std::path::Path) -> Result<[u8; 32], String> {
    let desc: Value = serde_json::from_slice(
        &std::fs::read(descriptor_path).map_err(|e| format!("re-read dkms descriptor: {e}"))?,
    )
    .map_err(|e| format!("parse dkms descriptor: {e}"))?;
    let nodes = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(Value::as_array)
        .ok_or("threshold descriptor carries no node list to pin")?;
    if nodes.len() != 2 {
        return Err("threshold descriptor must list exactly two nodes for a 2-of-2 node-set".to_string());
    }
    let vk_of = |i: usize| -> Result<Vec<u8>, String> {
        let s = nodes[i]
            .get("verifying_key_b64")
            .and_then(Value::as_str)
            .ok_or("threshold descriptor node is missing verifying_key_b64")?;
        B64.decode(s).map_err(|e| e.to_string())
    };
    Ok(ddrm_envelope::threshold_node_set_id(2, &vk_of(0)?, &vk_of(1)?))
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
    /// 2-of-2 THRESHOLD (Day 99–100): node B's escrowed share. When present, the `KeyHandle` supplies
    /// it in the `release` session context so the key-provider dual-recovers BOTH nodes. `None` for the
    /// single-node rail.
    wrapped_cek_share2_b64: Option<String>,
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
    /// The recipient the CEK (or, in the threshold case, share-1) was escrowed to — checked against
    /// the relaunched authority's published recipient to prove the authority identity is STABLE.
    recipient_pub_b64: String,
    /// 2-of-2 THRESHOLD (Day 99–100): node B's escrowed share-2 (`None` for the single-node rail).
    /// The CEK was XOR-split at publish (`split_cek_xor`); share-1 rides `wrapped_cek_b64` (escrowed to
    /// node A), share-2 rides this (escrowed to node B's recipient).
    wrapped_cek_share2_b64: Option<String>,
    /// Node B's published verifying key — the decrypt boundary needs it (`authority_vk2_b64`) to
    /// unwrap share-2 in-VM. `None` for the single-node rail.
    vk2_b64: Option<String>,
    /// 2-of-2 THRESHOLD (Day 101–102): the durably-pinned NODE-SET IDENTITY — a hash over `(t, vk_a,
    /// vk_b)` (`ddrm_envelope::threshold_node_set_id`). The producer escrowed the two shares to THIS
    /// node-set; the open RE-DERIVES it from the published descriptor and fails closed if a node was
    /// silently swapped. `None` for the single-node rail.
    node_set_id_b64: Option<String>,
}

impl PublishEscrow {
    fn to_json(&self) -> Value {
        let mut v = json!({
            "schema": "elastos.publish.escrow.fixture/v1",
            "kid_hex": self.kid_hex,
            "wrapped_cek_b64": self.wrapped_cek_b64,
            "producer_vk_b64": self.producer_vk_b64,
            "content_hash_b64": self.content_hash_b64,
            "nonce_b64": self.nonce_b64,
            "recipient_pub_b64": self.recipient_pub_b64,
        });
        if let Some(share2) = &self.wrapped_cek_share2_b64 {
            v["wrapped_cek_share2_b64"] = json!(share2);
        }
        if let Some(vk2) = &self.vk2_b64 {
            v["vk2_b64"] = json!(vk2);
        }
        if let Some(id) = &self.node_set_id_b64 {
            v["node_set_id_b64"] = json!(id);
        }
        v
    }

    fn from_json(v: &Value) -> Result<Self, String> {
        let field = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("publish escrow fixture is missing `{k}`"))
        };
        let opt = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(Self {
            wrapped_cek_share2_b64: opt("wrapped_cek_share2_b64"),
            vk2_b64: opt("vk2_b64"),
            node_set_id_b64: opt("node_set_id_b64"),
            kid_hex: field("kid_hex")?,
            wrapped_cek_b64: field("wrapped_cek_b64")?,
            producer_vk_b64: field("producer_vk_b64")?,
            content_hash_b64: field("content_hash_b64")?,
            nonce_b64: field("nonce_b64")?,
            recipient_pub_b64: field("recipient_pub_b64")?,
        })
    }
}

/// Provision ONE secret-holding dKMS node: spawn it on its node-local master store, read back its
/// PUBLISHED identity (vk + escrow recipient), and shut the provisioning child down (the long-lived
/// daemon is started separately). The MASTER stays in `store_path` — the runtime created it via the
/// node but NEVER reads it. Returns the node's `(verifying_key_b64, recipient_pub_b64)` pins.
fn provision_dkms_node(node_bin: &str, store_path: &str) -> Result<(String, String), String> {
    let mut node = Capsule::spawn("dkms-authority(provision)", node_bin)?;
    let init = ok_data(
        &node.call(&json!({ "op": "init", "config": { "authority_key_store": store_path } }))?,
        "dkms-authority init (provision)",
    )?;
    let vk = init["seal_verifying_key_b64"].as_str()
        .ok_or("dkms-authority node did not publish a verifying key")?
        .to_string();
    let recipient = init["seal_recipient_pub_b64"].as_str()
        .ok_or("dkms-authority node did not publish a recipient key")?
        .to_string();
    node.shutdown();
    Ok((vk, recipient))
}

/// Seal `share` (the whole CEK in the single-node case, or one XOR share in the threshold case) to a
/// node's published recipient under the SHARED escrow AAD `(suite ‖ kid16 ‖ recipient_pub)` — exactly
/// what that node will recompute + unseal in its own boundary. The producer signs it; the runtime
/// only ever holds the SEALED bytes.
fn escrow_share_to_recipient(
    recipient_pub_b64: &str,
    share: &[u8],
    kid16: &[u8; 16],
    producer_signer: &ddrm_envelope::seal::MlDsaSealSigner,
) -> Result<String, String> {
    let recipient_bytes = B64.decode(recipient_pub_b64).map_err(|e| e.to_string())?;
    let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_bytes)
        .ok_or("authority published a malformed escrow recipient key")?;
    let escrow = escrow_aad(SUITE_PQ_HYBRID, kid16, &recipient_bytes);
    Ok(B64.encode(
        ddrm_envelope::seal::seal_bound(&recipient_public, share, &escrow, producer_signer).to_bytes(),
    ))
}

/// PUBLISH-TIME escrow (the producer role, run ONCE before any open): bring the
/// durable-key-store authority up, read its STABLE published recipient, escrow the content
/// CEK to it under the shared escrow AAD, and write a durable publish fixture. Mirrors PC2
/// escrowing the CEK to the stable `DEFAULT_AUTHORITY` at encode time (`dashPackager.ts`
/// `encryptMediaCEK`). After this, the open path NEVER escrows — it reads the fixture.
///
/// 2-of-2 THRESHOLD (Day 99–100): when `threshold` is set (dkms only), TWO secret-holding nodes are
/// provisioned (distinct stores/sockets), the CEK is XOR-split (`split_cek_xor`) so node A escrows
/// share-1 and node B escrows share-2 — NEITHER node ever sees the whole CEK — and the published
/// descriptor carries a `threshold` block (`t:2`, both nodes) the key-provider resolves into a
/// dual-recover rail. The fixture then also carries `wrapped_cek_share2_b64` + node B's `vk2_b64`.
#[allow(clippy::too_many_arguments)]
fn publish_escrow(
    key_bin: &str,
    key_store_path: &str,
    fixture_path: &std::path::Path,
    backend: AuthorityBackend,
    descriptor_path: &std::path::Path,
    dkms_node_bin: Option<&str>,
    node_store_path: &str,
    node_endpoint: &str,
    threshold: bool,
    node2_store_path: &str,
    node2_endpoint: &str,
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
    //
    // For a 2-of-2 threshold, `node_b` carries `(node_a_vk, node_b_vk, node_b_recipient)` — node A's vk
    // is threaded through so the producer can pin the node-set identity over BOTH vks. `None` otherwise.
    let (recipient_pub_b64, node_b): (String, Option<(String, String, String)>) = match backend {
        AuthorityBackend::Reference => {
            let mut key = Capsule::spawn("key-provider(publish)", key_bin)?;
            let init = ok_data(
                &key.call(&json!({
                    "op": "init",
                    "config": { "backend": "reference", "authority_key_store": key_store_path }
                }))?,
                "key init (publish)",
            )?;
            init["seal_verifying_key_b64"].as_str()
                .ok_or("key-provider did not publish a seal verifying key (build with --features key-authority-ref)")?;
            let recipient = init["seal_recipient_pub_b64"].as_str()
                .ok_or("key-provider did not publish an escrow recipient key (build with --features key-authority-ref)")?
                .to_string();
            key.shutdown();
            (recipient, None)
        }
        AuthorityBackend::Dkms => {
            let node_bin = dkms_node_bin.ok_or("dkms backend requires a dkms_authority_bin in the config")?;
            // Node A — the single-node rail's node, and the FIRST threshold node.
            let (vk_a, recipient_a) = provision_dkms_node(node_bin, node_store_path)?;
            // Node B — provisioned ONLY for a 2-of-2 threshold, with its OWN node-local store.
            let node_b = if threshold {
                let (vk_b, recipient_b) = provision_dkms_node(node_bin, node2_store_path)?;
                if vk_b == vk_a {
                    return Err("the two dkms nodes derived the SAME identity — a 2-of-2 split needs two DISTINCT secret-holders".to_string());
                }
                Some((vk_a.clone(), vk_b, recipient_b))
            } else {
                None
            };
            // Publish the PUBLIC-ONLY descriptor — pins + endpoint, NOTHING secret. For threshold, ALSO
            // carry a `threshold` block (`t:2`, both nodes' public identities) the key-provider resolves
            // into a dual-recover rail. The masters live ONLY in the node stores (never read here).
            let mut descriptor = json!({
                "schema": "elastos.dkms.authority/v2",
                "verifying_key_b64": vk_a,
                "recipient_pub_b64": recipient_a,
                "authority_endpoint": node_endpoint,
            });
            if let Some((_vk_a, vk_b, recipient_b)) = &node_b {
                descriptor["threshold"] = json!({
                    "t": 2,
                    "nodes": [
                        { "verifying_key_b64": vk_a, "recipient_pub_b64": recipient_a, "authority_endpoint": node_endpoint },
                        { "verifying_key_b64": vk_b, "recipient_pub_b64": recipient_b, "authority_endpoint": node2_endpoint },
                    ],
                });
            }
            let bytes = serde_json::to_vec_pretty(&descriptor).map_err(|e| e.to_string())?;
            std::fs::write(descriptor_path, bytes)
                .map_err(|e| format!("write dkms authority descriptor: {e}"))?;
            (recipient_a, node_b)
        }
    };

    let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x70u8; 32]);
    let cek_bytes = B64.decode(GOLDEN_CEK_B64).map_err(|e| e.to_string())?;
    let kid16 = [0xC5u8; 16];
    let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();

    // SINGLE NODE: escrow the WHOLE CEK to the authority's recipient. THRESHOLD: XOR-split the CEK and
    // escrow share-1 to node A + share-2 to node B's recipient (neither node ever sees the whole key),
    // and PIN the node-set identity (a hash over both nodes' vks + t) so a later open detects a swap.
    let (wrapped_cek_b64, wrapped_cek_share2_b64, vk2_b64, node_set_id_b64) = match &node_b {
        None => (
            escrow_share_to_recipient(&recipient_pub_b64, &cek_bytes, &kid16, &producer_signer)?,
            None,
            None,
            None,
        ),
        Some((vk_a, vk_b, recipient_b)) => {
            // A uniform random mask hides the CEK information-theoretically in either share alone.
            let seed = ddrm_envelope::random_seed();
            let mask: Vec<u8> = seed.iter().copied().take(cek_bytes.len()).collect();
            let (share1, share2) = ddrm_envelope::split_cek_xor(&cek_bytes, &mask)?;
            // The producer escrowed the two shares to THIS node-set (node A's vk + node B's vk, t=2).
            let vk_a_bytes = B64.decode(vk_a).map_err(|e| e.to_string())?;
            let vk_b_bytes = B64.decode(vk_b).map_err(|e| e.to_string())?;
            let node_set_id = ddrm_envelope::threshold_node_set_id(2, &vk_a_bytes, &vk_b_bytes);
            (
                escrow_share_to_recipient(&recipient_pub_b64, &share1, &kid16, &producer_signer)?,
                Some(escrow_share_to_recipient(recipient_b, &share2, &kid16, &producer_signer)?),
                Some(vk_b.clone()),
                Some(B64.encode(node_set_id)),
            )
        }
    };

    let content_hash = b"consumer-smoke-content-hash-0001".to_vec(); // 32 bytes
    let nonce = b"consumer-smoke-nonce-1".to_vec();
    let fixture = PublishEscrow {
        kid_hex,
        wrapped_cek_b64,
        producer_vk_b64: B64.encode(&producer_vk),
        content_hash_b64: B64.encode(&content_hash),
        nonce_b64: B64.encode(&nonce),
        recipient_pub_b64,
        wrapped_cek_share2_b64,
        vk2_b64,
        node_set_id_b64,
    };
    let bytes = serde_json::to_vec_pretty(&fixture.to_json()).map_err(|e| e.to_string())?;
    std::fs::write(fixture_path, bytes).map_err(|e| format!("write publish fixture: {e}"))?;
    Ok(fixture)
}

/// A complete `RightsDecisionReceiptV1` JSON the dkms node can deserialize (deny_unknown_fields).
fn probe_receipt(allowed: bool, content_id: &str, principal_id: &str, right: &str) -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "probe",
        "content_id": content_id,
        "principal_id": principal_id,
        "session_id": "probe-session",
        "right": right,
        "provider": "rights-provider",
        "allowed": allowed,
        "issued_at": 1,
        "expires_at": u64::MAX,
    })
}

/// Adversarial probe against the REAL dkms-authority node binary (verify mode only): prove, cross
/// binary, that **(a)** a tampered/wrong NODE IDENTITY is rejected at the handshake — the node's
/// attestation over a fresh challenge verifies under the descriptor-PINNED vk but NOT under a
/// flipped vk or a replayed challenge — **(b)** the node REQUIRES a live, node-verified SESSION
/// TOKEN on every recover (no/expired/forged/tampered token is refused, even with a perfectly valid
/// escrow + receipt) and a token minted for one challenge cannot authorize a recover under a
/// tampered challenge/binding — **(c)** the node REFUSES a recover whose authorization does not bind
/// the content/principal — and **(d)** ONE handshake session authorizes MANY successful recovers
/// over the long-lived node (the persistent-session shape). The runtime-core analogue of PC2 pinning
/// the Lit network identity (`universal-decrypt-chipotle.js:577`–`:590`), resurrecting a per-view
/// session to gate recovery (`secureViewSession.ts:81`–`:128`), and re-running `hasAccessByContentId`
/// in the TEE (`:560`–`:568`) rather than trusting the caller.
fn dkms_node_adversarial_probe(
    sock_path: &str,
    pinned_vk_b64: &str,
    material: &KeyOpenMaterial,
    caller_seed: [u8; 32],
) -> Result<(), String> {
    // CONNECT to the running daemon over the framed socket (no spawn) — the SAME wire production uses.
    let mut node = NodeSocket::connect(sock_path)?;
    ok_data(&node.call(&json!({ "op": "init", "config": {} }))?, "dkms-authority init (probe)")?;

    // The probe's KNOWN caller identity (Day 95–96): derived from the SAME seed the runtime
    // provisioned into the node's allow-list, so the probe's happy path is an allow-listed caller.
    let (caller_signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);

    // KNOWN-CALLER GATE: a caller whose identity is NOT on the node's allow-list is refused at hello,
    // before any token is minted (an UNKNOWN ephemeral key the runtime never provisioned).
    let (_unknown_signer, unknown_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x71u8; 32]);
    let unknown_hello = node.call(&json!({
        "op": "hello",
        "challenge_b64": B64.encode(ddrm_envelope::random_seed()),
        "caller_pub_b64": B64.encode(&unknown_vk),
        "now_unix": NOW_UNIX,
    }))?;
    if unknown_hello.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node served an UNKNOWN caller — the allow-list is not enforced".to_string());
    }

    // (a) IDENTITY HANDSHAKE — the node proves possession of the master-derived signing key AND
    // mints a node-signed SESSION TOKEN bound to this challenge + the probe's pubkey + a bounded
    // expiry. On the TCP transport (Day 105–108) this hello ALSO establishes the encrypted channel
    // (the node refuses plaintext recovers there), verifying the node's ATTESTED channel key under
    // the pinned identity — every subsequent probe call then travels sealed, like production.
    let challenge = ddrm_envelope::random_seed();
    let hello = if sock_path.starts_with("tcp:") {
        node.establish_channel(pinned_vk_b64, caller_seed, challenge, NOW_UNIX)?
    } else {
        ok_data(
            &node.call(&json!({
                "op": "hello",
                "challenge_b64": B64.encode(challenge),
                "caller_pub_b64": B64.encode(&caller_vk),
                "now_unix": NOW_UNIX,
            }))?,
            "dkms-authority hello (probe)",
        )?
    };
    if hello["verifying_key_b64"].as_str() != Some(pinned_vk_b64) {
        return Err("dkms node hello advertised a vk that does not match the pinned descriptor".to_string());
    }
    let attestation = B64
        .decode(hello["attestation_b64"].as_str().unwrap_or(""))
        .map_err(|e| format!("node attestation is not base64: {e}"))?;
    let pinned = B64.decode(pinned_vk_b64).map_err(|e| e.to_string())?;
    let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned)
        .ok_or("pinned dkms vk is malformed")?;
    if !ddrm_envelope::verify_attestation(&verifier, &challenge, &attestation) {
        return Err("the genuine node attestation failed to verify under the pinned vk".to_string());
    }
    // A flipped (impersonator) vk must NOT verify this attestation → a swapped node is rejected.
    let mut tampered = pinned.clone();
    tampered[0] ^= 1;
    if let Some(bad_verifier) = ddrm_envelope::MlDsa65Verifier::from_encoded(&tampered) {
        if ddrm_envelope::verify_attestation(&bad_verifier, &challenge, &attestation) {
            return Err("a tampered node identity (flipped vk) wrongly verified — pinning is broken".to_string());
        }
    }
    // A replayed challenge must NOT verify under the genuine vk → no replay across nonces.
    let mut replay = challenge;
    replay[0] ^= 1;
    if ddrm_envelope::verify_attestation(&verifier, &replay, &attestation) {
        return Err("a replayed challenge wrongly verified — the attestation is not challenge-bound".to_string());
    }
    // Capture the live session token the node minted (the credential every recover must present).
    let session_token = hello["session_token"].clone();
    if !session_token.is_object() {
        return Err("dkms node hello returned no session token".to_string());
    }
    step(13, "dkms node IDENTITY pinned + verified OVER THE SOCKET: the node's framed attestation verifies under the descriptor vk; a flipped vk + a replayed challenge are both rejected; and the node minted a CALLER-BOUND session token");

    // A GENUINE recover bundle: REAL publish-escrow material (recovers to the node recipient) + a
    // coherent allowed receipt + the live session token + a valid POSSESSION PROOF (signed under the
    // ephemeral key the token is bound to). Used for the happy (c)/(d) path and as the base the
    // adversarial cases vary (so a refusal is provably the gate, not a broken bundle).
    const PROBE_CONTENT: &str = "bafContent";
    const PROBE_PRINCIPAL: &str = "did:key:zViewer";
    const PROBE_SESSION: &str = "probe-session";
    // Sign the recover binding under `signer` for `token`'s challenge + the freshness counter `seq`.
    let proof = |token: &Value, signer: &ddrm_envelope::seal::MlDsaSealSigner, seq: u64| -> String {
        let chal = B64.decode(token["challenge_b64"].as_str().unwrap_or("")).unwrap_or_default();
        let sp = B64.decode(&material.session_pub_b64).unwrap_or_default();
        B64.encode(ddrm_envelope::sign_recover_proof(
            signer,
            &chal,
            PROBE_CONTENT.as_bytes(),
            material.kid_hex.as_bytes(),
            &sp,
            seq,
        ))
    };
    let genuine = |token: &Value, now: u64, seq: u64| {
        json!({
            "op": "recover",
            "wrapped_cek_b64": material.wrapped_cek_b64,
            "scheme": SUITE_PQ_HYBRID,
            "kid_hex": material.kid_hex,
            "producer_vk_b64": material.producer_vk_b64,
            "decrypt_session_pub_b64": material.session_pub_b64,
            "aad_b64": material.aad_b64,
            "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
            "content_hash_b64": material.content_hash_b64,
            "nonce_b64": material.nonce_b64,
            "rights_receipt": probe_receipt(true, PROBE_CONTENT, PROBE_PRINCIPAL, "view"),
            "content_id": PROBE_CONTENT,
            "principal_id": PROBE_PRINCIPAL,
            "session_id": PROBE_SESSION,
            "right": "view",
            "session_token": token,
            "caller_sig_b64": proof(token, &caller_signer, seq),
            "recover_seq": seq,
            "now_unix": now,
        })
    };

    // (b) SESSION + POSSESSION GATE — the node refuses recover without a live token, EVEN with valid
    // escrow + receipt: NO token, EXPIRED, FORGED signature, tampered CHALLENGE — and (Day 93–94) a
    // captured token replayed WITHOUT the caller signature, or with a signature under the WRONG key.
    let mut no_token = genuine(&session_token, NOW_UNIX, 1);
    no_token.as_object_mut().unwrap().remove("session_token");
    if node.call(&no_token)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node recovered with NO session token — the session gate is broken".to_string());
    }
    let expires_at = session_token["expires_at"].as_u64().unwrap_or(0);
    if node.call(&genuine(&session_token, expires_at + 1, 1))?.get("status").and_then(Value::as_str)
        == Some("ok")
    {
        return Err("the node recovered with an EXPIRED session token — expiry is not enforced".to_string());
    }
    let mut forged_token = session_token.clone();
    forged_token["sig_b64"] = json!(B64.encode([0u8; 8]));
    if node.call(&genuine(&forged_token, NOW_UNIX, 1))?.get("status").and_then(Value::as_str)
        == Some("ok")
    {
        return Err("the node recovered with a FORGED session token — the signature is not verified".to_string());
    }
    let mut other_challenge = session_token.clone();
    other_challenge["challenge_b64"] = json!(B64.encode([0x99u8; 32]));
    if node.call(&genuine(&other_challenge, NOW_UNIX, 1))?.get("status").and_then(Value::as_str)
        == Some("ok")
    {
        return Err("a session token bound to one challenge authorized a recover under a DIFFERENT challenge — binding is broken".to_string());
    }
    // POSSESSION: a captured token replayed WITHOUT the caller signature is refused.
    let mut no_proof = genuine(&session_token, NOW_UNIX, 1);
    no_proof.as_object_mut().unwrap().remove("caller_sig_b64");
    if node.call(&no_proof)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node recovered with NO possession proof — a captured bearer token is replayable".to_string());
    }
    // POSSESSION: a captured token replayed with a signature under the WRONG key is refused.
    let (wrong_signer, _wrong_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x5cu8; 32]);
    let mut wrong_proof = genuine(&session_token, NOW_UNIX, 1);
    wrong_proof["caller_sig_b64"] = json!(proof(&session_token, &wrong_signer, 1));
    if node.call(&wrong_proof)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node recovered with a possession proof under the WRONG key — the token is not caller-bound".to_string());
    }
    step(14, "dkms node SESSION + POSSESSION GATE over the socket: recover with NO / EXPIRED / FORGED token, a tampered-challenge token, NO possession proof, and a WRONG-KEY proof are ALL refused (a captured bearer token is non-replayable)");

    // (c) NODE RE-AUTHORIZATION — with a LIVE token + valid proof, the node still refuses recover
    // whose authorization does not bind the declared content/principal.
    let denied = {
        let mut d = genuine(&session_token, NOW_UNIX, 1);
        d["rights_receipt"] = probe_receipt(false, PROBE_CONTENT, PROBE_PRINCIPAL, "view");
        d
    };
    if node.call(&denied)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node recovered for a DENIED receipt — re-authorization is broken".to_string());
    }
    let mismatched = {
        let mut m = genuine(&session_token, NOW_UNIX, 1);
        m["rights_receipt"] = probe_receipt(true, "bafOTHER", PROBE_PRINCIPAL, "view");
        m
    };
    if node.call(&mismatched)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node recovered for a receipt bound to DIFFERENT content — re-authorization is broken".to_string());
    }
    step(15, "dkms node RE-AUTHORIZED in its own boundary: even WITH a live session + proof, it refused recover for a DENIED receipt and a receipt bound to other content (the node never trusts the caller's claim)");

    // (d) MANY SEGMENTS OVER ONE SOCKET CONNECTION + SESSION — the SAME live token + connection drives
    // repeated SUCCESSFUL recovers over the long-lived node; each carries a STRICTLY-ADVANCING
    // freshness counter and returns sealed material, never the raw CEK. No re-connect/re-handshake.
    // All the failing adversarial calls above used seq 1 and did NOT commit (they were refused), so
    // the node's session counter is still 0 here; the successful recovers consume seqs 1, 2, 3.
    for seq in 1u64..=3 {
        let ok = ok_data(&node.call(&genuine(&session_token, NOW_UNIX, seq))?, "dkms genuine recover (probe)")?;
        let sealed = ok["material"]["sealed_cek_b64"].as_str().unwrap_or_default();
        if sealed.is_empty() {
            return Err(format!("recover seq {seq} over the reused socket session returned no sealed material"));
        }
        if serde_json::to_string(&ok).unwrap_or_default().contains(GOLDEN_CEK_B64) {
            return Err("the raw CEK leaked from a reused-session recover".to_string());
        }
    }
    // (d.1) ANTI-REPLAY: replaying a consumed recover frame VERBATIM (a stale freshness counter) is
    // refused, even though its token + possession proof are otherwise valid — so a captured recover
    // cannot be re-driven (Day 95–96).
    let replay = genuine(&session_token, NOW_UNIX, 3);
    if node.call(&replay)?.get("status").and_then(Value::as_str) == Some("ok") {
        return Err("the node re-ran a REPLAYED recover (stale freshness counter) — anti-replay is broken".to_string());
    }
    step(16, "dkms node ONE socket connection + session → MANY recovers (strictly-advancing freshness): three SUCCESSFUL recovers over the SAME connection + live token (sealed only), and a REPLAYED recover frame is refused — the persistent open-once/recover-many shape with anti-replay");

    // Close THIS connection before the framing probe: the daemon serves connections SEQUENTIALLY
    // (one session per connection), so it cannot accept the framing probe's fresh connections until
    // this one ends. Dropping the socket lets the daemon's accept loop move on.
    drop(node);

    // (e) MALFORMED FRAME FAILS CLOSED WITHOUT WEDGING THE DAEMON — a fresh connection that sends a
    // torn/oversized frame is refused, and a subsequent fresh connection still completes a session
    // (as the allow-listed KNOWN caller).
    dkms_malformed_frame_is_refused(sock_path, caller_seed)?;
    step(17, "dkms node FRAMING fails closed: a torn AND an oversized frame on fresh connections are each refused (the daemon drops the connection, never wedges), and a clean session afterwards still succeeds");

    Ok(())
}

/// Drive a torn + an oversized frame at the daemon over RAW connections of any stream type and
/// assert each is refused (an error frame or a dropped connection — both fail-closed).
fn raw_malformed_frames<S: std::io::Read + std::io::Write>(
    connect: impl Fn() -> Result<S, String>,
    shutdown_write: impl Fn(&S) -> Result<(), String>,
) -> Result<(), String> {
    // Torn frame: a header promising 64 bytes followed by only 3, then half-close the write side.
    let mut torn = connect()?;
    torn.write_all(&64u32.to_be_bytes()).map_err(|e| e.to_string())?;
    torn.write_all(b"abc").map_err(|e| e.to_string())?;
    shutdown_write(&torn)?;
    let mut torn_reader = BufReader::new(torn);
    // Either an explicit error frame, or a dropped connection — both are fail-closed.
    if let Ok(Some(bytes)) = ddrm_envelope::frame::read_frame(&mut torn_reader) {
        let resp: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if resp["status"].as_str() == Some("ok") {
            return Err("the daemon accepted a torn frame — framing is not fail-closed".to_string());
        }
    }

    // Oversized frame: a length header beyond MAX_FRAME_BYTES must be refused before allocating.
    let mut huge = connect()?;
    let over = ddrm_envelope::frame::MAX_FRAME_BYTES + 1;
    huge.write_all(&over.to_be_bytes()).map_err(|e| e.to_string())?;
    shutdown_write(&huge)?;
    let mut huge_reader = BufReader::new(huge);
    if let Ok(Some(bytes)) = ddrm_envelope::frame::read_frame(&mut huge_reader) {
        let resp: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if resp["status"].as_str() == Some("ok") {
            return Err("the daemon accepted an oversized frame — framing is not fail-closed".to_string());
        }
    }
    Ok(())
}

/// Prove the daemon's framed transport fails closed: a torn frame and an oversized frame each get an
/// `invalid_frame` refusal (or a dropped connection) WITHOUT wedging the daemon — a fresh connection
/// afterwards still does a clean init/hello round-trip. Transport-generic (Unix path or `tcp:`).
fn dkms_malformed_frame_is_refused(sock_path: &str, caller_seed: [u8; 32]) -> Result<(), String> {
    match sock_path.strip_prefix("tcp:") {
        Some(addr) => raw_malformed_frames(
            || std::net::TcpStream::connect(addr).map_err(|e| e.to_string()),
            |s| s.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string()),
        )?,
        None => raw_malformed_frames(
            || std::os::unix::net::UnixStream::connect(sock_path).map_err(|e| e.to_string()),
            |s| s.shutdown(std::net::Shutdown::Write).map_err(|e| e.to_string()),
        )?,
    }

    // The daemon is NOT wedged: a fresh connection still completes a clean init/hello round-trip as
    // the allow-listed KNOWN caller (an unknown caller would be refused by the Day 95–96 gate).
    let mut fresh = NodeSocket::connect(sock_path)?;
    ok_data(&fresh.call(&json!({ "op": "init", "config": {} }))?, "post-malformed init")?;
    let (_s, vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);
    ok_data(
        &fresh.call(&json!({
            "op": "hello",
            "challenge_b64": B64.encode([0x44u8; 32]),
            "caller_pub_b64": B64.encode(&vk),
            "now_unix": NOW_UNIX,
        }))?,
        "post-malformed hello",
    )?;
    Ok(())
}

/// NETWORK-CHANNEL adversarial gates (Day 105–108, verify mode, `tcp` transport only): prove, against
/// the LIVE node daemon over its REAL TCP listener, that the hostile-network edges fail closed:
/// **(28)** a PLAINTEXT recover with NO channel is refused (`channel_required`) — the network never
/// carries an unencrypted recover; **(29)** a connection that DOWNGRADES to plaintext after the
/// channel is established is dropped without service (and the daemon is not wedged); **(30)** a
/// MITM-TAMPERED sealed frame (one ciphertext byte flipped) is dropped without service; **(31)** a
/// WRONG-NODE channel KEM key (an attacker terminating the TCP connection and substituting its own
/// key under the relayed attestation) fails verification under the descriptor-PINNED identity, so
/// the client refuses the channel before delegating anything. PC2 has no analogue of any of these:
/// its dDRM network boundary is HTTPS with `rejectUnauthorized: false` (`chipotle-client.ts:840`) —
/// the channel authenticates nothing and tampering is only caught (for provisioning) by the payload
/// signature (`chipotle-client.ts:737`–`:795`), never for the decrypt path.
fn dkms_tcp_channel_adversarial_gates(
    endpoint: &str,
    pinned_vk_b64: &str,
    caller_seed: [u8; 32],
) -> Result<(), String> {
    let (_caller_signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);

    // --- (28) PLAINTEXT RECOVER, NO CHANNEL → refused at the transport gate. The recover body is
    // well-formed (it parses), so the refusal is provably the channel gate, not a parse error.
    let mut plain = NodeSocket::connect(endpoint)?;
    ok_data(&plain.call(&json!({ "op": "init", "config": {} }))?, "tcp plaintext init")?;
    ok_data(
        &plain.call(&json!({
            "op": "hello",
            "challenge_b64": B64.encode(ddrm_envelope::random_seed()),
            "caller_pub_b64": B64.encode(&caller_vk),
            "now_unix": NOW_UNIX,
        }))?,
        "tcp plaintext hello (no channel offered)",
    )?;
    let refused = plain.call(&json!({
        "op": "recover",
        "wrapped_cek_b64": "AAAA", "scheme": SUITE_PQ_HYBRID,
        "kid_hex": "00", "producer_vk_b64": "AAAA", "decrypt_session_pub_b64": "AAAA",
        "ciphertext_b64": "AAAA", "content_hash_b64": "AAAA", "nonce_b64": "AAAA",
        "rights_receipt": probe_receipt(true, "bafContent", "did:key:zViewer", "view"),
        "content_id": "bafContent", "principal_id": "did:key:zViewer",
        "session_id": "probe-session", "right": "view",
        "session_token": { "challenge_b64": "AAAA", "caller_pub_b64": "AAAA", "expires_at": 1, "sig_b64": "AAAA" },
        "caller_sig_b64": "AAAA", "recover_seq": 1, "now_unix": NOW_UNIX,
    }))?;
    if refused["code"].as_str() != Some("channel_required") {
        return Err(format!(
            "a PLAINTEXT recover over TCP must be refused with channel_required, got: {refused}"
        ));
    }
    drop(plain);
    step(28, "dkms node over TCP: a PLAINTEXT recover (no encrypted channel) is refused at the transport gate (`channel_required`) — the hostile network never carries an unencrypted recover");

    // --- (29) PLAINTEXT DOWNGRADE after establishment → the connection is dropped without service.
    let mut down = NodeSocket::connect(endpoint)?;
    ok_data(&down.call(&json!({ "op": "init", "config": {} }))?, "downgrade init")?;
    down.establish_channel(pinned_vk_b64, caller_seed, ddrm_envelope::random_seed(), NOW_UNIX)?;
    let plaintext_status = serde_json::to_vec(&json!({ "op": "status" })).map_err(|e| e.to_string())?;
    match down.raw_round_trip(&plaintext_status) {
        Ok(None) | Err(_) => {} // dropped — fail-closed
        Ok(Some(bytes)) => {
            return Err(format!(
                "the node answered a PLAINTEXT frame on an established channel (downgrade served): {}",
                String::from_utf8_lossy(&bytes)
            ))
        }
    }
    drop(down);
    // The daemon is NOT wedged: a fresh, honest channel still serves.
    let mut after_down = NodeSocket::connect(endpoint)?;
    ok_data(&after_down.call(&json!({ "op": "init", "config": {} }))?, "post-downgrade init")?;
    after_down.establish_channel(pinned_vk_b64, caller_seed, ddrm_envelope::random_seed(), NOW_UNIX)?;
    ok_data(&after_down.call(&json!({ "op": "status" }))?, "post-downgrade sealed status")?;
    drop(after_down);
    step(29, "dkms node over TCP: a connection that DOWNGRADES to plaintext after the channel is established is DROPPED without service (and the daemon serves the next honest channel)");

    // --- (30) MITM TAMPER: a correctly-sealed frame with ONE ciphertext byte flipped → dropped.
    let mut mitm = NodeSocket::connect(endpoint)?;
    ok_data(&mitm.call(&json!({ "op": "init", "config": {} }))?, "tamper init")?;
    mitm.establish_channel(pinned_vk_b64, caller_seed, ddrm_envelope::random_seed(), NOW_UNIX)?;
    let tampered = mitm.tampered_sealed_frame(&json!({ "op": "status" }))?;
    match mitm.raw_round_trip(&tampered) {
        Ok(None) | Err(_) => {} // dropped — the AEAD/signature, not a heuristic, is the gate
        Ok(Some(bytes)) => {
            return Err(format!(
                "the node served a TAMPERED sealed frame (MITM not detected): {}",
                String::from_utf8_lossy(&bytes)
            ))
        }
    }
    drop(mitm);
    // Not wedged: a fresh honest channel still serves.
    let mut after_mitm = NodeSocket::connect(endpoint)?;
    ok_data(&after_mitm.call(&json!({ "op": "init", "config": {} }))?, "post-tamper init")?;
    after_mitm.establish_channel(pinned_vk_b64, caller_seed, ddrm_envelope::random_seed(), NOW_UNIX)?;
    ok_data(&after_mitm.call(&json!({ "op": "status" }))?, "post-tamper sealed status")?;
    drop(after_mitm);
    step(30, "dkms node over TCP: a MITM-TAMPERED sealed frame (one ciphertext byte flipped) is DROPPED fail-closed — channel integrity is cryptographic, per frame");

    // --- (31) WRONG-NODE CHANNEL KEY under the pinned identity: relay the node's GENUINE hello but
    // substitute the attacker's own KEM key (the attacker-terminates-TCP shape) — the channel-key
    // attestation no longer verifies under the descriptor-PINNED vk, so the client refuses the
    // channel. (The genuine pair verifies; ONLY the substitution is what breaks it.)
    let mut probe = NodeSocket::connect(endpoint)?;
    ok_data(&probe.call(&json!({ "op": "init", "config": {} }))?, "wrong-key init")?;
    let challenge = ddrm_envelope::random_seed();
    let hello = ok_data(
        &probe.call(&json!({
            "op": "hello",
            "challenge_b64": B64.encode(challenge),
            "caller_pub_b64": B64.encode(&caller_vk),
            "now_unix": NOW_UNIX,
            "channel_pub_b64": B64.encode(ddrm_envelope::session_public_bytes(&ddrm_envelope::mint_session().1)),
        }))?,
        "wrong-key hello",
    )?;
    let pinned = B64.decode(pinned_vk_b64).map_err(|e| e.to_string())?;
    let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned).ok_or("pinned vk malformed")?;
    let node_channel_pub = B64
        .decode(hello["channel"]["node_channel_pub_b64"].as_str().unwrap_or(""))
        .map_err(|e| e.to_string())?;
    let channel_sig = B64
        .decode(hello["channel"]["channel_sig_b64"].as_str().unwrap_or(""))
        .map_err(|e| e.to_string())?;
    if !ddrm_envelope::verify_channel_key(&verifier, &challenge, &node_channel_pub, &channel_sig) {
        return Err("the GENUINE node channel key failed to verify — the happy path is broken".to_string());
    }
    let (_attacker_secret, attacker_pub) = ddrm_envelope::mint_session();
    let attacker_pub = ddrm_envelope::session_public_bytes(&attacker_pub);
    if ddrm_envelope::verify_channel_key(&verifier, &challenge, &attacker_pub, &channel_sig) {
        return Err(
            "an attacker-substituted channel KEM key VERIFIED under the pinned identity — the channel binding is broken"
                .to_string(),
        );
    }
    drop(probe);
    step(31, "dkms node over TCP: an attacker terminating the connection CANNOT substitute its own channel KEM key — the channel-key attestation verifies ONLY for the genuine (challenge, key) pair under the descriptor-pinned identity");

    Ok(())
}

/// 2-of-2 THRESHOLD probe (Day 97–98, verify mode): prove END TO END, across TWO REAL dKMS node
/// daemons, that the CEK is split so NO SINGLE NODE ever holds the whole content key, and the runtime
/// reconstructs it ONLY in the decrypt boundary. This probe plays BOTH the producer (it XOR-splits the
/// CEK and escrows share-1 to node A, share-2 to node B) and the decrypt boundary (it mints a session
/// key, recovers a re-sealed share from EACH node over the full session/possession/freshness gates,
/// and reconstructs `cek = share1 ⊕ share2` in-boundary). It asserts: (a) the happy 2-of-2 reconstructs
/// the exact CEK; (b) ONE share alone is useless (it is NOT the CEK); (c) a forged second share — one
/// NOT sealed by the trusted node — fails closed. This is the runtime's explicit, owned analogue of
/// Lit's opaque `decryptAndCombine` threshold (PC2 `non-media-decrypt.js:76`), where PC2 cannot inspect
/// the share set. The two daemons are this probe's OWN (distinct stores/sockets/allow-lists); they are
/// torn down on return.
fn dkms_threshold_probe(node_bin: &str, work_dir: &std::path::Path) -> Result<(), String> {
    // Two distinct secret-holding nodes: separate master stores, sockets, and the SAME known caller
    // allow-listed on both (one runtime identity, two nodes).
    let caller_seed = ddrm_envelope::random_seed();
    let (caller_signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);
    let caller_vk_b64 = B64.encode(&caller_vk);

    let store_a = work_dir.join("thr-node-a.json").to_string_lossy().into_owned();
    let store_b = work_dir.join("thr-node-b.json").to_string_lossy().into_owned();
    let sock_a = work_dir.join("thr-node-a.sock").to_string_lossy().into_owned();
    let sock_b = work_dir.join("thr-node-b.sock").to_string_lossy().into_owned();
    let _daemon_a = start_dkms_daemon(node_bin, &sock_a, &store_a, &caller_vk_b64)?;
    let _daemon_b = start_dkms_daemon(node_bin, &sock_b, &store_b, &caller_vk_b64)?;

    // The producer's identity (escrows both shares) + the content split.
    let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0x9au8; 32]);
    let producer_vk_b64 = B64.encode(&producer_vk);
    let cek = B64.decode(GOLDEN_CEK_B64).map_err(|e| e.to_string())?;
    let mask: Vec<u8> = (0u8..cek.len() as u8).map(|b| b ^ 0x5A).collect();
    let (share1, share2) = ddrm_envelope::split_cek_xor(&cek, &mask)?;
    let kid16 = [0xC5u8; 16];
    let kid_hex: String = kid16.iter().map(|b| format!("{b:02x}")).collect();

    // The decrypt-boundary stand-in: mint an in-boundary session key (the nodes re-seal their shares to
    // it) and bind everything to ONE decrypt transcript AAD (same for both nodes; only the node vk
    // differs). Content-binding fields mirror the rail's transcript.
    let (session_secret, session_public) = ddrm_envelope::mint_session();
    let session_pub_bytes = ddrm_envelope::session_public_bytes(&session_public);
    let session_pub_b64 = B64.encode(&session_pub_bytes);
    let content_hash = [0xABu8; 32];
    let nonce = [0xCDu8; 12];
    let aad = DecryptTranscriptV1 {
        suite_id: SUITE_PQ_HYBRID,
        provider_id: "decrypt",
        principal_id: "did:key:zViewer",
        session_id: "probe-session",
        object_cid: "bafThreshold",
        content_hash: &content_hash,
        action: "view",
        viewer_interface: "reader",
        output_kind: "page-image",
        expires_at: EXPIRES_AT,
        release_receipt_hash: [0u8; 32],
        decrypt_session_pub: &session_pub_bytes,
        nonce: &nonce,
        // The probe is its OWN self-consistent boundary stand-in (it seals + unwraps under
        // this AAD); the live run-path's node-set AAD binding is proven by the rail gates.
        node_set_id: None,
    }
    .to_aad();
    let aad_b64 = B64.encode(&aad);
    let content_hash_b64 = B64.encode(content_hash);
    let nonce_b64 = B64.encode(nonce);

    // Recover a re-sealed share from one node: escrow the share to ITS recipient, connect, run the full
    // identity + session + possession + freshness gates, and return the node's re-sealed share bytes.
    let recover_share = |sock: &str, share: &[u8]| -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut node = NodeSocket::connect(sock)?;
        let init = ok_data(&node.call(&json!({ "op": "init", "config": {} }))?, "threshold node init")?;
        let node_vk_b64 = init["seal_verifying_key_b64"].as_str().ok_or("node published no vk")?.to_string();
        let recipient_b64 = init["seal_recipient_pub_b64"].as_str().ok_or("node published no recipient")?;
        let recipient_bytes = B64.decode(recipient_b64).map_err(|e| e.to_string())?;
        let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_bytes)
            .ok_or("node published a malformed recipient")?;

        // Producer escrows THIS share to THIS node's recipient (so node A only ever sees share-1, etc).
        let escrow = escrow_aad(SUITE_PQ_HYBRID, &kid16, &recipient_bytes);
        let wrapped = B64.encode(
            ddrm_envelope::seal::seal_bound(&recipient_public, share, &escrow, &producer_signer).to_bytes(),
        );

        let challenge = ddrm_envelope::random_seed();
        let hello = ok_data(
            &node.call(&json!({
                "op": "hello",
                "challenge_b64": B64.encode(challenge),
                "caller_pub_b64": B64.encode(&caller_vk),
                "now_unix": NOW_UNIX,
            }))?,
            "threshold node hello",
        )?;
        let token = hello["session_token"].clone();
        let chal = B64.decode(token["challenge_b64"].as_str().unwrap_or("")).unwrap_or_default();
        let caller_sig_b64 = B64.encode(ddrm_envelope::sign_recover_proof(
            &caller_signer,
            &chal,
            b"bafThreshold",
            kid_hex.as_bytes(),
            &session_pub_bytes,
            1,
        ));
        let recover = ok_data(
            &node.call(&json!({
                "op": "recover",
                "wrapped_cek_b64": wrapped,
                "scheme": SUITE_PQ_HYBRID,
                "kid_hex": kid_hex,
                "producer_vk_b64": producer_vk_b64,
                "decrypt_session_pub_b64": session_pub_b64,
                "aad_b64": aad_b64,
                "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                "content_hash_b64": content_hash_b64,
                "nonce_b64": nonce_b64,
                "rights_receipt": probe_receipt(true, "bafThreshold", "did:key:zViewer", "view"),
                "content_id": "bafThreshold",
                "principal_id": "did:key:zViewer",
                "session_id": "probe-session",
                "right": "view",
                "session_token": token,
                "caller_sig_b64": caller_sig_b64,
                "recover_seq": 1u64,
                "now_unix": NOW_UNIX,
            }))?,
            "threshold node recover",
        )?;
        let sealed_b64 = recover["material"]["sealed_cek_b64"].as_str().ok_or("node returned no sealed share")?;
        // The raw CEK / either raw share must NEVER appear on the wire.
        if serde_json::to_string(&recover).unwrap_or_default().contains(GOLDEN_CEK_B64) {
            return Err("the raw CEK leaked from a threshold node recover".to_string());
        }
        let sealed = B64.decode(sealed_b64).map_err(|e| e.to_string())?;
        let node_vk = B64.decode(&node_vk_b64).map_err(|e| e.to_string())?;
        Ok((sealed, node_vk))
    };

    // Recover a re-sealed share from EACH node — each node independently holds + recovers ONLY its share.
    let (sealed1, vk_a) = recover_share(&sock_a, &share1)?;
    let (sealed2, vk_b) = recover_share(&sock_b, &share2)?;
    if vk_a == vk_b {
        return Err("the two threshold daemons derived the SAME identity — not two distinct nodes".to_string());
    }
    if sealed1 == sealed2 {
        return Err("the two nodes returned identical sealed shares — not a real 2-of-2 split".to_string());
    }
    step(18, "dkms 2-of-2: TWO distinct node daemons each escrowed + recovered ONLY their OWN share over the full session/possession/freshness gates — neither node ever saw the whole CEK");

    // DECRYPT-BOUNDARY ROLE: unwrap BOTH re-sealed shares in-boundary (each verified under ITS node's
    // vk, bound to the SAME transcript) and reconstruct the CEK. The combine happens ONLY here.
    let verifier_a = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk_a).ok_or("node A vk malformed")?;
    let verifier_b = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk_b).ok_or("node B vk malformed")?;
    let env1 = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed1).map_err(|e| format!("{e:?}"))?;
    let env2 = ddrm_envelope::PqSealedEnvelope::from_bytes(&sealed2).map_err(|e| format!("{e:?}"))?;
    let rec1 = ddrm_envelope::hybrid_unwrap_bound(&session_secret, &env1, &aad, &verifier_a)
        .map_err(|e| format!("share-1 unwrap failed: {e:?}"))?;
    let rec2 = ddrm_envelope::hybrid_unwrap_bound(&session_secret, &env2, &aad, &verifier_b)
        .map_err(|e| format!("share-2 unwrap failed: {e:?}"))?;
    let reconstructed = ddrm_envelope::combine_cek_xor(rec1.as_slice(), rec2.as_slice())?;
    if reconstructed.as_slice() != cek.as_slice() {
        return Err("2-of-2 reconstruction did not recover the CEK".to_string());
    }
    // (b) ONE share alone is useless — it is NOT the CEK.
    if rec1.as_slice() == cek.as_slice() || rec2.as_slice() == cek.as_slice() {
        return Err("a single node's share equals the CEK — the split is not secure".to_string());
    }
    step(19, "dkms 2-of-2: the decrypt boundary unwrapped BOTH node-sealed shares (each under ITS node's vk) and reconstructed the CEK in-boundary — while neither share alone is the key");

    // (c) A FORGED second share — sealed by a key that is NOT node B — fails closed under node B's vk,
    // even bound to the right transcript: an attacker who controls one node cannot mint the other's seal.
    let (forger, _fvk) = ddrm_envelope::seal::mldsa_seal_keypair([0x77u8; 32]);
    let forged = ddrm_envelope::seal::seal_bound(&session_public, rec2.as_slice(), &aad, &forger);
    if ddrm_envelope::hybrid_unwrap_bound(&session_secret, &forged, &aad, &verifier_b).is_ok() {
        return Err("a forged second share verified under node B's vk — the threshold is forgeable".to_string());
    }
    step(20, "dkms 2-of-2: a FORGED second share (not sealed by node B) fails closed under node B's vk — a single-node attacker cannot fabricate the missing share");
    Ok(())
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
        // The runtime-injected session context. 2-of-2 THRESHOLD: when node B's escrowed share-2 is
        // bound, supply it as `wrapped_cek_share2_b64` so the key-provider dual-recovers BOTH nodes —
        // the key-provider NEVER reconstructs the CEK; it only welds two re-sealed shares.
        let mut session_ctx = json!({
            "decrypt_session_pub_b64": m.session_pub_b64,
            "producer_vk_b64": m.producer_vk_b64,
            "aad_b64": m.aad_b64,
            "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
            "content_hash_b64": m.content_hash_b64,
            "nonce_b64": m.nonce_b64,
            "now_unix": NOW_UNIX,
        });
        if let Some(share2) = &m.wrapped_cek_share2_b64 {
            session_ctx["wrapped_cek_share2_b64"] = json!(share2);
        }
        let mut guard = self.key.borrow_mut();
        let key = guard.as_mut().ok_or("key capsule was already torn down")?;
        let release = ok_data(
            &key.call(&json!({
                "op": "release",
                "request": request,
                "session": session_ctx,
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
        if let Some(share2) = &m.wrapped_cek_share2_b64 {
            if release_str.contains(share2) {
                return Err("the second share escrow blob was echoed by the key authority".to_string());
            }
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
    /// 2-of-2 THRESHOLD: node B's verifying key (`authority_vk2_b64`), so the boundary can unwrap the
    /// second sealed share in-VM. `None` for the single-node rail.
    authority_vk2_b64: Option<String>,
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
        // The boundary trusts node A's vk (`authority_vk_b64`); for 2-of-2 it ALSO trusts node B's vk
        // (`authority_vk2_b64`) to unwrap share-2 — both sealed shares are unwrapped + XOR-combined
        // ONLY inside this boundary (the CEK never exists whole before here).
        let mut init_config = json!({ "authority_vk_b64": vk_b64 });
        if let Some(vk2) = &self.authority_vk2_b64 {
            init_config["authority_vk2_b64"] = json!(vk2);
        }
        let init = ok_data(
            &decrypt.call(&json!({ "op": "init", "config": init_config }))?,
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

/// Which TRANSPORT the dKMS node daemons listen on (Day 105–108). `Unix` (default): host-local
/// Unix-domain sockets, permissioned by the filesystem. `Tcp`: the node taken OFF localhost — a real
/// network listener; the rail then REQUIRES the app-layer encrypted, mutually-authenticated channel
/// (a `tcp:` node refuses plaintext recovers), and the network-fault semantics (connect/read
/// timeouts, drop-fails-closed) are live.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DkmsTransport {
    Unix,
    Tcp,
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
    /// 2-of-2 THRESHOLD (Day 99–100): when `authority.threshold == true`, the runtime provisions a
    /// SECOND secret-holding node (its own store/socket/allow-list), XOR-splits the CEK at publish so
    /// each node escrows only ONE share, publishes a `threshold` descriptor (both nodes), and the
    /// `DrmHost` run-path drives the full 2-of-2 release + decrypt — the CEK is never whole before the
    /// decrypt boundary. Requires `backend == dkms` (an external secret-holding authority); fail-closed
    /// otherwise. We provision BOTH nodes from the SAME node binary, so this is a boolean knob rather
    /// than a handed-in node-B descriptor path: the descriptor's `threshold` block is what the
    /// key-provider consumes, and the runtime owns producing it.
    threshold: bool,
    /// The dKMS node TRANSPORT (`authority.transport`, Day 105–108): `"unix"` (default) or `"tcp"`.
    /// `tcp` provisions the node daemon(s) on real network listeners (`tcp:127.0.0.1:PORT`
    /// endpoints in the published descriptor) and drives the whole rail — including the encrypted
    /// channel + the network adversarial gates — over TCP. Requires `backend == dkms`.
    dkms_transport: DkmsTransport,
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
        // `authority` is an OBJECT (room to carry per-backend descriptors later); today its
        // `backend` tag + (for dkms) the node binary + optional `threshold`/`transport` knobs are
        // read. Absent → reference (back-compat). Fail-closed on an unknown tag or a non-object
        // `authority`.
        let (authority, dkms_authority_bin, threshold, dkms_transport) = match obj.get("authority") {
            None => (AuthorityBackend::Reference, None, false, DkmsTransport::Unix),
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
                let threshold = match auth.get("threshold") {
                    None => false,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err("config `authority.threshold` must be a boolean".to_string()),
                };
                // 2-of-2 threshold needs an EXTERNAL secret-holding authority (two nodes). It is
                // meaningless for the in-runtime `reference` authority (which holds the master itself),
                // so fail closed rather than silently ignore the request.
                if threshold && backend != AuthorityBackend::Dkms {
                    return Err("config `authority.threshold` requires `authority.backend` == \"dkms\" (an external secret-holding authority)".to_string());
                }
                // The TRANSPORT knob (Day 105–108): `"unix"` (default) or `"tcp"`. It addresses the
                // dKMS node daemons, so it is meaningless for the in-runtime reference authority —
                // fail closed rather than silently ignore.
                let dkms_transport = match auth.get("transport").and_then(Value::as_str) {
                    None => DkmsTransport::Unix,
                    Some("unix") => DkmsTransport::Unix,
                    Some("tcp") => DkmsTransport::Tcp,
                    Some(other) => {
                        return Err(format!(
                            "config `authority.transport` must be \"unix\" or \"tcp\", got {other:?}"
                        ))
                    }
                };
                if dkms_transport == DkmsTransport::Tcp && backend != AuthorityBackend::Dkms {
                    return Err("config `authority.transport` == \"tcp\" requires `authority.backend` == \"dkms\" (it addresses the external node daemons)".to_string());
                }
                (backend, node_bin, threshold, dkms_transport)
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
            threshold,
            dkms_transport,
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
    // The node's listening ENDPOINT — the runtime CONNECTS here (it does not own the node's
    // process). Published as the descriptor `authority_endpoint`. Day 105–108: on the `tcp`
    // transport this is a REAL loopback network address (`tcp:127.0.0.1:PORT`) instead of a
    // Unix-domain socket path — the node taken off localhost's filesystem boundary.
    let node_sock_path = match cfg.dkms_transport {
        DkmsTransport::Unix => work_dir.join("dkms-authority.sock").to_string_lossy().into_owned(),
        DkmsTransport::Tcp => pick_tcp_endpoint()?,
    };
    // 2-of-2 THRESHOLD (Day 99–100): node B's OWN node-local master store + listening endpoint
    // (distinct from node A's). Used only when `cfg.threshold` — each node holds ONLY its own share.
    let node2_store_path = work_dir.join("dkms-node-b-master.json").to_string_lossy().into_owned();
    let node2_sock_path = match cfg.dkms_transport {
        DkmsTransport::Unix => work_dir.join("dkms-authority-b.sock").to_string_lossy().into_owned(),
        DkmsTransport::Tcp => pick_tcp_endpoint()?,
    };
    // The runtime's OWN stable caller identity (Day 95–96): a per-run seed → a KNOWN ML-DSA identity
    // the node's allow-list recognizes. The same seed is handed to the key-provider (so the RAIL
    // connects as this known caller) AND to the adversarial probe (so its happy path is allow-listed);
    // the node's allow-list is provisioned with this identity's PUBLIC key. NOT a secret the node
    // holds — the runtime legitimately holds its own identity key (never the dKMS master or a CEK).
    let caller_seed = ddrm_envelope::random_seed();
    let caller_seed_b64 = B64.encode(caller_seed);
    let (_caller_signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);
    let caller_vk_b64 = B64.encode(&caller_vk);
    if cfg.authority == AuthorityBackend::Dkms {
        // Grant the node DAEMON its store via the env it resolves — the key-provider client that
        // connects to the node never passes or sees this path; it's the node's own concern.
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
        &node_sock_path,
        cfg.threshold,
        &node2_store_path,
        &node2_sock_path,
    )?;
    // For `dkms`, START the external NODE DAEMON listening on its socket BEFORE the rail comes up, so
    // the key-provider can CONNECT to it (rather than spawn it). The guard kills + reaps it on any
    // return path. The master is born + stays in the daemon's node-local store.
    // `mut` (and no leading underscore): the node-fault gates KILL + RESTART these guards to prove the
    // live rail fails closed when a secret-holder goes down. On non-threshold/reference paths they are
    // held only for their Drop (teardown) — referenced via the gates below so no unused warning.
    let mut dkms_daemon = if cfg.authority == AuthorityBackend::Dkms {
        let node_bin = cfg
            .dkms_authority_bin
            .as_deref()
            .ok_or("dkms backend requires a dkms_authority_bin in the config")?;
        // Provision the daemon's KNOWN-caller allow-list with the runtime's caller identity, so only
        // this runtime (the key-provider rail + the probe, both deriving the same identity) is served.
        Some(start_dkms_daemon(node_bin, &node_sock_path, &node_store_path, &caller_vk_b64)?)
    } else {
        None
    };
    // 2-of-2 THRESHOLD: ALSO start node B's daemon (its own store/socket, the SAME known caller
    // allow-listed) so the key-provider's dual-recover can reach BOTH secret-holders.
    let mut dkms_daemon_b = if cfg.authority == AuthorityBackend::Dkms && cfg.threshold {
        let node_bin = cfg
            .dkms_authority_bin
            .as_deref()
            .ok_or("dkms threshold requires a dkms_authority_bin in the config")?;
        Some(start_dkms_daemon(node_bin, &node2_sock_path, &node2_store_path, &caller_vk_b64)?)
    } else {
        None
    };
    // The open path binds to the selected backend via its `init` config ONLY — the publish →
    // launch → open → recover/re-seal flow below is byte-identical across backends.
    let key_init_config = match cfg.authority {
        AuthorityBackend::Reference => json!({ "backend": "reference", "authority_key_store": key_store_path }),
        AuthorityBackend::Dkms => json!({
            "backend": "dkms",
            "dkms_authority_descriptor": descriptor_path.to_string_lossy(),
            // The rail connects to the node as the runtime's KNOWN caller identity (allow-listed).
            "dkms_caller_seed_b64": caller_seed_b64,
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
            // THRESHOLD consistency: a threshold open MUST publish a `threshold` block (the key-provider
            // resolves two nodes from it); a single-node open MUST NOT (else the key-provider would
            // dual-recover but the runtime supplies only one share). Fail closed on a mismatch so a
            // config/descriptor desync can never silently degrade the guarantee.
            let has_threshold = desc.get("threshold").is_some();
            if cfg.threshold && !has_threshold {
                return Err("authority.threshold is set but the published descriptor carries no `threshold` block".to_string());
            }
            if !cfg.threshold && has_threshold {
                return Err("the published descriptor carries a `threshold` block but authority.threshold is not set".to_string());
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
                // 2-of-2 THRESHOLD: the boundary needs node B's vk to unwrap share-2 in-VM
                // (`authority_vk2_b64`). `None` for the single-node rail.
                authority_vk2_b64: escrow.vk2_b64.clone(),
            }),
            Box::new(RightsLauncher {
                rights_bin: rights_bin.cloned(),
                cell: rights_cell.clone(),
                chain_attestation: attestation,
                chain_mode: chain_mode.clone(),
            }),
        ],
        // Day 103–104: the sink stamps the threshold node-set identity into every durable open
        // record (None on the single-node rail — the record is then byte-identical to the lib's).
        Box::new(NodeSetStampingSink {
            store,
            node_set_id_b64: escrow.node_set_id_b64.clone(),
        }),
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
    // 2-of-2 THRESHOLD (Day 101–102): PIN the node-set. The producer durably recorded the identity of
    // the node-set it escrowed the two shares to (`node_set_id` over both nodes' vks + t). RE-DERIVE it
    // from the PUBLISHED descriptor's `threshold` block and fail closed if they differ — so a descriptor
    // whose node-set was silently swapped (one node re-pointed at a DIFFERENT secret-holder than the
    // producer escrowed to) is DETECTED before the rail recovers anything. PC2 cannot do this: its
    // node-set lives inside Lit's opaque network, so a swapped member is uninspectable.
    // Day 103–104: the verified node-set is then carried forward (`live_node_set`) and welded into
    // the decrypt-transcript AAD below, so the binding is also CRYPTOGRAPHIC at the boundary.
    let live_node_set: Option<[u8; 32]> = if cfg.threshold {
        let pinned = fixture
            .node_set_id_b64
            .as_ref()
            .ok_or("a threshold publish fixture must pin a node_set_id")?;
        let derived = derive_node_set_from_descriptor(&descriptor_path)?;
        if B64.encode(derived) != *pinned {
            return Err(
                "the published descriptor's node-set does NOT match the producer-pinned node_set_id — a node was swapped"
                    .to_string(),
            );
        }
        Some(derived)
    } else {
        None
    };
    // The session binding is the ONLY per-open part: compute the transcript AAD over the
    // decrypt boundary's freshly-minted session key, then bind the key transport's material
    // (wrapped CEK + producer vk + content hash + nonce come from the publish fixture).
    // On the threshold rail the AAD ALSO carries the verified node-set id (Day 103–104) —
    // the boundary independently derives the same id from ITS pinned vks, so a node-set
    // swap fails the AEAD open itself.
    let session_pub = B64.decode(&session_pub_b64).map_err(|e| e.to_string())?;
    let content_hash = B64.decode(&fixture.content_hash_b64).map_err(|e| e.to_string())?;
    let nonce = B64.decode(&fixture.nonce_b64).map_err(|e| e.to_string())?;
    let aad = transcript_aad(
        &session_pub,
        &content_hash,
        &nonce,
        live_node_set.as_ref().map(|i| i.as_slice()),
    );
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
        // 2-of-2 THRESHOLD: node B's escrowed share-2 (from the publish fixture); the key-provider
        // dual-recovers BOTH nodes when present. `None` for the single-node rail.
        wrapped_cek_share2_b64: fixture.wrapped_cek_share2_b64.clone(),
    });
    step(3, &format!(
        "runtime-core host: authority recipient STABLE across relaunch; bound key material from the publish fixture + the per-open session key{}",
        if cfg.threshold { " (2-of-2 threshold: BOTH share escrows bound, neither node holds the whole CEK)" } else { "" },
    ));

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
        // Day 103–104: the threshold open's records are AUDITABLE — each durable record carries
        // the node-set identity that served it, equal to the producer-pinned id; a single-node
        // open's records carry no such field (byte-identical to the pre-threshold record).
        match &fixture.node_set_id_b64 {
            Some(pin) => {
                if record["node_set_id_b64"].as_str() != Some(pin.as_str()) {
                    return Err(format!(
                        "durable record {name} does not carry the serving node-set identity (auditability)"
                    ));
                }
            }
            None => {
                if record.get("node_set_id_b64").is_some() {
                    return Err(format!("durable record {name} carries a node-set id on a single-node rail"));
                }
            }
        }
    }
    step(8, &format!(
        "runtime-core host: PERSISTED the runtime-owned post-steps (release_receipt + audit) as durable CEK-free records; read back through a fresh DurableEventStore{}",
        if cfg.threshold { " — each record STAMPED with the serving node-set identity (auditable)" } else { "" },
    ));

    // In `open` mode (the operator path) we are done: a real open ran and a durable, CEK-free
    // record persisted. `verify` mode (the consumer smoke) additionally drives the two
    // adversarial fail-closed gates below before tearing the rail down.
    if cfg.mode == OpenMode::Verify {
    // --- fail-closed #1 (crypto binding): a replayed/altered transcript must not open.
    // Re-seal to a DIFFERENT nonce while the material still names the original — the
    // boundary rebuilds the original transcript and the seal cannot open.
    let bad_nonce = b"consumer-smoke-nonce-9".to_vec();
    let bad_aad = transcript_aad(
        &session_pub,
        &content_hash,
        &bad_nonce,
        live_node_set.as_ref().map(|i| i.as_slice()),
    );
    let mut bad_req = key_release_request_base(&kid_hex, &wrapped_cek_b64);
    bad_req
        .as_object_mut()
        .expect("key release request is an object")
        .insert("rights_receipt".to_string(), fallback_rights_receipt());
    let mut bad_session = json!({
        "decrypt_session_pub_b64": session_pub_b64,
        "producer_vk_b64": producer_vk_b64,
        "aad_b64": B64.encode(&bad_aad),
        "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
        "content_hash_b64": B64.encode(&content_hash),
        "nonce_b64": B64.encode(&nonce),
        "now_unix": NOW_UNIX,
    });
    // THRESHOLD: the live key capsule has TWO nodes provisioned, so a release must supply share-2 or
    // it fails closed for the WRONG reason — here we want the release to SUCCEED (re-sealing BOTH
    // shares to the bad transcript) and the DECRYPT to be the thing that fails closed.
    if let Some(share2) = &escrow.wrapped_cek_share2_b64 {
        bad_session["wrapped_cek_share2_b64"] = json!(share2);
    }
    let bad_release = ok_data(
        &key_cell
            .borrow_mut()
            .as_mut()
            .ok_or("key capsule torn down before the raw transcript gate")?
            .call(&json!({
                "op": "release",
                "request": bad_req,
                "session": bad_session,
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

    // --- fail-closed #3 + #4 (THRESHOLD wiring, Day 99–100): only when this run is a 2-of-2 rail.
    if cfg.threshold {
        // #3: the live THRESHOLD key capsule (two nodes provisioned) must REFUSE a release that omits
        // the second share — it must never silently degrade to a one-node recover. Drive the live key
        // capsule directly with a well-formed, correctly-bound release whose session DROPS
        // `wrapped_cek_share2_b64`; the real key-provider fails closed.
        let mut single_req = key_release_request_base(&kid_hex, &wrapped_cek_b64);
        single_req
            .as_object_mut()
            .expect("key release request is an object")
            .insert("rights_receipt".to_string(), fallback_rights_receipt());
        let single_resp = key_cell
            .borrow_mut()
            .as_mut()
            .ok_or("key capsule torn down before the threshold single-share gate")?
            .call(&json!({
                "op": "release",
                "request": single_req,
                "session": {
                    "decrypt_session_pub_b64": session_pub_b64,
                    "producer_vk_b64": producer_vk_b64,
                    "aad_b64": B64.encode(&aad),
                    "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                    "content_hash_b64": B64.encode(&content_hash),
                    "nonce_b64": B64.encode(&nonce),
                    "now_unix": NOW_UNIX,
                    // NOTE: no `wrapped_cek_share2_b64` — the threshold rail must refuse this.
                }
            }))?;
        if single_resp.get("status").and_then(Value::as_str) != Some("error") {
            return Err(format!(
                "the threshold key authority opened with only ONE share — it must fail closed without the second: {single_resp}"
            ));
        }
        step(21, "key-provider (2-of-2): a release supplying only ONE share failed closed — the threshold rail refuses to degrade to a single node");

        // #4: a fresh key capsule whose dkms descriptor requests an UNIMPLEMENTED threshold shape
        // (3-of-N) must FAIL CLOSED at init — the runtime never silently downgrades a stronger
        // threshold to what it can do. Cross-binary against the real key binary.
        let bad_desc_path = work_dir.join("dkms-authority-3ofN.json");
        let bad_desc = json!({
            "schema": "elastos.dkms.authority/v2",
            "verifying_key_b64": "AAAA",
            "recipient_pub_b64": "BBBB",
            "authority_endpoint": node_sock_path,
            "threshold": {
                "t": 3,
                "nodes": [
                    { "verifying_key_b64": "AAAA", "recipient_pub_b64": "BBBB", "authority_endpoint": node_sock_path },
                    { "verifying_key_b64": "CCCC", "recipient_pub_b64": "DDDD", "authority_endpoint": node2_sock_path },
                    { "verifying_key_b64": "EEEE", "recipient_pub_b64": "FFFF", "authority_endpoint": node_sock_path },
                ],
            },
        });
        std::fs::write(&bad_desc_path, serde_json::to_vec_pretty(&bad_desc).map_err(|e| e.to_string())?)
            .map_err(|e| format!("write 3-of-N descriptor: {e}"))?;
        let mut fresh_key = Capsule::spawn("key-provider(3-of-N gate)", key_bin)?;
        let init_resp = fresh_key.call(&json!({
            "op": "init",
            "config": {
                "backend": "dkms",
                "dkms_authority_descriptor": bad_desc_path.to_string_lossy(),
                "dkms_caller_seed_b64": caller_seed_b64,
            }
        }))?;
        fresh_key.shutdown();
        if init_resp.get("status").and_then(Value::as_str) != Some("error") {
            return Err(format!(
                "a 3-of-N threshold descriptor must fail closed at key-provider init (only 2-of-2 is implemented): {init_resp}"
            ));
        }
        step(22, "key-provider (2-of-2): a 3-of-N threshold descriptor failed closed at init — the runtime never silently downgrades a stronger threshold");

        // --- fail-closed #5 + #6 (NODE FAULT, Day 101–102): the LIVE 2-of-2 rail must fail closed if
        // EITHER secret-holder goes down — NO partial CEK, NO single-node fallback, NO record persisted.
        // PC2 cannot express this: a downed node lives inside Lit's opaque network, so its only recourse
        // is to retry the whole opaque RPC (chipotle-client.ts:575); it has no per-node fault semantics.
        // We OWN the two nodes, so we drive the real fault through the production host.
        tamper.set(false); // step 10 left the plan source in tamper mode; restore the honest plan.
        let persisted_threshold = DurableEventStore::load(&receipts_dir)?.len();
        let node_bin = cfg
            .dkms_authority_bin
            .as_deref()
            .ok_or("threshold node-fault gate requires a dkms_authority_bin")?;

        // #5: node B DOWN → the dual-recover fails at node B; host.open fails closed, persists nothing.
        if let Some(mut g) = dkms_daemon_b.take() {
            let _ = g.child.kill();
            let _ = g.child.wait();
        }
        if host.open(&cid(), VIEWER).is_ok() {
            return Err("the 2-of-2 rail opened with node B DOWN — it must fail closed, never fall back to one node".to_string());
        }
        if DurableEventStore::load(&receipts_dir)?.len() != persisted_threshold {
            return Err("a node-B-down open must persist no runtime-event record".to_string());
        }
        // RESTART node B: the node-set AAD gate below (gate 26) drives a LIVE dual-recover, so both
        // secret-holders must be reachable again.
        dkms_daemon_b = Some(start_dkms_daemon(node_bin, &node2_sock_path, &node2_store_path, &caller_vk_b64)?);
        step(23, "key-provider (2-of-2): node B DOWN → the live rail failed closed (no partial CEK, no single-node fallback, no record persisted); node B restored");

        // #6: node A DOWN → the dual-recover fails at node A (recovered first); same fail-closed property.
        if let Some(mut g) = dkms_daemon.take() {
            let _ = g.child.kill();
            let _ = g.child.wait();
        }
        if host.open(&cid(), VIEWER).is_ok() {
            return Err("the 2-of-2 rail opened with node A DOWN — it must fail closed".to_string());
        }
        if DurableEventStore::load(&receipts_dir)?.len() != persisted_threshold {
            return Err("a node-A-down open must persist no runtime-event record".to_string());
        }
        // RESTART node A: the post-shutdown adversarial probe (steps 13–17) connects to node A's socket.
        dkms_daemon = Some(start_dkms_daemon(node_bin, &node_sock_path, &node_store_path, &caller_vk_b64)?);
        step(24, "key-provider (2-of-2): node A DOWN → the live rail failed closed; the runtime never degrades a 2-of-2 to a single node");

        // --- fail-closed #7 (NODE-SET SWAP, Day 101–102): the producer durably PINNED the identity of
        // the node-set it escrowed to (`node_set_id`). Prove a descriptor whose node B was silently
        // re-pointed at a DIFFERENT secret-holder is DETECTED — its re-derived node-set id no longer
        // matches the pin. PC2 cannot do this: its node-set is opaque inside Lit, so a swapped member is
        // uninspectable. (The decrypt boundary independently rejects the swapped node's seal under the
        // pinned node-B vk — Day 97–98 step 20 — so the swap fails at BOTH the descriptor + the boundary.)
        let pinned = fixture
            .node_set_id_b64
            .as_ref()
            .ok_or("threshold fixture must pin a node_set_id")?;
        let desc: Value = serde_json::from_slice(
            &std::fs::read(&descriptor_path).map_err(|e| format!("re-read descriptor for swap gate: {e}"))?,
        )
        .map_err(|e| format!("parse descriptor for swap gate: {e}"))?;
        let nodes = desc["threshold"]["nodes"].as_array().ok_or("descriptor has no node list")?;
        let vk_a = B64.decode(nodes[0]["verifying_key_b64"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
        // A rogue secret-holder's vk (a DISTINCT identity the attacker controls).
        let (_rogue_signer, rogue_vk) = ddrm_envelope::seal::mldsa_seal_keypair([0xE7u8; 32]);
        let swapped_id = ddrm_envelope::threshold_node_set_id(2, &vk_a, &rogue_vk);
        if B64.encode(swapped_id) == *pinned {
            return Err("a node-set with node B swapped to a rogue identity matched the pinned id — the pin is not binding".to_string());
        }
        step(25, "runtime-core host: a node-set with node B SWAPPED to a rogue secret-holder is DETECTED (node-set-id pin mismatch) — a silently swapped node never passes the open");

        // --- fail-closed #8 (NODE-SET AAD BINDING, Day 103–104): the node-set is welded into the
        // decrypt-transcript AAD itself, so a release bound to a DIFFERENT node-set fails the AEAD
        // open AT THE BOUNDARY — live, cross-binary, even though both nodes re-sealed honestly and
        // every per-share signature verifies. Drive the LIVE key capsule with a well-formed release
        // whose AAD names a FORGED node-set (the nodes treat the AAD as opaque bytes, so the release
        // SUCCEEDS), then prove the LIVE decrypt capsule — which derives the TRUE node-set from its
        // own pinned vks — refuses to open it.
        let forged_set = ddrm_envelope::threshold_node_set_id(2, &vk_a, &rogue_vk);
        let forged_aad = transcript_aad(&session_pub, &content_hash, &nonce, Some(&forged_set));
        let mut forged_req = key_release_request_base(&kid_hex, &wrapped_cek_b64);
        forged_req
            .as_object_mut()
            .expect("key release request is an object")
            .insert("rights_receipt".to_string(), fallback_rights_receipt());
        let forged_release = ok_data(
            &key_cell
                .borrow_mut()
                .as_mut()
                .ok_or("key capsule torn down before the node-set AAD gate")?
                .call(&json!({
                    "op": "release",
                    "request": forged_req,
                    "session": {
                        "decrypt_session_pub_b64": session_pub_b64,
                        "producer_vk_b64": producer_vk_b64,
                        "aad_b64": B64.encode(&forged_aad),
                        "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
                        "content_hash_b64": B64.encode(&content_hash),
                        "nonce_b64": B64.encode(&nonce),
                        "now_unix": NOW_UNIX,
                        "wrapped_cek_share2_b64": escrow.wrapped_cek_share2_b64.clone()
                            .ok_or("threshold fixture must carry share-2 for the node-set AAD gate")?,
                    }
                }))?,
            "key release (forged node-set AAD)",
        )?;
        let mut forged_open_req = decrypt_request_base();
        forged_open_req
            .as_object_mut()
            .expect("decrypt request is an object")
            .insert("release_receipt".to_string(), release_receipt_json());
        forged_open_req["object_cid"] = json!(cid());
        forged_open_req["viewer_interface"] = json!(VIEWER);
        let forged_open = decrypt_cell
            .borrow_mut()
            .as_mut()
            .ok_or("decrypt capsule torn down before the node-set AAD gate")?
            .call(&json!({
                "op": "open_session_v1",
                "request": forged_open_req,
                "material": forged_release["material"].clone(),
                "now_unix": NOW_UNIX,
            }))?;
        if forged_open.get("data").and_then(|d| d.get("decision")).and_then(Value::as_str) == Some("opened") {
            return Err(format!("a release bound to a FORGED node-set must NOT open at the boundary: {forged_open}"));
        }
        step(26, "decrypt-provider (2-of-2): a release whose transcript names a FORGED node-set failed the AEAD open at the boundary — the node-set binding is cryptographic, not just descriptor parse");

        // --- fail-closed #9 (ROTATION SAFETY, Day 103–104): a publish escrowed to node-set {A,B}
        // can never be opened against a ROTATED node-set {A,B'} — provision a REAL fresh node B'
        // (its own store → a genuinely distinct identity), publish a rotated descriptor naming it,
        // and prove the OLD fixture's pin no longer matches via the SAME derivation `run()` enforces.
        // A rotation is a NEW publish (new fixture + descriptor pair); a stale fixture fails closed.
        let rotated_store = work_dir.join("dkms-node-b-rotated.json").to_string_lossy().into_owned();
        let (vk_b_rotated, recipient_b_rotated) = provision_dkms_node(node_bin, &rotated_store)?;
        let desc_now: Value = serde_json::from_slice(
            &std::fs::read(&descriptor_path).map_err(|e| format!("re-read descriptor for rotation gate: {e}"))?,
        )
        .map_err(|e| format!("parse descriptor for rotation gate: {e}"))?;
        if vk_b_rotated == desc_now["threshold"]["nodes"][1]["verifying_key_b64"].as_str().unwrap_or("") {
            return Err("the rotated node B derived the SAME identity as the original — rotation produced no new secret-holder".to_string());
        }
        let rotated_desc_path = work_dir.join("dkms-authority-rotated.json");
        let mut rotated_desc = desc_now.clone();
        rotated_desc["threshold"]["nodes"][1] = json!({
            "verifying_key_b64": vk_b_rotated,
            "recipient_pub_b64": recipient_b_rotated,
            "authority_endpoint": node2_sock_path,
        });
        std::fs::write(
            &rotated_desc_path,
            serde_json::to_vec_pretty(&rotated_desc).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("write rotated descriptor: {e}"))?;
        let rotated_id = derive_node_set_from_descriptor(&rotated_desc_path)?;
        if B64.encode(rotated_id) == *pinned {
            return Err("a ROTATED node-set matched the old fixture's pin — rotation is not detectable".to_string());
        }
        // And the rotated descriptor IS self-consistent for a NEW publish: re-deriving from it is
        // stable (a fresh fixture pinned to the rotated set would open — rotation is forward-safe).
        if derive_node_set_from_descriptor(&rotated_desc_path)? != rotated_id {
            return Err("the rotated node-set derivation is not deterministic".to_string());
        }
        step(27, "runtime-core host: ROTATION is fail-closed — a REAL freshly-provisioned node B' yields a node-set the old fixture's pin REFUSES (a stale publish can never open against a rotated node-set), while the rotated descriptor re-derives stably for a new publish");
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

        // Verify mode: prove the AUTHENTICATED channel cross-binary — a tampered node identity is
        // rejected at the handshake, and the node refuses a recover whose authorization does not
        // bind the content/principal. The happy-path open above already proved the genuine identity
        // verifies + a content-bound recover decrypts; here we prove the adversarial edges.
        if cfg.mode == OpenMode::Verify {
            let desc: Value = serde_json::from_slice(&before).map_err(|e| format!("parse dkms descriptor: {e}"))?;
            let pinned_vk = desc
                .get("verifying_key_b64")
                .and_then(Value::as_str)
                .ok_or("dkms descriptor missing verifying_key_b64")?;
            // The probe connects to the SAME running daemon the rail used, over the published socket.
            let endpoint = desc
                .get("authority_endpoint")
                .and_then(Value::as_str)
                .ok_or("dkms descriptor missing authority_endpoint")?;
            let probe_material = key_material
                .borrow()
                .clone()
                .ok_or("dkms verify probe needs the bound key material from the open")?;
            dkms_node_adversarial_probe(endpoint, pinned_vk, &probe_material, caller_seed)?;

            // NETWORK CHANNEL (Day 105–108): on the TCP transport, ALSO drive the hostile-network
            // gates against the live daemon — plaintext recover refused, downgrade dropped,
            // MITM-tampered frame dropped, wrong-node channel key refused under the pinned identity.
            if cfg.dkms_transport == DkmsTransport::Tcp {
                dkms_tcp_channel_adversarial_gates(endpoint, pinned_vk, caller_seed)?;
            }

            // 2-of-2 THRESHOLD (Day 97–98): prove, across TWO real node daemons, that the CEK is split
            // so no single node holds the whole key, the boundary reconstructs it only in-boundary, and
            // a single/forged share fails closed. Self-contained (its own two daemons).
            if let Some(node_bin) = cfg.dkms_authority_bin.as_deref() {
                dkms_threshold_probe(node_bin, &work_dir)?;
            }
        }
    }
    // The cells kept for the raw gate are now stale (the host tore the processes down); drop
    // them and clean up the durable artifacts (key store + fixture + receipts) unless the
    // config asks to keep them.
    drop(rights_cell);
    drop(key_cell);
    drop(decrypt_cell);
    // Tear down the external dkms daemons LAST — they had to outlive the post-shutdown probes
    // (the adversarial probe connects to node A's socket). Explicit drop so the restarted node-A
    // guard (Day 101–102 node-fault gate) is observably consumed here, not just at scope end.
    drop(dkms_daemon);
    drop(dkms_daemon_b);
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
    fn threshold_parses_for_dkms_and_defaults_off() {
        let base = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        // Default: no threshold.
        assert!(!OpenConfig::from_json(&base).unwrap().threshold);

        // dkms + threshold:true parses.
        let mut thr = base.clone();
        thr.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node", "threshold": true }),
        );
        let parsed = OpenConfig::from_json(&thr).unwrap();
        assert!(parsed.threshold && parsed.authority == AuthorityBackend::Dkms);

        // dkms + threshold:false is explicit single-node.
        let mut single = base.clone();
        single.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node", "threshold": false }),
        );
        assert!(!OpenConfig::from_json(&single).unwrap().threshold);
    }

    #[test]
    fn threshold_requires_dkms_and_a_boolean() {
        // threshold on the reference backend fails closed (no external secret-holders to split across).
        let mut ref_thr = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        ref_thr.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "reference", "threshold": true }),
        );
        let err = OpenConfig::from_json(&ref_thr).expect_err("threshold needs dkms");
        assert!(err.contains("threshold"), "the error names the bad field: {err}");

        // A non-boolean threshold fails closed.
        let mut bad = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        bad.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node", "threshold": "yes" }),
        );
        assert!(OpenConfig::from_json(&bad)
            .expect_err("a non-boolean threshold must fail closed")
            .contains("threshold"));
    }

    #[test]
    fn transport_parses_for_dkms_and_defaults_to_unix() {
        let base = json!({ "key_bin": "/k", "decrypt_bin": "/d", "drm_bin": "/m" });
        // Default: unix (host-local sockets).
        assert!(OpenConfig::from_json(&base).unwrap().dkms_transport == DkmsTransport::Unix);

        // dkms + transport:"tcp" parses (the node off localhost).
        let mut tcp = base.clone();
        tcp.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node", "transport": "tcp" }),
        );
        assert!(OpenConfig::from_json(&tcp).unwrap().dkms_transport == DkmsTransport::Tcp);

        // transport:"tcp" on the reference backend fails closed (there is no node daemon to address).
        let mut ref_tcp = base.clone();
        ref_tcp.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "reference", "transport": "tcp" }),
        );
        assert!(OpenConfig::from_json(&ref_tcp)
            .expect_err("tcp transport needs the dkms backend")
            .contains("transport"));

        // An unknown transport fails closed.
        let mut bad = base.clone();
        bad.as_object_mut().unwrap().insert(
            "authority".into(),
            json!({ "backend": "dkms", "dkms_authority_bin": "/node", "transport": "carrier-pigeon" }),
        );
        assert!(OpenConfig::from_json(&bad)
            .expect_err("an unknown transport must fail closed")
            .contains("transport"));
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
