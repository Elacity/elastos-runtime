//! ElastOS protected-content custody node provider capsule.
//!
//! This source-only provider may be registered by Runtime as the inactive
//! `custody` route. It does not replace the still-active provisional
//! key-provider product path.

use std::env;
use std::io::{self, BufRead, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use custody_provider::{
    load_state_from_root, parse_runtime_issuer_hex, parse_state_root_from_provider_init,
    provision_state_root, provisioning_receipt, PROVISIONING_SCHEMA_V1,
};
use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{
    CanonicalContract, KeyReleaseError, NodePublicKey, RuntimeOperationIssuerKeyV1,
    SignedRuntimeReleaseOperationV1,
};
use elastos_protected_content_custody::{
    CustodyError, DurableReplayClaimStoreV1, NodeCustodySecretKeyV1, NodeLocalShareStoreV1,
    RecipientPublicKeyV1,
};
use elastos_protected_content_provider_contracts::{
    CustodyProviderRequestOpV1, CustodyProviderRequestValidationErrorV1, CustodyProviderResponseV1,
    ProviderFailureCodeV1, RightsProviderResponseV1, ValidatedCustodyProviderRequestV1,
    ValidatedRightsProviderRequestV1, CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
    CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1, MAX_PROVIDER_FRAME_BYTES_V1,
};
use elastos_protected_content_rights::{
    chain_rights_evidence_request, evaluate_validated_rights_with_evidence_at,
    parse_chain_rights_evidence_data, PrivateCustodyRightsRequestV1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const INIT_ERROR_CODE: &str = "invalid_config";
const REQUEST_ERROR_CODE: &str = "invalid_request";
const BACKEND_ERROR_CODE: &str = "backend_unavailable";
const RIGHTS_DENIED_ERROR_CODE: &str = "rights_denied";

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ControlRequest {
    Init { config: Value },
    Status,
    Shutdown,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ProviderResponse {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Error {
        code: &'static str,
        message: &'static str,
    },
}

impl ProviderResponse {
    fn ok(data: Value) -> Self {
        Self::Ok { data: Some(data) }
    }

    fn empty_ok() -> Self {
        Self::Ok { data: None }
    }

    fn error(code: &'static str, message: &'static str) -> Self {
        Self::Error { code, message }
    }
}

struct CustodyProvider {
    state: Option<ConfiguredCustodyProvider>,
}

struct ConfiguredCustodyProvider {
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    node_public_key: NodePublicKey,
    node_signing_key: SigningKey,
    node_custody_secret: NodeCustodySecretKeyV1,
    share_store: NodeLocalShareStoreV1,
    replay_store: DurableReplayClaimStoreV1,
}

impl CustodyProvider {
    fn new() -> Self {
        Self { state: None }
    }

    fn handle_frame(&mut self, frame: &[u8]) -> (ProviderResponse, bool) {
        let value = match serde_json::from_slice::<Value>(frame) {
            Ok(value) => value,
            Err(_) => return (invalid_request(), false),
        };
        if reject_runtime_envelope_fields(&value).is_err() {
            return (invalid_request(), false);
        }
        let op = value.get("op").and_then(Value::as_str).map(str::to_owned);
        match op.as_deref() {
            Some("init" | "status" | "shutdown") => {
                if !control_request_has_exact_fields(&value, op.as_deref().unwrap_or_default()) {
                    return (invalid_request(), false);
                }
                match serde_json::from_value::<ControlRequest>(value) {
                    Ok(ControlRequest::Init { config }) => (self.init(config), false),
                    Ok(ControlRequest::Status) => (self.status(), false),
                    Ok(ControlRequest::Shutdown) => (ProviderResponse::empty_ok(), true),
                    Err(_) => (invalid_request(), false),
                }
            }
            Some("provision_node_share" | "release_contribution") => {
                let Ok(bytes) = serde_json::to_vec(&value) else {
                    return (invalid_request(), false);
                };
                (self.handle_custody_request(&bytes), false)
            }
            Some("prepare_evidence" | "settle_evidence") => {
                (self.handle_private_rights_request(&value), false)
            }
            _ => (invalid_request(), false),
        }
    }

    #[cfg(test)]
    fn handle_line(&mut self, line: &str) -> ProviderResponse {
        self.handle_frame(line.as_bytes()).0
    }

    fn init(&mut self, config: Value) -> ProviderResponse {
        self.state = None;
        match load_provider_state(config) {
            Ok(state) => {
                self.state = Some(state);
                self.status()
            }
            Err(_) => ProviderResponse::error(
                INIT_ERROR_CODE,
                "custody provider configuration is invalid",
            ),
        }
    }

    fn status(&self) -> ProviderResponse {
        ProviderResponse::ok(json!({
            "provider": "custody",
            "version": PROVIDER_VERSION,
            "configured": self.state.is_some(),
            "supported_operations": ["status", "provision_node_share", "release_contribution", "shutdown"],
            "request_schema": CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
            "response_schema": CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1,
        }))
    }

    fn handle_custody_request(&mut self, bytes: &[u8]) -> ProviderResponse {
        let Some(state) = self.state.as_mut() else {
            return ProviderResponse::error(
                BACKEND_ERROR_CODE,
                "custody provider is not configured",
            );
        };
        let now = now_unix_seconds();
        let request = match ValidatedCustodyProviderRequestV1::decode_and_validate_at(
            bytes,
            state.expected_runtime_issuer,
            state.node_public_key,
            now,
        ) {
            Ok(request) => request,
            Err(CustodyProviderRequestValidationErrorV1::RightsDenied) => return rights_denied(),
            Err(_) => return invalid_request(),
        };
        match request.op() {
            CustodyProviderRequestOpV1::ProvisionNodeShare => {
                let provision = match request.provision_node_share() {
                    Ok(provision) => provision,
                    Err(_) => return invalid_request(),
                };
                match state.share_store.provision_node_share(
                    provision.custody_node_provisioning_record(),
                    provision.signed_runtime_custody_provisioning(),
                    state.expected_runtime_issuer,
                    &state.node_custody_secret,
                    now,
                ) {
                    Ok(_) => typed_response(CustodyProviderResponseV1::new_provisioned(provision)),
                    Err(_) => ProviderResponse::error(
                        BACKEND_ERROR_CODE,
                        "custody provider could not provision the node share",
                    ),
                }
            }
            CustodyProviderRequestOpV1::ReleaseContribution => {
                let release = match request.release_contribution() {
                    Ok(release) => release,
                    Err(_) => return invalid_request(),
                };
                let operation = release.authenticated_runtime_release_operation().clone();
                let key_envelope = operation.binding().key_envelope();
                let node_share = match state.share_store.load_node_share(
                    key_envelope,
                    state.expected_runtime_issuer,
                    &state.node_custody_secret,
                ) {
                    Ok(value) => value,
                    Err(_) => {
                        return typed_response(CustodyProviderResponseV1::new_failure(
                            release,
                            ProviderFailureCodeV1::BackendUnavailable,
                        ));
                    }
                };
                let recipient_public_key = match RecipientPublicKeyV1::new(
                    *operation.statement().recipient_public_key().as_bytes(),
                ) {
                    Ok(value) => value,
                    Err(_) => return invalid_request(),
                };
                match state.replay_store.claim_or_replay_node_contribution(
                    operation,
                    release.signed_node_rights_decision(),
                    node_share.node_share(),
                    &state.node_signing_key,
                    &state.node_custody_secret,
                    &recipient_public_key,
                    now,
                    release
                        .signed_node_rights_decision()
                        .statement()
                        .expires_at(),
                    now,
                ) {
                    Ok(contribution) => {
                        typed_response(CustodyProviderResponseV1::new_contribution(&contribution))
                    }
                    Err(CustodyError::Release(KeyReleaseError::RightsDenied)) => rights_denied(),
                    Err(_) => typed_response(CustodyProviderResponseV1::new_failure(
                        release,
                        ProviderFailureCodeV1::BackendUnavailable,
                    )),
                }
            }
        }
    }

    fn handle_private_rights_request(&mut self, value: &Value) -> ProviderResponse {
        let Some(state) = self.state.as_ref() else {
            return ProviderResponse::error(
                BACKEND_ERROR_CODE,
                "custody provider is not configured",
            );
        };
        let now = now_unix_seconds();
        let frame = match serde_json::to_vec(value)
            .ok()
            .and_then(|bytes| PrivateCustodyRightsRequestV1::from_json_slice(&bytes).ok())
        {
            Some(frame) => frame,
            None => return invalid_request(),
        };
        let request_bytes = match serde_json::to_vec(frame.request_json()) {
            Ok(bytes) => bytes,
            Err(_) => return invalid_request(),
        };
        let request =
            match ValidatedRightsProviderRequestV1::decode_and_validate_at(&request_bytes, now) {
                Ok(request) => request,
                Err(_) => return invalid_request(),
            };
        if request.selected_node_public_key() != state.node_public_key {
            return invalid_request();
        }
        match frame {
            PrivateCustodyRightsRequestV1::PrepareEvidence { .. } => {
                match chain_rights_evidence_request_from_validated_request(frame.request_json()) {
                    Ok(chain_request) => ProviderResponse::ok(chain_request),
                    Err(()) => invalid_request(),
                }
            }
            PrivateCustodyRightsRequestV1::SettleEvidence { .. } => {
                let evidence = match frame
                    .chain_data()
                    .and_then(|data| parse_chain_rights_evidence_data(data).ok())
                {
                    Some(evidence) => evidence,
                    None => return invalid_request(),
                };
                match evaluate_validated_rights_with_evidence_at(
                    &state.node_signing_key,
                    &request,
                    evidence,
                    now,
                ) {
                    Ok(response) => typed_rights_response(response),
                    Err(_) => invalid_request(),
                }
            }
        }
    }
}

enum CliCommand {
    Serve,
    Provision {
        base_path: String,
        trusted_runtime_issuer: String,
    },
}

fn parse_cli_command() -> Result<CliCommand, ()> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Ok(CliCommand::Serve);
    };
    if command != "provision" {
        return Err(());
    }
    let mut base_path = None;
    let mut trusted_runtime_issuer = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--base-path" => base_path = args.next(),
            "--trusted-runtime-issuer" => trusted_runtime_issuer = args.next(),
            _ => return Err(()),
        }
    }
    match (base_path, trusted_runtime_issuer) {
        (Some(base_path), Some(trusted_runtime_issuer)) => Ok(CliCommand::Provision {
            base_path,
            trusted_runtime_issuer,
        }),
        _ => Err(()),
    }
}

