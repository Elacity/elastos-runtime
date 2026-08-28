use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{RuntimeReleaseAuditIdV1, TerminalReceiptIssuerKey};
use elastos_protected_content_provider_contracts::{
    DecryptProviderRequestV1, DecryptProviderResponseStatusV1, DecryptProviderResponseV1,
    ViewerMediaPartSelectorV1,
};
use protected_content_decrypt_provider::PROTECTED_CONTENT_DECRYPT_PROVIDER_TARGET;
use serde_json::json;

mod support;

struct ProviderProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl ProviderProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_protected-content-decrypt-provider"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self {
            stdin: Some(child.stdin.take().unwrap()),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            stderr: child.stderr.take(),
            child,
        }
    }

    fn request_json(&mut self, value: serde_json::Value) -> serde_json::Value {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &value).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
        self.read_response()
    }

    fn request_raw_line(&mut self, line: &[u8]) -> serde_json::Value {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        self.read_response()
    }

    fn read_response(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn shutdown_and_assert_clean(mut self) {
        let response = self.request_json(json!({"op": "shutdown"}));
        assert_eq!(response["status"], "ok");
        let status = self.child.wait().unwrap();
        assert!(status.success());
        self.assert_empty_stderr();
    }

    fn close_stdin_and_assert_clean(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().unwrap();
        assert!(status.success());
        self.assert_empty_stderr();
    }

    fn assert_empty_stderr(&mut self) {
        let mut stderr = String::new();
        self.stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    }
}

fn init_config(runtime_seed: u8) -> serde_json::Value {
    let key = SigningKey::from_bytes(&[runtime_seed; 32]);
    json!({
        "base_path": "",
        "allowed_paths": [],
        "read_only": false,
        "encryption_key": "",
        "extra": {
            "trusted_runtime_issuer": format!("0x{}", hex::encode(key.verifying_key().to_bytes()))
        }
    })
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn local_json_runtime_invocation_abi() -> serde_json::Value {
    json!({
        "schema": "elastos.provider.transfer-abi/v1",
        "transfer": "json",
        "transport": "runtime-local-provider-plane",
        "range_supported": false,
        "progress_supported": false,
        "progress_mode": "none",
        "transport_native_stream": false,
        "backpressure": "not_applicable",
        "cancel_supported": false
    })
}

fn wrap_runtime_request(request: &DecryptProviderRequestV1) -> serde_json::Value {
    let mut value = serde_json::to_value(request).unwrap();
    let op = value
        .get("op")
        .and_then(serde_json::Value::as_str)
        .unwrap()
        .to_string();
    value.as_object_mut().unwrap().insert(
        "_runtime_invocation".to_string(),
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "runtime",
            "target": PROTECTED_CONTENT_DECRYPT_PROVIDER_TARGET,
            "op": op,
            "capability": format!(
                "provider:runtime->{PROTECTED_CONTENT_DECRYPT_PROVIDER_TARGET}:{op}"
            ),
            "transport": "runtime-local-provider-plane",
            "carrier": null,
            "transfer": "json",
            "range": null,
            "progress": null,
            "abi": local_json_runtime_invocation_abi()
        }),
    );
    value
}

fn typed_ok_response(response: serde_json::Value) -> DecryptProviderResponseV1 {
    assert_eq!(response["status"], "ok");
    DecryptProviderResponseV1::from_json_slice(
        &serde_json::to_vec(response.get("data").unwrap()).unwrap(),
    )
    .unwrap()
}

#[test]
fn process_accepts_init_status_and_shutdown_without_stderr() {
    let mut process = ProviderProcess::start();

    let status_before = process.request_json(json!({"op": "status"}));
    assert_eq!(status_before["status"], "ok");
    assert_eq!(status_before["data"]["configured"], false);

    let init = process.request_json(json!({
        "op": "init",
        "config": init_config(0x42)
    }));
    assert_eq!(init["status"], "ok");
    assert_eq!(init["data"]["configured"], true);

    let status_after = process.request_json(json!({"op": "status"}));
    assert_eq!(status_after["status"], "ok");
    assert_eq!(status_after["data"]["configured"], true);

    process.shutdown_and_assert_clean();
}

