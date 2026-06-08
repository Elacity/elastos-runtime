//! ElastOS Decrypt Provider Capsule
//!
//! Fail-closed protected-content decrypt/render boundary. App capsules never
//! receive raw CEKs, broad plaintext authority, filesystem authority,
//! key-backend SDK objects, KMS credentials, chain RPC, wallet RPC, or provider credentials
//! through this provider.

use elastos_common::protected_content::{
    DecryptSessionRequestV1, ReleaseReceiptV1, DECRYPT_SESSION_REQUEST_SCHEMA,
    DECRYPT_SESSION_SCHEMA, PROTECTED_CONTENT_ACTIONS, PROTECTED_CONTENT_OUTPUTS,
    RELEASE_RECEIPT_SCHEMA,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

// CENC/AES-128-CTR decrypt engine vendored from PC2 `cenc-decrypt`. Held here as a
// provider-internal backend; wired into open_session/render behind the fail-closed
// contract in a later step (see docs/convergence/CONVERGENCE_PLAYBOOK.md §6).
#[allow(dead_code)]
mod cenc;
mod envelope;
// PQ-hybrid CEK-seal de-risking island (feature `pq-envelope`): the post-quantum
// analogue of `envelope.rs`, proving x25519+ml-kem-768 -> AEAD unwrap recovers a
// CEK in `Zeroizing`. Not wired into dispatch; see DDRM_DECRYPT_RAIL.md §PQ.
#[cfg(feature = "pq-envelope")]
mod pq_envelope;
// Portable golden-vector schema (features `vectors` / `rail-shim`):
// substrate-independent fixtures the engines and the rail shim are replayed
// against. See src/vector_format.rs.
#[cfg(any(feature = "vectors", feature = "rail-shim", feature = "pq-mldsa"))]
mod vector_format;
// Rail transport shim (feature `rail-shim`): adapter from a sealed-CEK carrier to
// the proven unwrap->cenc engines. Tested island, not wired into dispatch.
#[cfg(feature = "rail-shim")]
mod rail_shim;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: Value,
    },
    Status,
    OpenSession {
        request: Box<DecryptSessionRequestV1>,
    },
    Render {
        request: Box<DecryptSessionRequestV1>,
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
struct DecryptProvider;

impl DecryptProvider {
    fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Init { config } => self.init(config),
            Request::Status => self.status(),
            Request::OpenSession { request } => self.open_session(*request),
            Request::Render { request } => self.render(*request),
            Request::Shutdown => Response::empty_ok(),
        }
    }

    fn init(&mut self, _config: Value) -> Response {
        Response::ok(json!({
            "provider": "decrypt",
            "protocol_version": "1.0",
            "configured": false,
            "supported_operations": ["status", "open_session", "render"],
        }))
    }

    fn status(&self) -> Response {
        Response::ok(json!({
            "provider": "decrypt",
            "version": PROVIDER_VERSION,
            "configured": false,
            "supported_operations": ["status", "open_session", "render"],
            "supported_outputs": PROTECTED_CONTENT_OUTPUTS,
            "blocked_authority": [
                "raw_cek",
                "raw_plaintext",
                "filesystem",
                "key_backend_sdk",
                "kms_node_credentials",
                "chain_rpc",
                "wallet_rpc",
                "provider_credentials"
            ],
            "next_required_providers": [
                "key-provider"
            ],
        }))
    }

    fn open_session(&self, request: DecryptSessionRequestV1) -> Response {
        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "decrypt sessions require a configured key release and decrypt/render backend",
        )
    }

    fn render(&self, request: DecryptSessionRequestV1) -> Response {
        if let Err(err) = validate_decrypt_session_request(&request) {
            return Response::error("invalid_request", err);
        }
        Response::error(
            "not_configured",
            "rendering requires a configured key release and decrypt/render backend",
        )
    }
}

