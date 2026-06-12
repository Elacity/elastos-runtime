//! LIVE 2-of-3 dKMS recover proof — drives the THREE REAL production authority daemons.
//!
//! This binary is the runtime CLIENT + the producer + the decrypt-boundary combine, run
//! against a PUBLIC-ONLY v2 descriptor that pins three live `dkms-authority` nodes
//! (`tcp:10.66.66.x:9443` over the dkms0 mesh). It:
//!
//!   1. mints a CEK and Shamir-splits it 2-of-3 over GF(256) into three INDEXED shares;
//!   2. for each node, escrows that node's indexed share to the node's PUBLISHED recipient
//!      key (sealed under the SHARED escrow AAD) — so no node ever receives more than its
//!      own share, and the producer never contacts the node to do it;
//!   3. opens the SAME app-layer encrypted, mutually-authenticated channel the production
//!      `key-provider` uses — a `hello` that offers a fresh client channel-KEM key and
//!      verifies the node's identity attestation AND its channel-key attestation under the
//!      PINNED descriptor vk (a swapped node fails closed), then switches to sealed frames;
//!   4. drives the full `recover` over that channel under our ALLOW-LISTED caller identity,
//!      with a live session token + a per-recover possession proof + freshness counter;
//!   5. unwraps each node's re-sealed share in-boundary (verified under THAT node's vk) and
//!      reconstructs the CEK from ANY TWO — proving any-2-of-3 opens and the rail survives a
//!      dead node, while a single share is NOT the key.
//!
//! The raw CEK and the raw shares NEVER cross a node boundary. Everything reuses
//! `ddrm-envelope`, so there is no private re-implementation of the wire/escrow/split.
//!
//! Usage:
//!   dkms-live-recover <descriptor.json> <caller.seed (base64 of 32 bytes)>
//! Exit: 0 = PASS, 1 = FAIL.

use base64::Engine;
use ddrm_envelope::transcript::{escrow_aad, DecryptTranscriptV1};
use serde_json::{json, Value};
use std::io::{BufReader, Write};
use std::time::Instant;

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const SUITE: &str = ddrm_envelope::SUITE_PQ_HYBRID;

// One self-consistent decrypt-boundary stand-in. The recover authorization, the possession
// proof, and the transcript AAD all bind these — the same shape the rail's transcript uses.
const CONTENT: &str = "bafLiveRecover";
const PRINCIPAL: &str = "did:key:zViewer";
const SESSION: &str = "probe-session";
const RIGHT: &str = "view";
const NOW_UNIX: u64 = 1_850_000_000;
const EXPIRES_AT: u64 = 1_900_000_000;

// A structurally-required field on the recover bundle (the segment the boundary would decrypt).
// The node binds the transcript via the caller-supplied content_hash/aad, not by re-hashing this,
// so any well-formed ciphertext satisfies the gate; we pass the committed golden CENC sample.
const GOLDEN_CIPHERTEXT_B64: &str = "AAAAPG1vb2YAAAA0dHJhZgAAABR0cnVuAAACAAAAAAEAAAAgAAAAGHNlbmMAAAAAAAAAASIiIiIiIiIiAAAAKG1kYXScNDPiT64BF0MfL13dprDn+6eX7LyGcmlu1lMPPiQQpA==";

/// One live authority as pinned by the descriptor: where it lives + the public identity we
/// pin it to (its master-derived signing key) + the recipient we escrow its share to.
struct NodePin {
    label: String,
    endpoint: String,
    vk_b64: String,
    recipient_pub_b64: String,
}

/// The established encrypted channel to a node — the production channel discipline: requests
/// sealed to the node's ATTESTED channel-KEM key under our caller identity, responses opened
/// with our ephemeral secret and verified under the PINNED node identity.
struct Channel {
    channel_id: Vec<u8>,
    node_pub: ddrm_envelope::SessionKemPublic,
    secret: ddrm_envelope::SessionKemSecret,
    node_verifier: ddrm_envelope::MlDsa65Verifier,
    signer: ddrm_envelope::seal::MlDsaSealSigner,
    send_seq: u64,
    recv_seq: u64,
}