#[test]
fn process_rejects_malformed_and_oversized_frames_and_stays_alive() {
    let mut process = ProviderProcess::start();

    let malformed = process.request_raw_line(br#"{"op":"status""#);
    assert_eq!(malformed["status"], "error");
    assert_eq!(malformed["code"], "invalid_request");

    let invalid_utf8 = process.request_raw_line(&[0xff, 0xfe, 0xfd]);
    assert_eq!(invalid_utf8["status"], "error");
    assert_eq!(invalid_utf8["code"], "invalid_request");

    let trailing = process.request_raw_line(br#"{"op":"status"}[]"#);
    assert_eq!(trailing["status"], "error");
    assert_eq!(trailing["code"], "invalid_request");

    let oversized = process.request_raw_line(&vec![
        b'a';
        elastos_protected_content_provider_contracts::MAX_PROVIDER_FRAME_BYTES_V1
            + 1
    ]);
    assert_eq!(oversized["status"], "error");
    assert_eq!(oversized["code"], "invalid_request");

    let strict_control = process.request_json(json!({"op": "status", "extra": 1}));
    assert_eq!(strict_control["status"], "error");
    assert_eq!(strict_control["code"], "invalid_request");

    let status = process.request_json(json!({"op": "status"}));
    assert_eq!(status["status"], "ok");
    assert_eq!(status["data"]["configured"], false);

    process.shutdown_and_assert_clean();
}

#[test]
fn process_exits_cleanly_on_eof_without_stderr() {
    let process = ProviderProcess::start();
    process.close_stdin_and_assert_clean();
}

#[test]
fn process_prepare_open_read_close_replay_and_restart_absence_flow() {
    let runtime_seed = 0x42;
    let media_seed = 0x21;
    let terminal_issuer_seed = 0x61;
    let base_time = now_unix_seconds();

    let envelope = support::custody_envelope_for_media(media_seed, base_time);
    let binding = support::binding_for_envelope(&envelope);
    let media_identity = support::media_identity(media_seed);
    let (protected_init_segment, encrypted_segments, _, _) = support::media_components(media_seed);
    let original_init = protected_init_segment.clone();
    let original_segment = encrypted_segments[1].clone();

    let prepare_audit = RuntimeReleaseAuditIdV1::new(support::digest(0xa1)).unwrap();
    let prepare_request = DecryptProviderRequestV1::new_prepare_recipient(
        &binding,
        prepare_audit,
        elastos_protected_content_contracts::RightsActionV1::View,
        support::runtime_issuer(runtime_seed),
        support::issued_at(base_time),
        base_time + 30,
    )
    .unwrap();

    let mut process = ProviderProcess::start();
    let init = process.request_json(json!({
        "op": "init",
        "config": init_config(runtime_seed)
    }));
    assert_eq!(init["status"], "ok");

    let prepare_a = typed_ok_response(process.request_json(wrap_runtime_request(&prepare_request)));
    let prepare_b = typed_ok_response(process.request_json(wrap_runtime_request(&prepare_request)));
    assert_eq!(prepare_a, prepare_b);
    assert_eq!(
        prepare_a.status(),
        DecryptProviderResponseStatusV1::PreparedRecipient
    );

    let open_audit = RuntimeReleaseAuditIdV1::new(support::digest(0xa2)).unwrap();
    let operation = support::make_signed_runtime_release_operation(
        runtime_seed,
        open_audit,
        &envelope,
        prepare_a.recipient_public_key().unwrap(),
        prepare_a.recipient_identity().unwrap(),
        base_time,
    );
    let contributions = vec![
        support::make_signed_node_contribution(&operation, &envelope, runtime_seed, 1, base_time),
        support::make_signed_node_contribution(&operation, &envelope, runtime_seed, 2, base_time),
    ];
    let terminal_receipt = support::make_signed_terminal_receipt(
        &operation,
        &contributions,
        terminal_issuer_seed,
        base_time,
    );
    let open_request = DecryptProviderRequestV1::new_open_viewer_session(
        *prepare_a.prepared_recipient_handle().unwrap(),
        &operation,
        TerminalReceiptIssuerKey::new(
            SigningKey::from_bytes(&[terminal_issuer_seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap(),
        &envelope,
        &media_identity,
        &protected_init_segment,
        &contributions,
        &terminal_receipt,
    )
    .unwrap();

    let opened_a = typed_ok_response(process.request_json(wrap_runtime_request(&open_request)));
    let opened_b = typed_ok_response(process.request_json(wrap_runtime_request(&open_request)));
    assert_eq!(opened_a, opened_b);
    assert_eq!(
        opened_a.status(),
        DecryptProviderResponseStatusV1::ViewerSessionOpened
    );
    assert_eq!(
        opened_a.viewer_session_handle().unwrap(),
        prepare_a.prepared_recipient_handle().unwrap()
    );

    let read_init_request = DecryptProviderRequestV1::new_read_viewer_media_part(
        open_audit,
        *opened_a.viewer_session_handle().unwrap(),
        ViewerMediaPartSelectorV1::init(),
    )
    .unwrap();
    let clear_init =
        typed_ok_response(process.request_json(wrap_runtime_request(&read_init_request)));
    assert_eq!(
        clear_init.status(),
        DecryptProviderResponseStatusV1::ViewerMediaPart
    );
    assert_eq!(
        clear_init.viewer_media_part_selector().unwrap(),
        &ViewerMediaPartSelectorV1::init()
    );
    assert!(clear_init
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"avc1"));
    assert!(clear_init
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"mp4a"));
    assert!(!clear_init
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"sinf"));

    let read_segment_selector =
        ViewerMediaPartSelectorV1::segment(1, encrypted_segments[1].clone()).unwrap();
    let read_segment_request = DecryptProviderRequestV1::new_read_viewer_media_part(
        open_audit,
        *opened_a.viewer_session_handle().unwrap(),
        read_segment_selector.clone(),
    )
    .unwrap();
    let clear_segment =
        typed_ok_response(process.request_json(wrap_runtime_request(&read_segment_request)));
    assert_eq!(
        clear_segment.status(),
        DecryptProviderResponseStatusV1::ViewerMediaPart
    );
    assert_eq!(
        clear_segment.viewer_media_part_selector().unwrap(),
        &read_segment_selector
    );
    assert!(clear_segment
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"moof"));
    assert!(clear_segment
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"mdat"));
    assert!(!clear_segment
        .clear_media_part()
        .unwrap()
        .windows(4)
        .any(|w| w == b"senc"));
    assert_eq!(protected_init_segment, original_init);
    assert_eq!(encrypted_segments[1], original_segment);

    let close_request = DecryptProviderRequestV1::new_close_viewer_session(
        open_audit,
        *opened_a.viewer_session_handle().unwrap(),
    )
    .unwrap();
    let closed_a = typed_ok_response(process.request_json(wrap_runtime_request(&close_request)));
    let closed_b = typed_ok_response(process.request_json(wrap_runtime_request(&close_request)));
    assert_eq!(closed_a, closed_b);
    assert_eq!(
        closed_a.status(),
        DecryptProviderResponseStatusV1::ClosedViewerSession
    );

    let old_handle = *opened_a.viewer_session_handle().unwrap();
    process.shutdown_and_assert_clean();

    let mut restarted = ProviderProcess::start();
    let init = restarted.request_json(json!({
        "op": "init",
        "config": init_config(runtime_seed)
    }));
    assert_eq!(init["status"], "ok");

    let absent_after_restart =
        typed_ok_response(restarted.request_json(wrap_runtime_request(&close_request)));
    assert_eq!(
        absent_after_restart.status(),
        DecryptProviderResponseStatusV1::ViewerSessionAlreadyAbsent
    );
    assert_eq!(
        absent_after_restart.viewer_session_handle().unwrap(),
        &old_handle
    );

    restarted.shutdown_and_assert_clean();
}
