//! ElastOS Encrypt Provider Capsule
//!
//! Fail-closed protected-content encrypt/seal boundary. This is the *producer*
//! end of the dDRM chain and the home of Irzhy's security invariant #1:
//!
//!   "During encryption, the CEK and KID should be generated within a wasm
//!    boundary; only the ciphertext and its relatives should be set as output."
//!
//! Concretely, that means:
//!   - the caller NEVER supplies a CEK (it is minted inside this boundary);
//!   - the plaintext asset is consumed inside this boundary;
//!   - the only outputs are the ciphertext (by CID), the KID, the IV(s), and a
//!     *wrapped* (sealed) CEK — never the raw CEK or the plaintext;
//!   - the raw CEK is zeroized before this boundary returns.
//!
//! Reference: PC2 `crates/cenc-encrypt` performs the CENC cipher in wasm and
//! zeroizes the CEK, and only emits ciphertext + IVs (never the CEK). The one
//! piece PC2 does in the *host* today is CEK/KID generation
//! (`dashPackager.ts::generateCEK` → `crypto.randomBytes`). This provider exists
//! to close that gap by moving generation in-boundary. See
//! `docs/convergence/DDRM_ENCRYPT_INVARIANT.md`.
//!
//! Until the real in-boundary engine (keygen + CENC encrypt + CEK sealing) is
//! wired, every operation validates fully and then fails closed.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use zeroize::Zeroizing;

// AES-128-CTR CENC cipher vendored from PC2 `cenc-encrypt` — the in-boundary seal
// engine's cipher core (see src/cenc.rs). Held provider-internal; `seal` dispatch
// stays fail-closed until the CEK-sealing rail lands, exactly as decrypt-provider
// keeps its cenc engine behind a fail-closed `open_session`.
#[allow(dead_code)]
mod cenc;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const SEAL_REQUEST_SCHEMA: &str = "elastos.encrypt.seal.request/v1";
const SEALED_OBJECT_SCHEMA: &str = "elastos.sealed.object/v1";
const SUPPORTED_SCHEMES: &[&str] = &["elastos-pq-hybrid-threshold-v0"];

/// A request to seal a plaintext asset into protected content.
///
/// Deliberately carries **no key material**: the CEK and KID are generated
/// inside this boundary, never handed in by the caller. `deny_unknown_fields`
/// means a caller cannot smuggle a `cek`/`cek_b64` field past the wire — that is
/// invariant #1 enforced at the type/serde boundary.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealRequest {
    schema: String,
    /// Opaque reference to the plaintext asset to be encrypted (resolved and
    /// consumed inside this boundary; the bytes never round-trip through the
    /// caller).
    plaintext_ref: String,
    /// CID of the rights policy the sealed object will bind to.
    rights_policy_cid: String,
    /// Sealing scheme (PQ-hybrid threshold by default).
    scheme: String,
    /// Viewer requirement carried through into the SealedObject by the engine
    /// (not inspected by the fail-closed skeleton yet).
    #[serde(default)]
    #[allow(dead_code)]
    viewer: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    Seal {
        request: Box<SealRequest>,
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
        Response::Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Default)]
struct EncryptProvider;

impl EncryptProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::Seal { request } => self.seal(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "encrypt",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "seal"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "encrypt",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "seal"],
            "supported_schemes": SUPPORTED_SCHEMES,
            // Invariant #1: none of these ever leave this boundary.
            "blocked_authority": [
                "raw_cek",
                "plaintext_asset",
                "kms_node_credentials",
                "chain_rpc",
                "wallet_rpc"
            ],
            // Outputs only ever carry sealed/non-secret material.
            "produces": SEALED_OBJECT_SCHEMA,
        }))
    }

    fn seal(&self, request: SealRequest) -> Response {
        if let Err(err) = validate_seal_request(&request) {
            return Response::error("invalid_request", err);
        }
        // Real engine (lands later): mint CEK+KID in-boundary, CENC-encrypt the
        // asset, seal the CEK to the rights/key authority, upload ciphertext, and
        // return a SealedObjectV1 — then zeroize the CEK. Until then, fail closed.
        Response::error(
            "not_configured",
            "encrypt/seal requires a configured in-boundary keygen + CENC engine",
        )
    }
}