/// Decrypt a protected-content segment using session material (the decrypt-step core).
///
/// Branch-by-Abstraction seam (see `docs/convergence/DDRM_DECRYPT_RAIL.md`): this is the
/// decrypt-step backend for the Hybrid rail, where the decrypt boundary *receives* its
/// material rather than reaching out for it. It is intentionally not yet reachable from
/// `open_session`/`render` — the CEK + ciphertext transport rail is an open architecture
/// decision. It is exercised directly by tests to prove the engine is correct at the
/// provider boundary and that the CEK never escapes this function.
///
/// The vendored cenc engine owns the CEK lifetime: it decodes `cek_b64`, uses it, and
/// zeroizes it on every return path. The returned plaintext is consumed only by the
/// scoped output sink inside the isolation boundary; it is never placed in a
/// caller-visible `Response`.
#[allow(dead_code)]
fn decrypt_session_segment(
    cek_b64: &str,
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    let command = json!({ "cek_b64": cek_b64, "iv_size": 8 }).to_string();
    let (result_json, output) = cenc::process(&command, ciphertext_segment, init_segment);
    let meta: Value = serde_json::from_str(&result_json).map_err(|err| err.to_string())?;
    if meta.get("success").and_then(Value::as_bool) != Some(true) {
        let message = meta
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("decrypt failed");
        return Err(message.to_string());
    }
    let plaintext = output.ok_or_else(|| "decrypt produced no output".to_string())?;
    Ok((plaintext, meta))
}

/// Rail-landing composition (PREP — gated behind the `rail-prep` feature; not yet
/// wired into `open_session`/`render`).
///
/// This joins the chain's two tested islands into the single in-boundary operation
/// the Hybrid decrypt rail will invoke once Anders confirms the CEK-transport rail
/// (`docs/convergence/DDRM_DECRYPT_RAIL.md`): the upstream CEK-sealing envelope
/// unwrap (`envelope::{parse, ecdh_unwrap, extract_cek}`) immediately followed by
/// the decrypt-step core (`decrypt_session_segment`). It mirrors PC2
/// `ddrm-decrypt::session::unwrap_envelope` (recover CEK) → cenc segment decrypt,
/// so the CEK:
///   - materializes only after a correct ECDH unwrap against the session secret key;
///   - is held in `Zeroizing` storage for its whole (short) lifetime;
///   - is consumed by the cenc engine inside this boundary and zeroized there;
///   - never appears in the scoped, caller-facing response (see `scoped_session_response`).
///
/// Keeping it behind a feature flag means the default build and the 25-test default
/// suite are unchanged (Parallel Change): the live wiring becomes a one-step swap
/// into dispatch once the rail and session-key provisioning land.
#[cfg(feature = "rail-prep")]
#[allow(dead_code)]
fn decrypt_sealed_segment(
    session_secret_key: &p256::SecretKey,
    sealed_envelope: &[u8],
    ciphertext_segment: &[u8],
    init_segment: Option<&[u8]>,
) -> Result<(Vec<u8>, Value), String> {
    use base64::Engine as _;
    use zeroize::Zeroizing;

    let parsed = envelope::parse(sealed_envelope).map_err(|err| format!("{err:?}"))?;
    let plaintext =
        envelope::ecdh_unwrap(session_secret_key, &parsed).map_err(|err| format!("{err:?}"))?;
    let cek = envelope::extract_cek(&plaintext).map_err(|err| format!("{err:?}"))?;

    // Bridge the recovered CEK into the cenc engine's command surface. The base64
    // form is held in `Zeroizing` so it is scrubbed from linear memory on drop,
    // keeping the CEK contained across this internal hand-off.
    let cek_b64 = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(cek.as_slice()));
    decrypt_session_segment(&cek_b64, ciphertext_segment, init_segment)
}

/// Build the scoped, containment-safe decrypt-session response for the caller.
///
/// Carries session and output metadata only. The raw CEK and the decrypted plaintext
/// never cross this boundary to the caller (app/viewer capsule).
#[allow(dead_code)]
fn scoped_session_response(request: &DecryptSessionRequestV1, decrypt_meta: &Value) -> Response {
    Response::ok(json!({
        "schema": DECRYPT_SESSION_SCHEMA,
        "session_id": request.session_id,
        "object_cid": request.object_cid,
        "viewer_interface": request.viewer_interface,
        "output_kind": request.output_kind,
        "is_protected": decrypt_meta.get("is_protected"),
        "sample_count": decrypt_meta.get("sample_count"),
        "expires_at": request.expires_at,
    }))
}

