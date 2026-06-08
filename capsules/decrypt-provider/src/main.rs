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
}
