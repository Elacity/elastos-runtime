//! ElastOS protected-content custody node provider capsule.
//!
//! This source-only provider is intentionally unregistered in the current
//! product. It proves the one-node custody authority path without replacing the
//! still-active provisional key-provider route.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read as _, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{
    KeyReleaseError, NodePublicKey, RuntimeOperationIssuerKeyV1,
};
use elastos_protected_content_custody::{
    CustodyError, DurableReplayClaimStoreV1, NodeCustodySecretKeyV1, NodeLocalShareStoreV1,
    RecipientPublicKeyV1,
};
use elastos_protected_content_provider_contracts::{
    CustodyProviderRequestOpV1, CustodyProviderRequestValidationErrorV1, CustodyProviderResponseV1,
    ProviderFailureCodeV1, ValidatedCustodyProviderRequestV1, CUSTODY_PROVIDER_REQUEST_SCHEMA_V1,
    CUSTODY_PROVIDER_RESPONSE_SCHEMA_V1, MAX_PROVIDER_FRAME_BYTES_V1,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const MAX_CONFIG_PATH_BYTES: usize = 4096;
const MAX_SECRET_FILE_BYTES: usize = 128;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderInitConfigV1 {
    trusted_runtime_issuer_path: String,
    node_custody_secret_path: String,
    node_signing_key_path: String,
    data_root_path: String,
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
                (self.handle_custody_request(frame), false)
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

fn load_provider_state(config: Value) -> Result<ConfiguredCustodyProvider, ()> {
    let extra = config
        .get("extra")
        .filter(|value| value.is_object())
        .ok_or(())?;
    let parsed: ProviderInitConfigV1 = serde_json::from_value(extra.clone()).map_err(|_| ())?;
    let expected_runtime_issuer_bytes =
        read_hex32_file(path_from_config(&parsed.trusted_runtime_issuer_path)?)?;
    let expected_runtime_issuer =
        RuntimeOperationIssuerKeyV1::new(*expected_runtime_issuer_bytes).map_err(|_| ())?;
    let node_custody_secret = NodeCustodySecretKeyV1::from_guarded_bytes(read_hex32_file(
        path_from_config(&parsed.node_custody_secret_path)?,
    )?)
    .map_err(|_| ())?;
    let node_signing_seed = read_hex32_file(path_from_config(&parsed.node_signing_key_path)?)?;
    let node_signing_key = SigningKey::from_bytes(&node_signing_seed);
    let node_public_key =
        NodePublicKey::new(node_signing_key.verifying_key().to_bytes()).map_err(|_| ())?;
    let data_root = path_from_config(&parsed.data_root_path)?;
    validate_owner_only_directory(&data_root)?;
    Ok(ConfiguredCustodyProvider {
        expected_runtime_issuer,
        node_public_key,
        share_store: NodeLocalShareStoreV1::new(node_public_key, data_root.join("node-shares")),
        replay_store: DurableReplayClaimStoreV1::new(node_public_key, data_root.join("replay")),
        node_signing_key,
        node_custody_secret,
    })
}

fn path_from_config(value: &str) -> Result<PathBuf, ()> {
    if value.is_empty()
        || value.len() > MAX_CONFIG_PATH_BYTES
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(());
    }
    let path = PathBuf::from(value);
    validate_absolute_components(&path)?;
    Ok(path)
}

fn validate_absolute_components(path: &Path) -> Result<(), ()> {
    if !path.is_absolute() {
        return Err(());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => {
                current.push(name);
                let metadata = fs::symlink_metadata(&current).map_err(|_| ())?;
                if metadata.file_type().is_symlink() {
                    return Err(());
                }
                validate_component_owner_and_mode(&metadata, current.parent().is_none())?;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => return Err(()),
        }
    }
    Ok(())
}

fn read_hex32_file(path: PathBuf) -> Result<Zeroizing<[u8; 32]>, ()> {
    let metadata = fs::symlink_metadata(&path).map_err(|_| ())?;
    validate_owner_only_file_metadata(&metadata)?;
    let file = open_owner_only_file(&path)?;
    let open_metadata = file.metadata().map_err(|_| ())?;
    validate_owner_only_file_metadata(&open_metadata)?;
    require_same_file(&metadata, &open_metadata)?;
    if open_metadata.len() as usize > MAX_SECRET_FILE_BYTES {
        return Err(());
    }
    let mut raw = Zeroizing::new(Vec::with_capacity(open_metadata.len() as usize));
    file.take((MAX_SECRET_FILE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| ())?;
    if raw.len() > MAX_SECRET_FILE_BYTES {
        return Err(());
    }
    parse_hex32_bytes(&raw)
}

fn parse_hex32_bytes(raw: &[u8]) -> Result<Zeroizing<[u8; 32]>, ()> {
    let value = std::str::from_utf8(raw)
        .map_err(|_| ())?
        .trim_end_matches('\n');
    let hex = value.strip_prefix("0x").unwrap_or(value);
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(());
    }
    let mut out = [0u8; 32];
    for (index, pair) in hex.as_bytes().chunks_exact(2).enumerate() {
        out[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(Zeroizing::new(out))
}

fn hex_nibble(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(()),
    }
}

fn validate_owner_only_directory(path: &Path) -> Result<(), ()> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    validate_owner_mode_and_links(&metadata, 0o700, None)
}

fn validate_owner_only_file_metadata(metadata: &fs::Metadata) -> Result<(), ()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(());
    }
    validate_owner_mode_and_links(metadata, 0o600, Some(1))
}