fn validate_decrypt_session_request(request: &DecryptSessionRequestV1) -> Result<(), String> {
    if request.schema != DECRYPT_SESSION_REQUEST_SCHEMA {
        return Err("decrypt session request schema is unsupported".to_string());
    }
    require_non_empty(&request.request_id, "request_id")?;
    require_non_empty(&request.principal_id, "principal_id")?;
    require_non_empty(&request.session_id, "session_id")?;
    require_identifier(&request.object_cid, "object_cid")?;
    validate_action(&request.action)?;
    require_non_empty(&request.viewer_interface, "viewer_interface")?;
    validate_release_receipt(&request.release_receipt)?;
    validate_output_kind(&request.output_kind)?;
    require_non_empty(&request.reason, "reason")?;
    if request.expires_at == 0 {
        return Err("expires_at is required".to_string());
    }
    Ok(())
}

fn validate_release_receipt(receipt: &ReleaseReceiptV1) -> Result<(), String> {
    if receipt.schema != RELEASE_RECEIPT_SCHEMA {
        return Err("release receipt schema is unsupported".to_string());
    }
    require_non_empty(&receipt.request_id, "release_receipt.request_id")?;
    if receipt.status != "released" {
        return Err("release receipt status must be released".to_string());
    }
    Ok(())
}

fn validate_action(action: &str) -> Result<(), String> {
    if PROTECTED_CONTENT_ACTIONS.contains(&action) {
        Ok(())
    } else {
        Err(format!("unsupported protected-content action: {action}"))
    }
}

