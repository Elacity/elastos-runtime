use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use ed25519_dalek::SigningKey;
use elastos_protected_content_contracts::{NodePublicKey, RuntimeOperationIssuerKeyV1};
use elastos_protected_content_custody::NodeCustodySecretKeyV1;
pub use elastos_protected_content_provider_contracts::{
    parse_and_verify_provisioning_output, provisioning_receipt,
    ProvisionedCustodyProviderPublicKeys, ProvisioningOutputError,
    CUSTODY_PROVIDER_PROVISIONING_RECEIPT_PROVIDER_ID_V1 as PROVISIONING_PROVIDER_ID,
    CUSTODY_PROVIDER_PROVISIONING_RECEIPT_SCHEMA_V1 as PROVISIONING_SCHEMA_V1,
};
use serde::Deserialize;
use zeroize::Zeroizing;

const MAX_CONFIG_PATH_BYTES: usize = 4096;
const MAX_SECRET_FILE_BYTES: usize = 128;
const STAGING_ROOT_PREFIX: &str = ".custody-provider-stage-";
const TRUSTED_RUNTIME_ISSUER_FILE: &str = "trusted-runtime-issuer";
const NODE_CUSTODY_SECRET_FILE: &str = "node-custody-secret";
const NODE_SIGNING_KEY_FILE: &str = "node-signing-key";
const DATA_ROOT_DIR: &str = "data";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustodyProviderStateRootError {
    InvalidConfig,
    MissingOrUnsafe,
    Conflict,
    ProvisioningFailed,
}

impl std::fmt::Display for CustodyProviderStateRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfig => "custody provider configuration is invalid",
            Self::MissingOrUnsafe => "custody provider state root is missing or unsafe",
            Self::Conflict => "custody provider state root conflicts with the requested identity",
            Self::ProvisioningFailed => "custody provider provisioning failed",
        })
    }
}

impl std::error::Error for CustodyProviderStateRootError {}