fn typed_response(
    result: Result<CustodyProviderResponseV1, impl std::fmt::Debug>,
) -> ProviderResponse {
    let Ok(response) = result else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "custody provider response failed");
    };
    let Ok(bytes) = response.to_json_vec() else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "custody provider response failed");
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => ProviderResponse::ok(value),
        Err(_) => ProviderResponse::error(BACKEND_ERROR_CODE, "custody provider response failed"),
    }
}

fn typed_rights_response(response: RightsProviderResponseV1) -> ProviderResponse {
    let Ok(bytes) = response.to_json_vec() else {
        return ProviderResponse::error(BACKEND_ERROR_CODE, "custody provider response failed");
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => ProviderResponse::ok(value),
        Err(_) => ProviderResponse::error(BACKEND_ERROR_CODE, "custody provider response failed"),
    }
}

fn invalid_request() -> ProviderResponse {
    ProviderResponse::error(REQUEST_ERROR_CODE, "custody provider request is invalid")
}

fn rights_denied() -> ProviderResponse {
    ProviderResponse::error(
        RIGHTS_DENIED_ERROR_CODE,
        "custody provider release was denied",
    )
}

fn control_request_has_exact_fields(value: &Value, op: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match op {
        "init" => object.len() == 2 && object.contains_key("op") && object.contains_key("config"),
        "status" | "shutdown" => object.len() == 1 && object.contains_key("op"),
        _ => false,
    }
}