/// A framed TCP client to a live node, upgraded to sealed frames once the channel is established.
struct NodeSocket {
    writer: Box<dyn Write>,
    reader: Box<dyn std::io::Read>,
    channel: Option<Channel>,
}

impl NodeSocket {
    fn connect(endpoint: &str) -> Result<Self, String> {
        let addr = endpoint
            .strip_prefix("tcp:")
            .ok_or_else(|| format!("live recover requires a tcp: endpoint (got {endpoint})"))?;
        let stream = std::net::TcpStream::connect(addr)
            .map_err(|e| format!("connect {addr}: {e}"))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(20)))
            .map_err(|e| e.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        Ok(Self { writer: Box::new(stream), reader: Box::new(reader), channel: None })
    }

    /// One framed request, one framed response — sealed both ways once the channel is up.
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
            Ok(Some(b)) => b,
            Ok(None) => return Err("node closed the connection".to_string()),
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

    /// Seal `req` for the established channel and return the wire bytes (advances send_seq) — used by
    /// the adversarial gates to corrupt or bypass a frame the node would otherwise accept.
    fn seal_for_channel(&mut self, req: &Value) -> Result<Vec<u8>, String> {
        let ch = self.channel.as_mut().ok_or("seal needs an established channel")?;
        let payload = serde_json::to_vec(req).map_err(|e| e.to_string())?;
        ch.send_seq += 1;
        let aad = ddrm_envelope::channel_frame_aad(&ch.channel_id, 0, ch.send_seq);
        Ok(ddrm_envelope::seal::seal_bound(&ch.node_pub, &payload, &aad, &ch.signer).to_bytes())
    }

    /// Write one RAW frame (bypassing channel sealing) and read the response, if any. A fail-closed
    /// node DROPS the connection on a plaintext/tampered frame → `Ok(None)` (or an error).
    fn raw_round_trip(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, String> {
        ddrm_envelope::frame::write_frame(&mut self.writer, frame).map_err(|e| format!("write raw: {e}"))?;
        ddrm_envelope::frame::read_frame(&mut self.reader).map_err(|e| format!("read raw: {e}"))
    }

    /// Establish the encrypted channel: `hello` with a fresh client channel key, verify the
    /// node's identity attestation + its CHANNEL-KEY attestation under the PINNED vk (a
    /// substituted key fails closed), and switch to sealed frames. Returns the hello data.
    fn establish_channel(
        &mut self,
        pinned_vk_b64: &str,
        caller_seed: [u8; 32],
        challenge: [u8; 32],
    ) -> Result<Value, String> {
        let (signer, caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);
        let (secret, client_pub) = ddrm_envelope::mint_session();
        let hello = self.call(&json!({
            "op": "hello",
            "challenge_b64": B64.encode(challenge),
            "caller_pub_b64": B64.encode(&caller_vk),
            "now_unix": NOW_UNIX,
            "channel_pub_b64": B64.encode(ddrm_envelope::session_public_bytes(&client_pub)),
        }))?;
        let data = ok_data(&hello, "hello (channel establishment)")?;
        if data["verifying_key_b64"].as_str() != Some(pinned_vk_b64) {
            return Err("node hello advertised a vk that does not match the pinned descriptor".to_string());
        }
        let pinned = B64.decode(pinned_vk_b64).map_err(|e| e.to_string())?;
        let node_verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&pinned)
            .ok_or("pinned vk is malformed")?;
        let attestation = B64
            .decode(data["attestation_b64"].as_str().unwrap_or(""))
            .map_err(|e| format!("attestation is not base64: {e}"))?;
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
        self.channel = Some(Channel {
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
}

fn ok_data(resp: &Value, ctx: &str) -> Result<Value, String> {
    if resp.get("status").and_then(Value::as_str) == Some("ok") {
        Ok(resp.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!("{ctx}: expected ok, got {resp}"))
    }
}

/// A coherent, self-consistent allowed rights receipt the node re-authorizes the recover against.
fn rights_receipt() -> Value {
    json!({
        "schema": "elastos.rights.decision.receipt/v1",
        "request_id": "live-recover",
        "content_id": CONTENT,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": RIGHT,
        "provider": "rights-provider",
        "allowed": true,
        "issued_at": 1,
        "expires_at": u64::MAX,
    })
}

/// Read the three pinned nodes from a PUBLIC-ONLY v2 descriptor, in x-order (node i -> x=i+1).
fn load_pins(desc: &Value) -> Result<Vec<NodePin>, String> {
    let nodes = desc
        .get("threshold")
        .and_then(|t| t.get("nodes"))
        .and_then(|n| n.as_array())
        .ok_or("descriptor has no threshold.nodes array")?;
    if nodes.len() != 3 {
        return Err(format!("expected 3 nodes for a 2-of-3 quorum, descriptor lists {}", nodes.len()));
    }
    let field = |n: &Value, k: &str| -> Result<String, String> {
        n.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("descriptor node is missing `{k}`"))
    };
    let mut pins = Vec::with_capacity(3);
    for (i, n) in nodes.iter().enumerate() {
        pins.push(NodePin {
            label: format!("node{}", (b'A' + i as u8) as char),
            endpoint: field(n, "authority_endpoint")?,
            vk_b64: field(n, "verifying_key_b64")?,
            recipient_pub_b64: field(n, "recipient_pub_b64")?,
        });
    }
    Ok(pins)
}

/// The bundle a recover needs that is the SAME across all three nodes (only the node identity +
/// its escrowed share differ): the producer identity, the decrypt session, and the transcript AAD.
struct Boundary {
    producer_signer: ddrm_envelope::seal::MlDsaSealSigner,
    producer_vk_b64: String,
    caller_seed: [u8; 32],
    caller_signer: ddrm_envelope::seal::MlDsaSealSigner,
    session_secret: ddrm_envelope::SessionKemSecret,
    session_pub_bytes: Vec<u8>,
    session_pub_b64: String,
    kid16: [u8; 16],
    kid_hex: String,
    aad: Vec<u8>,
    aad_b64: String,
    content_hash_b64: String,
    nonce_b64: String,
}

/// Build the canonical `recover` request: escrow `share` to `pin`'s recipient under the shared AAD,
/// then sign the possession proof over THIS session's challenge. The same request the happy path
/// sends and the adversarial gates corrupt.
fn recover_value(token: &Value, pin: &NodePin, share: &[u8], b: &Boundary) -> Result<Value, String> {
    let recipient_bytes = B64.decode(&pin.recipient_pub_b64).map_err(|e| e.to_string())?;
    let recipient_public = ddrm_envelope::session_public_from_bytes(&recipient_bytes)
        .ok_or("node published a malformed recipient")?;
    let escrow = escrow_aad(SUITE, &b.kid16, &recipient_bytes);
    let wrapped = B64.encode(
        ddrm_envelope::seal::seal_bound(&recipient_public, share, &escrow, &b.producer_signer).to_bytes(),
    );
    let chal = B64.decode(token["challenge_b64"].as_str().unwrap_or("")).unwrap_or_default();
    let caller_sig_b64 = B64.encode(ddrm_envelope::sign_recover_proof(
        &b.caller_signer,
        &chal,
        CONTENT.as_bytes(),
        b.kid_hex.as_bytes(),
        &b.session_pub_bytes,
        1,
    ));
    Ok(json!({
        "op": "recover",
        "wrapped_cek_b64": wrapped,
        "scheme": SUITE,
        "kid_hex": b.kid_hex,
        "producer_vk_b64": b.producer_vk_b64,
        "decrypt_session_pub_b64": b.session_pub_b64,
        "aad_b64": b.aad_b64,
        "ciphertext_b64": GOLDEN_CIPHERTEXT_B64,
        "content_hash_b64": b.content_hash_b64,
        "nonce_b64": b.nonce_b64,
        "rights_receipt": rights_receipt(),
        "content_id": CONTENT,
        "principal_id": PRINCIPAL,
        "session_id": SESSION,
        "right": RIGHT,
        "session_token": token,
        "caller_sig_b64": caller_sig_b64,
        "recover_seq": 1u64,
        "now_unix": NOW_UNIX,
    }))
}

/// Connect, `init` (load the node's durable master into this fresh connection — idempotent + safe on
/// a live node), and open the encrypted, mutually-authenticated channel (pins the node identity).
/// Returns the open socket + the live session token.
fn connect_and_open(pin: &NodePin, b: &Boundary) -> Result<(NodeSocket, Value), String> {
    let mut node = NodeSocket::connect(&pin.endpoint)?;
    ok_data(&node.call(&json!({ "op": "init", "config": {} }))?, &format!("{} init", pin.label))?;
    let hello = node.establish_channel(&pin.vk_b64, b.caller_seed, ddrm_envelope::random_seed())?;
    let token = hello["session_token"].clone();
    if !token.is_object() {
        return Err("node hello returned no session token".to_string());
    }
    Ok((node, token))
}

/// Escrow `share` to `pin`'s recipient, open the channel, drive the full recover, and return the
/// node's RE-SEALED share bytes (sealed to our decrypt session, under the node's identity).
fn recover_share(pin: &NodePin, share: &[u8], b: &Boundary) -> Result<(Vec<u8>, std::time::Duration), String> {
    let started = Instant::now();
    let (mut node, token) = connect_and_open(pin, b)?;
    let req = recover_value(&token, pin, share, b)?;
    let recover = ok_data(&node.call(&req)?, &format!("{} recover", pin.label))?;
    let sealed_b64 = recover["material"]["sealed_cek_b64"]
        .as_str()
        .ok_or("node returned no sealed share")?;
    let sealed = B64.decode(sealed_b64).map_err(|e| e.to_string())?;
    Ok((sealed, started.elapsed()))
}

/// WAN ADVERSARIAL GATES against one live authority: prove the encrypted channel holds over the real
/// network. (4) a MITM-tampered sealed recover (one flipped ciphertext byte) is DROPPED; (5) a
/// plaintext-DOWNGRADE recover on the established channel is DROPPED. Both must fail closed — the
/// node returns no usable share.
fn adversarial_gates(pin: &NodePin, share: &[u8], b: &Boundary) -> Result<(), String> {
    // (4) MITM-tamper: establish a real channel, seal a real recover, flip one byte, send raw.
    let (mut node, token) = connect_and_open(pin, b)?;
    let req = recover_value(&token, pin, share, b)?;
    let mut sealed = node.seal_for_channel(&req)?;
    let last = sealed.len() - 1;
    sealed[last] ^= 0x01;
    match node.raw_round_trip(&sealed) {
        Ok(None) | Err(_) => {}
        Ok(Some(_)) => {
            return Err("a MITM-tampered sealed frame was answered — the channel AEAD is not enforced".to_string())
        }
    }
    println!("  {} : MITM-tampered recover frame DROPPED ✓", pin.label);

    // (5) plaintext downgrade: establish a real channel, then send the recover as PLAINTEXT bytes.
    let (mut node2, token2) = connect_and_open(pin, b)?;
    let req2 = recover_value(&token2, pin, share, b)?;
    let plaintext = serde_json::to_vec(&req2).map_err(|e| e.to_string())?;
    match node2.raw_round_trip(&plaintext) {
        Ok(None) | Err(_) => {}
        Ok(Some(bytes)) => {
            // A node that answers at all must NOT have served a usable recover (it would be sealed
            // garbage we cannot open); a plaintext `ok` recover here is an outright downgrade.
            if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                if v.get("status").and_then(Value::as_str) == Some("ok") {
                    return Err("a plaintext-downgrade recover was served — the channel is not required".to_string());
                }
            }
        }
    }
    println!("  {} : plaintext-downgrade recover DROPPED ✓", pin.label);
    Ok(())
}

