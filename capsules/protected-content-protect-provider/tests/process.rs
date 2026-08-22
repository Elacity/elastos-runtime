use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{
    CustodyCommitteeAuthorizationIdentityV1, CustodyEpochIdentityV1, CustodyPoolIdentityV1,
    Digest32, NodeCustodyPublicKeyV1, NodePublicKey,
};
use elastos_protected_content_provider_contracts::{
    CencFmp4MediaIdentityV1, ProtectProviderRequestV1, ProtectProviderResponseStatusV1,
    ProtectionSessionNodeV1,
};
use serde_json::json;

struct ProviderProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl ProviderProcess {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_protected-content-protect-provider"))
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

fn init_config() -> serde_json::Value {
    json!({
        "base_path": "",
        "allowed_paths": [],
        "read_only": false,
        "encryption_key": "",
        "extra": {}
    })
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

fn wrap_runtime_request(request: &ProtectProviderRequestV1) -> serde_json::Value {
    let mut value = serde_json::to_value(request).unwrap();
    let op = value["op"].as_str().unwrap().to_string();
    value.as_object_mut().unwrap().insert(
        "_runtime_invocation".to_string(),
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "runtime",
            "target": "protect",
            "op": op,
            "capability": format!("provider:runtime->protect:{op}"),
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

fn typed_ok_response(
    response: serde_json::Value,
) -> elastos_protected_content_provider_contracts::ProtectProviderResponseV1 {
    assert_eq!(response["status"], "ok");
    elastos_protected_content_provider_contracts::ProtectProviderResponseV1::from_json_slice(
        &serde_json::to_vec(response.get("data").unwrap()).unwrap(),
    )
    .unwrap()
}

fn digest(seed: u8) -> Digest32 {
    Digest32::new([seed; 32])
}

fn node_public_key(seed: u8) -> NodePublicKey {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    NodePublicKey::new(signing.verifying_key().to_bytes()).unwrap()
}

fn node_custody_public_key() -> NodeCustodyPublicKeyV1 {
    elastos_protected_content_custody::NodeCustodySecretKeyV1::generate()
        .unwrap()
        .public_key()
        .unwrap()
}

fn nodes() -> Vec<ProtectionSessionNodeV1> {
    vec![
        ProtectionSessionNodeV1::new(node_public_key(1), node_custody_public_key()).unwrap(),
        ProtectionSessionNodeV1::new(node_public_key(2), node_custody_public_key()).unwrap(),
        ProtectionSessionNodeV1::new(node_public_key(3), node_custody_public_key()).unwrap(),
    ]
}

fn make_box(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + content.len());
    out.extend_from_slice(&(u32::try_from(8 + content.len()).unwrap()).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(content);
    out
}

fn make_fullbox(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(4 + payload.len());
    content.push(version);
    content.extend_from_slice(&flags.to_be_bytes()[1..]);
    content.extend_from_slice(payload);
    make_box(kind, &content)
}

fn make_avc1_entry() -> Vec<u8> {
    let mut payload = vec![0u8; 78];
    payload[24..26].copy_from_slice(&1920u16.to_be_bytes());
    payload[26..28].copy_from_slice(&1080u16.to_be_bytes());
    payload[40..42].copy_from_slice(&1u16.to_be_bytes());
    make_box(b"avc1", &payload)
}

fn clear_init_segment() -> Vec<u8> {
    let ftyp = make_box(b"ftyp", b"isom\0\0\0\0isomiso6");
    let stsd = {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&make_avc1_entry());
        make_fullbox(b"stsd", 0, 0, &payload)
    };
    let stbl = make_box(b"stbl", &stsd);
    let minf = make_box(b"minf", &stbl);
    let hdlr = {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(b"vide");
        payload.extend_from_slice(&[0u8; 12]);
        make_fullbox(b"hdlr", 0, 0, &payload)
    };
    let mdia = {
        let mut content = Vec::new();
        content.extend_from_slice(&hdlr);
        content.extend_from_slice(&minf);
        make_box(b"mdia", &content)
    };
    let trak = {
        let tkhd = {
            let mut payload = vec![0u8; 76];
            payload[8..12].copy_from_slice(&1u32.to_be_bytes());
            make_fullbox(b"tkhd", 0, 0x000007, &payload)
        };
        let mut content = Vec::new();
        content.extend_from_slice(&tkhd);
        content.extend_from_slice(&mdia);
        make_box(b"trak", &content)
    };
    let mvex = {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&1u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        let trex = make_fullbox(b"trex", 0, 0, &payload);
        make_box(b"mvex", &trex)
    };
    let moov = {
        let mut content = Vec::new();
        content.extend_from_slice(&trak);
        content.extend_from_slice(&mvex);
        make_box(b"moov", &content)
    };
    [ftyp, moov].concat()
}

fn clear_segment(track_id: u32, payload: &[u8]) -> Vec<u8> {
    const TFHD_FLAGS_PRODUCER_V1: u32 = 0x020038;
    const TRUN_FLAG_DATA_OFFSET: u32 = 0x000001;
    const TRUN_FLAG_SAMPLE_SIZE: u32 = 0x000200;

    let mfhd = make_fullbox(b"mfhd", 0, 0, &1u32.to_be_bytes());
    let tfhd = {
        let mut payload_bytes = Vec::new();
        payload_bytes.extend_from_slice(&track_id.to_be_bytes());
        payload_bytes.extend_from_slice(&1u32.to_be_bytes());
        payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
        payload_bytes.extend_from_slice(&0u32.to_be_bytes());
        make_fullbox(b"tfhd", 0, TFHD_FLAGS_PRODUCER_V1, &payload_bytes)
    };
    let tfdt = make_fullbox(b"tfdt", 0, 0, &1u32.to_be_bytes());
    let trun = {
        let mut payload_bytes = Vec::new();
        payload_bytes.extend_from_slice(&1u32.to_be_bytes());
        payload_bytes.extend_from_slice(&0i32.to_be_bytes());
        payload_bytes.extend_from_slice(&(u32::try_from(payload.len()).unwrap()).to_be_bytes());
        make_fullbox(
            b"trun",
            0,
            TRUN_FLAG_DATA_OFFSET | TRUN_FLAG_SAMPLE_SIZE,
            &payload_bytes,
        )
    };
    let traf = {
        let mut content = Vec::new();
        content.extend_from_slice(&tfhd);
        content.extend_from_slice(&tfdt);
        content.extend_from_slice(&trun);
        make_box(b"traf", &content)
    };
    let mut moof = {
        let mut content = Vec::new();
        content.extend_from_slice(&mfhd);
        content.extend_from_slice(&traf);
        make_box(b"moof", &content)
    };
    let data_offset_at = moof.len() - trun.len() + 16;
    let sample_offset = (moof.len() + 8) as i32;
    moof[data_offset_at..data_offset_at + 4].copy_from_slice(&sample_offset.to_be_bytes());
    [moof, make_box(b"mdat", payload)].concat()
}

fn open_request(clear_init: &[u8], segment_count: u32) -> ProtectProviderRequestV1 {
    ProtectProviderRequestV1::new_open_protection_session(
        digest(0x31),
        CustodyPoolIdentityV1::new(digest(0x41), 32).unwrap(),
        CustodyEpochIdentityV1::new(digest(0x42), 32).unwrap(),
        CustodyCommitteeAuthorizationIdentityV1::new(digest(0x43), 32).unwrap(),
        "video/mp4",
        "avc1.640028",
        segment_count,
        clear_init,
        nodes(),
    )
    .unwrap()
}

#[test]
fn process_accepts_init_status_shutdown_and_clean_eof() {
    let mut process = ProviderProcess::start();

    let status_before = process.request_json(json!({"op": "status"}));
    assert_eq!(status_before["status"], "ok");
    assert_eq!(status_before["data"]["configured"], false);

    let init = process.request_json(json!({"op": "init", "config": init_config()}));
    assert_eq!(init["status"], "ok");
    assert_eq!(init["data"]["configured"], true);

    let status_after = process.request_json(json!({"op": "status"}));
    assert_eq!(status_after["status"], "ok");
    assert_eq!(status_after["data"]["configured"], true);

    process.shutdown_and_assert_clean();

    let process = ProviderProcess::start();
    process.close_stdin_and_assert_clean();
}

#[test]
fn process_rejects_malformed_and_oversized_frames_and_stays_alive() {
    let mut process = ProviderProcess::start();

    let malformed = process.request_raw_line(br#"{"op":"status""#);
    assert_eq!(malformed["status"], "error");
    assert_eq!(malformed["code"], "invalid_request");

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

    let init = process.request_json(json!({"op": "init", "config": init_config()}));
    assert_eq!(init["status"], "ok");

    process.shutdown_and_assert_clean();
}

#[test]
fn process_open_segment_finalize_success_path_is_framed_and_bound() {
    let clear_init = clear_init_segment();
    let clear_segments = [clear_segment(1, b"hello"), clear_segment(1, b"world!")];
    let request = open_request(&clear_init, u32::try_from(clear_segments.len()).unwrap());

    let mut process = ProviderProcess::start();
    let init = process.request_json(json!({"op": "init", "config": init_config()}));
    assert_eq!(init["status"], "ok");

    let opened = typed_ok_response(process.request_json(wrap_runtime_request(&request)));
    assert_eq!(
        opened.status(),
        ProtectProviderResponseStatusV1::ProtectionSessionOpened,
        "open failure_code={:?}",
        opened.failure_code()
    );
    let handle = opened.protection_session_handle().unwrap().unwrap();
    let protected_init = opened.protected_init_segment().unwrap().to_vec();

    let mut protected_segments = Vec::new();
    for (index, clear_segment) in clear_segments.iter().enumerate() {
        let response = typed_ok_response(
            process.request_json(wrap_runtime_request(
                &ProtectProviderRequestV1::new_protect_media_segment(
                    handle,
                    u32::try_from(index).unwrap(),
                    clear_segment,
                )
                .unwrap(),
            )),
        );
        assert_eq!(
            response.status(),
            ProtectProviderResponseStatusV1::MediaSegmentProtected
        );
        protected_segments.push(response.protected_segment().unwrap().to_vec());
    }

    let finalized = typed_ok_response(process.request_json(wrap_runtime_request(
        &ProtectProviderRequestV1::new_finalize_protection_session(handle).unwrap(),
    )));
    assert_eq!(
        finalized.status(),
        ProtectProviderResponseStatusV1::ProtectionSessionFinalized
    );
    let media_identity = finalized.media_identity().unwrap().unwrap();
    let expected = CencFmp4MediaIdentityV1::new_from_bytes(
        &protected_init,
        &protected_segments,
        "video/mp4",
        "avc1.640028",
    )
    .unwrap();
    assert_eq!(media_identity, expected);
    let envelope = finalized.custody_envelope().unwrap().unwrap();
    assert_eq!(
        envelope.manifest().encrypted_content(),
        media_identity.encrypted_content()
    );
    assert_eq!(envelope.manifest().threshold().required(), 2);
    assert_eq!(envelope.manifest().threshold().total(), 3);

    process.shutdown_and_assert_clean();
}