pub struct LoadedCustodyProviderState {
    pub expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    pub node_public_key: NodePublicKey,
    pub node_signing_key: SigningKey,
    pub node_custody_secret: NodeCustodySecretKeyV1,
    pub data_root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderInitConfigV1 {
    base_path: String,
    #[serde(default)]
    allowed_paths: Vec<String>,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    encryption_key: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    extra: serde_json::Value,
}

#[derive(Debug, Clone)]
struct CustodyProviderStatePaths {
    root: PathBuf,
    trusted_runtime_issuer: PathBuf,
    node_custody_secret: PathBuf,
    node_signing_key: PathBuf,
    data_root: PathBuf,
}

impl CustodyProviderStatePaths {
    fn derive(root: PathBuf) -> Self {
        Self {
            trusted_runtime_issuer: root.join(TRUSTED_RUNTIME_ISSUER_FILE),
            node_custody_secret: root.join(NODE_CUSTODY_SECRET_FILE),
            node_signing_key: root.join(NODE_SIGNING_KEY_FILE),
            data_root: root.join(DATA_ROOT_DIR),
            root,
        }
    }
}

pub fn parse_state_root_from_provider_init(
    config: &serde_json::Value,
) -> Result<PathBuf, CustodyProviderStateRootError> {
    let parsed: ProviderInitConfigV1 = serde_json::from_value(config.clone())
        .map_err(|_| CustodyProviderStateRootError::InvalidConfig)?;
    if parsed.read_only
        || !parsed.allowed_paths.is_empty()
        || !parsed.encryption_key.is_empty()
        || !parsed.extra.is_null()
    {
        return Err(CustodyProviderStateRootError::InvalidConfig);
    }
    path_from_config_value(&parsed.base_path)
}

pub fn parse_runtime_issuer_hex(
    value: &str,
) -> Result<RuntimeOperationIssuerKeyV1, CustodyProviderStateRootError> {
    let bytes = parse_hex32_bytes(value.as_bytes())
        .map_err(|_| CustodyProviderStateRootError::InvalidConfig)?;
    RuntimeOperationIssuerKeyV1::new(*bytes)
        .map_err(|_| CustodyProviderStateRootError::InvalidConfig)
}

pub fn validate_state_root_path(root: &Path) -> Result<(), CustodyProviderStateRootError> {
    validate_absolute_path_syntax(root)?;
    validate_existing_path_components(root)?;
    validate_owner_only_directory(root)
}

pub fn load_state_from_root(
    root: &Path,
) -> Result<LoadedCustodyProviderState, CustodyProviderStateRootError> {
    let root = normalize_root_path(root)?;
    let paths = CustodyProviderStatePaths::derive(root);
    validate_existing_path_components(&paths.root)?;
    validate_owner_only_directory(&paths.root)?;
    let expected_runtime_issuer_bytes = read_hex32_file(&paths.trusted_runtime_issuer)?;
    let expected_runtime_issuer = RuntimeOperationIssuerKeyV1::new(*expected_runtime_issuer_bytes)
        .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    let node_custody_secret =
        NodeCustodySecretKeyV1::from_guarded_bytes(read_hex32_file(&paths.node_custody_secret)?)
            .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    let node_signing_seed = read_hex32_file(&paths.node_signing_key)?;
    let node_signing_key = SigningKey::from_bytes(&node_signing_seed);
    let node_public_key = NodePublicKey::new(node_signing_key.verifying_key().to_bytes())
        .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    validate_owner_only_directory(&paths.data_root)?;
    Ok(LoadedCustodyProviderState {
        expected_runtime_issuer,
        node_public_key,
        node_signing_key,
        node_custody_secret,
        data_root: paths.data_root,
    })
}

pub fn provision_state_root(
    root: &Path,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
) -> Result<ProvisionedCustodyProviderPublicKeys, CustodyProviderStateRootError> {
    let root = normalize_root_path(root)?;
    let paths = CustodyProviderStatePaths::derive(root.clone());
    if paths.root.exists() {
        let loaded = load_state_from_root(&paths.root)?;
        if loaded.expected_runtime_issuer != expected_runtime_issuer {
            return Err(CustodyProviderStateRootError::Conflict);
        }
        return Ok(ProvisionedCustodyProviderPublicKeys {
            node_public_key: loaded.node_public_key,
            node_custody_public_key: loaded.node_custody_secret.public_key().unwrap(),
        });
    }

    let parent = paths
        .root
        .parent()
        .ok_or(CustodyProviderStateRootError::ProvisioningFailed)?;
    validate_existing_path_components(parent)?;
    validate_owner_only_directory(parent)?;

    let stage_root = create_stage_root(parent)?;
    let stage_paths = CustodyProviderStatePaths::derive(stage_root.clone());
    let result =
        provision_state_root_in_stage(&paths, &stage_paths, expected_runtime_issuer, parent);
    if stage_root.exists() {
        remove_created_stage_root(&stage_root)?;
    }
    result
}

fn provision_state_root_in_stage(
    final_paths: &CustodyProviderStatePaths,
    stage_paths: &CustodyProviderStatePaths,
    expected_runtime_issuer: RuntimeOperationIssuerKeyV1,
    parent: &Path,
) -> Result<ProvisionedCustodyProviderPublicKeys, CustodyProviderStateRootError> {
    create_owner_only_directory(&stage_paths.data_root)?;
    write_owner_only_hex32_file(
        &stage_paths.trusted_runtime_issuer,
        expected_runtime_issuer.as_bytes(),
    )?;

    let node_custody_secret = generate_nonzero_secret()?;
    write_owner_only_hex32_file(&stage_paths.node_custody_secret, &node_custody_secret)?;

    let node_signing_seed = generate_nonzero_secret()?;
    write_owner_only_hex32_file(&stage_paths.node_signing_key, &node_signing_seed)?;

    sync_directory(&stage_paths.data_root)?;
    sync_directory(&stage_paths.root)?;

    match rename_without_replacement(&stage_paths.root, &final_paths.root) {
        Ok(()) => {}
        Err(CustodyProviderStateRootError::Conflict) if final_paths.root.exists() => {
            let loaded = load_state_from_root(&final_paths.root)?;
            if loaded.expected_runtime_issuer != expected_runtime_issuer {
                return Err(CustodyProviderStateRootError::Conflict);
            }
            return Ok(ProvisionedCustodyProviderPublicKeys {
                node_public_key: loaded.node_public_key,
                node_custody_public_key: loaded.node_custody_secret.public_key().unwrap(),
            });
        }
        Err(error) => return Err(error),
    }

    sync_directory(parent)?;

    let loaded = load_state_from_root(&final_paths.root)?;
    if loaded.expected_runtime_issuer != expected_runtime_issuer {
        return Err(CustodyProviderStateRootError::Conflict);
    }
    Ok(ProvisionedCustodyProviderPublicKeys {
        node_public_key: loaded.node_public_key,
        node_custody_public_key: loaded.node_custody_secret.public_key().unwrap(),
    })
}

fn create_stage_root(parent: &Path) -> Result<PathBuf, CustodyProviderStateRootError> {
    for _ in 0..8 {
        let mut nonce = [0u8; 8];
        getrandom::getrandom(&mut nonce)
            .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
        let stage_root = parent.join(format!(
            "{STAGING_ROOT_PREFIX}{}-{}",
            std::process::id(),
            nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        ));
        match create_owner_only_directory(&stage_root) {
            Ok(()) => return Ok(stage_root),
            Err(CustodyProviderStateRootError::ProvisioningFailed) if stage_root.exists() => {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    Err(CustodyProviderStateRootError::ProvisioningFailed)
}

fn remove_created_stage_root(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(CustodyProviderStateRootError::ProvisioningFailed)?;
    if !file_name.starts_with(STAGING_ROOT_PREFIX) {
        return Err(CustodyProviderStateRootError::ProvisioningFailed);
    }
    validate_owner_only_directory(path)
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    fs::remove_dir_all(path).map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)
}

#[cfg(target_os = "linux")]
fn rename_without_replacement(from: &Path, to: &Path) -> Result<(), CustodyProviderStateRootError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    let result = unsafe {
        nix::libc::renameat2(
            nix::libc::AT_FDCWD,
            from.as_ptr(),
            nix::libc::AT_FDCWD,
            to.as_ptr(),
            nix::libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::AlreadyExists => Err(CustodyProviderStateRootError::Conflict),
            _ => Err(CustodyProviderStateRootError::ProvisioningFailed),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn rename_without_replacement(from: &Path, to: &Path) -> Result<(), CustodyProviderStateRootError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    let to = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    let result =
        unsafe { nix::libc::renamex_np(from.as_ptr(), to.as_ptr(), nix::libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        match std::io::Error::last_os_error().kind() {
            std::io::ErrorKind::AlreadyExists => Err(CustodyProviderStateRootError::Conflict),
            _ => Err(CustodyProviderStateRootError::ProvisioningFailed),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
fn rename_without_replacement(
    _from: &Path,
    _to: &Path,
) -> Result<(), CustodyProviderStateRootError> {
    Err(CustodyProviderStateRootError::ProvisioningFailed)
}

fn generate_nonzero_secret() -> Result<Zeroizing<[u8; 32]>, CustodyProviderStateRootError> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::getrandom(&mut *bytes)
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(CustodyProviderStateRootError::ProvisioningFailed);
    }
    Ok(bytes)
}

fn create_owner_only_directory(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    validate_owner_only_directory(path)
}

fn write_owner_only_hex32_file(
    path: &Path,
    bytes: &[u8; 32],
) -> Result<(), CustodyProviderStateRootError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    write!(file, "0x{}", hex_bytes(bytes))
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    file.sync_all()
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    let metadata = file
        .metadata()
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    validate_owner_only_file_metadata(&metadata)
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    let file =
        open_directory(path).map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)?;
    file.sync_all()
        .map_err(|_| CustodyProviderStateRootError::ProvisioningFailed)
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn path_from_config_value(value: &str) -> Result<PathBuf, CustodyProviderStateRootError> {
    if value.is_empty()
        || value.len() > MAX_CONFIG_PATH_BYTES
        || value
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(CustodyProviderStateRootError::InvalidConfig);
    }
    let path = PathBuf::from(value);
    validate_absolute_path_syntax(&path)?;
    Ok(path)
}

fn normalize_root_path(path: &Path) -> Result<PathBuf, CustodyProviderStateRootError> {
    validate_absolute_path_syntax(path)?;
    Ok(path.to_path_buf())
}

fn validate_absolute_path_syntax(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    if !path.is_absolute() {
        return Err(CustodyProviderStateRootError::MissingOrUnsafe);
    }
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => {}
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(CustodyProviderStateRootError::MissingOrUnsafe)
            }
        }
    }
    Ok(())
}

fn validate_existing_path_components(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => current.push(component.as_os_str()),
            Component::Normal(name) => {
                current.push(name);
                let metadata = fs::symlink_metadata(&current)
                    .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
                if metadata.file_type().is_symlink() {
                    return Err(CustodyProviderStateRootError::MissingOrUnsafe);
                }
                validate_component_owner_and_mode(&metadata)?;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(CustodyProviderStateRootError::MissingOrUnsafe)
            }
        }
    }
    Ok(())
}