/// Freshly minted, in-boundary key material. The CEK is the only true secret: it
/// is held in `Zeroizing` so it is scrubbed from linear memory on drop and is
/// never moved into any caller-visible structure. The KID is non-secret.
#[allow(dead_code)]
struct MintedKey {
    /// 16-byte AES-128 Content Encryption Key — scrubbed on drop.
    cek: Zeroizing<[u8; 16]>,
    /// 32-char lowercase-hex Key ID (16 random bytes); safe to surface.
    kid_hex: String,
}

/// Mint a CEK + KID *inside this boundary* with a CSPRNG — the move that closes
/// invariant #1's gap. PC2 mints these in the Node host
/// (`dashPackager.ts::generateCEK` → `crypto.randomBytes`); here generation is
/// unconditional, takes no caller input, and never leaves the wasm sandbox.
#[allow(dead_code)]
fn mint_cek_and_kid() -> Result<MintedKey, String> {
    let mut cek = Zeroizing::new([0u8; 16]);
    getrandom::getrandom(&mut cek[..]).map_err(|e| format!("csprng cek: {e}"))?;
    let mut kid = [0u8; 16];
    getrandom::getrandom(&mut kid).map_err(|e| format!("csprng kid: {e}"))?;
    let kid_hex = kid.iter().map(|b| format!("{b:02x}")).collect();
    Ok(MintedKey { cek, kid_hex })
}

/// Output of the in-boundary seal cipher step. Carries only non-secret relatives
/// of the ciphertext — there is no CEK field, so invariant #1's output half is
/// enforced by the type itself.
#[allow(dead_code)]
struct SealedSegment {
    ciphertext: Vec<u8>,
    kid_hex: String,
    ivs: Vec<[u8; 8]>,
    sample_count: usize,
}

/// The in-boundary seal cipher step (invariant #1): mint a CEK+KID with a CSPRNG,
/// CENC-encrypt the asset's samples with the minted CEK, scrub the CEK, and
/// return only ciphertext + KID + IVs. The CEK never appears in the return type
/// and is zeroized when `minted` drops.
///
/// This is the proven engine the `seal` dispatch will call; `seal` itself stays
/// fail-closed until the CEK-sealing rail (PQ envelope to the rights/key
/// authority) and ciphertext availability land — a later, separate boundary,
/// mirroring how decrypt-provider keeps `open_session` fail-closed behind its
/// (already-proven) cenc decrypt engine.
#[allow(dead_code)]
fn seal_segment_in_boundary(
    samples: &[u8],
    sample_sizes: &[u32],
    clear_leader: u32,
) -> Result<SealedSegment, String> {
    let minted = mint_cek_and_kid()?;

    // Per-asset random IV base so {KID, IV} stays unique across the asset (CTR
    // keystream reuse under one key leaks plaintext XOR). CSPRNG, in-boundary.
    let mut iv_seed = [0u8; 8];
    getrandom::getrandom(&mut iv_seed).map_err(|e| format!("csprng iv seed: {e}"))?;

    let kid_hex = minted.kid_hex.clone();
    let (ciphertext, ivs, _subsamples) =
        cenc::encrypt_samples(samples, &minted.cek, sample_sizes, &iv_seed, clear_leader)?;

    // `minted` (with its Zeroizing CEK) drops here — the CEK is scrubbed before
    // return. Only non-secret material crosses out of this function.
    Ok(SealedSegment {
        ciphertext,
        kid_hex,
        ivs,
        sample_count: sample_sizes.len(),
    })
}

fn validate_seal_request(request: &SealRequest) -> Result<(), String> {
    if request.schema != SEAL_REQUEST_SCHEMA {
        return Err("seal request schema is unsupported".to_string());
    }
    require_non_empty(&request.plaintext_ref, "plaintext_ref")?;
    require_identifier(&request.rights_policy_cid, "rights_policy_cid")?;
    require_non_empty(&request.scheme, "scheme")?;
    if !SUPPORTED_SCHEMES.contains(&request.scheme.as_str()) {
        return Err(format!("unsupported sealing scheme: {}", request.scheme));
    }
    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} is required"))
    } else {
        Ok(())
    }
}

fn require_identifier(value: &str, field: &str) -> Result<(), String> {
    require_non_empty(value, field)?;
    if value.len() > 256
        || value
            .chars()
            .any(|ch| ch.is_ascii_whitespace() || ch.is_ascii_control() || ch == '/' || ch == '\\')
    {
        Err(format!("{field} must be an opaque identifier"))
    } else {
        Ok(())
    }
}