fn reject_runtime_envelope_fields(value: &Value) -> Result<(), ()> {
    let object = value.as_object().ok_or(())?;
    if object.contains_key("_runtime_invocation") || object.contains_key("_runtime_transfer") {
        return Err(());
    }
    Ok(())
}

fn chain_rights_evidence_request_from_validated_request(request: &Value) -> Result<Value, ()> {
    let object = request.as_object().ok_or(())?;
    if object.get("op").and_then(Value::as_str) != Some("evaluate") {
        return Err(());
    }
    let signed_runtime_release_operation_bytes = object
        .get("signed_runtime_release_operation")
        .cloned()
        .ok_or(())
        .and_then(|value| serde_json::from_value::<Vec<u8>>(value).map_err(|_| ()))?;
    let signed_runtime_release_operation = SignedRuntimeReleaseOperationV1::from_canonical_bytes(
        &signed_runtime_release_operation_bytes,
    )
    .map_err(|_| ())?;
    chain_rights_evidence_request(&signed_runtime_release_operation).map_err(|_| ())
}

fn load_provider_state(config: Value) -> Result<ConfiguredCustodyProvider, ()> {
    let root = parse_state_root_from_provider_init(&config).map_err(|_| ())?;
    let loaded = load_state_from_root(&root).map_err(|_| ())?;
    Ok(ConfiguredCustodyProvider {
        expected_runtime_issuer: loaded.expected_runtime_issuer,
        node_public_key: loaded.node_public_key,
        share_store: NodeLocalShareStoreV1::new(
            loaded.node_public_key,
            loaded.data_root.join("node-shares"),
        ),
        replay_store: DurableReplayClaimStoreV1::new(
            loaded.node_public_key,
            loaded.data_root.join("replay"),
        ),
        node_signing_key: loaded.node_signing_key,
        node_custody_secret: loaded.node_custody_secret,
    })
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn read_provider_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut frame = Vec::new();
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if frame.is_empty() && !oversized {
                return Ok(None);
            }
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            let chunk = &available[..newline];
            if !oversized {
                if frame.len().saturating_add(chunk.len()) > MAX_PROVIDER_FRAME_BYTES_V1 {
                    oversized = true;
                } else {
                    frame.extend_from_slice(chunk);
                }
            }
            reader.consume(newline + 1);
            return Ok(Some(if oversized { Err(()) } else { Ok(frame) }));
        }

        let consumed = available.len();
        if !oversized {
            if frame.len().saturating_add(consumed) > MAX_PROVIDER_FRAME_BYTES_V1 {
                oversized = true;
            } else {
                frame.extend_from_slice(available);
            }
        }
        reader.consume(consumed);
    }
}