fn validate_owner_mode_and_links(
    metadata: &fs::Metadata,
    exact_mode: u32,
    expected_links: Option<u64>,
) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use nix::unistd::geteuid;
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o777 != exact_mode {
            return Err(());
        }
        if let Some(expected) = expected_links {
            if metadata.nlink() != expected {
                return Err(());
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = exact_mode;
        let _ = expected_links;
    }
    Ok(())
}

fn validate_component_owner_and_mode(
    metadata: &fs::Metadata,
    is_filesystem_root: bool,
) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use nix::unistd::geteuid;
        use std::os::unix::fs::MetadataExt;

        if is_filesystem_root {
            return Ok(());
        }
        let uid = metadata.uid();
        if uid != geteuid().as_raw() && uid != 0 {
            return Err(());
        }
        let mode = metadata.mode() & 0o7777;
        let writable_by_others = mode & 0o022 != 0;
        let sticky = mode & 0o1000 != 0;
        if writable_by_others && !sticky {
            return Err(());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        let _ = is_filesystem_root;
    }
    Ok(())
}

fn require_same_file(pre: &fs::Metadata, opened: &fs::Metadata) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if pre.dev() != opened.dev() || pre.ino() != opened.ino() || pre.len() != opened.len() {
            return Err(());
        }
    }
    #[cfg(not(unix))]
    {
        if pre.len() != opened.len() {
            return Err(());
        }
    }
    Ok(())
}

fn open_owner_only_file(path: &Path) -> Result<File, ()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|_| ())
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
            Err(_) => (
                ProviderResponse::error(REQUEST_ERROR_CODE, "custody provider request is invalid"),
                false,
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
    eprintln!("custody-provider: starting v{PROVIDER_VERSION}");
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout();
    let mut provider = CustodyProvider::new();
    run_provider_loop(&mut input, &mut stdout, &mut provider);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::fs::{symlink, OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CONFIG_ID: AtomicUsize = AtomicUsize::new(0);

    fn temp_root() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        temp
    }

    fn write_owner_only(path: &Path, value: &str) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(path).unwrap();
        file.write_all(value.as_bytes()).unwrap();
        file.sync_all().unwrap();
    }

    fn hex32(seed: u8) -> String {
        format!(
            "0x{}",
            [seed; 32]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }

    fn init_config(root: &Path) -> Value {
        let root = fs::canonicalize(root).unwrap();
        let id = CONFIG_ID.fetch_add(1, Ordering::Relaxed);
        let case = root.join(format!("case-{id}"));
        fs::create_dir(&case).unwrap();
        fs::set_permissions(&case, fs::Permissions::from_mode(0o700)).unwrap();
        let data = case.join("data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = case.join("runtime");
        let custody = case.join("custody");
        let signing = case.join("signing");
        let runtime_key = SigningKey::from_bytes(&[0x42; 32]);
        write_owner_only(
            &runtime,
            &format!("0x{}", hex_bytes(&runtime_key.verifying_key().to_bytes())),
        );
        write_owner_only(&custody, &hex32(1));
        write_owner_only(&signing, &hex32(1));
        json!({
            "extra": {
                "trusted_runtime_issuer_path": runtime,
                "node_custody_secret_path": custody,
                "node_signing_key_path": signing,
                "data_root_path": data
            }
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
    fn init_rejects_missing_direct_unsafe_or_linked_config_inputs_and_clears_state() {
        let temp = temp_root();
        let mut provider = CustodyProvider::new();
        assert!(matches!(
            provider.init(init_config(temp.path())),
            ProviderResponse::Ok { .. }
        ));

        let bad = provider.init(json!({"trusted_runtime_issuer_path": "direct"}));
        assert!(matches!(
            bad,
            ProviderResponse::Error {
                code: INIT_ERROR_CODE,
                ..
            }
        ));
        assert!(provider.state.is_none());

        let config = init_config(temp.path());
        let runtime = config["extra"]["trusted_runtime_issuer_path"]
            .as_str()
            .unwrap();
        fs::set_permissions(runtime, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            provider.init(config),
            ProviderResponse::Error { .. }
        ));

        let config = init_config(temp.path());
        let custody = PathBuf::from(
            config["extra"]["node_custody_secret_path"]
                .as_str()
                .unwrap(),
        );
        fs::remove_file(&custody).unwrap();
        symlink(temp.path().join("missing"), &custody).unwrap();
        assert!(matches!(
            provider.init(config),
            ProviderResponse::Error { .. }
        ));

        let config = init_config(temp.path());
        let signing = PathBuf::from(config["extra"]["node_signing_key_path"].as_str().unwrap());
        let hard = temp.path().join("signing-hardlink");
        fs::hard_link(&signing, &hard).unwrap();
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
        relative["extra"]["trusted_runtime_issuer_path"] = json!("runtime");
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
        let linked_runtime = linked_parent.join("runtime");
        write_owner_only(&real_parent.join("runtime"), &hex32(0x42));
        let mut symlinked_parent = init_config(temp.path());
        symlinked_parent["extra"]["trusted_runtime_issuer_path"] = json!(linked_runtime);
        assert!(matches!(
            provider.init(symlinked_parent),
            ProviderResponse::Error { .. }
        ));

        let symlinked_data = init_config(temp.path());
        let data = PathBuf::from(symlinked_data["extra"]["data_root_path"].as_str().unwrap());
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
}
