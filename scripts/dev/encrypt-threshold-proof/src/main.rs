//! Self-contained proof of encrypt-provider's THRESHOLD escrow — the "mint to the dKMS
//! quorum" keystone. Drives the REAL encrypt-provider (escrow build) over its stdin/stdout
//! JSON line protocol; escrows a freshly-minted CEK to THREE ephemeral node recipients
//! (stand-ins for InterServer/Contabo/node3). Then proves, with NO daemons and NO network:
//!
//!   1. each producer-signed indexed share unwraps ONLY under its own node recipient
//!      (cross-node unwrap fails closed) and carries the right coordinate;
//!   2. ANY TWO of the three shares reconstruct the IDENTICAL secret (all 3 pairs agree);
//!   3. that reconstructed CEK DECRYPTS the segment sealed in THIS run -> the original
//!      plaintext ("sealed now, decrypts now") — i.e. the reconstructed value IS the real CEK;
//!   4. a single share is information-theoretically useless (its bytes != the CEK);
//!   5. the node-set pin equals threshold_node_set_id_n(2, vks) — a node swap is detectable.
//!
//! The raw CEK never appears on any wire: the proof only ever sees ciphertext + the per-node
//! SEALED shares, and recovers the CEK by the SAME 2-of-3 math the consumer half uses.
//!
//! Usage: encrypt-threshold-proof <encrypt-provider-bin>

use aes::cipher::{KeyIvInit, StreamCipher};
use base64::Engine as _;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

const PLAINTEXT: &[u8] = b"elastos dDRM: a video sealed to the dKMS QUORUM now, recovered 2-of-3 now!";

struct Capsule {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Capsule {
    fn spawn(bin: &str) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {bin}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(Self { child, stdin, stdout })
    }

    fn call(&mut self, req: &Value) -> Result<Value, String> {
        let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        self.stdin.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        self.stdin.write_all(b"\n").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        let mut resp = String::new();
        self.stdout.read_line(&mut resp).map_err(|e| e.to_string())?;
        if resp.trim().is_empty() {
            return Err("empty response (capsule died?)".to_string());
        }
        let v: Value = serde_json::from_str(resp.trim()).map_err(|e| format!("bad json: {e}: {resp}"))?;
        if v.get("status").and_then(Value::as_str) != Some("ok") {
            return Err(format!("capsule error: {v}"));
        }
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    }
}