fn run_provider_loop<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    provider: &mut CustodyProvider,
) {
    loop {
        let (response, should_shutdown) = match read_provider_frame(input) {
            Ok(Some(Ok(frame))) => provider.handle_frame(&frame),
            Ok(Some(Err(()))) => (invalid_request(), false),
            Ok(None) => break,
            // A transport read error is not recoverable: retrying re-reads the
            // same failing descriptor and spins. Report once, then exit.
            Err(_) => (
                ProviderResponse::error(REQUEST_ERROR_CODE, "custody provider request is invalid"),
                true,
            ),
        };
        if serde_json::to_writer(&mut *output, &response).is_err() {
            break;
        }
        if writeln!(output).and_then(|()| output.flush()).is_err() {
            break;
        }
        if should_shutdown {
            break;
        }
    }
}

fn main() {
    match parse_cli_command() {
        Ok(CliCommand::Serve) => {}
        Ok(CliCommand::Provision {
            base_path,
            trusted_runtime_issuer,
        }) => {
            if run_provision_command(&base_path, &trusted_runtime_issuer).is_err() {
                eprintln!("custody-provider: provisioning failed");
                std::process::exit(1);
            }
            return;
        }
        Err(()) => {
            eprintln!("custody-provider: usage: custody-provider provision --base-path <abs-root> --trusted-runtime-issuer <0x...>");
            std::process::exit(2);
        }
    }
    eprintln!("custody-provider: starting v{PROVIDER_VERSION}");
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    let mut provider = CustodyProvider::new();
    run_provider_loop(&mut input, &mut stdout, &mut provider);
}