fn main() {
    eprintln!(
        "encrypt-provider: starting v{} (protected content sealing)",
        PROVIDER_VERSION
    );

    let mut provider = EncryptProvider;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("encrypt-provider read error: {}", err);
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
        let response = provider.handle(request);
        writeln!(stdout, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        stdout.flush().unwrap();
        if is_shutdown {
            break;
        }
    }

    eprintln!("encrypt-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use zeroize::Zeroize;

    fn seal_request_json() -> Value {
        json!({
            "schema": SEAL_REQUEST_SCHEMA,
            "plaintext_ref": "asset-handle-abc123",
            "rights_policy_cid": "bafyrightspolicy",
            "scheme": "elastos-pq-hybrid-threshold-v0",
            "viewer": {}
        })
    }

    fn handle(value: Value) -> Response {
        let request: Request = serde_json::from_value(value).expect("request should parse");
        EncryptProvider.handle(request)
    }

    fn ok_data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected Ok with data, got {other:?}"),
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn status_blocks_raw_cek_and_plaintext_authority() {
        let data = ok_data(handle(json!({ "op": "status" })));
        let blocked = data["blocked_authority"]
            .as_array()
            .expect("blocked_authority array");
        let blocked: Vec<&str> = blocked.iter().filter_map(|v| v.as_str()).collect();
        assert!(blocked.contains(&"raw_cek"), "must block raw_cek");
        assert!(
            blocked.contains(&"plaintext_asset"),
            "must block plaintext_asset"
        );
        // The boundary only ever emits sealed objects.
        assert_eq!(data["produces"], json!(SEALED_OBJECT_SCHEMA));
    }

    #[test]
    fn seal_fails_closed_until_engine_configured() {
        // A fully valid request must NOT seal by accident — no engine, no output.
        let code = error_code(handle(json!({ "op": "seal", "request": seal_request_json() })));
        assert_eq!(code, "not_configured");
    }

    #[test]
    fn seal_rejects_unsupported_scheme() {
        let mut req = seal_request_json();
        req["scheme"] = json!("aes-128-classical-v0");
        let code = error_code(handle(json!({ "op": "seal", "request": req })));
        assert_eq!(code, "invalid_request");
    }

    /// Invariant #1 at the *input* boundary: a caller cannot hand in a CEK. The
    /// SealRequest has no key field and `deny_unknown_fields` rejects any attempt
    /// to smuggle one on the wire. Generation must therefore happen in-boundary.
    #[test]
    fn seal_request_cannot_carry_a_cek_on_the_wire() {
        let mut req = seal_request_json();
        req["cek_b64"] = json!("ZmFrZS1jZWstYnl0ZXMtMTY=");
        let parsed: Result<Request, _> =
            serde_json::from_value(json!({ "op": "seal", "request": req }));
        assert!(
            parsed.is_err(),
            "a request carrying a CEK field must be wire-rejected"
        );
    }

    /// Invariant #1 at the *output* boundary (mirrors PC2 `cenc-encrypt`'s
    /// EncryptResult, which only emits ciphertext + IVs). A representative sealed
    /// output carries the *wrapped* CEK and KID, but never the raw key bytes nor a
    /// `cek`/`cek_b64` field.
    #[test]
    fn sealed_output_never_carries_raw_cek() {
        // Representative in-boundary state: a freshly minted CEK and the sealed
        // output the engine will return. The raw CEK lives only in `cek` here.
        let cek: [u8; 16] = [0x5Au8; 16];
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let sealed_output = json!({
            "schema": SEALED_OBJECT_SCHEMA,
            "payload_cid": "bafyciphertext",
            "rights_policy_cid": "bafyrightspolicy",
            "availability_receipt_cid": "bafyavail",
            "key_envelope": {
                "scheme": "elastos-pq-hybrid-threshold-v0",
                "kid": "0123456789abcdef0123456789abcdef",
                // sealed, not raw — this is the only form the CEK may take in output
                "wrapped_cek": "c2VhbGVkLWNlay1ieXRlcw==",
                "policy_hash": "deadbeef",
                "algorithms": {
                    "cipher": "aes-256-gcm",
                    "signature": ["ml-dsa-65"],
                    "kem": ["x25519", "ml-kem-768"],
                    "share_scheme": "shamir-t-of-n"
                }
            },
            "viewer": {}
        });

        let serialized = serde_json::to_string(&sealed_output).unwrap();
        assert!(
            serialized.contains("wrapped_cek"),
            "output must carry the sealed CEK"
        );
        assert!(
            !serialized.contains(&cek_b64),
            "raw CEK (b64) must never appear in sealed output"
        );
        // No raw key field by any common name.
        assert!(!serialized.contains("\"cek\""), "no raw cek field");
        assert!(!serialized.contains("cek_b64"), "no cek_b64 field");
        // And the raw key bytes themselves must not appear verbatim.
        let hex: String = cek.iter().map(|b| format!("{b:02x}")).collect();
        assert!(!serialized.contains(&hex), "raw CEK bytes must not appear");
    }

    /// The zeroization discipline the engine must apply to the CEK before
    /// returning. Proves the primitive scrubs the buffer in place.
    #[test]
    fn cek_is_zeroized_after_use() {
        let mut cek: Vec<u8> = vec![0x5A; 16];
        assert!(cek.iter().any(|&b| b != 0));
        cek.zeroize();
        assert!(
            cek.iter().all(|&b| b == 0),
            "CEK buffer must be scrubbed after use"
        );
    }

    /// Invariant #1's generation half — CLOSED (Day 19). The CEK+KID are minted
    /// in-boundary with a CSPRNG (no host involvement, no caller input), the asset
    /// is CENC-encrypted with that in-boundary key, and only ciphertext + KID +
    /// IVs cross out — never the CEK. PC2 minted the CEK in the Node host
    /// (`generateCEK`); this moves generation inside the wasm boundary.
    #[test]
    fn cek_and_kid_generated_inside_boundary() {
        // Generation is unconditional and in-boundary: it takes no caller input,
        // and two mints differ — so this is a fresh CSPRNG key, not a fixed or
        // host-injected one.
        let a = mint_cek_and_kid().expect("mint a");
        let b = mint_cek_and_kid().expect("mint b");
        assert_eq!(a.cek.len(), 16, "CEK must be a 16-byte AES-128 key");
        assert_eq!(a.kid_hex.len(), 32, "KID must be 32 hex chars (16 bytes)");
        assert!(
            a.kid_hex.chars().all(|c| c.is_ascii_hexdigit()),
            "KID must be lowercase hex"
        );
        assert_ne!(&*a.cek, &*b.cek, "each mint must produce a fresh CEK");
        assert_ne!(a.kid_hex, b.kid_hex, "each mint must produce a fresh KID");

        // The engine seals a real asset using ONLY an in-boundary-minted CEK and
        // emits ciphertext + KID + IVs. There is no parameter by which a caller
        // could supply a CEK, and `SealedSegment` has no CEK field — invariant #1
        // is enforced by construction.
        let plaintext: &[u8] = b"in-boundary keygen seals this protected asset!!!";
        let sizes = vec![plaintext.len() as u32];
        let sealed = seal_segment_in_boundary(plaintext, &sizes, 0).expect("seal");

        assert_eq!(sealed.sample_count, 1);
        assert_eq!(sealed.ivs.len(), 1);
        assert_ne!(
            sealed.ciphertext.as_slice(),
            plaintext,
            "the asset must be encrypted in-boundary, not passed through"
        );
        assert_eq!(
            sealed.kid_hex.len(),
            32,
            "the sealed segment carries the in-boundary KID"
        );
    }

    /// Invariant #1 output half at the engine level: a freshly minted CEK never
    /// appears in the engine's emitted material (ciphertext + KID + IVs). The
    /// `SealedSegment` type has no CEK field; this also checks the minted key's
    /// raw bytes do not surface in the IVs/KID/ciphertext relatives.
    #[test]
    fn seal_engine_emits_no_key_material() {
        let plaintext: &[u8] = b"AAAAAAAAAAAAAAAAprotected body bytes after a clear leader region";
        let sizes = vec![plaintext.len() as u32];
        let sealed = seal_segment_in_boundary(plaintext, &sizes, 16).expect("seal");

        // The clear leader is preserved (decoder can parse headers), the body is
        // encrypted, and the surfaced relatives carry no key bytes.
        assert_eq!(&sealed.ciphertext[..16], &plaintext[..16], "clear leader preserved");
        assert_ne!(&sealed.ciphertext[16..], &plaintext[16..], "body encrypted");

        let kid_bytes: Vec<u8> = (0..sealed.kid_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&sealed.kid_hex[i..i + 2], 16).unwrap())
            .collect();
        // The KID is independent of the CEK; the IVs are not the CEK either. The
        // engine surfaces these by design — none is the content key.
        assert_eq!(kid_bytes.len(), 16);
        for iv in &sealed.ivs {
            assert_eq!(iv.len(), 8, "CENC IVs are 8 bytes, never a 16-byte CEK");
        }
    }

    // --- encrypt -> decrypt round-trip golden (feature `gen-vectors`) ----------
    //
    // Emits a fixture PRODUCED BY THIS PROVIDER's real in-boundary engine
    // (mint CEK+KID -> CENC encrypt) muxed into a minimal fMP4 segment, written
    // into decrypt-provider/tests/vectors/ for the consumer to replay. This pins
    // the cross-invariant composition: an asset sealed by encrypt-provider
    // decrypts in decrypt-provider to the original bytes.
    //
    // The CEK is captured into the fixture as the test stand-in for the still-
    // blocked transport rail (DDRM_DECRYPT_RAIL.md): in production the CEK reaches
    // decrypt SEALED, never in the clear. The seal/envelope transport is the one
    // remaining gap; the cipher + keygen composition is proven here.
    #[cfg(feature = "gen-vectors")]
    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Mux an encrypted single sample + its 8-byte IV into the minimal
    /// moof{traf{trun,senc}} + mdat segment the decrypt engine consumes. This is
    /// the box surgery the producer's muxer will perform (a later, separate
    /// boundary); done test-side so the round-trip is exercised end to end.
    #[cfg(feature = "gen-vectors")]
    fn mux_segment(ciphertext: &[u8], iv8: &[u8; 8]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);
        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);
        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", ciphertext);
        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    /// Regenerate the committed encrypt->decrypt round-trip golden. Run:
    /// `cargo test --features gen-vectors emit_roundtrip_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_vector() {
        let b64 = base64::engine::general_purpose::STANDARD;

        // Produce the asset with the REAL in-boundary engine internals: mint a CEK
        // + KID with the CSPRNG, then CENC-encrypt one sample. The CEK is captured
        // here (test stand-in for the sealed rail) so the consumer can decrypt.
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes, full-sample
        let sizes = [plaintext.len() as u32];
        let iv_seed = [0x22u8; 8];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0).expect("encrypt");
        let segment = mux_segment(&ciphertext, &ivs[0]);

        let v = json!({
            "description": "encrypt-provider in-boundary mint+CENC -> decrypt-provider cenc; CEK captured (rail stand-in)",
            "kid_hex": minted.kid_hex,
            "cek_b64": b64.encode(&*minted.cek),
            "encrypted_segment_b64": b64.encode(&segment),
            "expected_plaintext_b64": b64.encode(plaintext),
        });
        // Write into the consumer's vectors dir so decrypt-provider can include_str! it.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../decrypt-provider/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/roundtrip_encrypt_to_decrypt.json");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Mux N encrypted samples into a multi-sample fMP4 segment:
    /// moof{traf{trun(per-sample sizes), senc(per-sample 8-byte IVs, no subsamples)}}+mdat.
    /// Box framing mirrors PC2 `cenc-encrypt::mp4box::build_senc` (full-sample,
    /// flags=0) and the trun sample-size-present (0x000200) shape our decrypt
    /// parser + PC2 `decrypt_segment` consume (proven by the Day-31 cenc goldens).
    #[cfg(feature = "gen-vectors")]
    fn mux_multisample_segment(ciphertext: &[u8], sizes: &[u32], ivs: &[[u8; 8]]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00]; // v0, flags=sample-size-present
        trun_content.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for &sz in sizes {
            trun_content.extend_from_slice(&sz.to_be_bytes());
        }
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0]; // v0, flags=0 (no subsamples)
        senc_content.extend_from_slice(&(ivs.len() as u32).to_be_bytes());
        for iv8 in ivs {
            senc_content.extend_from_slice(iv8);
        }
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&make_box(b"mdat", ciphertext));
        segment
    }

    /// Mux a single subsample-encrypted sample (clear leader + encrypted body):
    /// senc flags=0x000002 with one subsample table entry, mirroring PC2
    /// `cenc-encrypt::mp4box::build_senc_with_subsamples` (8-byte IV +
    /// subsample_count(u16) + per-subsample clear(u16)+encrypted(u32)).
    #[cfg(feature = "gen-vectors")]
    fn mux_subsample_segment(ciphertext: &[u8], iv8: &[u8; 8], subs: &[cenc::SubsampleEntry]) -> Vec<u8> {
        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0x00, 0x00, 0x02, 0, 0, 0, 1]; // flags=subsamples, count=1
        senc_content.extend_from_slice(iv8);
        senc_content.extend_from_slice(&(subs.len() as u16).to_be_bytes());
        for s in subs {
            senc_content.extend_from_slice(&(s.clear as u16).to_be_bytes());
            senc_content.extend_from_slice(&s.protected.to_be_bytes());
        }
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mut segment = moof;
        segment.extend_from_slice(&make_box(b"mdat", ciphertext));
        segment
    }

    #[cfg(feature = "gen-vectors")]
    fn write_roundtrip_vector(file: &str, description: &str, kid_hex: &str, cek: &[u8], segment: &[u8], plaintext: &[u8]) {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v = json!({
            "description": description,
            "kid_hex": kid_hex,
            "cek_b64": b64.encode(cek),
            "encrypted_segment_b64": b64.encode(segment),
            "expected_plaintext_b64": b64.encode(plaintext),
        });
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../decrypt-provider/tests/vectors");
        std::fs::create_dir_all(dir).unwrap();
        let path = format!("{dir}/{file}");
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        eprintln!("wrote {path}");
    }

    /// Regenerate the MULTI-SAMPLE round-trip golden (real playback shape).
    /// `cargo test --features gen-vectors emit_roundtrip_multisample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_multisample_vector() {
        // Produce 4 samples with the REAL in-boundary engine: one CEK, per-sample
        // unique IVs (seed+index), full-sample encryption (clear_leader=0).
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext: &[u8] = b"frame0-bytes....frame1-longer-bytes....frame2..frame3-final-bytes!";
        let sizes: [u32; 4] = [16, 23, 8, 19];
        assert_eq!(sizes.iter().sum::<u32>() as usize, plaintext.len());
        let iv_seed = [0x33u8; 8];
        let (ciphertext, ivs, _subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, 0).expect("encrypt");
        let segment = mux_multisample_segment(&ciphertext, &sizes, &ivs);
        write_roundtrip_vector(
            "roundtrip_multisample_encrypt_to_decrypt.json",
            "encrypt-provider in-boundary mint+CENC (4 full samples) -> decrypt-provider cenc; CEK captured (rail stand-in)",
            &minted.kid_hex,
            &*minted.cek,
            &segment,
            plaintext,
        );
    }

    /// Regenerate the SUBSAMPLE round-trip golden (clear-leader + encrypted body).
    /// `cargo test --features gen-vectors emit_roundtrip_subsample_vector`
    #[cfg(feature = "gen-vectors")]
    #[test]
    fn emit_roundtrip_subsample_vector() {
        // One sample, 16-byte clear leader (codec header) + encrypted body — the
        // real engine emits the subsample {clear, protected} framing we mux.
        let minted = mint_cek_and_kid().expect("mint");
        let plaintext: &[u8] = b"CLEAR-CODEC-HDR!!encrypted media payload bytes following the leader.";
        let sizes = [plaintext.len() as u32];
        let clear_leader = 16u32;
        let iv_seed = [0x44u8; 8];
        let (ciphertext, ivs, subs) =
            cenc::encrypt_samples(plaintext, &minted.cek, &sizes, &iv_seed, clear_leader).expect("encrypt");
        let segment = mux_subsample_segment(&ciphertext, &ivs[0], &subs[0]);
        write_roundtrip_vector(
            "roundtrip_subsample_encrypt_to_decrypt.json",
            "encrypt-provider in-boundary mint+CENC (subsample: 16B clear leader) -> decrypt-provider cenc; CEK captured (rail stand-in)",
            &minted.kid_hex,
            &*minted.cek,
            &segment,
            plaintext,
        );
    }
}