fn read_hex32_file(path: &Path) -> Result<Zeroizing<[u8; 32]>, CustodyProviderStateRootError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    validate_owner_only_file_metadata(&metadata)?;
    let file =
        open_owner_only_file(path).map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    let open_metadata = file
        .metadata()
        .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    validate_owner_only_file_metadata(&open_metadata)?;
    require_same_file(&metadata, &open_metadata)?;
    if open_metadata.len() as usize > MAX_SECRET_FILE_BYTES {
        return Err(CustodyProviderStateRootError::MissingOrUnsafe);
    }
    let mut raw = Zeroizing::new(Vec::with_capacity(open_metadata.len() as usize));
    file.take((MAX_SECRET_FILE_BYTES + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    if raw.len() > MAX_SECRET_FILE_BYTES {
        return Err(CustodyProviderStateRootError::MissingOrUnsafe);
    }
    parse_hex32_bytes(&raw).map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)
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

fn hex_bytes(bytes: &[u8; 32]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn validate_owner_only_directory(path: &Path) -> Result<(), CustodyProviderStateRootError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| CustodyProviderStateRootError::MissingOrUnsafe)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CustodyProviderStateRootError::MissingOrUnsafe);
    }
    validate_owner_mode_and_links(&metadata, 0o700, None)
}