fn run_provision_command(base_path: &str, trusted_runtime_issuer: &str) -> Result<(), ()> {
    let root = parse_state_root_from_provider_init(&json!({
        "base_path": base_path,
        "allowed_paths": [],
        "read_only": false,
        "encryption_key": "",
        "extra": null,
    }))
    .map_err(|_| ())?;
    let runtime_issuer = parse_runtime_issuer_hex(trusted_runtime_issuer).map_err(|_| ())?;
    let provisioned = provision_state_root(&root, runtime_issuer).map_err(|_| ())?;
    let receipt = provisioning_receipt(
        runtime_issuer,
        provisioned.node_public_key,
        provisioned.node_custody_public_key,
    );
    serde_json::to_writer(
        io::stdout(),
        &json!({
            "status": "ok",
            "data": {
                "schema": PROVISIONING_SCHEMA_V1,
                "provider": "custody",
                "node_public_key": format!("0x{}", hex_bytes(provisioned.node_public_key.as_bytes())),
                "node_custody_public_key": format!("0x{}", hex_bytes(provisioned.node_custody_public_key.as_bytes())),
                "receipt": format!("0x{}", hex_bytes(&receipt)),
            }
        }),
    )
    .map_err(|_| ())?;
    println!();
    Ok(())
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self};
    use std::io::Cursor;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CONFIG_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_root() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        temp
    }

    fn runtime_issuer_hex(seed: u8) -> String {
        let key = SigningKey::from_bytes(&[seed; 32]);
        format!("0x{}", hex_bytes(&key.verifying_key().to_bytes()))
    }

    fn init_config(root: &Path) -> Value {
        let root = fs::canonicalize(root).unwrap();
        let id = CONFIG_ID.fetch_add(1, Ordering::Relaxed);
        let case = root.join(format!("case-{id}"));
        provision_state_root(
            &case,
            parse_runtime_issuer_hex(&runtime_issuer_hex(0x42)).unwrap(),
        )
        .unwrap();
        json!({
            "base_path": case,
            "allowed_paths": [],
            "read_only": false,
            "encryption_key": "",
            "extra": null
        })
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn provider_loop_responses(input: Vec<u8>) -> Vec<Value> {
        let mut input = Cursor::new(input);
        let mut output = Vec::new();
        let mut provider = CustodyProvider::new();
        run_provider_loop(&mut input, &mut output, &mut provider);
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("persistent transport failure"))
        }
    }

    impl BufRead for FailingReader {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            Err(io::Error::other("persistent transport failure"))
        }

        fn consume(&mut self, _amount: usize) {}
    }

    #[test]
    fn provider_loop_exits_after_one_error_frame_on_transport_read_failure() {
        let mut input = FailingReader;
        let mut output = Vec::new();
        let mut provider = CustodyProvider::new();
        run_provider_loop(&mut input, &mut output, &mut provider);
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["status"], "error");
    }

    fn oversized_frame_for(op: &str) -> Vec<u8> {
        let mut frame = format!(r#"{{"op":"{op}","padding":""#).into_bytes();
        frame.resize(MAX_PROVIDER_FRAME_BYTES_V1 + 1, b'a');
        frame.push(b'\n');
        frame
    }

    #[test]
    fn init_status_and_shutdown_are_redacted_and_strict() {
        let temp = temp_root();
        let mut provider = CustodyProvider::new();
        let response = provider.init(init_config(temp.path()));
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(encoded.contains("\"configured\":true"));
        assert!(encoded.contains(CUSTODY_PROVIDER_REQUEST_SCHEMA_V1));
        assert!(!encoded.contains(temp.path().to_str().unwrap()));
        assert!(!encoded.contains("0x0101"));
        assert!(!encoded.contains("carrier"));
        assert!(!encoded.contains("\"port\""));

        let response = provider.handle_line(r#"{"op":"shutdown"}"#);
        assert_eq!(serde_json::to_value(response).unwrap()["status"], "ok");
    }

    #[test]
    fn init_rejects_missing_unsafe_and_extra_config_inputs_and_clears_state() {
        let temp = temp_root();
        let mut provider = CustodyProvider::new();
        assert!(matches!(
            provider.init(init_config(temp.path())),
            ProviderResponse::Ok { .. }
        ));

        let bad = provider.init(json!({"base_path": "direct"}));
        assert!(matches!(
            bad,
            ProviderResponse::Error {
                code: INIT_ERROR_CODE,
                ..
            }
        ));
        assert!(provider.state.is_none());

        let config = init_config(temp.path());
        let root = PathBuf::from(config["base_path"].as_str().unwrap());
        fs::set_permissions(
            root.join("node-signing-key"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(matches!(
            provider.init(config),
            ProviderResponse::Error { .. }
        ));

        let config = init_config(temp.path());
        let root = PathBuf::from(config["base_path"].as_str().unwrap());
        let custody = root.join("node-custody-secret");
        fs::remove_file(&custody).unwrap();
        symlink(temp.path().join("missing"), &custody).unwrap();
        assert!(matches!(
            provider.init(config),
            ProviderResponse::Error { .. }
        ));

        let mut config = init_config(temp.path());
        config["extra"] = json!({"unexpected": true});
        assert!(matches!(
            provider.init(config),
            ProviderResponse::Error { .. }
        ));
    }

    #[test]
    fn init_rejects_relative_paths_symlinked_parent_and_data_root() {
        let temp = temp_root();
        let mut provider = CustodyProvider::new();

        let mut relative = init_config(temp.path());
        relative["base_path"] = json!("runtime");
        assert!(matches!(
            provider.init(relative),
            ProviderResponse::Error {
                code: INIT_ERROR_CODE,
                ..
            }
        ));

        let safe_root = fs::canonicalize(temp.path()).unwrap();
        let real_parent = safe_root.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        fs::set_permissions(&real_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let linked_parent = safe_root.join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        let mut symlinked_parent = init_config(temp.path());
        symlinked_parent["base_path"] = json!(linked_parent);
        assert!(matches!(
            provider.init(symlinked_parent),
            ProviderResponse::Error { .. }
        ));

        let symlinked_data = init_config(temp.path());
        let root = PathBuf::from(symlinked_data["base_path"].as_str().unwrap());
        let data = root.join("data");
        fs::remove_dir(&data).unwrap();
        let target = safe_root.join("actual-data-root");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&target, &data).unwrap();
        assert!(matches!(
            provider.init(symlinked_data),
            ProviderResponse::Error { .. }
        ));
    }

    #[test]
    fn provider_loop_enforces_frame_limit_and_recovers_next_frame() {
        let mut input = vec![b' '; MAX_PROVIDER_FRAME_BYTES_V1];
        input.push(b'\n');
        input.extend_from_slice(&oversized_frame_for("status"));
        input.extend_from_slice(&oversized_frame_for("provision_node_share"));
        input.extend_from_slice(br#"{"op":"status"}"#);
        input.push(b'\n');
        input.extend_from_slice(br#"{"op":"shutdown"}"#);
        input.push(b'\n');

        let responses = provider_loop_responses(input);
        assert_eq!(responses.len(), 5);
        assert_eq!(responses[0]["status"], "error");
        assert_eq!(responses[1]["status"], "error");
        assert_eq!(responses[2]["status"], "error");
        assert_eq!(responses[3]["status"], "ok");
        assert_eq!(responses[3]["data"]["provider"], "custody");
        assert_eq!(responses[4]["status"], "ok");
    }

    #[test]
    fn malformed_requests_fail_with_fixed_redacted_error() {
        let temp = temp_root();
        let mut provider = CustodyProvider::new();
        assert!(matches!(
            provider.init(init_config(temp.path())),
            ProviderResponse::Ok { .. }
        ));
        for line in [
            r#"{"op":"provision_node_share","has_access":true}"#,
            r#"{"op":"release_contribution","custody_envelope":[]}"#,
            r#"{"op":"unknown","path":"/tmp/secret"}"#,
        ] {
            let encoded = serde_json::to_string(&provider.handle_line(line)).unwrap();
            assert!(encoded.contains(REQUEST_ERROR_CODE));
            assert!(!encoded.contains("/tmp/secret"));
            assert!(!encoded.contains("custody_envelope"));
            assert!(!encoded.contains("has_access"));
        }
    }

    fn runtime_invocation(op: &str, target: &str, carrier: Value) -> Value {
        json!({
            "schema": "elastos.provider.invocation/v1",
            "source": "runtime",
            "target": target,
            "op": op,
            "capability": format!("provider:runtime->{target}:{op}"),
            "transport": "runtime-local-provider-plane",
            "carrier": carrier,
            "transfer": "json",
            "range": null,
            "progress": null,
            "abi": {}
        })
    }

    #[test]
    fn status_rejects_runtime_invocation_and_transfer_envelopes() {
        let mut provider = CustodyProvider::new();
        for envelope in [
            runtime_invocation("status", "key", Value::Null),
            runtime_invocation("status", "custody", json!({"peer_did": "did:key:zX"})),
            json!({
                "schema": "elastos.provider.invocation/v1",
                "source": "runtime",
                "target": "custody",
                "op": "status",
                "transport": "carrier-provider-plane",
                "carrier": Value::Null
            }),
        ] {
            assert!(matches!(
                provider.handle_line(
                    &json!({
                        "op": "status",
                        "_runtime_invocation": envelope,
                    })
                    .to_string(),
                ),
                ProviderResponse::Error {
                    code: REQUEST_ERROR_CODE,
                    ..
                }
            ));
        }
        assert!(matches!(
            provider.handle_line(
                &json!({
                    "op": "status",
                    "_runtime_transfer": {
                        "schema": "elastos.provider.transfer/v1"
                    },
                })
                .to_string(),
            ),
            ProviderResponse::Error {
                code: REQUEST_ERROR_CODE,
                ..
            }
        ));
    }

    #[test]
    fn chain_request_rejects_string_signed_runtime_release_operation() {
        let mut request_value = json!({
            "op": "evaluate",
            "signed_runtime_release_operation": "0xdeadbeef",
        });
        request_value["signed_runtime_release_operation"] = Value::String("0xdeadbeef".to_string());
        assert!(chain_rights_evidence_request_from_validated_request(&request_value).is_err());
    }

    #[test]
    fn chain_request_rejects_malformed_canonical_bytes() {
        let mut request_value = json!({
            "op": "evaluate",
            "signed_runtime_release_operation": [1, 2, 3],
        });
        request_value["signed_runtime_release_operation"] = json!([1, 2, 3]);
        assert!(chain_rights_evidence_request_from_validated_request(&request_value).is_err());
    }

    #[test]
    fn chain_request_rejects_wrong_op() {
        let request_value = json!({
            "op": "status",
            "signed_runtime_release_operation": [1, 2, 3],
        });
        assert!(chain_rights_evidence_request_from_validated_request(&request_value).is_err());
    }
}