fn validate_output_kind(output_kind: &str) -> Result<(), String> {
    if PROTECTED_CONTENT_OUTPUTS.contains(&output_kind) {
        Ok(())
    } else {
        Err(format!(
            "unsupported protected-content output: {output_kind}"
        ))
    }
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
        || value == "."
        || value == ".."
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
        "decrypt-provider: starting v{} (protected content decrypt/render)",
        PROVIDER_VERSION
    );

    let mut provider = DecryptProvider;
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("decrypt-provider read error: {}", err);
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

    eprintln!("decrypt-provider exiting");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decrypt_request() -> DecryptSessionRequestV1 {
        DecryptSessionRequestV1 {
            schema: DECRYPT_SESSION_REQUEST_SCHEMA.to_string(),
            request_id: "decrypt:test".to_string(),
            principal_id: "person:local:test".to_string(),
            session_id: "session:test".to_string(),
            object_cid: "bafybeigprotectedcontent".to_string(),
            action: "view".to_string(),
            viewer_interface: "elastos.viewer/document@1".to_string(),
            release_receipt: ReleaseReceiptV1 {
                schema: RELEASE_RECEIPT_SCHEMA.to_string(),
                request_id: "key-release:test".to_string(),
                object_cid: "bafybeigprotectedcontent".to_string(),
                principal_id: "person:local:test".to_string(),
                session_id: "session:test".to_string(),
                action: "view".to_string(),
                provider: "key-provider".to_string(),
                status: "released".to_string(),
                issued_at: 1_800_000_000,
                expires_at: 1_900_000_000,
            },
            output_kind: "rendered".to_string(),
            reason: "open protected document".to_string(),
            expires_at: 1_900_000_000,
        }
    }

    fn error_code(response: Response) -> String {
        match response {
            Response::Error { code, .. } => code,
            other => panic!("expected error, got {other:?}"),
        }
    }

    fn ok_data(response: Response) -> Value {
        match response {
            Response::Ok { data: Some(data) } => data,
            other => panic!("expected ok data, got {other:?}"),
        }
    }

    #[test]
    fn status_advertises_blocked_raw_authority() {
        let provider = DecryptProvider;
        let data = ok_data(provider.status());

        assert_eq!(data["provider"], "decrypt");
        assert_eq!(data["configured"], false);
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_cek")));
        assert!(data["blocked_authority"]
            .as_array()
            .unwrap()
            .contains(&json!("raw_plaintext")));
    }

    #[test]
    fn open_session_fails_closed_until_backend_exists() {
        let provider = DecryptProvider;
        assert_eq!(
            error_code(provider.open_session(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn render_fails_closed_until_backend_exists() {
        let provider = DecryptProvider;
        assert_eq!(
            error_code(provider.render(decrypt_request())),
            "not_configured"
        );
    }

    #[test]
    fn open_session_rejects_unsupported_output_kind() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.output_kind = "raw_plaintext".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_path_like_object_ids() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.object_cid = "../secret".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    #[test]
    fn open_session_rejects_dot_segment_object_ids() {
        let provider = DecryptProvider;
        let mut request = decrypt_request();
        request.object_cid = "..".to_string();

        assert_eq!(
            error_code(provider.open_session(request)),
            "invalid_request"
        );
    }

    // --- decrypt-step core seam (Branch-by-Abstraction; see DDRM_DECRYPT_RAIL.md) ---

    use aes::cipher::{KeyIvInit, StreamCipher};
    use base64::Engine;

    type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;

    fn make_box(box_type: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let size = (8 + content.len()) as u32;
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(box_type);
        b.extend_from_slice(content);
        b
    }

    /// Minimal single-sample encrypted fMP4 segment: moof{traf{trun,senc}} + mdat{ciphertext}.
    fn build_encrypted_segment(plaintext: &[u8], cek: &[u8; 16], iv8: &[u8; 8]) -> Vec<u8> {
        let mut iv16 = [0u8; 16];
        iv16[..8].copy_from_slice(iv8);
        let mut ciphertext = plaintext.to_vec();
        let mut cipher = Aes128Ctr::new(cek.into(), (&iv16).into());
        cipher.apply_keystream(&mut ciphertext);

        let mut trun_content = vec![0u8, 0x00, 0x02, 0x00, 0, 0, 0, 1];
        trun_content.extend_from_slice(&(plaintext.len() as u32).to_be_bytes());
        let trun = make_box(b"trun", &trun_content);

        let mut senc_content = vec![0u8, 0, 0, 0, 0, 0, 0, 1];
        senc_content.extend_from_slice(iv8);
        let senc = make_box(b"senc", &senc_content);

        let mut traf_content = trun;
        traf_content.extend_from_slice(&senc);
        let traf = make_box(b"traf", &traf_content);
        let moof = make_box(b"moof", &traf);
        let mdat = make_box(b"mdat", &ciphertext);

        let mut segment = moof;
        segment.extend_from_slice(&mdat);
        segment
    }

    #[test]
    fn decrypt_session_segment_recovers_plaintext() {
        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();

        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_off = moof_len + 8;
        assert_eq!(&output[mdat_off..mdat_off + plaintext.len()], plaintext);
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));
    }

    /// Cross-invariant round-trip: replay the golden PRODUCED BY encrypt-provider's
    /// real in-boundary engine (mint CEK+KID -> CENC encrypt -> mux) and prove THIS
    /// provider decrypts it back to the producer's original bytes, with the CEK
    /// staying off the scoped boundary. Pins #1 (produce) ↔ #2 (consume) on one
    /// artifact. Regenerate the fixture with:
    ///   (cd ../encrypt-provider && cargo test --features gen-vectors emit_roundtrip_vector)
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_encrypt_to_decrypt.json"
        )))
        .unwrap();

        // The producer surfaced a 16-byte KID; the CEK never appears in the KID.
        assert_eq!(v.kid_hex.len(), 32, "producer KID is 16 bytes (32 hex)");

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "decrypt-provider must recover the exact bytes encrypt-provider sealed"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        // Containment at the consumer edge: the scoped response leaks neither the
        // (rail stand-in) CEK nor the recovered plaintext.
        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    /// Replay the producer's MULTI-SAMPLE round-trip golden (real playback shape):
    /// encrypt-provider's real engine sealed 4 samples with per-sample IVs; this
    /// provider must recover the exact concatenated plaintext, report N samples,
    /// and leak neither CEK nor plaintext across the scoped boundary.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_multisample_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_multisample_encrypt_to_decrypt.json"
        )))
        .unwrap();

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "decrypt-provider must recover every sample encrypt-provider sealed"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert!(
            meta["sample_count"].as_u64().unwrap() >= 2,
            "the golden is a multi-sample segment"
        );

        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    /// Replay the producer's SUBSAMPLE round-trip golden (clear leader + encrypted
    /// body): the real engine left a 16-byte codec header in the clear and
    /// encrypted the remainder; this provider must reconstruct the full sample
    /// (clear bytes untouched, body decrypted) back to the producer's plaintext.
    #[cfg(all(feature = "vectors", not(feature = "gen-vectors")))]
    #[test]
    fn encrypt_to_decrypt_subsample_round_trip_golden() {
        let b64 = base64::engine::general_purpose::STANDARD;
        let v: crate::vector_format::RoundTripVector = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/vectors/roundtrip_subsample_encrypt_to_decrypt.json"
        )))
        .unwrap();

        let cek_b64 = v.cek_b64.clone();
        let segment = b64.decode(&v.encrypted_segment_b64).unwrap();
        let expected = b64.decode(&v.expected_plaintext_b64).unwrap();

        let (output, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let mdat_off = segment.len() - expected.len();
        // The clear leader survives untouched and the body decrypts: the whole
        // sample equals the producer's original plaintext.
        assert_eq!(
            &output[mdat_off..],
            expected.as_slice(),
            "subsample reconstruction must equal the producer's plaintext"
        );
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains(&cek_b64), "CEK must not cross the boundary");
        assert!(
            !serialized.contains(std::str::from_utf8(&expected).unwrap()),
            "plaintext must not cross the boundary"
        );
    }

    #[test]
    fn decrypt_session_segment_fails_closed_on_bad_cek() {
        let short_cek = base64::engine::general_purpose::STANDARD.encode([0u8; 8]);
        assert!(decrypt_session_segment(&short_cek, &[], None).is_err());
    }

    #[test]
    fn scoped_session_response_leaks_neither_cek_nor_plaintext() {
        let plaintext = b"the quick brown fox jumps over!!";
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (_plaintext_bytes, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let response = scoped_session_response(&decrypt_request(), &meta);
        let serialized = serde_json::to_string(&response).unwrap();

        assert!(
            !serialized.contains(&cek_b64),
            "CEK must never cross the provider boundary to the caller"
        );
        let plaintext_str = std::str::from_utf8(plaintext).unwrap();
        assert!(
            !serialized.contains(plaintext_str),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
    }

    // --- decrypt -> player consumer contract (Day 13) -----------------------
    //
    // The chain's downstream boundary. Both viewer capsules consume scoped
    // output ONLY; neither ever receives the CEK. Pins PC2's contract where the
    // media player gets decrypted fMP4 segments and the non-media player gets
    // render_only plaintext — in both cases addressed by an opaque session, with
    // key material confined to this provider (Irzhy invariant #2 at the edge).

    /// A media-player session (video/audio): streamed segments.
    fn media_decrypt_request() -> DecryptSessionRequestV1 {
        let mut request = decrypt_request();
        request.action = "stream".to_string();
        request.viewer_interface = "elastos.viewer/media@1".to_string();
        request.output_kind = "stream".to_string();
        request.release_receipt.action = "stream".to_string();
        request.reason = "open protected media stream".to_string();
        request
    }

    /// Field names that, if they ever appeared in a scoped response, would mean
    /// key material or raw content escaped the provider boundary.
    const FORBIDDEN_SCOPED_KEYS: &[&str] = &[
        "cek",
        "cek_b64",
        "iv",
        "iv_b64",
        "key",
        "keys",
        "plaintext",
        "decrypted",
        "secret",
        "private_key",
        "rendered_bytes",
        "output",
    ];

    /// Keys the scoped response is allowed to carry — metadata only.
    const ALLOWED_SCOPED_KEYS: &[&str] = &[
        "schema",
        "session_id",
        "object_cid",
        "viewer_interface",
        "output_kind",
        "is_protected",
        "sample_count",
        "expires_at",
    ];

    fn assert_scoped_response_is_metadata_only(request: &DecryptSessionRequestV1) {
        // A representative decrypt meta as produced by the cenc engine.
        let meta = json!({ "is_protected": true, "sample_count": 1 });
        let data = ok_data(scoped_session_response(request, &meta));
        let obj = data.as_object().expect("scoped response must be an object");

        for key in obj.keys() {
            assert!(
                ALLOWED_SCOPED_KEYS.contains(&key.as_str()),
                "scoped response carried an unexpected key `{key}` for {}",
                request.viewer_interface
            );
            assert!(
                !FORBIDDEN_SCOPED_KEYS.contains(&key.as_str()),
                "scoped response leaked forbidden key `{key}` for {}",
                request.viewer_interface
            );
        }

        // The player references the session by opaque id, never by key material.
        assert_eq!(data["session_id"], json!(request.session_id));
    }

    #[test]
    fn media_player_scoped_response_is_metadata_only() {
        assert_scoped_response_is_metadata_only(&media_decrypt_request());
    }

    #[test]
    fn non_media_player_scoped_response_is_metadata_only() {
        assert_scoped_response_is_metadata_only(&decrypt_request());
    }

    /// Media-player variant of the containment check: a real decrypted segment
    /// must not let the CEK or plaintext reach the scoped (player-facing) output.
    #[test]
    fn media_segment_decrypt_keeps_cek_and_plaintext_off_the_player_boundary() {
        let plaintext = b"the quick brown fox jumps over!!";
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);

        let (_segment_bytes, meta) = decrypt_session_segment(&cek_b64, &segment, None).unwrap();
        let serialized =
            serde_json::to_string(&scoped_session_response(&media_decrypt_request(), &meta)).unwrap();

        assert!(!serialized.contains(&cek_b64), "CEK must not reach the media player");
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted media must not reach the player as plaintext in the scoped response"
        );
    }

    // --- rail-landing composition (PREP, feature = "rail-prep") ----------------
    //
    // Proves the end-to-end in-boundary flow the Hybrid decrypt rail will invoke
    // once Anders confirms the CEK-transport rail: a session-sealed CEK envelope +
    // an encrypted media segment go in; scoped metadata comes out; the CEK and the
    // decrypted bytes never cross the provider boundary. Gated behind the feature so
    // the default suite stays at 25; run with:  cargo test --features rail-prep

    /// Seal a CEK to `session_pk` exactly as the upstream sealer (Lit/key-provider)
    /// would — independently constructing the wire format so the round-trip pins the
    /// rail contract end to end. Mirrors `envelope.rs`'s sealer.
    #[cfg(feature = "rail-prep")]
    fn seal_cek_envelope(session_pk: &p256::PublicKey, cek: &[u8], version: u8) -> Vec<u8> {
        use aes::Aes256;
        use cbc::Encryptor as CbcEncryptor;
        use cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        use elliptic_curve::sec1::ToEncodedPoint;
        use p256::ecdh::EphemeralSecret;
        use rand_core::OsRng;
        type Aes256CbcEnc = CbcEncryptor<Aes256>;

        let eph = EphemeralSecret::random(&mut OsRng);
        let eph_point = eph.public_key().to_encoded_point(true);
        let eph_bytes = eph_point.as_bytes();
        let shared = eph.diffie_hellman(session_pk);
        let key_bytes = shared.raw_secret_bytes();

        let mut inner = Vec::new();
        inner.extend_from_slice(&0u32.to_be_bytes()); // metaSize
        inner.extend_from_slice(&1u32.to_be_bytes()); // keyCount
        inner.extend_from_slice(cek);

        let iv: [u8; 16] = if version == 0x03 {
            let mut iv = [0u8; 16];
            getrandom::getrandom(&mut iv).unwrap();
            iv
        } else {
            let mut iv = [0u8; 16];
            iv.copy_from_slice(&eph_bytes[..16]);
            iv
        };

        let cipher = Aes256CbcEnc::new(key_bytes.as_slice().into(), (&iv).into());
        let mut buf = vec![0u8; inner.len() + 16];
        buf[..inner.len()].copy_from_slice(&inner);
        let ct_len = cipher
            .encrypt_padded_mut::<Pkcs7>(&mut buf, inner.len())
            .unwrap()
            .len();
        let ciphertext = &buf[..ct_len];

        let mut env = Vec::new();
        env.extend_from_slice(&[0, 0, 0, version]);
        env.extend_from_slice(&(eph_bytes.len() as u16).to_be_bytes());
        env.extend_from_slice(eph_bytes);
        if version == 0x03 {
            env.extend_from_slice(&iv);
        }
        env.extend_from_slice(&0u16.to_be_bytes()); // empty signature
        env.extend_from_slice(&[0u8; 33]); // signer pubkey (skipped)
        env.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        env.extend_from_slice(ciphertext);
        env
    }

    #[cfg(feature = "rail-prep")]
    #[test]
    fn sealed_segment_decrypts_end_to_end_and_keeps_cek_off_the_boundary() {
        use p256::SecretKey;
        use rand_core::OsRng;

        let plaintext = b"the quick brown fox jumps over!!"; // 32 bytes
        let cek = [0x11u8; 16];
        let iv8 = [0x22u8; 8];

        let session_sk = SecretKey::random(&mut OsRng);
        let sealed = seal_cek_envelope(&session_sk.public_key(), &cek, 0x03);
        let segment = build_encrypted_segment(plaintext, &cek, &iv8);

        // The whole rail step: sealed CEK envelope + encrypted segment -> plaintext
        // recovered inside the boundary, CEK recovered only via ECDH unwrap.
        let (output, meta) = decrypt_sealed_segment(&session_sk, &sealed, &segment, None).unwrap();

        let moof_len = segment.len() - (8 + plaintext.len());
        let mdat_off = moof_len + 8;
        assert_eq!(&output[mdat_off..mdat_off + plaintext.len()], plaintext);
        assert_eq!(meta["is_protected"], json!(true));
        assert_eq!(meta["sample_count"], json!(1));

        // Containment: neither the CEK nor the plaintext reaches the scoped response,
        // and the sealed envelope never carried the raw CEK in cleartext.
        let cek_b64 = base64::engine::general_purpose::STANDARD.encode(cek);
        let serialized =
            serde_json::to_string(&scoped_session_response(&decrypt_request(), &meta)).unwrap();
        assert!(
            !serialized.contains(&cek_b64),
            "CEK must never cross the provider boundary to the caller"
        );
        assert!(
            !serialized.contains(std::str::from_utf8(plaintext).unwrap()),
            "decrypted plaintext must never cross the provider boundary to the caller"
        );
        assert!(
            !sealed.windows(cek.len()).any(|w| w == cek),
            "sealed envelope must not contain the raw CEK"
        );
    }

    #[cfg(feature = "rail-prep")]
    #[test]
    fn sealed_segment_fails_closed_on_wrong_session_key() {
        use p256::SecretKey;
        use rand_core::OsRng;

        let cek = [0x11u8; 16];
        let session_sk = SecretKey::random(&mut OsRng);
        let wrong_sk = SecretKey::random(&mut OsRng);
        let sealed = seal_cek_envelope(&session_sk.public_key(), &cek, 0x03);
        let segment = build_encrypted_segment(b"the quick brown fox jumps over!!", &cek, &[0x22u8; 8]);

        // A wrong session key cannot unwrap the envelope -> the whole step fails
        // closed before any segment decryption is attempted.
        assert!(decrypt_sealed_segment(&wrong_sk, &sealed, &segment, None).is_err());
    }
}