fn validate_owner_only_file_metadata(
    metadata: &fs::Metadata,
) -> Result<(), CustodyProviderStateRootError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CustodyProviderStateRootError::MissingOrUnsafe);
    }
    validate_owner_mode_and_links(metadata, 0o600, Some(1))
}

fn validate_owner_mode_and_links(
    metadata: &fs::Metadata,
    exact_mode: u32,
    expected_links: Option<u64>,
) -> Result<(), CustodyProviderStateRootError> {
    #[cfg(unix)]
    {
        use nix::unistd::geteuid;
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != geteuid().as_raw() || metadata.mode() & 0o777 != exact_mode {
            return Err(CustodyProviderStateRootError::MissingOrUnsafe);
        }
        if let Some(expected) = expected_links {
            if metadata.nlink() != expected {
                return Err(CustodyProviderStateRootError::MissingOrUnsafe);
            }
        }
    }
    Ok(())
}

fn validate_component_owner_and_mode(
    metadata: &fs::Metadata,
) -> Result<(), CustodyProviderStateRootError> {
    #[cfg(unix)]
    {
        use nix::unistd::geteuid;
        use std::os::unix::fs::MetadataExt;

        let uid = metadata.uid();
        if uid != geteuid().as_raw() && uid != 0 {
            return Err(CustodyProviderStateRootError::MissingOrUnsafe);
        }
        let mode = metadata.mode() & 0o7777;
        let writable_by_others = mode & 0o022 != 0;
        let sticky = mode & 0o1000 != 0;
        if writable_by_others && !sticky {
            return Err(CustodyProviderStateRootError::MissingOrUnsafe);
        }
    }
    Ok(())
}