/// Unwrap a node's re-sealed share in-boundary, verified under THAT node's pinned vk + the AAD.
fn open_share(sealed: &[u8], vk_b64: &str, b: &Boundary) -> Result<Vec<u8>, String> {
    let vk = B64.decode(vk_b64).map_err(|e| e.to_string())?;
    let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&vk).ok_or("node vk malformed")?;
    let env = ddrm_envelope::PqSealedEnvelope::from_bytes(sealed).map_err(|e| format!("{e:?}"))?;
    Ok(ddrm_envelope::hybrid_unwrap_bound(&b.session_secret, &env, &b.aad, &verifier)
        .map_err(|e| format!("share unwrap failed: {e:?}"))?
        .to_vec())
}

/// `describe <tcp:HOST:PORT>` — connect to a running authority and print its PUBLISHED identity
/// (`verifying_key_b64` + `recipient_pub_b64`) as one JSON line. Used to assemble/verify a
/// descriptor; reads only public material (the pre-channel `init` identity reply).
fn describe(endpoint: &str) -> Result<(), String> {
    let mut node = NodeSocket::connect(endpoint)?;
    let init = ok_data(&node.call(&json!({ "op": "init", "config": {} }))?, "init (describe)")?;
    let vk = init["seal_verifying_key_b64"].as_str().ok_or("node published no vk")?;
    let recipient = init["seal_recipient_pub_b64"].as_str().ok_or("node published no recipient")?;
    println!(
        "{}",
        json!({
            "authority_endpoint": endpoint,
            "verifying_key_b64": vk,
            "recipient_pub_b64": recipient,
        })
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("describe") {
        let endpoint = args.get(2).ok_or("usage: dkms-live-recover describe <tcp:HOST:PORT>")?;
        return describe(endpoint);
    }
    let desc_path = args.get(1).cloned().unwrap_or_else(|| "dkms-authority.v2.json".to_string());
    let seed_path = args.get(2).cloned().unwrap_or_else(|| "caller.seed".to_string());

    println!("==============================================================");
    println!(" LIVE 2-of-3 dKMS recover — against the REAL production quorum");
    println!("==============================================================");

    let desc: Value = serde_json::from_slice(
        &std::fs::read(&desc_path).map_err(|e| format!("read descriptor {desc_path}: {e}"))?,
    )
    .map_err(|e| format!("parse descriptor: {e}"))?;
    let pins = load_pins(&desc)?;
    for (i, p) in pins.iter().enumerate() {
        println!("  {} (x={}) -> {}", p.label, i + 1, p.endpoint);
    }

    // Our ALLOW-LISTED caller identity (the seed the operator provisioned into every node's
    // allow-list). base64 of 32 raw bytes.
    let seed_raw = std::fs::read_to_string(&seed_path).map_err(|e| format!("read caller seed {seed_path}: {e}"))?;
    let seed_bytes = B64.decode(seed_raw.trim()).map_err(|e| format!("caller seed is not base64: {e}"))?;
    let caller_seed: [u8; 32] = seed_bytes
        .as_slice()
        .try_into()
        .map_err(|_| format!("caller seed must be 32 bytes, got {}", seed_bytes.len()))?;
    let (caller_signer, _caller_vk) = ddrm_envelope::seal::mldsa_seal_keypair(caller_seed);

    // Producer identity (authenticates the escrow blobs) — ephemeral, the recover carries its vk.
    let (producer_signer, producer_vk) = ddrm_envelope::seal::mldsa_seal_keypair(ddrm_envelope::random_seed());
    let producer_vk_b64 = B64.encode(&producer_vk);

    // Mint a fresh CEK and Shamir-split it 2-of-3 over GF(256): shares = [p(1), p(2), p(3)].
    let cek = ddrm_envelope::random_seed().to_vec();
    let coeff = ddrm_envelope::random_seed().to_vec();
    let shares = ddrm_envelope::split_cek_shamir2(&cek, &coeff)?;
    let kid16 = [0xC5u8; 16];
    let kid_hex: String = kid16.iter().map(|x| format!("{x:02x}")).collect();

    // The decrypt-boundary session the nodes re-seal their shares to, and the ONE transcript AAD.
    let (session_secret, session_public) = ddrm_envelope::mint_session();
    let session_pub_bytes = ddrm_envelope::session_public_bytes(&session_public);
    let session_pub_b64 = B64.encode(&session_pub_bytes);
    let content_hash = [0xABu8; 32];
    let nonce = [0xCDu8; 12];
    let aad = DecryptTranscriptV1 {
        suite_id: SUITE,
        provider_id: "decrypt",
        principal_id: PRINCIPAL,
        session_id: SESSION,
        object_cid: CONTENT,
        content_hash: &content_hash,
        action: RIGHT,
        viewer_interface: "reader",
        output_kind: "page-image",
        expires_at: EXPIRES_AT,
        release_receipt_hash: [0u8; 32],
        decrypt_session_pub: &session_pub_bytes,
        nonce: &nonce,
        node_set_id: None,
    }
    .to_aad();

    let boundary = Boundary {
        producer_signer,
        producer_vk_b64,
        caller_seed,
        caller_signer,
        session_secret,
        session_pub_bytes,
        session_pub_b64,
        kid16,
        kid_hex,
        aad: aad.clone(),
        aad_b64: B64.encode(&aad),
        content_hash_b64: B64.encode(content_hash),
        nonce_b64: B64.encode(nonce),
    };

    // Recover each node's re-sealed share over the live mesh, then open it in-boundary.
    println!("\n-- recovering re-sealed shares from each live authority over dkms0 --");
    let mut opened: Vec<Vec<u8>> = Vec::with_capacity(3);
    for (i, pin) in pins.iter().enumerate() {
        let (sealed, rtt) = recover_share(pin, &shares[i], &boundary)
            .map_err(|e| format!("{} ({}): {e}", pin.label, pin.endpoint))?;
        // The raw CEK must NEVER appear on the wire from a node.
        if sealed == cek {
            return Err(format!("{} returned the raw CEK — sealing is broken", pin.label));
        }
        let share = open_share(&sealed, &pin.vk_b64, &boundary)?;
        if share.as_slice() == cek.as_slice() {
            return Err(format!("{}'s share alone equals the CEK — the split is not secure", pin.label));
        }
        println!("  {} (x={}) recovered + re-sealed share in {} ms", pin.label, i + 1, rtt.as_millis());
        opened.push(share);
    }
    println!("PASS gate 1: every live node escrowed + recovered ONLY its own share (none == the CEK)");

    // ANY TWO of the three reconstruct the CEK — and each pair survives the THIRD node being down.
    println!("\n-- reconstructing the CEK from each 2-of-3 pair (the rail survives a dead node) --");
    let pairs = [(0usize, 1usize), (0, 2), (1, 2)];
    for (a, b_) in pairs {
        let xa = (a + 1) as u8;
        let xb = (b_ + 1) as u8;
        let recon = ddrm_envelope::combine_cek_shamir2(xa, &opened[a], xb, &opened[b_])?;
        if recon.as_slice() != cek.as_slice() {
            return Err(format!("pair ({},{}) did not reconstruct the CEK", pins[a].label, pins[b_].label));
        }
        println!(
            "  {} + {}  (x={},{})  ->  CEK reconstructed ✓",
            pins[a].label, pins[b_].label, xa, xb
        );
    }
    println!("PASS gate 2: any 2-of-3 live authorities reconstruct the CEK — one node down still opens");

    // BELOW QUORUM fails closed: one node is not a quorum (a single share is not the key), and the
    // combine REFUSES a single node's share presented twice (two copies of one share != a quorum).
    if ddrm_envelope::combine_cek_shamir2(1, &opened[0], 1, &opened[0]).is_ok() {
        return Err("the combine accepted a single node's share as a quorum — below-quorum is not fail-closed".to_string());
    }
    println!("PASS gate 3: below quorum fails closed — a single live node cannot reconstruct the CEK");

    // WAN adversarial gates against node A (the farthest authority — real network in the path).
    println!("\n-- network adversarial gates over the WAN (channel integrity on the real path) --");
    adversarial_gates(&pins[0], &shares[0], &boundary)?;
    println!("PASS gate 4+5: tampered + plaintext-downgrade recovers are dropped over the live mesh");

    println!("\n==============================================================");
    println!(" dkms-live-recover: PASS — real 2-of-3 recover over the live mesh");
    println!("==============================================================");
    Ok(())
}

fn main() {
    match run() {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("\ndkms-live-recover: FAIL — {e}");
            std::process::exit(1);
        }
    }
}