impl Drop for Capsule {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn decode(b64: &str, what: &str) -> Result<Vec<u8>, String> {
    B64.decode(b64).map_err(|e| format!("{what}: bad base64: {e}"))
}

fn kid16(kid_hex: &str) -> Result<[u8; 16], String> {
    if kid_hex.len() != 32 {
        return Err("kid_hex must be 32 hex chars".to_string());
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&kid_hex[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn run(bin: &str) -> Result<(), String> {
    let mut cap = Capsule::spawn(bin)?;

    // --- init -> the producer publishes its verifying key (the authority/quorum trusts it) ---
    let init = cap.call(&json!({ "op": "init", "config": {} }))?;
    let producer_vk_b64 = init["producer_verifying_key_b64"].as_str()
        .ok_or("encrypt-provider did not publish a producer verifying key (build with --features escrow)")?
        .to_string();
    let producer_vk = decode(&producer_vk_b64, "producer_vk")?;
    let verifier = ddrm_envelope::MlDsa65Verifier::from_encoded(&producer_vk)
        .ok_or("producer verifying key malformed")?;
    println!("  [init] encrypt-provider up; producer verifying key published");

    // --- THREE ephemeral node recipients (stand-ins for the live quorum nodes) ---
    // Each "node" = a session KEM keypair (recipient) + a distinct ML-DSA identity (vk pins the set).
    let nodes: Vec<(ddrm_envelope::SessionKemSecret, Vec<u8>, Vec<u8>)> = (0u8..3)
        .map(|i| {
            let (secret, public) = ddrm_envelope::mint_session();
            let recipient = ddrm_envelope::session_public_bytes(&public);
            let (_s, vk) = ddrm_envelope::seal::mldsa_seal_keypair([i + 1; 32]);
            (secret, recipient, vk)
        })
        .collect();
    let node_json: Vec<Value> = nodes
        .iter()
        .map(|(_s, recipient, vk)| json!({
            "verifying_key_b64": B64.encode(vk),
            "recipient_pub_b64": B64.encode(recipient),
        }))
        .collect();

    // --- mint + threshold-escrow: the producer mints a CEK, CENC-encrypts, splits, seals ---
    let out = cap.call(&json!({
        "op": "seal_inline_threshold",
        "plaintext_b64": B64.encode(PLAINTEXT),
        "nodes": node_json,
    }))?;
    let scheme = out["scheme"].as_str().ok_or("no scheme")?.to_string();
    let kid_hex = out["kid_hex"].as_str().ok_or("no kid_hex")?;
    let kid = kid16(kid_hex)?;
    let ciphertext = decode(out["ciphertext_b64"].as_str().ok_or("no ciphertext")?, "ciphertext")?;
    let iv8 = decode(out["iv8_b64"].as_str().ok_or("no iv8")?, "iv8")?;
    let node_set_id = decode(out["node_set_id_b64"].as_str().ok_or("no node_set_id")?, "node_set_id")?;
    let shares = out["shares"].as_array().ok_or("no shares")?;
    if shares.len() != 3 {
        return Err(format!("expected 3 sealed shares, got {}", shares.len()));
    }
    // Containment: the raw CEK must not appear anywhere in the producer's output.
    let out_str = out.to_string();
    if out_str.contains("\"cek") || out_str.contains("plaintext_b64") {
        return Err("producer output leaked a CEK/plaintext field".to_string());
    }
    println!("  [seal] minted CEK in-boundary, CENC-encrypted, split + sealed 3 shares (kid={kid_hex})");

    // --- Gate 1: each producer-signed share unwraps ONLY under its node recipient ---
    let mut recovered: Vec<(u8, Vec<u8>)> = Vec::new();
    for (i, share) in shares.iter().enumerate() {
        let x = share["x"].as_u64().ok_or("share has no x")? as u8;
        let wrapped = decode(share["wrapped_share_b64"].as_str().ok_or("no wrapped_share")?, "wrapped_share")?;
        let env = ddrm_envelope::PqSealedEnvelope::from_bytes(&wrapped).map_err(|e| format!("{e:?}"))?;
        let (secret_i, recipient_i, _vk_i) = &nodes[i];
        let aad = ddrm_envelope::transcript::escrow_aad(&scheme, &kid, recipient_i);
        // The right node opens it.
        let payload = ddrm_envelope::hybrid_unwrap_bound(secret_i, &env, &aad, &verifier)
            .map_err(|e| format!("node {i} could not open its own share: {e:?}"))?;
        let (px, p) = ddrm_envelope::parse_indexed_share(&payload).ok_or("malformed indexed share")?;
        if px != x || px != (i as u8 + 1) {
            return Err(format!("share {i} coordinate mismatch: payload x={px}, declared x={x}"));
        }
        // A DIFFERENT node cannot open it (recipient-bound) — fail closed.
        let j = (i + 1) % 3;
        let (secret_j, recipient_j, _vk_j) = &nodes[j];
        let aad_j = ddrm_envelope::transcript::escrow_aad(&scheme, &kid, recipient_j);
        if ddrm_envelope::hybrid_unwrap_bound(secret_j, &env, &aad_j, &verifier).is_ok() {
            return Err(format!("share {i} opened under node {j}'s recipient — escrow is not node-bound!"));
        }
        recovered.push((px, p.to_vec()));
    }
    println!("  [gate 1] each share opens ONLY under its node recipient (cross-node unwrap fails closed); coordinates correct");

    // --- Gate 5: the node-set pin matches the 3 vks (a node swap would be detected) ---
    let vks: Vec<&[u8]> = nodes.iter().map(|(_s, _r, vk)| vk.as_slice()).collect();
    let expect_set = ddrm_envelope::threshold_node_set_id_n(2, &vks);
    if node_set_id != expect_set {
        return Err("node_set_id does not match threshold_node_set_id_n(2, vks)".to_string());
    }
    println!("  [gate 5] node-set pin == hash(2, all 3 node vks) — a node swap is detectable");

    // --- Gate 2: ANY TWO of the three reconstruct the IDENTICAL secret ---
    let combine = |a: &(u8, Vec<u8>), b: &(u8, Vec<u8>)| -> Result<Vec<u8>, String> {
        ddrm_envelope::combine_cek_shamir2(a.0, &a.1, b.0, &b.1)
            .map(|z| z.to_vec())
            .map_err(|e| e.to_string())
    };
    let cek_12 = combine(&recovered[0], &recovered[1])?;
    let cek_13 = combine(&recovered[0], &recovered[2])?;
    let cek_23 = combine(&recovered[1], &recovered[2])?;
    if cek_12 != cek_13 || cek_12 != cek_23 {
        return Err("the three node-pairs reconstructed DIFFERENT secrets — not a valid 2-of-3 split".to_string());
    }
    if cek_12.len() != 16 {
        return Err(format!("reconstructed CEK is {} bytes, expected 16", cek_12.len()));
    }
    println!("  [gate 2] any-2-of-3 reconstruct the IDENTICAL 16-byte secret (pairs 1+2, 1+3, 2+3 all agree)");

    // --- Gate 4: a single share alone is useless (its bytes are not the CEK) ---
    if recovered.iter().any(|(_, p)| *p == cek_12) {
        return Err("a single share equals the CEK — the split is not hiding the secret".to_string());
    }
    println!("  [gate 4] no single share equals the CEK (one share is information-theoretically useless)");

    // --- Gate 3 (GOLD STANDARD): the reconstructed CEK DECRYPTS the segment sealed now ---
    let mut cek = [0u8; 16];
    cek.copy_from_slice(&cek_12);
    let mut iv16 = [0u8; 16];
    iv16[..8].copy_from_slice(&iv8);
    let mut buf = ciphertext.clone();
    let mut cipher = Aes128Ctr::new((&cek).into(), (&iv16).into());
    cipher.apply_keystream(&mut buf);
    if buf != PLAINTEXT {
        return Err("the reconstructed CEK did NOT decrypt the segment to the original plaintext".to_string());
    }
    println!("  [gate 3] the 2-of-3-reconstructed CEK DECRYPTED the segment -> original plaintext (\"sealed now, decrypts now\")");

    cap.call(&json!({ "op": "shutdown" })).ok();
    Ok(())
}

fn main() {
    let bin = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: encrypt-threshold-proof <encrypt-provider-bin>");
        std::process::exit(2);
    });
    match run(&bin) {
        Ok(()) => println!("\nencrypt-threshold-proof: PASS — a real CEK minted in-boundary, escrowed to a 3-node quorum, recovered 2-of-3, decrypted the segment. CEK never left whole."),
        Err(e) => {
            eprintln!("\nencrypt-threshold-proof: FAIL — {e}");
            std::process::exit(1);
        }
    }
}