fn require_same_file(
    pre: &fs::Metadata,
    opened: &fs::Metadata,
) -> Result<(), CustodyProviderStateRootError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if pre.dev() != opened.dev() || pre.ino() != opened.ino() || pre.len() != opened.len() {
            return Err(CustodyProviderStateRootError::MissingOrUnsafe);
        }
    }
    #[cfg(not(unix))]
    {
        if pre.len() != opened.len() {
            return Err(CustodyProviderStateRootError::MissingOrUnsafe);
        }
    }
    Ok(())
}

fn open_owner_only_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn runtime_issuer(seed: u8) -> RuntimeOperationIssuerKeyV1 {
        RuntimeOperationIssuerKeyV1::new(
            SigningKey::from_bytes(&[seed; 32])
                .verifying_key()
                .to_bytes(),
        )
        .unwrap()
    }

    fn hex_string(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    }

    fn valid_provisioning_output(seed: u8) -> serde_json::Value {
        let expected_runtime_issuer = runtime_issuer(seed);
        let node_signing_key = SigningKey::from_bytes(&[0x22; 32]);
        let node_public_key =
            NodePublicKey::new(node_signing_key.verifying_key().to_bytes()).unwrap();
        let node_custody_public_key =
            NodeCustodySecretKeyV1::from_guarded_bytes(Zeroizing::new([0x33; 32]))
                .unwrap()
                .public_key()
                .unwrap();
        let receipt = provisioning_receipt(
            expected_runtime_issuer,
            node_public_key,
            node_custody_public_key,
        );
        serde_json::json!({
            "status": "ok",
            "data": {
                "schema": PROVISIONING_SCHEMA_V1,
                "provider": PROVISIONING_PROVIDER_ID,
                "node_public_key": format!("0x{}", hex_bytes(node_public_key.as_bytes())),
                "node_custody_public_key": format!(
                    "0x{}",
                    hex_string(node_custody_public_key.as_bytes())
                ),
                "receipt": format!("0x{}", hex_bytes(&receipt)),
            }
        })
    }

    #[cfg(unix)]
    fn owner_only_dir(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn provision_state_root_is_idempotent_for_exact_identity_and_conflicts_otherwise() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = fs::canonicalize(temp.path()).unwrap();
        let parent = temp_root.join("parent");
        owner_only_dir(&parent);
        let root = parent.join("custody");
        let issuer = runtime_issuer(0x42);

        let first = provision_state_root(&root, issuer).unwrap();
        let second = provision_state_root(&root, issuer).unwrap();
        assert_eq!(first, second);
        let loaded = load_state_from_root(&root).unwrap();
        assert_eq!(loaded.node_public_key, first.node_public_key);
        assert_eq!(
            loaded.node_custody_secret.public_key().unwrap(),
            first.node_custody_public_key
        );
        assert!(matches!(
            provision_state_root(&root, runtime_issuer(0x55)),
            Err(CustodyProviderStateRootError::Conflict)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_exact_provisioning_reuses_identity_and_leaves_no_stage_root() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = fs::canonicalize(temp.path()).unwrap();
        let parent = temp_root.join("parent");
        owner_only_dir(&parent);
        let root = parent.join("custody");
        let issuer = runtime_issuer(0x42);

        let thread_root = root.clone();
        let thread_issuer = issuer;
        let first = std::thread::spawn(move || provision_state_root(&thread_root, thread_issuer));
        let second = provision_state_root(&root, issuer).unwrap();
        let first = first.join().unwrap().unwrap();
        assert_eq!(first, second);
        assert!(fs::read_dir(&parent).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_ROOT_PREFIX)));
    }

    #[cfg(unix)]
    #[test]
    fn provision_state_root_rejects_partial_existing_root_without_repair() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = fs::canonicalize(temp.path()).unwrap();
        let parent = temp_root.join("parent");
        owner_only_dir(&parent);
        let root = parent.join("custody");
        owner_only_dir(&root);
        owner_only_dir(&root.join(DATA_ROOT_DIR));

        assert!(matches!(
            provision_state_root(&root, runtime_issuer(0x42)),
            Err(CustodyProviderStateRootError::MissingOrUnsafe)
        ));
        assert!(root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn validate_state_root_path_rejects_missing_and_unsafe_roots() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = fs::canonicalize(temp.path()).unwrap();
        let parent = temp_root.join("parent");
        owner_only_dir(&parent);
        let missing = parent.join("missing");
        assert!(matches!(
            validate_state_root_path(&missing),
            Err(CustodyProviderStateRootError::MissingOrUnsafe)
        ));

        let linked = parent.join("linked");
        symlink(temp.path(), &linked).unwrap();
        assert!(matches!(
            validate_state_root_path(&linked),
            Err(CustodyProviderStateRootError::MissingOrUnsafe)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn parse_state_root_from_provider_init_rejects_extra_truth() {
        let temp = tempfile::tempdir().unwrap();
        let temp_root = fs::canonicalize(temp.path()).unwrap();
        let parent = temp_root.join("parent");
        owner_only_dir(&parent);
        let config = serde_json::json!({
            "base_path": parent,
            "allowed_paths": ["*"],
            "read_only": false,
            "encryption_key": "",
            "extra": null,
        });
        assert!(matches!(
            parse_state_root_from_provider_init(&config),
            Err(CustodyProviderStateRootError::InvalidConfig)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_accepts_valid_canonical_output() {
        let output = valid_provisioning_output(0x42);
        assert!(parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)).is_ok());
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_wrong_schema() {
        let mut output = valid_provisioning_output(0x42);
        output["data"]["schema"] = serde_json::json!("wrong-schema");
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_wrong_provider() {
        let mut output = valid_provisioning_output(0x42);
        output["data"]["provider"] = serde_json::json!("wrong-provider");
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_missing_field() {
        let mut output = valid_provisioning_output(0x42);
        output
            .get_mut("data")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("receipt");
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_additional_field() {
        let mut output = valid_provisioning_output(0x42);
        output["data"]["extra"] = serde_json::json!(null);
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_unprefixed_public_fields() {
        let mut output = valid_provisioning_output(0x42);
        let node_public_key = output["data"]["node_public_key"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x")
            .to_string();
        output["data"]["node_public_key"] = serde_json::json!(node_public_key);
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));

        let mut output = valid_provisioning_output(0x42);
        let receipt = output["data"]["receipt"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x")
            .to_string();
        output["data"]["receipt"] = serde_json::json!(receipt);
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_uppercase_public_fields() {
        let mut output = valid_provisioning_output(0x42);
        let node_public_key = output["data"]["node_public_key"]
            .as_str()
            .unwrap()
            .to_uppercase();
        output["data"]["node_public_key"] = serde_json::json!(node_public_key);
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));

        let mut output = valid_provisioning_output(0x42);
        let receipt = output["data"]["receipt"].as_str().unwrap().to_uppercase();
        output["data"]["receipt"] = serde_json::json!(receipt);
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidOutput)
        ));
    }

    #[test]
    fn parse_and_verify_provisioning_output_rejects_wrong_receipt() {
        let mut output = valid_provisioning_output(0x42);
        output["data"]["receipt"] =
            serde_json::json!("0x1111111111111111111111111111111111111111111111111111111111111111");
        assert!(matches!(
            parse_and_verify_provisioning_output(&output, runtime_issuer(0x42)),
            Err(ProvisioningOutputError::InvalidReceipt)
        ));
    }
}
