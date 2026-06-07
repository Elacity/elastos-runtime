use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use elastos_common::localhost::{parse_localhost_path, parse_localhost_uri};
use serde::{Deserialize, Serialize};

const PROVIDER_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

const SUPPORTED_OPS: &[&str] = &[
    "init",
    "ping",
    "shutdown",
    "resolve",
    "read",
    "list",
    "stat",
    "exists",
    "mounts",
    "adapters",
    "register_adapter",
    "unregister_adapter",
    "check_adapter",
    "mount",
    "unmount",
    "index",
    "health",
    "refresh",
    "head",
    "cache",
    "cache_status",
    "sync",
    "sync_status",
    "fork",
    "write",
    "delete",
    "mkdir",
];

const UNSUPPORTED_OPS: &[&str] = &[];
const BUILTIN_MONIKER: &str = "Elastos";
const MOUNT_TABLE_SCHEMA: &str = "elastos.webspace.mount-table/v1";
const MOUNT_RECORD_SCHEMA: &str = "elastos.webspace.mount/v1";
const INDEX_TABLE_SCHEMA: &str = "elastos.webspace.index-table/v1";
const INDEX_ENTRY_SCHEMA: &str = "elastos.webspace.index-entry/v1";
const HEAD_TABLE_SCHEMA: &str = "elastos.webspace.head-table/v1";
const HEAD_RECORD_SCHEMA: &str = "elastos.webspace.object-head/v1";
const OBJECT_TABLE_SCHEMA: &str = "elastos.webspace.object-table/v1";
const OBJECT_RECORD_SCHEMA: &str = "elastos.webspace.object/v1";
const ADAPTER_TABLE_SCHEMA: &str = "elastos.webspace.adapter-table/v1";
const ADAPTER_RECORD_SCHEMA: &str = "elastos.webspace.adapter/v1";
const DEFAULT_CACHE_POLICY: &str = "metadata-only";
const DEFAULT_SYNC_POLICY: &str = "manual";
const DEFAULT_EXTERNAL_RESOLVER: &str = "external";
const DEFAULT_READONLY_ACCESS_POLICY: &str = "resolver-readonly";
const DEFAULT_MUTABLE_ACCESS_POLICY: &str = "owner-writable";
const DEFAULT_ADAPTER_STATE: &str = "configured";
const PROVIDER_ID: &str = "webspace-provider";
const ADAPTER_HEALTH_STALE_AFTER_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        config: serde_json::Value,
    },
    Resolve {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        moniker: Option<String>,
    },
    Read {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(rename = "offset")]
        _offset: Option<u64>,
        #[serde(rename = "length")]
        _length: Option<u64>,
    },
    List {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Stat {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Exists {
        path: String,
        #[serde(rename = "token")]
        _token: String,
    },
    Mounts {
        #[serde(default, rename = "token")]
        _token: String,
    },
    Adapters {
        #[serde(default, rename = "token")]
        _token: String,
    },
    RegisterAdapter {
        resolver: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        endpoint_uri: Option<String>,
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        state: Option<String>,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default)]
        readonly_default: Option<bool>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    UnregisterAdapter {
        resolver: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    CheckAdapter {
        resolver: String,
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        state: Option<String>,
        #[serde(default)]
        error_code: Option<String>,
        #[serde(default)]
        capabilities: Vec<String>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Health {
        #[serde(default)]
        moniker: Option<String>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Mount {
        moniker: String,
        target_uri: String,
        #[serde(default)]
        namespace_uri: Option<String>,
        #[serde(default)]
        resolver: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        readonly: Option<bool>,
        #[serde(default)]
        cache_policy: Option<String>,
        #[serde(default)]
        sync_policy: Option<String>,
        #[serde(default)]
        access_policy: Option<String>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Unmount {
        moniker: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Index {
        moniker: String,
        #[serde(default)]
        entries: Vec<WebSpaceIndexInput>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Refresh {
        path: String,
        #[serde(default)]
        entries: Option<Vec<WebSpaceIndexInput>>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Head {
        path: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Cache {
        path: String,
        #[serde(default)]
        content: Option<Vec<u8>>,
        #[serde(default)]
        mime: Option<String>,
        #[serde(default)]
        source_receipt: Option<serde_json::Value>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    CacheStatus {
        path: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Sync {
        path: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    SyncStatus {
        path: String,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Fork {
        source_uri: String,
        moniker: String,
        #[serde(default)]
        target_uri: Option<String>,
        #[serde(default)]
        resolver: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        readonly: Option<bool>,
        #[serde(default)]
        cache_policy: Option<String>,
        #[serde(default)]
        sync_policy: Option<String>,
        #[serde(default)]
        access_policy: Option<String>,
        #[serde(default, rename = "token")]
        _token: String,
    },
    Write {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        content: Vec<u8>,
        #[serde(default)]
        append: bool,
    },
    Delete {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(default)]
        recursive: bool,
    },
    Mkdir {
        path: String,
        #[serde(rename = "token")]
        _token: String,
        #[serde(default)]
        parents: bool,
    },
    Ping,
    Shutdown,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Ok {
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug, Serialize)]
struct DirEntry {
    name: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    readonly: bool,
    access_policy: String,
    provider: String,
    resolver_state: String,
    resolver: String,
    cache_policy: String,
    sync_policy: String,
    kind: String,
    traversable: bool,
    object_id: String,
    head_id: String,
    cache_state: String,
    sync_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_uri: Option<String>,
}

#[derive(Debug, Serialize)]
struct FileStat {
    path: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    readonly: bool,
    access_policy: String,
    provider: String,
    resolver_state: String,
    resolver: String,
    cache_policy: String,
    sync_policy: String,
    kind: String,
    traversable: bool,
    object_id: String,
    head_id: String,
    cache_state: String,
    sync_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_uri: Option<String>,
    modified: Option<u64>,
    created: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct WebSpaceHandle {
    moniker: String,
    handle_uri: String,
    namespace_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_uri: Option<String>,
    resolver_state: String,
    resolver: String,
    cache_policy: String,
    sync_policy: String,
    readonly: bool,
    access_policy: String,
    kind: String,
    traversable: bool,
    size: u64,
    object_id: String,
    head_id: String,
    cache_state: String,
    sync_state: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    forked_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_step: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSpaceMount {
    #[serde(default = "mount_record_schema")]
    schema: String,
    moniker: String,
    target_uri: String,
    #[serde(default)]
    namespace_uri: Option<String>,
    #[serde(default = "default_external_resolver")]
    resolver: String,
    #[serde(default = "default_true")]
    readonly: bool,
    #[serde(default = "default_access_policy")]
    access_policy: String,
    #[serde(default = "default_cache_policy")]
    cache_policy: String,
    #[serde(default = "default_sync_policy")]
    sync_policy: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    forked_from: Option<String>,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    updated_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct MountTable {
    #[serde(default = "mount_table_schema")]
    schema: String,
    #[serde(default)]
    mounts: Vec<WebSpaceMount>,
}

#[derive(Debug, Clone, Deserialize)]
struct WebSpaceIndexInput {
    path: String,
    kind: String,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    resolver_state: Option<String>,
    #[serde(default)]
    readonly: Option<bool>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSpaceIndexEntry {
    #[serde(default = "index_entry_schema")]
    schema: String,
    moniker: String,
    path: String,
    name: String,
    kind: String,
    target_uri: String,
    resolver: String,
    resolver_state: String,
    readonly: bool,
    description: String,
    updated_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexTable {
    #[serde(default = "index_table_schema")]
    schema: String,
    #[serde(default)]
    entries: Vec<WebSpaceIndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSpaceHead {
    #[serde(default = "head_record_schema")]
    schema: String,
    handle_uri: String,
    #[serde(default)]
    target_uri: Option<String>,
    moniker: String,
    resolver: String,
    object_id: String,
    head_id: String,
    revision: String,
    readonly: bool,
    access_policy: String,
    cache_policy: String,
    sync_policy: String,
    cache_state: String,
    sync_state: String,
    #[serde(default)]
    forked_from: Option<String>,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    last_cached_at: Option<u64>,
    #[serde(default)]
    last_synced_at: Option<u64>,
    dirty: bool,
    status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSpaceObject {
    #[serde(default = "object_record_schema")]
    schema: String,
    moniker: String,
    path: String,
    name: String,
    kind: String,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    content: Vec<u8>,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    dirty: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ObjectTable {
    #[serde(default = "object_table_schema")]
    schema: String,
    #[serde(default)]
    objects: Vec<WebSpaceObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSpaceAdapter {
    #[serde(default = "adapter_record_schema")]
    schema: String,
    resolver: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    endpoint_uri: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default = "default_adapter_state")]
    state: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default = "default_true")]
    readonly_default: bool,
    #[serde(default)]
    description: String,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    last_checked_at: Option<u64>,
    #[serde(default)]
    last_check_result: Option<String>,
    #[serde(default)]
    last_check_error_code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AdapterTable {
    #[serde(default = "adapter_table_schema")]
    schema: String,
    #[serde(default)]
    adapters: Vec<WebSpaceAdapter>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HeadTable {
    #[serde(default = "head_table_schema")]
    schema: String,
    #[serde(default)]
    heads: Vec<WebSpaceHead>,
}

#[derive(Debug, Default, Deserialize)]
struct InitConfig {
    #[serde(default)]
    base_path: String,
}

#[derive(Debug, Default)]
struct ProviderState {
    base_path: Option<PathBuf>,
    mounts: Vec<WebSpaceMount>,
    index_entries: Vec<WebSpaceIndexEntry>,
    heads: Vec<WebSpaceHead>,
    objects: Vec<WebSpaceObject>,
    adapters: Vec<WebSpaceAdapter>,
}

#[derive(Debug, Clone)]
enum ResolvedPath {
    Root,
    Handle { handle: WebSpaceHandle },
    Meta { handle: WebSpaceHandle },
}

fn mount_record_schema() -> String {
    MOUNT_RECORD_SCHEMA.to_string()
}

fn mount_table_schema() -> String {
    MOUNT_TABLE_SCHEMA.to_string()
}

fn index_entry_schema() -> String {
    INDEX_ENTRY_SCHEMA.to_string()
}

fn index_table_schema() -> String {
    INDEX_TABLE_SCHEMA.to_string()
}

fn head_record_schema() -> String {
    HEAD_RECORD_SCHEMA.to_string()
}

fn head_table_schema() -> String {
    HEAD_TABLE_SCHEMA.to_string()
}

fn object_record_schema() -> String {
    OBJECT_RECORD_SCHEMA.to_string()
}

fn object_table_schema() -> String {
    OBJECT_TABLE_SCHEMA.to_string()
}

fn adapter_record_schema() -> String {
    ADAPTER_RECORD_SCHEMA.to_string()
}

fn adapter_table_schema() -> String {
    ADAPTER_TABLE_SCHEMA.to_string()
}

fn default_external_resolver() -> String {
    DEFAULT_EXTERNAL_RESOLVER.to_string()
}

fn default_cache_policy() -> String {
    DEFAULT_CACHE_POLICY.to_string()
}

fn default_sync_policy() -> String {
    DEFAULT_SYNC_POLICY.to_string()
}

fn default_access_policy() -> String {
    DEFAULT_READONLY_ACCESS_POLICY.to_string()
}

fn default_adapter_state() -> String {
    DEFAULT_ADAPTER_STATE.to_string()
}

fn default_true() -> bool {
    true
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn infer_namespace_uri(target_uri: &str) -> Option<String> {
    let (scheme, _) = target_uri.split_once("://")?;
    if scheme.trim().is_empty() {
        None
    } else {
        Some(format!("{}://", scheme.trim()))
    }
}

fn append_target_uri(target_uri: &str, parts: &[&str]) -> String {
    let suffix = parts
        .iter()
        .map(|part| part.trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if suffix.is_empty() {
        target_uri.to_string()
    } else {
        format!("{}/{}", target_uri.trim_end_matches('/'), suffix)
    }
}

fn stable_hex(input: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn object_id_for(handle: &WebSpaceHandle) -> String {
    format!(
        "object:webspace:{}",
        stable_hex(handle.target_uri.as_deref().unwrap_or(&handle.handle_uri))
    )
}

fn head_id_for(handle_uri: &str) -> String {
    format!("head:webspace:{}", stable_hex(handle_uri))
}

fn revision_for(handle: &WebSpaceHandle, updated_at: u64) -> String {
    format!(
        "rev:webspace:{}",
        stable_hex(&format!(
            "{}:{}:{}:{}",
            handle.handle_uri,
            handle.target_uri.as_deref().unwrap_or(""),
            handle.resolver,
            updated_at
        ))
    )
}

fn render_external_descriptor_size(handle_uri: &str, target_uri: Option<&str>, kind: &str) -> u64 {
    serde_json::to_vec(&serde_json::json!({
        "handle_uri": handle_uri,
        "target_uri": target_uri,
        "kind": kind,
    }))
    .map(|bytes| bytes.len() as u64)
    .unwrap_or(0)
}

fn cache_state_for(policy: &str, last_cached_at: Option<u64>) -> String {
    if policy == "none" {
        "cache_disabled".to_string()
    } else if last_cached_at.is_some() {
        "metadata_cached".to_string()
    } else {
        "not_cached".to_string()
    }
}

fn sync_state_for(policy: &str, dirty: bool, last_synced_at: Option<u64>) -> String {
    match (policy, dirty, last_synced_at) {
        ("none", _, _) => "sync_disabled".to_string(),
        ("manual", true, _) => "manual_pending".to_string(),
        ("manual", false, Some(_)) => "manual_synced".to_string(),
        ("manual", false, None) => "manual_idle".to_string(),
        (_, true, _) => "sync_pending".to_string(),
        (_, false, Some(_)) => "synced".to_string(),
        _ => "sync_idle".to_string(),
    }
}

fn valid_moniker(moniker: &str) -> bool {
    let trimmed = moniker.trim();
    !trimmed.is_empty()
        && !trimmed.contains('/')
        && !trimmed.contains('\\')
        && !trimmed.contains("://")
        && trimmed != "."
        && trimmed != ".."
}

impl ProviderState {
    fn configure(&mut self, config: serde_json::Value) -> Result<serde_json::Value, String> {
        let config: InitConfig =
            serde_json::from_value(config).map_err(|err| format!("invalid init config: {err}"))?;
        self.base_path = if config.base_path.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(config.base_path.trim()))
        };
        self.mounts = self.load_mounts()?;
        self.index_entries = self.load_indexes()?;
        self.heads = self.load_heads()?;
        self.objects = self.load_objects()?;
        self.adapters = self.load_adapters()?;
        Ok(init_payload(self))
    }

    fn mount_table_path(&self) -> Option<PathBuf> {
        self.base_path.as_ref().map(|base| {
            base.join("ElastOS")
                .join("SystemServices")
                .join("WebSpaces")
                .join("mounts.json")
        })
    }

    fn head_table_path(&self) -> Option<PathBuf> {
        self.base_path.as_ref().map(|base| {
            base.join("ElastOS")
                .join("SystemServices")
                .join("WebSpaces")
                .join("heads.json")
        })
    }

    fn index_table_path(&self) -> Option<PathBuf> {
        self.base_path.as_ref().map(|base| {
            base.join("ElastOS")
                .join("SystemServices")
                .join("WebSpaces")
                .join("indexes.json")
        })
    }

    fn object_table_path(&self) -> Option<PathBuf> {
        self.base_path.as_ref().map(|base| {
            base.join("ElastOS")
                .join("SystemServices")
                .join("WebSpaces")
                .join("objects.json")
        })
    }

    fn adapter_table_path(&self) -> Option<PathBuf> {
        self.base_path.as_ref().map(|base| {
            base.join("ElastOS")
                .join("SystemServices")
                .join("WebSpaces")
                .join("adapters.json")
        })
    }

    fn load_mounts(&self) -> Result<Vec<WebSpaceMount>, String> {
        let Some(path) = self.mount_table_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|err| {
            format!(
                "failed to read WebSpace mount table {}: {err}",
                path.display()
            )
        })?;
        let table: MountTable = serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid WebSpace mount table {}: {err}", path.display()))?;
        table
            .mounts
            .into_iter()
            .map(normalize_mount_record)
            .collect()
    }

    fn save_mounts(&self) -> Result<(), String> {
        let Some(path) = self.mount_table_path() else {
            return Err(
                "webspace-provider was not initialized with a persistent base_path".to_string(),
            );
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create WebSpace mount table directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        let table = MountTable {
            schema: MOUNT_TABLE_SCHEMA.to_string(),
            mounts: self.mounts.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|err| format!("failed to serialize WebSpace mount table: {err}"))?;
        write_json_atomic(&path, &bytes)
    }

    fn load_indexes(&self) -> Result<Vec<WebSpaceIndexEntry>, String> {
        let Some(path) = self.index_table_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|err| {
            format!(
                "failed to read WebSpace index table {}: {err}",
                path.display()
            )
        })?;
        let table: IndexTable = serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid WebSpace index table {}: {err}", path.display()))?;
        table
            .entries
            .into_iter()
            .map(normalize_index_record)
            .collect()
    }

    fn save_indexes(&self) -> Result<(), String> {
        let Some(path) = self.index_table_path() else {
            return Err(
                "webspace-provider was not initialized with a persistent base_path".to_string(),
            );
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create WebSpace index table directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        let table = IndexTable {
            schema: INDEX_TABLE_SCHEMA.to_string(),
            entries: self.index_entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|err| format!("failed to serialize WebSpace index table: {err}"))?;
        write_json_atomic(&path, &bytes)
    }

    fn load_heads(&self) -> Result<Vec<WebSpaceHead>, String> {
        let Some(path) = self.head_table_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|err| {
            format!(
                "failed to read WebSpace head table {}: {err}",
                path.display()
            )
        })?;
        let table: HeadTable = serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid WebSpace head table {}: {err}", path.display()))?;
        Ok(table.heads.into_iter().map(normalize_head_record).collect())
    }

    fn save_heads(&self) -> Result<(), String> {
        let Some(path) = self.head_table_path() else {
            return Err(
                "webspace-provider was not initialized with a persistent base_path".to_string(),
            );
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create WebSpace head table directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        let table = HeadTable {
            schema: HEAD_TABLE_SCHEMA.to_string(),
            heads: self.heads.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|err| format!("failed to serialize WebSpace head table: {err}"))?;
        write_json_atomic(&path, &bytes)
    }

    fn load_objects(&self) -> Result<Vec<WebSpaceObject>, String> {
        let Some(path) = self.object_table_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|err| {
            format!(
                "failed to read WebSpace object table {}: {err}",
                path.display()
            )
        })?;
        let table: ObjectTable = serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid WebSpace object table {}: {err}", path.display()))?;
        table
            .objects
            .into_iter()
            .map(normalize_object_record)
            .collect()
    }

    fn save_objects(&self) -> Result<(), String> {
        let Some(path) = self.object_table_path() else {
            return Err(
                "webspace-provider was not initialized with a persistent base_path".to_string(),
            );
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create WebSpace object table directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        let table = ObjectTable {
            schema: OBJECT_TABLE_SCHEMA.to_string(),
            objects: self.objects.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|err| format!("failed to serialize WebSpace object table: {err}"))?;
        write_json_atomic(&path, &bytes)
    }

    fn load_adapters(&self) -> Result<Vec<WebSpaceAdapter>, String> {
        let Some(path) = self.adapter_table_path() else {
            return Ok(Vec::new());
        };
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path).map_err(|err| {
            format!(
                "failed to read WebSpace adapter table {}: {err}",
                path.display()
            )
        })?;
        let table: AdapterTable = serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid WebSpace adapter table {}: {err}", path.display()))?;
        table
            .adapters
            .into_iter()
            .map(normalize_adapter_record)
            .collect()
    }

    fn save_adapters(&self) -> Result<(), String> {
        let Some(path) = self.adapter_table_path() else {
            return Err(
                "webspace-provider was not initialized with a persistent base_path".to_string(),
            );
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "failed to create WebSpace adapter table directory {}: {err}",
                    parent.display()
                )
            })?;
        }
        let table = AdapterTable {
            schema: ADAPTER_TABLE_SCHEMA.to_string(),
            adapters: self.adapters.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&table)
            .map_err(|err| format!("failed to serialize WebSpace adapter table: {err}"))?;
        write_json_atomic(&path, &bytes)
    }

    fn upsert_head_for_handle(
        &mut self,
        handle: &WebSpaceHandle,
        status: &str,
        dirty: bool,
    ) -> Result<WebSpaceHead, String> {
        self.upsert_lifecycle_head_for_handle(handle, status, dirty, false, false)
    }

    fn upsert_lifecycle_head_for_handle(
        &mut self,
        handle: &WebSpaceHandle,
        status: &str,
        dirty: bool,
        refresh_cache: bool,
        mark_synced: bool,
    ) -> Result<WebSpaceHead, String> {
        let now = now_unix_secs();
        let existing = self
            .heads
            .iter()
            .find(|head| head.handle_uri == handle.handle_uri)
            .cloned();
        let created_at = existing.as_ref().map(|head| head.created_at).unwrap_or(now);
        let last_cached_at = if handle.cache_policy == "none" {
            None
        } else if refresh_cache {
            Some(now)
        } else {
            existing
                .as_ref()
                .and_then(|head| head.last_cached_at)
                .or(Some(now))
        };
        let last_synced_at = if handle.sync_policy == "none" {
            None
        } else if mark_synced {
            Some(now)
        } else {
            existing.as_ref().and_then(|head| head.last_synced_at)
        };
        let dirty = if mark_synced { false } else { dirty };
        let head = WebSpaceHead {
            schema: HEAD_RECORD_SCHEMA.to_string(),
            handle_uri: handle.handle_uri.clone(),
            target_uri: handle.target_uri.clone(),
            moniker: handle.moniker.clone(),
            resolver: handle.resolver.clone(),
            object_id: object_id_for(handle),
            head_id: head_id_for(&handle.handle_uri),
            revision: revision_for(handle, now),
            readonly: handle.readonly,
            access_policy: handle.access_policy.clone(),
            cache_policy: handle.cache_policy.clone(),
            sync_policy: handle.sync_policy.clone(),
            cache_state: cache_state_for(&handle.cache_policy, last_cached_at),
            sync_state: sync_state_for(&handle.sync_policy, dirty, last_synced_at),
            forked_from: handle.forked_from.clone(),
            created_at,
            updated_at: now,
            last_cached_at,
            last_synced_at,
            dirty,
            status: status.to_string(),
        };
        if self.head_table_path().is_some() {
            self.heads
                .retain(|existing| existing.handle_uri != head.handle_uri);
            self.heads.push(head.clone());
            self.heads
                .sort_by(|left, right| left.handle_uri.cmp(&right.handle_uri));
            self.save_heads()?;
        }
        Ok(head)
    }

    fn refresh_handle(
        &mut self,
        path: String,
        entries: Option<Vec<WebSpaceIndexInput>>,
    ) -> Result<serde_json::Value, String> {
        let handle = handle_from_resolved_path(resolve_path(self, &rooted_webspace_path(&path))?)?;
        let indexed_entries = if let Some(entries) = entries {
            if handle.moniker == BUILTIN_MONIKER {
                return Err(
                    "built-in Elastos WebSpace does not accept external resolver indexes"
                        .to_string(),
                );
            }
            let mount_root = format!("localhost://WebSpaces/{}", handle.moniker);
            if handle.handle_uri != mount_root {
                return Err(format!(
                    "resolver index refresh must target the mounted WebSpace root: {mount_root}"
                ));
            }
            let refreshed = self.replace_index(&handle.moniker, entries)?;
            Some(refreshed)
        } else {
            None
        };
        let head = self.upsert_lifecycle_head_for_handle(
            &handle,
            "resolver_refreshed",
            false,
            true,
            false,
        )?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.refresh-receipt/v1",
            "action": "refreshed",
            "handle_uri": handle.handle_uri,
            "head": head,
            "index_entry_count": indexed_entries.as_ref().map(|entries| entries.len()),
            "entries": indexed_entries,
            "byte_materialized": false,
            "note": "Resolver metadata was refreshed. Remote bytes still require a resolver/cache worker."
        }))
    }

    fn cache_handle(
        &mut self,
        path: String,
        content: Option<Vec<u8>>,
        mime: Option<String>,
        source_receipt: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let handle = handle_from_resolved_path(resolve_path(self, &rooted_webspace_path(&path))?)?;
        if let Some(content) = content {
            return self.cache_content_handle(handle, content, mime, source_receipt);
        }
        let dirty = self
            .heads
            .iter()
            .find(|head| head.handle_uri == handle.handle_uri)
            .map(|head| head.dirty)
            .unwrap_or(false);
        let head =
            self.upsert_lifecycle_head_for_handle(&handle, "metadata_cached", dirty, true, false)?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.cache-receipt/v1",
            "action": "metadata_cached",
            "handle_uri": handle.handle_uri,
            "head": head,
            "content_cached": false,
            "note": "Metadata cache was refreshed. Content bytes remain resolver-owned."
        }))
    }

    fn cache_content_handle(
        &mut self,
        handle: WebSpaceHandle,
        content: Vec<u8>,
        mime: Option<String>,
        source_receipt: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        if handle.traversable {
            return Err(format!(
                "WebSpace content cache requires a file handle, not directory: {}",
                handle.handle_uri
            ));
        }
        if handle.kind == "metadata" {
            return Err("WebSpace metadata handles cannot be byte-cached".to_string());
        }
        let record = self
            .mount_by_moniker(&handle.moniker)
            .cloned()
            .ok_or_else(|| format!("unknown WebSpace moniker: {}", handle.moniker))?;
        if record.moniker == BUILTIN_MONIKER {
            return Err(
                "built-in Elastos WebSpace content is resolved by Runtime content providers, not cached here"
                    .to_string(),
            );
        }
        let parts = handle_index_parts(&handle);
        if parts.is_empty() {
            return Err("WebSpace content cache requires a child object path".to_string());
        }
        if parts.iter().any(|part| part == "_meta.json") {
            return Err("WebSpace metadata files cannot be byte-cached".to_string());
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(object) = self.exact_object(&record.moniker, &refs) {
            if object.kind == "directory" {
                return Err(format!(
                    "cannot byte-cache over WebSpace directory: {}",
                    object.path
                ));
            }
        }
        if let Some(entry) = self.exact_index_entry(&record.moniker, &refs) {
            if entry.kind == "directory" {
                return Err(format!(
                    "cannot byte-cache over indexed WebSpace directory: {}",
                    entry.path
                ));
            }
        }
        let now = now_unix_secs();
        let existing = self.exact_object(&record.moniker, &refs);
        let object = WebSpaceObject {
            schema: OBJECT_RECORD_SCHEMA.to_string(),
            moniker: record.moniker.clone(),
            path: normalized_index_path(&refs)?,
            name: parts.last().cloned().unwrap_or_default(),
            kind: "file".to_string(),
            target_uri: handle
                .target_uri
                .clone()
                .or_else(|| Some(append_target_uri(&record.target_uri, &refs))),
            mime: mime
                .map(|mime| mime.trim().to_string())
                .filter(|mime| !mime.is_empty())
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            content,
            created_at: existing
                .as_ref()
                .map(|object| object.created_at)
                .unwrap_or(now),
            updated_at: now,
            revision: String::new(),
            dirty: false,
        };
        let object = self.upsert_object(object)?;
        let cached = materialized_object_handle(&record, &object);
        let mut head = self.upsert_lifecycle_head_for_handle(
            &cached,
            "materialized_cached",
            false,
            true,
            false,
        )?;
        head.cache_state = "content_cached".to_string();
        self.heads
            .retain(|existing| existing.handle_uri != head.handle_uri);
        self.heads.push(head.clone());
        self.heads
            .sort_by(|left, right| left.handle_uri.cmp(&right.handle_uri));
        self.save_heads()?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.cache-receipt/v1",
            "action": "content_cached",
            "handle_uri": cached.handle_uri,
            "object": cached,
            "head": head,
            "content_cached": true,
            "dirty": false,
            "source_receipt": source_receipt,
            "note": "Resolver bytes were cached as clean provider-owned local content. This does not grant mutable sync authority."
        }))
    }

    fn sync_handle(&mut self, path: String) -> Result<serde_json::Value, String> {
        let handle = handle_from_resolved_path(resolve_path(self, &rooted_webspace_path(&path))?)?;
        let head =
            self.upsert_lifecycle_head_for_handle(&handle, "metadata_synced", false, true, true)?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.sync-receipt/v1",
            "action": "metadata_synced",
            "handle_uri": handle.handle_uri,
            "head": head,
            "content_synced": false,
            "note": "Provider-owned metadata/fork head is synced. Content-byte sync requires a resolver/sync worker."
        }))
    }

    fn mutable_mount_for_path(&self, path: &str) -> Result<(WebSpaceMount, Vec<String>), String> {
        let rooted = rooted_webspace_path(path);
        let (root, rest) = parse_localhost_uri(&rooted)
            .or_else(|| parse_localhost_path(&rooted))
            .ok_or_else(|| format!("invalid WebSpace path: {path}"))?;
        if root != "WebSpaces" {
            return Err(format!(
                "WebSpace path must be under localhost://WebSpaces: {path}"
            ));
        }
        let raw_parts = rest
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let Some(moniker) = raw_parts.first().map(|part| normalize_moniker(part)) else {
            return Err("WebSpace mutation requires a mounted WebSpace moniker".to_string());
        };
        if moniker == BUILTIN_MONIKER {
            return Err("built-in Elastos WebSpace is resolver-owned and read-only".to_string());
        }
        let record = self
            .mount_by_moniker(&moniker)
            .cloned()
            .ok_or_else(|| format!("unknown WebSpace moniker: {moniker}"))?;
        if record.readonly {
            return Err(format!(
                "WebSpace {} is resolver-owned/read-only; fork or mount it as mutable first",
                record.moniker
            ));
        }
        let object_parts = raw_parts
            .iter()
            .skip(1)
            .map(|part| part.trim_matches('/'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if object_parts.is_empty() {
            return Err("WebSpace mutation requires a child object path".to_string());
        }
        if object_parts.iter().any(|part| *part == "_meta.json") {
            return Err(
                "WebSpace metadata files are provider-owned and cannot be mutated".to_string(),
            );
        }
        let normalized = normalized_index_parts(&object_parts.join("/"))?;
        Ok((record, normalized))
    }

    fn directory_exists_for_parts(
        &self,
        record: &WebSpaceMount,
        parts: &[String],
    ) -> Result<bool, String> {
        if parts.is_empty() {
            return Ok(true);
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(object) = self.exact_object(&record.moniker, &refs) {
            return Ok(object.kind == "directory");
        }
        if let Some(entry) = self.exact_index_entry(&record.moniker, &refs) {
            return Ok(entry.kind == "directory");
        }
        Ok(self.has_object_children(&record.moniker, &refs)
            || self.has_index_children(&record.moniker, &refs))
    }

    fn ensure_parent_directory(
        &self,
        record: &WebSpaceMount,
        parts: &[String],
    ) -> Result<(), String> {
        if parts.is_empty() {
            return Ok(());
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(object) = self.exact_object(&record.moniker, &refs) {
            if object.kind == "directory" {
                return Ok(());
            }
            return Err(format!(
                "WebSpace parent is a materialized file, not a directory: {}",
                parts.join("/")
            ));
        }
        if let Some(entry) = self.exact_index_entry(&record.moniker, &refs) {
            if entry.kind == "directory" {
                return Ok(());
            }
            return Err(format!(
                "WebSpace parent is an indexed file, not a directory: {}",
                parts.join("/")
            ));
        }
        if self.has_object_children(&record.moniker, &refs)
            || self.has_index_children(&record.moniker, &refs)
        {
            return Ok(());
        }
        Err(format!(
            "WebSpace parent directory does not exist locally or in the resolver index: {}",
            parts.join("/")
        ))
    }

    fn ensure_parent_objects(
        &mut self,
        record: &WebSpaceMount,
        parent_parts: &[String],
    ) -> Result<(), String> {
        for depth in 1..=parent_parts.len() {
            let current = parent_parts[..depth].to_vec();
            if self.directory_exists_for_parts(record, &current)? {
                continue;
            }
            let refs = current.iter().map(String::as_str).collect::<Vec<_>>();
            let now = now_unix_secs();
            let object = WebSpaceObject {
                schema: OBJECT_RECORD_SCHEMA.to_string(),
                moniker: record.moniker.clone(),
                path: normalized_index_path(&refs)?,
                name: current.last().cloned().unwrap_or_default(),
                kind: "directory".to_string(),
                target_uri: Some(append_target_uri(&record.target_uri, &refs)),
                mime: "inode/directory".to_string(),
                content: Vec::new(),
                created_at: now,
                updated_at: now,
                revision: String::new(),
                dirty: true,
            };
            self.upsert_object(object)?;
        }
        Ok(())
    }

    fn write_handle(
        &mut self,
        path: String,
        content: Vec<u8>,
        append: bool,
    ) -> Result<serde_json::Value, String> {
        let (record, parts) = self.mutable_mount_for_path(&path)?;
        let parent_parts = parts[..parts.len().saturating_sub(1)].to_vec();
        self.ensure_parent_directory(&record, &parent_parts)?;
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(object) = self.exact_object(&record.moniker, &refs) {
            if object.kind == "directory" {
                return Err(format!(
                    "cannot write bytes over WebSpace directory: {}",
                    object.path
                ));
            }
        }
        if let Some(entry) = self.exact_index_entry(&record.moniker, &refs) {
            if entry.kind == "directory" {
                return Err(format!(
                    "cannot write bytes over indexed WebSpace directory: {}",
                    entry.path
                ));
            }
        }
        let existing = self.exact_object(&record.moniker, &refs);
        let now = now_unix_secs();
        let mut bytes = if append {
            existing
                .as_ref()
                .map(|object| object.content.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        bytes.extend(content);
        let object = WebSpaceObject {
            schema: OBJECT_RECORD_SCHEMA.to_string(),
            moniker: record.moniker.clone(),
            path: normalized_index_path(&refs)?,
            name: parts.last().cloned().unwrap_or_default(),
            kind: "file".to_string(),
            target_uri: Some(append_target_uri(&record.target_uri, &refs)),
            mime: "application/octet-stream".to_string(),
            content: bytes,
            created_at: existing
                .as_ref()
                .map(|object| object.created_at)
                .unwrap_or(now),
            updated_at: now,
            revision: String::new(),
            dirty: true,
        };
        let object = self.upsert_object(object)?;
        let handle = materialized_object_handle(&record, &object);
        let head = self.upsert_head_for_handle(&handle, "materialized_local", true)?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.write-receipt/v1",
            "action": if append { "appended" } else { "written" },
            "handle_uri": handle.handle_uri,
            "object": handle,
            "head": head,
            "byte_materialized": true,
        }))
    }

    fn mkdir_handle(&mut self, path: String, parents: bool) -> Result<serde_json::Value, String> {
        let (record, parts) = self.mutable_mount_for_path(&path)?;
        let parent_parts = parts[..parts.len().saturating_sub(1)].to_vec();
        if parents {
            self.ensure_parent_objects(&record, &parent_parts)?;
        } else {
            self.ensure_parent_directory(&record, &parent_parts)?;
        }
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(object) = self.exact_object(&record.moniker, &refs) {
            if object.kind != "directory" {
                return Err(format!(
                    "WebSpace object already exists as a file: {}",
                    object.path
                ));
            }
            let handle = materialized_object_handle(&record, &object);
            return Ok(serde_json::json!({
                "schema": "elastos.webspace.mkdir-receipt/v1",
                "action": "exists",
                "handle_uri": handle.handle_uri,
                "object": handle,
            }));
        }
        if let Some(entry) = self.exact_index_entry(&record.moniker, &refs) {
            if entry.kind != "directory" {
                return Err(format!(
                    "WebSpace indexed object already exists as a file: {}",
                    entry.path
                ));
            }
        }
        let now = now_unix_secs();
        let object = WebSpaceObject {
            schema: OBJECT_RECORD_SCHEMA.to_string(),
            moniker: record.moniker.clone(),
            path: normalized_index_path(&refs)?,
            name: parts.last().cloned().unwrap_or_default(),
            kind: "directory".to_string(),
            target_uri: Some(append_target_uri(&record.target_uri, &refs)),
            mime: "inode/directory".to_string(),
            content: Vec::new(),
            created_at: now,
            updated_at: now,
            revision: String::new(),
            dirty: true,
        };
        let object = self.upsert_object(object)?;
        let handle = materialized_object_handle(&record, &object);
        let head = self.upsert_head_for_handle(&handle, "materialized_directory", true)?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.mkdir-receipt/v1",
            "action": "created",
            "handle_uri": handle.handle_uri,
            "object": handle,
            "head": head,
        }))
    }

    fn delete_handle(
        &mut self,
        path: String,
        recursive: bool,
    ) -> Result<serde_json::Value, String> {
        let (record, parts) = self.mutable_mount_for_path(&path)?;
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        let handle = self
            .exact_object(&record.moniker, &refs)
            .map(|object| materialized_object_handle(&record, &object))
            .unwrap_or_else(|| materialized_virtual_folder_handle(&record, &refs));
        let removed = self.remove_objects(&record.moniker, &parts, recursive)?;
        let head = self.upsert_head_for_handle(&handle, "materialized_deleted", true)?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.delete-receipt/v1",
            "action": "deleted",
            "handle_uri": handle.handle_uri,
            "removed_count": removed.len(),
            "removed": removed,
            "head": head,
        }))
    }

    fn health_report(&self, moniker: Option<String>) -> Result<serde_json::Value, String> {
        let handles = known_mounts(self);
        let filtered = if let Some(moniker) = moniker {
            let moniker = normalize_moniker(&moniker);
            let matched = handles
                .into_iter()
                .filter(|handle| handle.moniker == moniker)
                .collect::<Vec<_>>();
            if matched.is_empty() {
                return Err(format!("unknown WebSpace moniker: {moniker}"));
            }
            matched
        } else {
            handles
        };
        let included_monikers = filtered
            .iter()
            .map(|handle| handle.moniker.as_str())
            .collect::<Vec<_>>();
        let includes_moniker = |moniker: &str| {
            included_monikers
                .iter()
                .any(|candidate| *candidate == moniker)
        };
        let mounts = filtered
            .iter()
            .map(|handle| self.health_for_handle(handle))
            .collect::<Vec<_>>();
        let index_entry_count = self
            .index_entries
            .iter()
            .filter(|entry| includes_moniker(&entry.moniker))
            .count();
        let head_count = self
            .heads
            .iter()
            .filter(|head| includes_moniker(&head.moniker))
            .count();
        let dirty_head_count = self
            .heads
            .iter()
            .filter(|head| includes_moniker(&head.moniker) && head.dirty)
            .count();
        let object_count = self
            .objects
            .iter()
            .filter(|object| includes_moniker(&object.moniker))
            .count();
        let user_mount_count = self
            .mounts
            .iter()
            .filter(|mount| includes_moniker(&mount.moniker))
            .count();
        let live_adapter_count = mounts
            .iter()
            .filter(|mount| mount["live_adapter"].as_bool().unwrap_or(false))
            .count();
        let configured_adapter_count = self.adapters.len();
        let connected_adapter_count = self
            .adapters
            .iter()
            .filter(|adapter| adapter.state == "connected")
            .count();
        let checked_adapter_count = self
            .adapters
            .iter()
            .filter(|adapter| adapter.last_checked_at.is_some())
            .count();
        let state = if mounts.iter().any(|mount| {
            matches!(
                mount["state"].as_str(),
                Some("dirty") | Some("mounted_no_index")
            )
        }) {
            "attention"
        } else {
            "metadata_ready"
        };
        Ok(serde_json::json!({
            "schema": "elastos.webspace.health/v1",
            "provider": PROVIDER_ID,
            "state": state,
            "persistent": self.mount_table_path().is_some(),
            "mount_count": mounts.len(),
            "user_mount_count": user_mount_count,
            "index_entry_count": index_entry_count,
            "head_count": head_count,
            "dirty_head_count": dirty_head_count,
            "object_count": object_count,
            "live_adapter_count": live_adapter_count,
            "configured_adapter_count": configured_adapter_count,
            "connected_adapter_count": connected_adapter_count,
            "checked_adapter_count": checked_adapter_count,
            "adapters": self.adapter_summary_table(),
            "mounts": mounts,
            "note": "Health reports resolver metadata readiness, dirty heads, registered adapter state, and safe adapter liveness checks. Remote byte availability still requires connected resolver/cache workers."
        }))
    }

    fn health_for_handle(&self, handle: &WebSpaceHandle) -> serde_json::Value {
        let index_entry_count = self
            .index_entries
            .iter()
            .filter(|entry| entry.moniker == handle.moniker)
            .count();
        let head_count = self
            .heads
            .iter()
            .filter(|head| head.moniker == handle.moniker)
            .count();
        let dirty_head_count = self
            .heads
            .iter()
            .filter(|head| head.moniker == handle.moniker && head.dirty)
            .count();
        let object_count = self
            .objects
            .iter()
            .filter(|object| object.moniker == handle.moniker)
            .count();
        let (live_adapter, adapter_state, adapter) = self.adapter_state_for(&handle.resolver);
        let adapter_next_step = adapter_next_step(
            &handle.resolver,
            &adapter_state,
            index_entry_count,
            live_adapter,
        );
        let state = if dirty_head_count > 0 {
            "dirty"
        } else if live_adapter || index_entry_count > 0 || head_count > 0 || object_count > 0 {
            "metadata_ready"
        } else {
            "mounted_no_index"
        };
        serde_json::json!({
            "schema": "elastos.webspace.resolver-health/v1",
            "moniker": handle.moniker,
            "handle_uri": handle.handle_uri,
            "target_uri": handle.target_uri,
            "resolver": handle.resolver,
            "resolver_state": handle.resolver_state,
            "state": state,
            "live_adapter": live_adapter,
            "adapter_state": adapter_state,
            "adapter": adapter.as_ref().map(adapter_public_summary),
            "readonly": handle.readonly,
            "access_policy": handle.access_policy,
            "cache_policy": handle.cache_policy,
            "sync_policy": handle.sync_policy,
            "index_entry_count": index_entry_count,
            "head_count": head_count,
            "dirty_head_count": dirty_head_count,
            "object_count": object_count,
            "cache_state": handle.cache_state,
            "sync_state": handle.sync_state,
            "next_step": adapter_next_step
        })
    }

    fn user_mounts(&self) -> Vec<WebSpaceMount> {
        self.mounts.clone()
    }

    fn adapter_by_resolver(&self, resolver: &str) -> Option<&WebSpaceAdapter> {
        let resolver = normalize_resolver_id(resolver);
        self.adapters
            .iter()
            .find(|adapter| adapter.resolver == resolver)
    }

    fn adapter_state_for(&self, resolver: &str) -> (bool, String, Option<WebSpaceAdapter>) {
        let resolver = normalize_resolver_id(resolver);
        if resolver == "builtin" {
            return (true, "builtin".to_string(), None);
        }
        let Some(adapter) = self.adapter_by_resolver(&resolver).cloned() else {
            return (false, "not_registered".to_string(), None);
        };
        let live = adapter.state == "connected";
        (live, adapter.state.clone(), Some(adapter))
    }

    fn adapter_summary_table(&self) -> serde_json::Value {
        serde_json::json!({
            "schema": ADAPTER_TABLE_SCHEMA,
            "builtin": builtin_adapter_summary(),
            "adapters": self.adapters.iter().map(adapter_public_summary).collect::<Vec<_>>(),
            "configured_adapter_count": self.adapters.len(),
            "connected_adapter_count": self.adapters.iter().filter(|adapter| adapter.state == "connected").count(),
            "checked_adapter_count": self.adapters.iter().filter(|adapter| adapter.last_checked_at.is_some()).count(),
            "note": "Registered adapters describe resolver availability, liveness receipts, and operator policy. They do not expose credentials and do not by themselves grant remote byte access."
        })
    }

    fn upsert_adapter(
        &mut self,
        resolver: String,
        label: Option<String>,
        endpoint_uri: Option<String>,
        provider: Option<String>,
        state: Option<String>,
        capabilities: Vec<String>,
        readonly_default: Option<bool>,
        description: Option<String>,
    ) -> Result<WebSpaceAdapter, String> {
        let resolver = normalize_resolver_id(&resolver);
        let now = now_unix_secs();
        let existing = self
            .adapters
            .iter()
            .find(|adapter| adapter.resolver == resolver)
            .cloned();
        let created_at = existing
            .as_ref()
            .map(|adapter| adapter.created_at)
            .unwrap_or(now);
        let adapter = normalize_adapter_record(WebSpaceAdapter {
            schema: ADAPTER_RECORD_SCHEMA.to_string(),
            resolver,
            label: label
                .or_else(|| existing.as_ref().map(|adapter| adapter.label.clone()))
                .unwrap_or_default(),
            endpoint_uri: endpoint_uri.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|adapter| adapter.endpoint_uri.clone())
            }),
            provider: provider.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|adapter| adapter.provider.clone())
            }),
            state: state
                .or_else(|| existing.as_ref().map(|adapter| adapter.state.clone()))
                .unwrap_or_else(default_adapter_state),
            capabilities: if capabilities.is_empty() {
                existing
                    .as_ref()
                    .map(|adapter| adapter.capabilities.clone())
                    .unwrap_or_default()
            } else {
                capabilities
            },
            readonly_default: readonly_default
                .or_else(|| existing.as_ref().map(|adapter| adapter.readonly_default))
                .unwrap_or(true),
            description: description
                .or_else(|| existing.as_ref().map(|adapter| adapter.description.clone()))
                .unwrap_or_default(),
            created_at,
            updated_at: now,
            last_checked_at: existing
                .as_ref()
                .and_then(|adapter| adapter.last_checked_at),
            last_check_result: existing
                .as_ref()
                .and_then(|adapter| adapter.last_check_result.clone()),
            last_check_error_code: existing
                .as_ref()
                .and_then(|adapter| adapter.last_check_error_code.clone()),
        })?;
        self.adapters
            .retain(|existing| existing.resolver != adapter.resolver);
        self.adapters.push(adapter.clone());
        self.adapters
            .sort_by(|left, right| left.resolver.cmp(&right.resolver));
        self.save_adapters()?;
        Ok(adapter)
    }

    fn check_adapter(
        &mut self,
        resolver: String,
        result: Option<String>,
        state: Option<String>,
        error_code: Option<String>,
        capabilities: Vec<String>,
    ) -> Result<serde_json::Value, String> {
        let resolver = normalize_resolver_id(&resolver);
        if resolver == "builtin" {
            return Err("built-in WebSpace adapter health is provider-owned".to_string());
        }
        let index = self
            .adapters
            .iter()
            .position(|adapter| adapter.resolver == resolver)
            .ok_or_else(|| format!("unknown WebSpace adapter resolver: {resolver}"))?;
        let now = now_unix_secs();
        let previous = adapter_public_summary(&self.adapters[index]);
        let mut adapter = self.adapters[index].clone();
        let result = normalize_adapter_check_result(
            result
                .as_deref()
                .unwrap_or_else(|| default_check_result_for_state(&adapter.state)),
        )?;
        let next_state = match state {
            Some(state) => normalize_adapter_state(&state)?,
            None => adapter_state_for_check_result(&result, &adapter.state).to_string(),
        };
        let next_error_code = if result == "failed" {
            normalize_optional_error_code(error_code)?
                .or_else(|| Some("adapter_unavailable".to_string()))
        } else {
            None
        };
        adapter.state = next_state;
        if !capabilities.is_empty() {
            adapter.capabilities = normalize_adapter_capabilities(capabilities);
        }
        adapter.last_checked_at = Some(now);
        adapter.last_check_result = Some(result);
        adapter.last_check_error_code = next_error_code;
        adapter.updated_at = now;
        let adapter = normalize_adapter_record(adapter)?;
        self.adapters[index] = adapter.clone();
        self.adapters
            .sort_by(|left, right| left.resolver.cmp(&right.resolver));
        self.save_adapters()?;
        Ok(serde_json::json!({
            "schema": "elastos.webspace.adapter-health-receipt/v1",
            "action": "checked",
            "previous": previous,
            "adapter": adapter_public_summary(&adapter),
            "byte_traversal_enabled": false,
            "note": "Adapter health is a safe resolver-readiness receipt. It does not expose credentials and does not by itself grant remote byte access."
        }))
    }

    fn unregister_adapter(&mut self, resolver: &str) -> Result<WebSpaceAdapter, String> {
        let resolver = normalize_resolver_id(resolver);
        if resolver == "builtin" {
            return Err("built-in WebSpace adapter cannot be unregistered".to_string());
        }
        let index = self
            .adapters
            .iter()
            .position(|adapter| adapter.resolver == resolver)
            .ok_or_else(|| format!("unknown WebSpace adapter resolver: {resolver}"))?;
        let adapter = self.adapters.remove(index);
        self.save_adapters()?;
        Ok(adapter)
    }

    fn mount_by_moniker(&self, moniker: &str) -> Option<&WebSpaceMount> {
        self.mounts.iter().find(|mount| mount.moniker == moniker)
    }

    fn replace_index(
        &mut self,
        moniker: &str,
        entries: Vec<WebSpaceIndexInput>,
    ) -> Result<Vec<WebSpaceIndexEntry>, String> {
        let record = self
            .mount_by_moniker(moniker)
            .cloned()
            .ok_or_else(|| format!("unknown WebSpace moniker: {moniker}"))?;
        let now = now_unix_secs();
        let mut normalized = entries
            .into_iter()
            .map(|entry| normalize_index_input(&record, entry, now))
            .collect::<Result<Vec<_>, _>>()?;
        normalized.sort_by(|left, right| left.path.cmp(&right.path));
        normalized.dedup_by(|left, right| left.path == right.path);
        self.index_entries
            .retain(|entry| entry.moniker != record.moniker);
        self.index_entries.extend(normalized.clone());
        self.index_entries.sort_by(|left, right| {
            left.moniker
                .cmp(&right.moniker)
                .then_with(|| left.path.cmp(&right.path))
        });
        self.save_indexes()?;
        Ok(normalized)
    }

    fn exact_index_entry(&self, moniker: &str, parts: &[&str]) -> Option<WebSpaceIndexEntry> {
        let path = normalized_index_path(parts).ok()?;
        self.index_entries
            .iter()
            .find(|entry| entry.moniker == moniker && entry.path == path)
            .cloned()
    }

    fn has_index_children(&self, moniker: &str, parts: &[&str]) -> bool {
        immediate_index_children(&self.index_entries, moniker, parts)
            .next()
            .is_some()
    }

    fn exact_object(&self, moniker: &str, parts: &[&str]) -> Option<WebSpaceObject> {
        let path = normalized_index_path(parts).ok()?;
        self.objects
            .iter()
            .find(|object| object.moniker == moniker && object.path == path)
            .cloned()
    }

    fn object_for_handle(&self, handle: &WebSpaceHandle) -> Option<WebSpaceObject> {
        let parts = handle_index_parts(handle);
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        self.exact_object(&handle.moniker, &refs)
    }

    fn has_object_children(&self, moniker: &str, parts: &[&str]) -> bool {
        immediate_object_children(&self.objects, moniker, parts)
            .next()
            .is_some()
    }

    fn upsert_object(&mut self, object: WebSpaceObject) -> Result<WebSpaceObject, String> {
        let object = normalize_object_record(object)?;
        self.objects.retain(|existing| {
            !(existing.moniker == object.moniker && existing.path == object.path)
        });
        self.objects.push(object.clone());
        self.objects.sort_by(|left, right| {
            left.moniker
                .cmp(&right.moniker)
                .then_with(|| left.path.cmp(&right.path))
        });
        self.save_objects()?;
        Ok(object)
    }

    fn remove_objects(
        &mut self,
        moniker: &str,
        parts: &[String],
        recursive: bool,
    ) -> Result<Vec<WebSpaceObject>, String> {
        let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
        let path = normalized_index_path(&refs)?;
        let child_prefix = format!("{path}/");
        let has_children = self
            .objects
            .iter()
            .any(|object| object.moniker == moniker && object.path.starts_with(&child_prefix));
        if has_children && !recursive {
            return Err(format!(
                "WebSpace directory is not empty; retry delete with recursive=true: {path}"
            ));
        }
        let mut removed = Vec::new();
        self.objects.retain(|object| {
            let matched = object.moniker == moniker
                && (object.path == path || object.path.starts_with(&child_prefix));
            if matched {
                removed.push(object.clone());
                false
            } else {
                true
            }
        });
        if removed.is_empty() {
            return Err(format!(
                "WebSpace object is resolver-owned or does not exist locally: {path}"
            ));
        }
        self.save_objects()?;
        Ok(removed)
    }

    fn upsert_mount(
        &mut self,
        moniker: String,
        target_uri: String,
        namespace_uri: Option<String>,
        resolver: Option<String>,
        description: Option<String>,
        readonly: Option<bool>,
        cache_policy: Option<String>,
        sync_policy: Option<String>,
        access_policy: Option<String>,
    ) -> Result<WebSpaceMount, String> {
        self.upsert_mount_with_fork(
            moniker,
            target_uri,
            namespace_uri,
            resolver,
            description,
            readonly,
            cache_policy,
            sync_policy,
            access_policy,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_mount_with_fork(
        &mut self,
        moniker: String,
        target_uri: String,
        namespace_uri: Option<String>,
        resolver: Option<String>,
        description: Option<String>,
        readonly: Option<bool>,
        cache_policy: Option<String>,
        sync_policy: Option<String>,
        access_policy: Option<String>,
        forked_from: Option<String>,
    ) -> Result<WebSpaceMount, String> {
        let moniker = moniker.trim().to_string();
        if moniker == BUILTIN_MONIKER {
            return Err(format!(
                "{BUILTIN_MONIKER} is a built-in WebSpace and cannot be remounted"
            ));
        }
        if !valid_moniker(&moniker) {
            return Err("WebSpace moniker must be a non-empty single path segment".to_string());
        }
        let target_uri = target_uri.trim().trim_end_matches('/').to_string();
        if !target_uri.contains("://") {
            return Err("WebSpace target_uri must be a scheme-qualified URI".to_string());
        }
        let now = now_unix_secs();
        let readonly = readonly.unwrap_or(true);
        let existing_created_at = self
            .mounts
            .iter()
            .find(|mount| mount.moniker == moniker)
            .map(|mount| mount.created_at)
            .unwrap_or(now);
        let record = normalize_mount_record(WebSpaceMount {
            schema: MOUNT_RECORD_SCHEMA.to_string(),
            moniker,
            namespace_uri: namespace_uri.or_else(|| infer_namespace_uri(&target_uri)),
            target_uri,
            resolver: resolver.unwrap_or_else(default_external_resolver),
            readonly,
            access_policy: normalized_access_policy(access_policy.as_deref(), readonly),
            cache_policy: cache_policy.unwrap_or_else(default_cache_policy),
            sync_policy: sync_policy.unwrap_or_else(default_sync_policy),
            description: description.unwrap_or_default(),
            forked_from,
            created_at: existing_created_at,
            updated_at: now,
        })?;
        self.mounts.retain(|mount| mount.moniker != record.moniker);
        self.mounts.push(record.clone());
        self.mounts
            .sort_by(|left, right| left.moniker.cmp(&right.moniker));
        self.save_mounts()?;
        Ok(record)
    }

    #[allow(clippy::too_many_arguments)]
    fn fork_mount(
        &mut self,
        source_uri: String,
        moniker: String,
        target_uri: Option<String>,
        resolver: Option<String>,
        description: Option<String>,
        readonly: Option<bool>,
        cache_policy: Option<String>,
        sync_policy: Option<String>,
        access_policy: Option<String>,
    ) -> Result<(WebSpaceMount, WebSpaceHead), String> {
        let source_uri = rooted_webspace_path(&source_uri);
        let source = handle_from_resolved_path(resolve_path(self, &source_uri)?)?;
        let moniker = moniker.trim().to_string();
        if self.mounts.iter().any(|mount| mount.moniker == moniker) || moniker == BUILTIN_MONIKER {
            return Err(format!(
                "WebSpace fork target moniker already exists: {moniker}"
            ));
        }
        let target_uri = target_uri
            .or_else(|| source.target_uri.clone())
            .unwrap_or_else(|| source.handle_uri.clone());
        let resolver = resolver.or_else(|| Some(source.resolver.clone()));
        let description = description.or_else(|| {
            Some(format!(
                "Mutable fork of {}. Bytes are not copied until a resolver/sync worker materializes this head.",
                source.handle_uri
            ))
        });
        let record = self.upsert_mount_with_fork(
            moniker.clone(),
            target_uri,
            source.namespace_uri.clone(),
            resolver,
            description,
            Some(readonly.unwrap_or(false)),
            cache_policy.or_else(|| Some(source.cache_policy.clone())),
            sync_policy.or_else(|| Some(source.sync_policy.clone())),
            access_policy.or_else(|| Some(DEFAULT_MUTABLE_ACCESS_POLICY.to_string())),
            Some(source.handle_uri.clone()),
        )?;
        let handle = mount_handle_from_record(&record);
        let head = self.upsert_head_for_handle(&handle, "forked_metadata_only", true)?;
        Ok((record, head))
    }

    fn unmount(&mut self, moniker: &str) -> Result<WebSpaceMount, String> {
        let moniker = moniker.trim();
        if moniker == BUILTIN_MONIKER {
            return Err(format!(
                "{BUILTIN_MONIKER} is a built-in WebSpace and cannot be unmounted"
            ));
        }
        let index = self
            .mounts
            .iter()
            .position(|mount| mount.moniker == moniker)
            .ok_or_else(|| format!("unknown WebSpace moniker: {moniker}"))?;
        let record = self.mounts.remove(index);
        self.save_mounts()?;
        self.index_entries
            .retain(|entry| entry.moniker != record.moniker);
        if self.index_table_path().is_some() {
            self.save_indexes()?;
        }
        self.heads.retain(|head| head.moniker != record.moniker);
        if self.head_table_path().is_some() {
            self.save_heads()?;
        }
        self.objects
            .retain(|object| object.moniker != record.moniker);
        if self.object_table_path().is_some() {
            self.save_objects()?;
        }
        Ok(record)
    }
}

fn normalize_mount_record(mut record: WebSpaceMount) -> Result<WebSpaceMount, String> {
    record.schema = MOUNT_RECORD_SCHEMA.to_string();
    record.moniker = record.moniker.trim().to_string();
    if record.moniker == BUILTIN_MONIKER {
        return Err(format!(
            "{BUILTIN_MONIKER} is reserved for the built-in WebSpace"
        ));
    }
    if !valid_moniker(&record.moniker) {
        return Err(format!("invalid WebSpace moniker: {}", record.moniker));
    }
    record.target_uri = record.target_uri.trim().trim_end_matches('/').to_string();
    if !record.target_uri.contains("://") {
        return Err(format!(
            "WebSpace {} has invalid target_uri: {}",
            record.moniker, record.target_uri
        ));
    }
    if record
        .namespace_uri
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        record.namespace_uri = infer_namespace_uri(&record.target_uri);
    }
    record.resolver = normalized_non_empty(&record.resolver, DEFAULT_EXTERNAL_RESOLVER);
    record.access_policy = normalized_access_policy(Some(&record.access_policy), record.readonly);
    record.cache_policy = normalized_non_empty(&record.cache_policy, DEFAULT_CACHE_POLICY);
    record.sync_policy = normalized_non_empty(&record.sync_policy, DEFAULT_SYNC_POLICY);
    if record.description.trim().is_empty() {
        record.description = format!(
            "Mounted WebSpace {} mapped to {}.",
            record.moniker, record.target_uri
        );
    } else {
        record.description = record.description.trim().to_string();
    }
    if record.created_at == 0 {
        record.created_at = now_unix_secs();
    }
    if record.updated_at == 0 {
        record.updated_at = record.created_at;
    }
    Ok(record)
}

fn normalized_access_policy(value: Option<&str>, readonly: bool) -> String {
    let fallback = if readonly {
        DEFAULT_READONLY_ACCESS_POLICY
    } else {
        DEFAULT_MUTABLE_ACCESS_POLICY
    };
    let trimmed = value.unwrap_or("").trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalized_index_parts(path: &str) -> Result<Vec<String>, String> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err("WebSpace index path must not be empty".to_string());
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/').filter(|part| !part.is_empty()) {
        let part = part.trim();
        if part == "." || part == ".." || part.contains('\\') || part.contains("://") {
            return Err(format!("invalid WebSpace index path segment: {part}"));
        }
        parts.push(part.to_string());
    }
    if parts.is_empty() {
        Err("WebSpace index path must not be empty".to_string())
    } else {
        Ok(parts)
    }
}

fn normalized_index_path(parts: &[&str]) -> Result<String, String> {
    let joined = parts
        .iter()
        .map(|part| part.trim_matches('/'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    normalized_index_parts(&joined).map(|parts| parts.join("/"))
}

fn normalize_index_kind(kind: &str) -> Result<String, String> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "directory" | "folder" => Ok("directory".to_string()),
        "file" | "object" => Ok("file".to_string()),
        other => Err(format!(
            "WebSpace index kind must be directory or file, got {other}"
        )),
    }
}

fn normalize_index_input(
    mount: &WebSpaceMount,
    input: WebSpaceIndexInput,
    now: u64,
) -> Result<WebSpaceIndexEntry, String> {
    let parts = normalized_index_parts(&input.path)?;
    let path = parts.join("/");
    let name = parts
        .last()
        .cloned()
        .ok_or_else(|| "WebSpace index path must not be empty".to_string())?;
    let kind = normalize_index_kind(&input.kind)?;
    let target_uri = input
        .target_uri
        .map(|target| target.trim().trim_end_matches('/').to_string())
        .filter(|target| !target.is_empty())
        .unwrap_or_else(|| {
            append_target_uri(
                &mount.target_uri,
                &parts.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        });
    if !target_uri.contains("://") {
        return Err(format!(
            "WebSpace index target_uri must be scheme-qualified: {target_uri}"
        ));
    }
    normalize_index_record(WebSpaceIndexEntry {
        schema: INDEX_ENTRY_SCHEMA.to_string(),
        moniker: mount.moniker.clone(),
        path,
        name,
        kind,
        target_uri,
        resolver: mount.resolver.clone(),
        resolver_state: input
            .resolver_state
            .unwrap_or_else(|| "indexed".to_string()),
        readonly: input.readonly.unwrap_or(mount.readonly),
        description: input.description.unwrap_or_default(),
        updated_at: now,
    })
}

fn normalize_index_record(mut record: WebSpaceIndexEntry) -> Result<WebSpaceIndexEntry, String> {
    if !valid_moniker(&record.moniker) || record.moniker == BUILTIN_MONIKER {
        return Err(format!(
            "invalid WebSpace index moniker: {}",
            record.moniker
        ));
    }
    let parts = normalized_index_parts(&record.path)?;
    record.schema = INDEX_ENTRY_SCHEMA.to_string();
    record.path = parts.join("/");
    record.name = parts.last().cloned().unwrap_or_else(|| record.path.clone());
    record.kind = normalize_index_kind(&record.kind)?;
    record.target_uri = record.target_uri.trim().trim_end_matches('/').to_string();
    if !record.target_uri.contains("://") {
        return Err(format!(
            "WebSpace index target_uri must be scheme-qualified: {}",
            record.target_uri
        ));
    }
    if record.resolver.trim().is_empty() {
        record.resolver = default_external_resolver();
    }
    if record.resolver_state.trim().is_empty() {
        record.resolver_state = "indexed".to_string();
    }
    if record.description.trim().is_empty() {
        record.description = format!(
            "Indexed {} from the {} WebSpace resolver.",
            record.kind, record.moniker
        );
    }
    if record.updated_at == 0 {
        record.updated_at = now_unix_secs();
    }
    Ok(record)
}

fn normalize_head_record(mut record: WebSpaceHead) -> WebSpaceHead {
    record.schema = HEAD_RECORD_SCHEMA.to_string();
    if record.object_id.trim().is_empty() {
        record.object_id = format!(
            "object:webspace:{}",
            stable_hex(record.target_uri.as_deref().unwrap_or(&record.handle_uri))
        );
    }
    if record.head_id.trim().is_empty() {
        record.head_id = head_id_for(&record.handle_uri);
    }
    if record.revision.trim().is_empty() {
        record.revision = format!(
            "rev:webspace:{}",
            stable_hex(&format!("{}:{}", record.handle_uri, record.updated_at))
        );
    }
    record.access_policy = normalized_access_policy(Some(&record.access_policy), record.readonly);
    record.cache_policy = normalized_non_empty(&record.cache_policy, DEFAULT_CACHE_POLICY);
    record.sync_policy = normalized_non_empty(&record.sync_policy, DEFAULT_SYNC_POLICY);
    record.cache_state = normalized_non_empty(
        &record.cache_state,
        &cache_state_for(&record.cache_policy, record.last_cached_at),
    );
    record.sync_state = normalized_non_empty(
        &record.sync_state,
        &sync_state_for(&record.sync_policy, record.dirty, record.last_synced_at),
    );
    record.status = normalized_non_empty(&record.status, "metadata_only");
    record
}

fn normalize_object_record(mut record: WebSpaceObject) -> Result<WebSpaceObject, String> {
    if !valid_moniker(&record.moniker) || record.moniker == BUILTIN_MONIKER {
        return Err(format!(
            "invalid WebSpace object moniker: {}",
            record.moniker
        ));
    }
    let parts = normalized_index_parts(&record.path)?;
    record.schema = OBJECT_RECORD_SCHEMA.to_string();
    record.path = parts.join("/");
    record.name = parts.last().cloned().unwrap_or_else(|| record.path.clone());
    record.kind = normalize_index_kind(&record.kind)?;
    if record.kind == "directory" {
        record.content.clear();
        record.mime = "inode/directory".to_string();
    } else if record.mime.trim().is_empty() {
        record.mime = "application/octet-stream".to_string();
    } else {
        record.mime = record.mime.trim().to_string();
    }
    record.target_uri = record
        .target_uri
        .map(|target_uri| target_uri.trim().trim_end_matches('/').to_string())
        .filter(|target_uri| !target_uri.is_empty());
    if record.created_at == 0 {
        record.created_at = now_unix_secs();
    }
    if record.updated_at == 0 {
        record.updated_at = record.created_at;
    }
    if record.revision.trim().is_empty() {
        record.revision = object_revision(&record);
    }
    Ok(record)
}

fn object_revision(object: &WebSpaceObject) -> String {
    format!(
        "rev:webspace-object:{}",
        stable_hex(&format!(
            "{}:{}:{}:{}:{}",
            object.moniker,
            object.path,
            object.kind,
            object.updated_at,
            object.content.len()
        ))
    )
}

fn normalized_non_empty(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|err| {
        format!(
            "failed to write WebSpace mount table {}: {err}",
            tmp.display()
        )
    })?;
    fs::rename(&tmp, path).map_err(|err| {
        format!(
            "failed to replace WebSpace mount table {} from {}: {err}",
            path.display(),
            tmp.display()
        )
    })
}

fn meta_path(handle: &WebSpaceHandle) -> String {
    format!("{}/_meta.json", handle.handle_uri.trim_end_matches('/'))
}

fn known_mounts(state: &ProviderState) -> Vec<WebSpaceHandle> {
    let mut handles = vec![mount_handle(
        "Elastos",
        Some("elastos://".to_string()),
        "Local interpreted handle into the broader elastos:// namespace.",
        Some(
            "List this handle to discover typed child spaces such as content, peer, did, and ai."
                .to_string(),
        ),
    )];
    handles.extend(state.user_mounts().iter().map(mount_handle_from_record));
    handles
}

fn normalize_moniker(moniker: &str) -> String {
    moniker
        .trim()
        .trim_matches('/')
        .trim_end_matches("://")
        .to_string()
}

fn normalize_resolver_id(resolver: &str) -> String {
    resolver.trim().to_ascii_lowercase()
}

fn valid_resolver_id(resolver: &str) -> bool {
    let trimmed = resolver.trim();
    !trimmed.is_empty()
        && !trimmed.contains('/')
        && !trimmed.contains('\\')
        && !trimmed.contains("://")
        && !trimmed.chars().any(char::is_control)
}

fn normalize_adapter_state(state: &str) -> Result<String, String> {
    match state.trim().to_ascii_lowercase().as_str() {
        "" => Ok(DEFAULT_ADAPTER_STATE.to_string()),
        "configured" | "connected" | "unavailable" | "disabled" => {
            Ok(state.trim().to_ascii_lowercase())
        }
        other => Err(format!(
            "WebSpace adapter state must be configured, connected, unavailable, or disabled; got {other}"
        )),
    }
}

fn normalize_adapter_check_result(result: &str) -> Result<String, String> {
    match result.trim().to_ascii_lowercase().as_str() {
        "" => Ok("unknown".to_string()),
        "ok" | "failed" | "skipped" | "unknown" => Ok(result.trim().to_ascii_lowercase()),
        other => Err(format!(
            "WebSpace adapter check result must be ok, failed, skipped, or unknown; got {other}"
        )),
    }
}

fn default_check_result_for_state(state: &str) -> &'static str {
    match state {
        "connected" => "ok",
        "unavailable" => "failed",
        "disabled" => "skipped",
        _ => "unknown",
    }
}

fn adapter_state_for_check_result<'a>(result: &str, current_state: &'a str) -> &'a str {
    match result {
        "ok" => "connected",
        "failed" => "unavailable",
        "skipped" => "disabled",
        _ => current_state,
    }
}

fn normalize_optional_error_code(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 96
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(
            "WebSpace adapter error_code must be a short opaque code, not a credential or message"
                .to_string(),
        );
    }
    Ok(Some(value))
}

fn normalize_adapter_capabilities(capabilities: Vec<String>) -> Vec<String> {
    let mut capabilities = capabilities
        .into_iter()
        .map(|capability| capability.trim().to_ascii_lowercase())
        .filter(|capability| !capability.is_empty())
        .filter(|capability| {
            capability
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        })
        .collect::<Vec<_>>();
    if capabilities.is_empty() {
        capabilities.push("metadata_index".to_string());
    }
    capabilities.sort();
    capabilities.dedup();
    capabilities
}

fn normalize_optional_uri(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !trimmed.contains("://") && !trimmed.starts_with("provider:") {
        return Err(format!(
            "WebSpace adapter endpoint_uri must be scheme-qualified or provider-qualified: {trimmed}"
        ));
    }
    Ok(Some(trimmed))
}

fn normalize_optional_label(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_optional_provider(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn normalize_adapter_record(mut record: WebSpaceAdapter) -> Result<WebSpaceAdapter, String> {
    record.schema = ADAPTER_RECORD_SCHEMA.to_string();
    record.resolver = normalize_resolver_id(&record.resolver);
    if record.resolver == "builtin" {
        return Err(
            "built-in WebSpace adapter is provider-owned and cannot be registered".to_string(),
        );
    }
    if !valid_resolver_id(&record.resolver) {
        return Err(format!(
            "invalid WebSpace adapter resolver id: {}",
            record.resolver
        ));
    }
    record.label = normalize_optional_label(Some(record.label))
        .unwrap_or_else(|| record.resolver.replace('-', " "));
    record.endpoint_uri = normalize_optional_uri(record.endpoint_uri)?;
    record.provider = normalize_optional_provider(record.provider);
    record.state = normalize_adapter_state(&record.state)?;
    record.capabilities = normalize_adapter_capabilities(record.capabilities);
    record.last_check_result = match record.last_check_result.take() {
        Some(result) => Some(normalize_adapter_check_result(&result)?),
        None => None,
    };
    record.last_check_error_code = normalize_optional_error_code(record.last_check_error_code)?;
    if record.description.trim().is_empty() {
        record.description = format!(
            "External WebSpace resolver adapter for {}.",
            record.resolver
        );
    } else {
        record.description = record.description.trim().to_string();
    }
    if record.created_at == 0 {
        record.created_at = now_unix_secs();
    }
    if record.updated_at == 0 {
        record.updated_at = record.created_at;
    }
    Ok(record)
}

fn redact_endpoint_uri(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return Some(value.to_string());
    };
    let (authority, suffix) = rest.split_once('/').unwrap_or((rest, ""));
    let redacted_authority = authority
        .rsplit_once('@')
        .map(|(_, host)| format!("redacted@{host}"))
        .unwrap_or_else(|| authority.to_string());
    if suffix.is_empty() {
        Some(format!("{scheme}://{redacted_authority}"))
    } else {
        Some(format!("{scheme}://{redacted_authority}/{suffix}"))
    }
}

fn builtin_adapter_summary() -> serde_json::Value {
    serde_json::json!({
        "schema": ADAPTER_RECORD_SCHEMA,
        "resolver": "builtin",
        "label": "Built-in ElastOS resolver",
        "state": "connected",
        "live": true,
        "health": {
            "schema": "elastos.webspace.adapter-health/v1",
            "status": "healthy",
            "last_checked_at": serde_json::Value::Null,
            "last_result": "ok",
            "stale": false,
            "next": "Built-in resolver health is provider-owned."
        },
        "capabilities": ["metadata_index", "read_descriptor", "local_runtime"],
        "readonly_default": true,
        "description": "Provider-owned resolver for localhost://WebSpaces/Elastos typed handles."
    })
}

fn adapter_public_summary(adapter: &WebSpaceAdapter) -> serde_json::Value {
    serde_json::json!({
        "schema": ADAPTER_RECORD_SCHEMA,
        "resolver": adapter.resolver.as_str(),
        "label": adapter.label.as_str(),
        "endpoint_uri": redact_endpoint_uri(adapter.endpoint_uri.as_deref()),
        "provider": adapter.provider.as_deref(),
        "state": adapter.state.as_str(),
        "live": adapter.state == "connected",
        "health": adapter_health_summary(adapter),
        "capabilities": adapter.capabilities.clone(),
        "readonly_default": adapter.readonly_default,
        "description": adapter.description.as_str(),
        "created_at": adapter.created_at,
        "updated_at": adapter.updated_at,
    })
}

fn adapter_health_summary(adapter: &WebSpaceAdapter) -> serde_json::Value {
    let now = now_unix_secs();
    let stale = adapter
        .last_checked_at
        .map(|checked| now.saturating_sub(checked) > ADAPTER_HEALTH_STALE_AFTER_SECS)
        .unwrap_or(false);
    let result = adapter.last_check_result.as_deref().unwrap_or("unknown");
    let status = if adapter.state == "disabled" {
        "disabled"
    } else if adapter.state == "unavailable" || result == "failed" {
        "unavailable"
    } else if stale {
        "stale"
    } else if result == "ok" {
        "healthy"
    } else if adapter.state == "connected" {
        "connected_unverified"
    } else if adapter.state == "configured" {
        "configured_unchecked"
    } else {
        "unknown"
    };
    serde_json::json!({
        "schema": "elastos.webspace.adapter-health/v1",
        "status": status,
        "last_checked_at": adapter.last_checked_at,
        "last_result": adapter.last_check_result.as_deref().unwrap_or("unknown"),
        "last_error_code": adapter.last_check_error_code.as_deref(),
        "stale": stale,
        "stale_after_seconds": ADAPTER_HEALTH_STALE_AFTER_SECS,
        "next": adapter_health_next_step(status)
    })
}

fn adapter_health_next_step(status: &str) -> &'static str {
    match status {
        "healthy" => "Adapter is recently checked; resolver traversal still depends on the adapter implementing requested capabilities.",
        "connected_unverified" => "Adapter is marked connected but has no recorded health check; run check_adapter from the adapter/operator plane.",
        "configured_unchecked" => "Adapter is configured but unchecked; start the adapter and record check_adapter before relying on live traversal.",
        "stale" => "Adapter health is stale; refresh check_adapter before claiming live traversal.",
        "unavailable" => "Adapter reported unavailable; inspect adapter/provider health before refresh/cache/sync.",
        "disabled" => "Adapter is disabled by policy.",
        _ => "Inspect adapter registration before live traversal.",
    }
}

fn adapter_next_step(
    resolver: &str,
    adapter_state: &str,
    index_entry_count: usize,
    live_adapter: bool,
) -> String {
    if resolver == "builtin" {
        return "Built-in resolver metadata is available.".to_string();
    }
    if live_adapter {
        return "Resolver adapter is connected; refresh/cache/sync can use its provider surface when the adapter implements those capabilities.".to_string();
    }
    match adapter_state {
        "configured" => {
            "Start or connect the registered resolver adapter, then refresh/cache/sync this WebSpace.".to_string()
        }
        "unavailable" => {
            "Registered resolver adapter is unavailable; inspect adapter health before refreshing this WebSpace.".to_string()
        }
        "disabled" => {
            "Registered resolver adapter is disabled by policy; enable or replace it before live traversal.".to_string()
        }
        "not_registered" if index_entry_count == 0 => {
            "Register the named resolver adapter or run a resolver index/refresh for metadata-only traversal.".to_string()
        }
        "not_registered" => {
            "Indexed metadata is available; register/connect the resolver adapter before live byte traversal.".to_string()
        }
        _ => "Inspect resolver adapter registration before live byte traversal.".to_string(),
    }
}

fn rooted_webspace_path(target: &str) -> String {
    if target.starts_with("localhost://") {
        target.to_string()
    } else {
        format!("localhost://WebSpaces/{}", target.trim_matches('/'))
    }
}

fn resolve_handle(state: &ProviderState, moniker: &str) -> Result<WebSpaceHandle, String> {
    let normalized = normalize_moniker(moniker);
    if normalized.is_empty() {
        return Err("missing WebSpace moniker".to_string());
    }
    resolve_handle_segments(state, std::slice::from_ref(&normalized.as_str()))
}

fn mount_handle(
    moniker: &str,
    namespace_uri: Option<String>,
    description: &str,
    next_step: Option<String>,
) -> WebSpaceHandle {
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri: format!("localhost://WebSpaces/{}", moniker),
        namespace_uri,
        target_uri: None,
        resolver_state: "mounted".to_string(),
        resolver: "builtin".to_string(),
        cache_policy: DEFAULT_CACHE_POLICY.to_string(),
        sync_policy: DEFAULT_SYNC_POLICY.to_string(),
        readonly: true,
        access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
        kind: "dynamic-webspace".to_string(),
        traversable: true,
        size: 0,
        object_id: format!(
            "object:webspace:{}",
            stable_hex(&format!("localhost://WebSpaces/{}", moniker))
        ),
        head_id: head_id_for(&format!("localhost://WebSpaces/{}", moniker)),
        cache_state: cache_state_for(DEFAULT_CACHE_POLICY, Some(0)),
        sync_state: sync_state_for(DEFAULT_SYNC_POLICY, false, None),
        description: description.to_string(),
        forked_from: None,
        next_step,
    }
}

fn mount_handle_from_record(record: &WebSpaceMount) -> WebSpaceHandle {
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: format!("localhost://WebSpaces/{}", record.moniker),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(record.target_uri.clone()),
        resolver_state: if record.readonly {
            "mounted-readonly".to_string()
        } else {
            "mounted-mutable".to_string()
        },
        resolver: record.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: record.readonly,
        access_policy: record.access_policy.clone(),
        kind: "mounted-webspace".to_string(),
        traversable: true,
        size: 0,
        object_id: format!("object:webspace:{}", stable_hex(&record.target_uri)),
        head_id: head_id_for(&format!("localhost://WebSpaces/{}", record.moniker)),
        cache_state: cache_state_for(&record.cache_policy, Some(record.updated_at)),
        sync_state: sync_state_for(&record.sync_policy, false, None),
        description: record.description.clone(),
        forked_from: record.forked_from.clone(),
        next_step: Some(
            "This mount resolves local WebSpace handles to its target URI. Actual network traversal requires the named resolver/provider to be available."
                .to_string(),
        ),
    }
}

fn mounted_child_handle(record: &WebSpaceMount, parts: &[&str]) -> WebSpaceHandle {
    let target_uri = append_target_uri(&record.target_uri, parts);
    let handle_uri = format!(
        "localhost://WebSpaces/{}/{}",
        record.moniker,
        parts
            .iter()
            .map(|part| part.trim_matches('/'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: handle_uri.clone(),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(target_uri.clone()),
        resolver_state: "mapped-unavailable".to_string(),
        resolver: record.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: record.readonly,
        access_policy: record.access_policy.clone(),
        kind: "external-object-handle".to_string(),
        traversable: false,
        size: render_external_descriptor_size(&handle_uri, Some(&target_uri), "external-object-handle"),
        object_id: format!("object:webspace:{}", stable_hex(&target_uri)),
        head_id: head_id_for(&handle_uri),
        cache_state: cache_state_for(&record.cache_policy, Some(record.updated_at)),
        sync_state: sync_state_for(&record.sync_policy, false, None),
        description: format!(
            "Typed handle mapped through the {} WebSpace. Attach resolver '{}' to open or sync live content.",
            record.moniker, record.resolver
        ),
        forked_from: record.forked_from.clone(),
        next_step: Some(
            "Live resolver invocation and provider streaming are required before this external handle can stream live content."
                .to_string(),
        ),
    }
}

fn indexed_entry_handle(record: &WebSpaceMount, entry: &WebSpaceIndexEntry) -> WebSpaceHandle {
    let handle_uri = format!("localhost://WebSpaces/{}/{}", record.moniker, entry.path);
    let traversable = entry.kind == "directory";
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: handle_uri.clone(),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(entry.target_uri.clone()),
        resolver_state: entry.resolver_state.clone(),
        resolver: entry.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: entry.readonly,
        access_policy: normalized_access_policy(None, entry.readonly),
        kind: if traversable {
            "indexed-directory".to_string()
        } else {
            "indexed-file".to_string()
        },
        traversable,
        size: if traversable {
            0
        } else {
            render_external_descriptor_size(&handle_uri, Some(&entry.target_uri), "indexed-file")
        },
        object_id: format!("object:webspace:{}", stable_hex(&entry.target_uri)),
        head_id: head_id_for(&handle_uri),
        cache_state: cache_state_for(&record.cache_policy, Some(entry.updated_at)),
        sync_state: sync_state_for(&record.sync_policy, false, None),
        description: entry.description.clone(),
        forked_from: record.forked_from.clone(),
        next_step: Some(
            "This handle came from a resolver index. Attach provider streaming before opening remote bytes."
                .to_string(),
        ),
    }
}

fn indexed_virtual_folder_handle(record: &WebSpaceMount, parts: &[&str]) -> WebSpaceHandle {
    let target_uri = append_target_uri(&record.target_uri, parts);
    let handle_uri = format!(
        "localhost://WebSpaces/{}/{}",
        record.moniker,
        parts
            .iter()
            .map(|part| part.trim_matches('/'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: handle_uri.clone(),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(target_uri.clone()),
        resolver_state: "indexed-virtual".to_string(),
        resolver: record.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: record.readonly,
        access_policy: record.access_policy.clone(),
        kind: "indexed-directory".to_string(),
        traversable: true,
        size: 0,
        object_id: format!("object:webspace:{}", stable_hex(&target_uri)),
        head_id: head_id_for(&handle_uri),
        cache_state: cache_state_for(&record.cache_policy, None),
        sync_state: sync_state_for(&record.sync_policy, false, None),
        description: format!(
            "Virtual folder inferred from the {} WebSpace resolver index.",
            record.moniker
        ),
        forked_from: record.forked_from.clone(),
        next_step: Some("List this folder to inspect indexed resolver children.".to_string()),
    }
}

fn folder_handle(
    moniker: &str,
    handle_uri: String,
    target_uri: Option<String>,
    description: &str,
    next_step: Option<String>,
) -> WebSpaceHandle {
    let object_id = format!(
        "object:webspace:{}",
        stable_hex(target_uri.as_deref().unwrap_or(&handle_uri))
    );
    let head_id = head_id_for(&handle_uri);
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri: handle_uri.clone(),
        namespace_uri: Some("elastos://".to_string()),
        target_uri: target_uri.clone(),
        resolver_state: "resolved".to_string(),
        resolver: "builtin".to_string(),
        cache_policy: DEFAULT_CACHE_POLICY.to_string(),
        sync_policy: DEFAULT_SYNC_POLICY.to_string(),
        readonly: true,
        access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
        kind: "folder-handle".to_string(),
        traversable: true,
        size: 0,
        object_id,
        head_id,
        cache_state: cache_state_for(DEFAULT_CACHE_POLICY, Some(0)),
        sync_state: sync_state_for(DEFAULT_SYNC_POLICY, false, None),
        description: description.to_string(),
        forked_from: None,
        next_step,
    }
}

fn file_handle(
    moniker: &str,
    handle_uri: String,
    target_uri: String,
    description: &str,
) -> WebSpaceHandle {
    let object_id = format!("object:webspace:{}", stable_hex(&target_uri));
    let head_id = head_id_for(&handle_uri);
    WebSpaceHandle {
        moniker: moniker.to_string(),
        handle_uri: handle_uri.clone(),
        namespace_uri: Some("elastos://".to_string()),
        target_uri: Some(target_uri.clone()),
        resolver_state: "resolved".to_string(),
        resolver: "builtin".to_string(),
        cache_policy: DEFAULT_CACHE_POLICY.to_string(),
        sync_policy: DEFAULT_SYNC_POLICY.to_string(),
        readonly: true,
        access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
        kind: "file-endpoint".to_string(),
        traversable: false,
        size: render_external_descriptor_size(&handle_uri, Some(&target_uri), "file-endpoint"),
        object_id,
        head_id,
        cache_state: cache_state_for(DEFAULT_CACHE_POLICY, Some(0)),
        sync_state: sync_state_for(DEFAULT_SYNC_POLICY, false, None),
        description: description.to_string(),
        forked_from: None,
        next_step: Some(
            "Read this handle for the current descriptor view, or inspect _meta.json for structured metadata."
                .to_string(),
        ),
    }
}

fn resolve_elastos_handle(parts: &[&str]) -> Result<WebSpaceHandle, String> {
    match parts {
        [] => Ok(mount_handle(
            "Elastos",
            Some("elastos://".to_string()),
            "Local interpreted handle into the broader elastos:// namespace.",
            Some(
                "List this handle to discover typed child spaces such as content, peer, did, and ai."
                    .to_string(),
            ),
        )),
        ["content"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/content".to_string(),
            Some("elastos://<cid>".to_string()),
            "Content-addressed objects in the Elastos WebSpace. Append a content id to resolve a file endpoint.",
            Some("Append a content id, for example localhost://WebSpaces/Elastos/content/<cid>.".to_string()),
        )),
        ["content", cid] if !cid.is_empty() => Ok(file_handle(
            "Elastos",
            format!("localhost://WebSpaces/Elastos/content/{}", cid),
            format!("elastos://{}", cid),
            "Typed file endpoint resolved from the Elastos WebSpace content-addressed namespace.",
        )),
        ["content", ..] => Err(
            "content endpoints do not support traversal beyond localhost://WebSpaces/Elastos/content/<cid>"
                .to_string(),
        ),
        ["peer"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/peer".to_string(),
            Some("elastos://peer/".to_string()),
            "Peer-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a peer identifier or ticket path segment.".to_string()),
        )),
        ["peer", peer_id] if !peer_id.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/peer/{}", peer_id),
                Some(format!("elastos://peer/{}", peer_id)),
                "Typed peer handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper peer traversal is not implemented yet.".to_string()),
            ))
        }
        ["peer", ..] => Err(
            "peer handles do not support traversal beyond localhost://WebSpaces/Elastos/peer/<peer-id> yet"
                .to_string(),
        ),
        ["did"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/did".to_string(),
            Some("elastos://did/".to_string()),
            "DID-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a DID or DID-method path segment.".to_string()),
        )),
        ["did", did] if !did.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/did/{}", did),
                Some(format!("elastos://did/{}", did)),
                "Typed DID handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper DID traversal is not implemented yet.".to_string()),
            ))
        }
        ["did", ..] => Err(
            "did handles do not support traversal beyond localhost://WebSpaces/Elastos/did/<did> yet"
                .to_string(),
        ),
        ["ai"] => Ok(folder_handle(
            "Elastos",
            "localhost://WebSpaces/Elastos/ai".to_string(),
            Some("elastos://ai/".to_string()),
            "AI-scoped dynamic space inside the broader Elastos WebSpace.",
            Some("Append a backend or model path segment.".to_string()),
        )),
        ["ai", backend] if !backend.is_empty() => {
            Ok(folder_handle(
                "Elastos",
                format!("localhost://WebSpaces/Elastos/ai/{}", backend),
                Some(format!("elastos://ai/{}", backend)),
                "Typed AI handle resolved through the Elastos WebSpace.",
                Some("Inspect _meta.json for the current typed handle view. Deeper AI traversal is not implemented yet.".to_string()),
            ))
        }
        ["ai", ..] => Err(
            "ai handles do not support traversal beyond localhost://WebSpaces/Elastos/ai/<backend> yet"
                .to_string(),
        ),
        [child, ..] => Err(format!(
            "unknown Elastos WebSpace child: {} (known typed children: content, peer, did, ai)",
            child
        )),
    }
}

fn resolve_handle_segments(
    state: &ProviderState,
    parts: &[&str],
) -> Result<WebSpaceHandle, String> {
    let Some((moniker, rest)) = parts.split_first() else {
        return Err("missing WebSpace moniker".to_string());
    };
    let normalized = normalize_moniker(moniker);
    if normalized.is_empty() {
        return Err("missing WebSpace moniker".to_string());
    }
    match normalized.as_str() {
        "Elastos" => resolve_elastos_handle(rest),
        _ => {
            let Some(record) = state.mount_by_moniker(&normalized) else {
                return Err(format!("unknown WebSpace moniker: {}", normalized));
            };
            if rest.is_empty() {
                Ok(mount_handle_from_record(record))
            } else if let Some(object) = state.exact_object(&record.moniker, rest) {
                Ok(materialized_object_handle(record, &object))
            } else if let Some(entry) = state.exact_index_entry(&record.moniker, rest) {
                Ok(indexed_entry_handle(record, &entry))
            } else if state.has_object_children(&record.moniker, rest) {
                Ok(materialized_virtual_folder_handle(record, rest))
            } else if state.has_index_children(&record.moniker, rest) {
                Ok(indexed_virtual_folder_handle(record, rest))
            } else {
                Ok(mounted_child_handle(record, rest))
            }
        }
    }
}

fn resolve_path(state: &ProviderState, path: &str) -> Result<ResolvedPath, String> {
    let trimmed = path.trim();
    let (_, rest) = parse_localhost_uri(trimmed)
        .or_else(|| parse_localhost_path(trimmed))
        .ok_or_else(|| format!("invalid rooted localhost path: {}", path))?;
    let rest = rest.trim_matches('/');
    if rest.is_empty() {
        return Ok(ResolvedPath::Root);
    }

    let mut parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return Ok(ResolvedPath::Root);
    }

    let wants_meta = parts.last().copied() == Some("_meta.json");
    if wants_meta {
        parts.pop();
    }

    let handle = resolve_handle_segments(state, &parts)?;
    if wants_meta {
        Ok(ResolvedPath::Meta { handle })
    } else {
        Ok(ResolvedPath::Handle { handle })
    }
}

fn handle_from_resolved_path(resolved: ResolvedPath) -> Result<WebSpaceHandle, String> {
    match resolved {
        ResolvedPath::Handle { handle } => Ok(handle),
        ResolvedPath::Meta { handle } => Err(format!(
            "resolve targets WebSpace handles, not metadata files: {}",
            meta_path(&handle)
        )),
        ResolvedPath::Root => Err("resolve requires a specific WebSpace moniker".to_string()),
    }
}

fn handle_index_parts(handle: &WebSpaceHandle) -> Vec<String> {
    let prefix = format!("localhost://WebSpaces/{}", handle.moniker);
    handle
        .handle_uri
        .strip_prefix(&prefix)
        .unwrap_or_default()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn immediate_index_children<'a>(
    entries: &'a [WebSpaceIndexEntry],
    moniker: &'a str,
    prefix_parts: &'a [&'a str],
) -> impl Iterator<Item = &'a WebSpaceIndexEntry> {
    entries.iter().filter(move |entry| {
        if entry.moniker != moniker {
            return false;
        }
        let Ok(parts) = normalized_index_parts(&entry.path) else {
            return false;
        };
        parts.len() > prefix_parts.len()
            && parts
                .iter()
                .take(prefix_parts.len())
                .zip(prefix_parts.iter())
                .all(|(left, right)| left == right)
    })
}

fn immediate_object_children<'a>(
    objects: &'a [WebSpaceObject],
    moniker: &'a str,
    prefix_parts: &'a [&'a str],
) -> impl Iterator<Item = &'a WebSpaceObject> {
    objects.iter().filter(move |object| {
        if object.moniker != moniker {
            return false;
        }
        let Ok(parts) = normalized_index_parts(&object.path) else {
            return false;
        };
        parts.len() > prefix_parts.len()
            && parts
                .iter()
                .take(prefix_parts.len())
                .zip(prefix_parts.iter())
                .all(|(left, right)| left == right)
    })
}

fn indexed_child_handles(
    state: &ProviderState,
    record: &WebSpaceMount,
    prefix_parts: &[String],
) -> Vec<(String, WebSpaceHandle)> {
    let prefix_refs = prefix_parts.iter().map(String::as_str).collect::<Vec<_>>();
    let mut children = BTreeMap::new();
    for entry in immediate_index_children(&state.index_entries, &record.moniker, &prefix_refs) {
        let Ok(parts) = normalized_index_parts(&entry.path) else {
            continue;
        };
        let Some(child_name) = parts.get(prefix_parts.len()).cloned() else {
            continue;
        };
        if parts.len() == prefix_parts.len() + 1 {
            children.insert(child_name, indexed_entry_handle(record, entry));
        } else {
            let child_parts = prefix_parts
                .iter()
                .cloned()
                .chain(std::iter::once(child_name.clone()))
                .collect::<Vec<_>>();
            let child_refs = child_parts.iter().map(String::as_str).collect::<Vec<_>>();
            children
                .entry(child_name)
                .or_insert_with(|| indexed_virtual_folder_handle(record, &child_refs));
        }
    }
    children.into_iter().collect()
}

fn materialized_child_handles(
    state: &ProviderState,
    record: &WebSpaceMount,
    prefix_parts: &[String],
) -> Vec<(String, WebSpaceHandle)> {
    let prefix_refs = prefix_parts.iter().map(String::as_str).collect::<Vec<_>>();
    let mut children = BTreeMap::new();
    for object in immediate_object_children(&state.objects, &record.moniker, &prefix_refs) {
        let Ok(parts) = normalized_index_parts(&object.path) else {
            continue;
        };
        let Some(child_name) = parts.get(prefix_parts.len()).cloned() else {
            continue;
        };
        if parts.len() == prefix_parts.len() + 1 {
            children.insert(child_name, materialized_object_handle(record, object));
        } else {
            let child_parts = prefix_parts
                .iter()
                .cloned()
                .chain(std::iter::once(child_name.clone()))
                .collect::<Vec<_>>();
            let child_refs = child_parts.iter().map(String::as_str).collect::<Vec<_>>();
            children
                .entry(child_name)
                .or_insert_with(|| materialized_virtual_folder_handle(record, &child_refs));
        }
    }
    children.into_iter().collect()
}

fn materialized_object_handle(record: &WebSpaceMount, object: &WebSpaceObject) -> WebSpaceHandle {
    let parts = normalized_index_parts(&object.path).unwrap_or_else(|_| vec![object.name.clone()]);
    let refs = parts.iter().map(String::as_str).collect::<Vec<_>>();
    let handle_uri = format!(
        "localhost://WebSpaces/{}/{}",
        record.moniker,
        parts.join("/")
    );
    let target_uri = object
        .target_uri
        .clone()
        .unwrap_or_else(|| append_target_uri(&record.target_uri, &refs));
    let traversable = object.kind == "directory";
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: handle_uri.clone(),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(target_uri.clone()),
        resolver_state: "materialized-local".to_string(),
        resolver: record.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: record.readonly,
        access_policy: record.access_policy.clone(),
        kind: if traversable {
            "materialized-directory".to_string()
        } else {
            "materialized-file".to_string()
        },
        traversable,
        size: if traversable {
            0
        } else {
            object.content.len() as u64
        },
        object_id: format!(
            "object:webspace:{}",
            stable_hex(&format!(
                "{}:{}:{}",
                object.moniker, object.path, object.revision
            ))
        ),
        head_id: head_id_for(&handle_uri),
        cache_state: "content_cached".to_string(),
        sync_state: sync_state_for(&record.sync_policy, object.dirty, None),
        description: format!(
            "Local materialized {} in the {} WebSpace.",
            object.kind, record.moniker
        ),
        forked_from: record.forked_from.clone(),
        next_step: if object.dirty {
            Some("Sync this mutable local object through its resolver when a sync worker is available.".to_string())
        } else {
            Some("This object is materialized locally.".to_string())
        },
    }
}

fn materialized_virtual_folder_handle(record: &WebSpaceMount, parts: &[&str]) -> WebSpaceHandle {
    let target_uri = append_target_uri(&record.target_uri, parts);
    let handle_uri = format!(
        "localhost://WebSpaces/{}/{}",
        record.moniker,
        parts
            .iter()
            .map(|part| part.trim_matches('/'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    );
    WebSpaceHandle {
        moniker: record.moniker.clone(),
        handle_uri: handle_uri.clone(),
        namespace_uri: record.namespace_uri.clone(),
        target_uri: Some(target_uri.clone()),
        resolver_state: "materialized-virtual".to_string(),
        resolver: record.resolver.clone(),
        cache_policy: record.cache_policy.clone(),
        sync_policy: record.sync_policy.clone(),
        readonly: record.readonly,
        access_policy: record.access_policy.clone(),
        kind: "materialized-directory".to_string(),
        traversable: true,
        size: 0,
        object_id: format!("object:webspace:{}", stable_hex(&target_uri)),
        head_id: head_id_for(&handle_uri),
        cache_state: "content_cached".to_string(),
        sync_state: sync_state_for(&record.sync_policy, true, None),
        description: format!(
            "Virtual folder inferred from local materialized objects in the {} WebSpace.",
            record.moniker
        ),
        forked_from: record.forked_from.clone(),
        next_step: Some("List this folder to inspect local materialized children.".to_string()),
    }
}

fn resolve_handle_request(
    state: &ProviderState,
    path: Option<String>,
    moniker: Option<String>,
) -> Result<WebSpaceHandle, String> {
    match (path, moniker) {
        (Some(path), _) => handle_from_resolved_path(resolve_path(state, &path)?),
        (None, Some(moniker)) => {
            if moniker.starts_with("localhost://") || moniker.contains('/') {
                let rooted = if moniker.starts_with("localhost://") {
                    moniker
                } else {
                    format!("localhost://WebSpaces/{}", moniker)
                };
                handle_from_resolved_path(resolve_path(state, &rooted)?)
            } else {
                resolve_handle(state, &moniker)
            }
        }
        (None, None) => Err("resolve requires path or moniker".to_string()),
    }
}

fn render_meta(handle: &WebSpaceHandle) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "moniker": handle.moniker,
        "handle_uri": handle.handle_uri,
        "namespace_uri": handle.namespace_uri,
        "target_uri": handle.target_uri,
        "resolver_state": handle.resolver_state,
        "resolver": handle.resolver,
        "cache_policy": handle.cache_policy,
        "sync_policy": handle.sync_policy,
        "readonly": handle.readonly,
        "access_policy": handle.access_policy,
        "kind": handle.kind,
        "traversable": handle.traversable,
        "size": handle.size,
        "object_id": handle.object_id,
        "head_id": handle.head_id,
        "cache_state": handle.cache_state,
        "sync_state": handle.sync_state,
        "description": handle.description,
        "forked_from": handle.forked_from,
        "next_step": handle.next_step,
        "note": "The WebSpace daemon owns the moniker first and returns a typed handle before any further traversal.",
    }))
    .unwrap_or_else(|_| b"{}".to_vec())
}

fn render_endpoint(handle: &WebSpaceHandle) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "handle_uri": handle.handle_uri,
        "target_uri": handle.target_uri,
        "kind": handle.kind,
        "description": handle.description,
        "resolver_state": handle.resolver_state,
        "resolver": handle.resolver,
        "cache_policy": handle.cache_policy,
        "sync_policy": handle.sync_policy,
        "readonly": handle.readonly,
        "access_policy": handle.access_policy,
        "size": handle.size,
        "object_id": handle.object_id,
        "head_id": handle.head_id,
        "cache_state": handle.cache_state,
        "sync_state": handle.sync_state,
    }))
    .unwrap_or_else(|_| b"{}".to_vec())
}

fn cache_status_from_head(head: &WebSpaceHead) -> serde_json::Value {
    let content_cached = head.status.starts_with("materialized_");
    serde_json::json!({
        "schema": "elastos.webspace.cache-status/v1",
        "handle_uri": head.handle_uri,
        "target_uri": head.target_uri,
        "object_id": head.object_id,
        "head_id": head.head_id,
        "access_policy": head.access_policy,
        "policy": head.cache_policy,
        "state": head.cache_state,
        "last_cached_at": head.last_cached_at,
        "content_cached": content_cached,
        "note": if content_cached {
            "Local materialized bytes are present in the WebSpace object table."
        } else {
            "This slice caches resolver metadata only. Content bytes require a resolver/cache worker."
        }
    })
}

fn sync_status_from_head(head: &WebSpaceHead) -> serde_json::Value {
    serde_json::json!({
        "schema": "elastos.webspace.sync-status/v1",
        "handle_uri": head.handle_uri,
        "target_uri": head.target_uri,
        "object_id": head.object_id,
        "head_id": head.head_id,
        "access_policy": head.access_policy,
        "policy": head.sync_policy,
        "state": head.sync_state,
        "dirty": head.dirty,
        "last_synced_at": head.last_synced_at,
        "note": "This slice records sync intent and dirty state. Live sync requires a resolver/sync worker."
    })
}

fn stat_for(resolved: &ResolvedPath, original_path: &str) -> FileStat {
    match resolved {
        ResolvedPath::Root => FileStat {
            path: original_path.to_string(),
            is_file: false,
            is_dir: true,
            size: 0,
            readonly: true,
            access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
            provider: PROVIDER_ID.to_string(),
            resolver_state: "root".to_string(),
            resolver: "builtin".to_string(),
            cache_policy: DEFAULT_CACHE_POLICY.to_string(),
            sync_policy: DEFAULT_SYNC_POLICY.to_string(),
            kind: "webspace-root".to_string(),
            traversable: true,
            object_id: "object:webspace:root".to_string(),
            head_id: "head:webspace:root".to_string(),
            cache_state: "metadata_cached".to_string(),
            sync_state: "manual_idle".to_string(),
            namespace_uri: None,
            target_uri: None,
            modified: None,
            created: None,
        },
        ResolvedPath::Handle { handle } => FileStat {
            path: original_path.to_string(),
            is_file: !handle.traversable,
            is_dir: handle.traversable,
            size: handle.size,
            readonly: handle.readonly,
            access_policy: handle.access_policy.clone(),
            provider: PROVIDER_ID.to_string(),
            resolver_state: handle.resolver_state.clone(),
            resolver: handle.resolver.clone(),
            cache_policy: handle.cache_policy.clone(),
            sync_policy: handle.sync_policy.clone(),
            kind: handle.kind.clone(),
            traversable: handle.traversable,
            object_id: handle.object_id.clone(),
            head_id: handle.head_id.clone(),
            cache_state: handle.cache_state.clone(),
            sync_state: handle.sync_state.clone(),
            namespace_uri: handle.namespace_uri.clone(),
            target_uri: handle.target_uri.clone(),
            modified: None,
            created: None,
        },
        ResolvedPath::Meta { handle } => FileStat {
            path: original_path.to_string(),
            is_file: true,
            is_dir: false,
            size: render_meta(handle).len() as u64,
            readonly: true,
            access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
            provider: PROVIDER_ID.to_string(),
            resolver_state: handle.resolver_state.clone(),
            resolver: handle.resolver.clone(),
            cache_policy: handle.cache_policy.clone(),
            sync_policy: handle.sync_policy.clone(),
            kind: "metadata".to_string(),
            traversable: false,
            object_id: handle.object_id.clone(),
            head_id: handle.head_id.clone(),
            cache_state: handle.cache_state.clone(),
            sync_state: handle.sync_state.clone(),
            namespace_uri: handle.namespace_uri.clone(),
            target_uri: handle.target_uri.clone(),
            modified: None,
            created: None,
        },
    }
}

fn dir_entry_from_handle(name: &str, handle: WebSpaceHandle) -> DirEntry {
    DirEntry {
        name: name.to_string(),
        is_file: !handle.traversable,
        is_dir: handle.traversable,
        size: handle.size,
        readonly: handle.readonly,
        access_policy: handle.access_policy.clone(),
        provider: PROVIDER_ID.to_string(),
        resolver_state: handle.resolver_state.clone(),
        resolver: handle.resolver.clone(),
        cache_policy: handle.cache_policy.clone(),
        sync_policy: handle.sync_policy.clone(),
        kind: handle.kind.clone(),
        traversable: handle.traversable,
        object_id: handle.object_id.clone(),
        head_id: handle.head_id.clone(),
        cache_state: handle.cache_state.clone(),
        sync_state: handle.sync_state.clone(),
        namespace_uri: handle.namespace_uri.clone(),
        target_uri: handle.target_uri.clone(),
    }
}

fn meta_dir_entry(handle: &WebSpaceHandle) -> DirEntry {
    DirEntry {
        name: "_meta.json".to_string(),
        is_file: true,
        is_dir: false,
        size: render_meta(handle).len() as u64,
        readonly: true,
        access_policy: DEFAULT_READONLY_ACCESS_POLICY.to_string(),
        provider: PROVIDER_ID.to_string(),
        resolver_state: handle.resolver_state.clone(),
        resolver: handle.resolver.clone(),
        cache_policy: handle.cache_policy.clone(),
        sync_policy: handle.sync_policy.clone(),
        kind: "metadata".to_string(),
        traversable: false,
        object_id: handle.object_id.clone(),
        head_id: handle.head_id.clone(),
        cache_state: handle.cache_state.clone(),
        sync_state: handle.sync_state.clone(),
        namespace_uri: handle.namespace_uri.clone(),
        target_uri: handle.target_uri.clone(),
    }
}

fn list_for(state: &ProviderState, resolved: &ResolvedPath) -> Result<Vec<DirEntry>, String> {
    match resolved {
        ResolvedPath::Root => Ok(known_mounts(state)
            .into_iter()
            .map(|entry| {
                let name = entry.moniker.clone();
                dir_entry_from_handle(&name, entry)
            })
            .collect()),
        ResolvedPath::Handle { handle } if !handle.traversable => {
            Err(format!("not a directory: {}", handle.handle_uri))
        }
        ResolvedPath::Handle { handle } => {
            let mut entries = vec![meta_dir_entry(handle)];

            match handle.handle_uri.as_str() {
                "localhost://WebSpaces/Elastos" => {
                    for child in ["content", "peer", "did", "ai"] {
                        entries.push(dir_entry_from_handle(
                            child,
                            resolve_elastos_handle(&[child])?,
                        ));
                    }
                }
                _ => {}
            }

            if let Some(record) = state.mount_by_moniker(&handle.moniker) {
                let prefix_parts = handle_index_parts(handle);
                let mut children = BTreeMap::new();
                for (name, child) in indexed_child_handles(state, record, &prefix_parts) {
                    children.insert(name, child);
                }
                for (name, child) in materialized_child_handles(state, record, &prefix_parts) {
                    children.insert(name, child);
                }
                for (name, child) in children {
                    entries.push(dir_entry_from_handle(&name, child));
                }
            }

            Ok(entries)
        }
        ResolvedPath::Meta { handle } => Err(format!("not a directory: {}", meta_path(handle))),
    }
}

fn ok(data: serde_json::Value) -> Response {
    Response::Ok { data: Some(data) }
}

fn error(code: &str, message: impl Into<String>) -> Response {
    Response::Error {
        code: code.to_string(),
        message: message.into(),
    }
}

fn init_payload(state: &ProviderState) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": "1.0",
        "provider": "webspace",
        "kind": "dynamic-resolver",
        "schema": "elastos.webspace.provider-init/v1",
        "persistent": state.mount_table_path().is_some(),
        "mount_table": state
            .mount_table_path()
            .map(|path| path.display().to_string()),
        "head_table": state
            .head_table_path()
            .map(|path| path.display().to_string()),
        "index_table": state
            .index_table_path()
            .map(|path| path.display().to_string()),
        "object_table": state
            .object_table_path()
            .map(|path| path.display().to_string()),
        "adapter_table": state
            .adapter_table_path()
            .map(|path| path.display().to_string()),
        "mount_count": known_mounts(state).len(),
        "index_entry_count": state.index_entries.len(),
        "head_count": state.heads.len(),
        "object_count": state.objects.len(),
        "configured_adapter_count": state.adapters.len(),
        "supported_ops": SUPPORTED_OPS,
        "unsupported_ops": UNSUPPORTED_OPS,
        "surface_note": "Resolver lifecycle slice: built-in and persisted mounts resolve to typed handles. Registered adapters describe external resolver readiness. Mutable user mounts can materialize local objects; live external traversal and provider streaming remain resolver responsibilities.",
    })
}

fn main() {
    if std::env::var("ELASTOS_DEBUG_PROVIDER_STARTUP")
        .ok()
        .as_deref()
        == Some("1")
    {
        eprintln!("webspace-provider: starting v{}", PROVIDER_VERSION);
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut state = ProviderState::default();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::to_string(&error("read_stdin_failed", err.to_string()))
                        .unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"serialize_failed\",\"message\":\"failed to serialize error\"}".to_string())
                );
                let _ = stdout.flush();
                break;
            }
        };

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Init { config }) => match state.configure(config) {
                Ok(payload) => ok(payload),
                Err(err) => error("init_failed", err),
            },
            Ok(Request::Resolve { path, moniker }) => {
                match resolve_handle_request(&state, path, moniker) {
                    Ok(handle) => ok(serde_json::to_value(handle).unwrap_or(serde_json::json!({}))),
                    Err(err) => error("resolve_failed", err),
                }
            }
            Ok(Request::Read { path, .. }) => match resolve_path(&state, &path) {
                Ok(ResolvedPath::Handle { handle }) if handle.kind == "materialized-file" => {
                    match state.object_for_handle(&handle) {
                        Some(object) => {
                            let size = object.content.len();
                            ok(serde_json::json!({
                                "content": object.content,
                                "size": size,
                            }))
                        }
                        None => error("read_failed", format!("materialized object missing from local table: {}", handle.handle_uri)),
                    }
                }
                Ok(ResolvedPath::Handle { handle }) if !handle.traversable => ok(serde_json::json!({
                    "content": render_endpoint(&handle),
                    "size": render_endpoint(&handle).len(),
                })),
                Ok(ResolvedPath::Meta { handle }) => ok(serde_json::json!({
                    "content": render_meta(&handle),
                    "size": render_meta(&handle).len(),
                })),
                Ok(_) => error(
                    "read_failed",
                    "WebSpace folder handles are traversable directories. Read localhost://WebSpaces/<moniker>/_meta.json for metadata or resolve a file endpoint such as localhost://WebSpaces/Elastos/content/<cid>.",
                ),
                Err(err) => error("read_failed", err),
            },
            Ok(Request::List { path, .. }) => match resolve_path(&state, &path) {
                Ok(resolved) => match list_for(&state, &resolved) {
                    Ok(entries) => ok(serde_json::to_value(entries).unwrap_or(serde_json::json!([]))),
                    Err(err) => error("list_failed", err),
                },
                Err(err) => error("list_failed", err),
            },
            Ok(Request::Stat { path, .. }) => match resolve_path(&state, &path) {
                Ok(resolved) => ok(serde_json::to_value(stat_for(&resolved, &path)).unwrap_or(serde_json::json!({}))),
                Err(err) => error("stat_failed", err),
            },
            Ok(Request::Exists { path, .. }) => ok(serde_json::json!({
                "exists": resolve_path(&state, &path).is_ok(),
            })),
            Ok(Request::Head { path, .. }) => match resolve_path(&state, &path)
                .and_then(handle_from_resolved_path)
                .and_then(|handle| state.upsert_head_for_handle(&handle, "metadata_only", false))
            {
                Ok(head) => ok(serde_json::to_value(head).unwrap_or(serde_json::json!({}))),
                Err(err) => error("head_failed", err),
            },
            Ok(Request::CacheStatus { path, .. }) => match resolve_path(&state, &path)
                .and_then(handle_from_resolved_path)
                .and_then(|handle| state.upsert_head_for_handle(&handle, "metadata_only", false))
            {
                Ok(head) => ok(cache_status_from_head(&head)),
                Err(err) => error("cache_status_failed", err),
            },
            Ok(Request::SyncStatus { path, .. }) => match resolve_path(&state, &path)
                .and_then(handle_from_resolved_path)
                .and_then(|handle| state.upsert_head_for_handle(&handle, "metadata_only", false))
            {
                Ok(head) => ok(sync_status_from_head(&head)),
                Err(err) => error("sync_status_failed", err),
            },
            Ok(Request::Mounts { .. }) => ok(serde_json::json!({
                "schema": MOUNT_TABLE_SCHEMA,
                "mounts": known_mounts(&state),
                "user_mounts": state.user_mounts(),
            })),
            Ok(Request::Adapters { .. }) => ok(state.adapter_summary_table()),
            Ok(Request::RegisterAdapter {
                resolver,
                label,
                endpoint_uri,
                provider,
                state: adapter_state,
                capabilities,
                readonly_default,
                description,
                ..
            }) => match state.upsert_adapter(
                resolver,
                label,
                endpoint_uri,
                provider,
                adapter_state,
                capabilities,
                readonly_default,
                description,
            ) {
                Ok(adapter) => ok(serde_json::json!({
                    "schema": "elastos.webspace.adapter-receipt/v1",
                    "action": "registered",
                    "adapter": adapter_public_summary(&adapter),
                })),
                Err(err) => error("register_adapter_failed", err),
            },
            Ok(Request::UnregisterAdapter { resolver, .. }) => {
                match state.unregister_adapter(&resolver) {
                    Ok(adapter) => ok(serde_json::json!({
                        "schema": "elastos.webspace.adapter-receipt/v1",
                        "action": "unregistered",
                        "adapter": adapter_public_summary(&adapter),
                    })),
                    Err(err) => error("unregister_adapter_failed", err),
                }
            }
            Ok(Request::CheckAdapter {
                resolver,
                result,
                state: adapter_state,
                error_code,
                capabilities,
                ..
            }) => match state.check_adapter(
                resolver,
                result,
                adapter_state,
                error_code,
                capabilities,
            ) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("check_adapter_failed", err),
            },
            Ok(Request::Health { moniker, .. }) => match state.health_report(moniker) {
                Ok(report) => ok(report),
                Err(err) => error("health_failed", err),
            },
            Ok(Request::Mount {
                moniker,
                target_uri,
                namespace_uri,
                resolver,
                description,
                readonly,
                cache_policy,
                sync_policy,
                access_policy,
                ..
            }) => match state.upsert_mount(
                moniker,
                target_uri,
                namespace_uri,
                resolver,
                description,
                readonly,
                cache_policy,
                sync_policy,
                access_policy,
            ) {
                Ok(record) => ok(serde_json::json!({
                    "schema": "elastos.webspace.mount-receipt/v1",
                    "action": "mounted",
                    "mount": record,
                })),
                Err(err) => error("mount_failed", err),
            },
            Ok(Request::Unmount { moniker, .. }) => match state.unmount(&moniker) {
                Ok(record) => ok(serde_json::json!({
                    "schema": "elastos.webspace.mount-receipt/v1",
                    "action": "unmounted",
                    "mount": record,
                })),
                Err(err) => error("unmount_failed", err),
            },
            Ok(Request::Index {
                moniker, entries, ..
            }) => match state.replace_index(&moniker, entries) {
                Ok(entries) => ok(serde_json::json!({
                    "schema": "elastos.webspace.index-receipt/v1",
                    "action": "indexed",
                    "moniker": moniker,
                    "entry_count": entries.len(),
                    "entries": entries,
                })),
                Err(err) => error("index_failed", err),
            },
            Ok(Request::Refresh { path, entries, .. }) => match state.refresh_handle(path, entries)
            {
                Ok(receipt) => ok(receipt),
                Err(err) => error("refresh_failed", err),
            },
            Ok(Request::Fork {
                source_uri,
                moniker,
                target_uri,
                resolver,
                description,
                readonly,
                cache_policy,
                sync_policy,
                access_policy,
                ..
            }) => match state.fork_mount(
                source_uri,
                moniker,
                target_uri,
                resolver,
                description,
                readonly,
                cache_policy,
                sync_policy,
                access_policy,
            ) {
                Ok((record, head)) => ok(serde_json::json!({
                    "schema": "elastos.webspace.fork-receipt/v1",
                    "action": "forked",
                    "mount": record,
                    "head": head,
                    "materialized": false,
                    "next_step": "Attach a resolver/cache worker to materialize bytes and sync the fork."
                })),
                Err(err) => error("fork_failed", err),
            },
            Ok(Request::Cache {
                path,
                content,
                mime,
                source_receipt,
                ..
            }) => match state.cache_handle(path, content, mime, source_receipt) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("cache_failed", err),
            },
            Ok(Request::Sync { path, .. }) => match state.sync_handle(path) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("sync_failed", err),
            },
            Ok(Request::Write {
                path,
                content,
                append,
                ..
            }) => match state.write_handle(path, content, append) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("write_failed", err),
            },
            Ok(Request::Delete {
                path, recursive, ..
            }) => match state.delete_handle(path, recursive) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("delete_failed", err),
            },
            Ok(Request::Mkdir {
                path, parents, ..
            }) => match state.mkdir_handle(path, parents) {
                Ok(receipt) => ok(receipt),
                Err(err) => error("mkdir_failed", err),
            },
            Ok(Request::Ping) => ok(serde_json::json!({ "pong": true })),
            Ok(Request::Shutdown) => {
                let response = ok(serde_json::json!({
                    "message": "WebSpace provider shutting down",
                }));
                let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap_or_default());
                let _ = stdout.flush();
                break;
            }
            Err(err) => error("invalid_request", err.to_string()),
        };

        let _ = writeln!(
            stdout,
            "{}",
            serde_json::to_string(&response).unwrap_or_else(|_| "{\"status\":\"error\",\"code\":\"serialize_failed\",\"message\":\"failed to serialize response\"}".to_string())
        );
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ProviderState {
        ProviderState::default()
    }

    fn temp_base(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "elastos-webspace-provider-{name}-{}",
            now_unix_secs()
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn resolves_elastos_mount() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos")
            .expect("should resolve Elastos mount");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.handle_uri, "localhost://WebSpaces/Elastos");
                assert!(handle.traversable);
                assert_eq!(handle.kind, "dynamic-webspace");
                assert_eq!(handle.resolver, "builtin");
            }
            _ => panic!("expected mounted handle"),
        }
    }

    #[test]
    fn resolves_content_endpoint() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos/content/QmExampleCid")
            .expect("should resolve content endpoint");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "file-endpoint");
                assert!(!handle.traversable);
                assert_eq!(handle.target_uri.as_deref(), Some("elastos://QmExampleCid"));
            }
            _ => panic!("expected file endpoint"),
        }
    }

    #[test]
    fn resolves_peer_folder() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos/peer/alice")
            .expect("should resolve peer folder");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "folder-handle");
                assert!(handle.traversable);
                assert_eq!(handle.target_uri.as_deref(), Some("elastos://peer/alice"));
            }
            _ => panic!("expected folder handle"),
        }
    }

    #[test]
    fn rejects_deeper_peer_traversal() {
        let state = state();
        let err = resolve_path(&state, "localhost://WebSpaces/Elastos/peer/alice/messages")
            .expect_err("deeper peer traversal should fail");
        assert!(err.contains("peer handles do not support traversal"));
    }

    #[test]
    fn rejects_deeper_did_traversal() {
        let state = state();
        let err = resolve_path(
            &state,
            "localhost://WebSpaces/Elastos/did/did:key:z6Mk/example",
        )
        .expect_err("deeper did traversal should fail");
        assert!(err.contains("did handles do not support traversal"));
    }

    #[test]
    fn rejects_deeper_ai_traversal() {
        let state = state();
        let err = resolve_path(&state, "localhost://WebSpaces/Elastos/ai/openai/gpt-5.4")
            .expect_err("deeper ai traversal should fail");
        assert!(err.contains("ai handles do not support traversal"));
    }

    #[test]
    fn init_payload_advertises_supported_and_unsupported_ops() {
        let payload = init_payload(&state());
        let supported = payload["supported_ops"]
            .as_array()
            .expect("supported ops should be an array");
        let unsupported = payload["unsupported_ops"]
            .as_array()
            .expect("unsupported ops should be an array");

        assert!(supported.iter().any(|value| value == "resolve"));
        assert!(supported.iter().any(|value| value == "read"));
        assert!(supported.iter().any(|value| value == "mount"));
        assert!(supported.iter().any(|value| value == "adapters"));
        assert!(supported.iter().any(|value| value == "register_adapter"));
        assert!(supported.iter().any(|value| value == "unregister_adapter"));
        assert!(supported.iter().any(|value| value == "check_adapter"));
        assert!(supported.iter().any(|value| value == "unmount"));
        assert!(supported.iter().any(|value| value == "index"));
        assert!(supported.iter().any(|value| value == "health"));
        assert!(supported.iter().any(|value| value == "refresh"));
        assert!(supported.iter().any(|value| value == "head"));
        assert!(supported.iter().any(|value| value == "cache"));
        assert!(supported.iter().any(|value| value == "cache_status"));
        assert!(supported.iter().any(|value| value == "sync"));
        assert!(supported.iter().any(|value| value == "sync_status"));
        assert!(supported.iter().any(|value| value == "fork"));
        assert!(supported.iter().any(|value| value == "write"));
        assert!(supported.iter().any(|value| value == "delete"));
        assert!(supported.iter().any(|value| value == "mkdir"));
        assert!(unsupported.is_empty());
    }

    #[test]
    fn lists_root_mounts() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces").expect("should resolve root");
        let entries = list_for(&state, &resolved).expect("should list root mounts");
        let names: Vec<_> = entries.iter().map(|entry| entry.name.clone()).collect();
        assert!(names.contains(&"Elastos".to_string()));
    }

    #[test]
    fn lists_elastos_children() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos")
            .expect("should resolve Elastos mount");
        let entries = list_for(&state, &resolved).expect("should list Elastos children");
        let names: Vec<_> = entries.iter().map(|entry| entry.name.clone()).collect();
        assert!(names.contains(&"_meta.json".to_string()));
        assert!(names.contains(&"content".to_string()));
        assert!(names.contains(&"peer".to_string()));
        assert!(names.contains(&"did".to_string()));
        assert!(names.contains(&"ai".to_string()));
        let content = entries
            .iter()
            .find(|entry| entry.name == "content")
            .expect("content child should be listed");
        assert_eq!(content.provider, "webspace-provider");
        assert_eq!(content.resolver_state, "resolved");
        assert_eq!(content.resolver, "builtin");
        assert_eq!(content.kind, "folder-handle");
        assert_eq!(content.target_uri.as_deref(), Some("elastos://<cid>"));
        assert!(content.readonly);
    }

    #[test]
    fn stat_exposes_resolver_metadata() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos/content/QmExampleCid")
            .expect("should resolve content endpoint");
        let stat = stat_for(
            &resolved,
            "localhost://WebSpaces/Elastos/content/QmExampleCid",
        );
        assert_eq!(stat.provider, "webspace-provider");
        assert_eq!(stat.resolver_state, "resolved");
        assert_eq!(stat.resolver, "builtin");
        assert_eq!(stat.kind, "file-endpoint");
        assert_eq!(stat.target_uri.as_deref(), Some("elastos://QmExampleCid"));
        assert!(stat.object_id.starts_with("object:webspace:"));
        assert!(stat.head_id.starts_with("head:webspace:"));
        assert_eq!(stat.cache_state, "metadata_cached");
        assert_eq!(stat.sync_state, "manual_idle");
        assert!(!stat.traversable);
        assert!(stat.readonly);
    }

    #[test]
    fn listing_content_endpoint_fails() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos/content/QmExampleCid")
            .expect("should resolve content endpoint");
        assert!(list_for(&state, &resolved).is_err());
    }

    #[test]
    fn listing_meta_path_fails() {
        let state = state();
        let resolved = resolve_path(&state, "localhost://WebSpaces/Elastos/_meta.json")
            .expect("should resolve metadata path");
        let err = list_for(&state, &resolved).expect_err("metadata path should not list");
        assert!(err.contains("not a directory"));
        assert!(err.contains("_meta.json"));
    }

    #[test]
    fn resolve_request_rejects_meta_path() {
        let state = state();
        let err = resolve_handle_request(
            &state,
            Some("localhost://WebSpaces/Elastos/_meta.json".to_string()),
            None,
        )
        .expect_err("resolve should stay handle-only");
        assert!(err.contains("not metadata files"));
        assert!(err.contains("_meta.json"));
    }

    #[test]
    fn custom_mount_persists_and_resolves_mapped_handles() {
        let base = temp_base("persist");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        let record = state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        assert_eq!(record.moniker, "Google");

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        let root = resolve_path(&reloaded, "localhost://WebSpaces").expect("root should resolve");
        let entries = list_for(&reloaded, &root).expect("root should list");
        assert!(entries.iter().any(|entry| entry.name == "Google"));

        let resolved = resolve_path(
            &reloaded,
            "localhost://WebSpaces/Google/Drive/Project X/file.pdf",
        )
        .expect("mapped child should resolve to external handle");
        match resolved {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "external-object-handle");
                assert_eq!(
                    handle.target_uri.as_deref(),
                    Some("google://drive/Drive/Project X/file.pdf")
                );
                assert_eq!(handle.resolver, "google-drive");
                assert_eq!(handle.resolver_state, "mapped-unavailable");
                assert!(!handle.traversable);
            }
            _ => panic!("expected external mapped handle"),
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn head_persists_for_custom_mapped_handle() {
        let base = temp_base("heads");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        let handle = handle_from_resolved_path(
            resolve_path(
                &state,
                "localhost://WebSpaces/Google/Drive/Project X/file.pdf",
            )
            .expect("mapped child should resolve"),
        )
        .expect("resolved path should be a handle");
        let head = state
            .upsert_head_for_handle(&handle, "metadata_only", false)
            .expect("head should persist");
        assert_eq!(head.schema, HEAD_RECORD_SCHEMA);
        assert_eq!(head.handle_uri, handle.handle_uri);
        assert!(head.object_id.starts_with("object:webspace:"));
        assert!(head.head_id.starts_with("head:webspace:"));
        assert_eq!(head.cache_state, "metadata_cached");
        assert_eq!(head.sync_state, "manual_idle");
        assert!(!head.dirty);

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        assert_eq!(reloaded.heads.len(), 1);
        assert_eq!(reloaded.heads[0].handle_uri, head.handle_uri);
        assert_eq!(reloaded.heads[0].head_id, head.head_id);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn custom_mount_index_persists_and_lists_children() {
        let base = temp_base("index");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        let indexed = state
            .replace_index(
                "Google",
                vec![
                    WebSpaceIndexInput {
                        path: "Drive".to_string(),
                        kind: "directory".to_string(),
                        target_uri: None,
                        resolver_state: Some("indexed".to_string()),
                        readonly: None,
                        description: Some("Drive folder from resolver index.".to_string()),
                    },
                    WebSpaceIndexInput {
                        path: "Drive/Project X/file.pdf".to_string(),
                        kind: "file".to_string(),
                        target_uri: None,
                        resolver_state: Some("indexed".to_string()),
                        readonly: None,
                        description: Some("Project file from resolver index.".to_string()),
                    },
                    WebSpaceIndexInput {
                        path: "Shared/report.md".to_string(),
                        kind: "file".to_string(),
                        target_uri: Some("google://drive/shared/report.md".to_string()),
                        resolver_state: Some("indexed".to_string()),
                        readonly: None,
                        description: Some("Shared report from resolver index.".to_string()),
                    },
                ],
            )
            .expect("index should persist");
        assert_eq!(indexed.len(), 3);

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        assert_eq!(reloaded.index_entries.len(), 3);

        let google = resolve_path(&reloaded, "localhost://WebSpaces/Google")
            .expect("Google mount should resolve");
        let entries = list_for(&reloaded, &google).expect("Google mount should list");
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"_meta.json"));
        assert!(names.contains(&"Drive"));
        assert!(names.contains(&"Shared"));
        let shared = entries
            .iter()
            .find(|entry| entry.name == "Shared")
            .expect("implicit parent folder should be listed");
        assert_eq!(shared.kind, "indexed-directory");
        assert_eq!(shared.resolver_state, "indexed-virtual");

        let project = resolve_path(&reloaded, "localhost://WebSpaces/Google/Drive/Project X")
            .expect("indexed virtual project folder should resolve");
        let project_entries = list_for(&reloaded, &project).expect("project folder should list");
        assert!(project_entries.iter().any(|entry| entry.name == "file.pdf"));

        let file = resolve_path(
            &reloaded,
            "localhost://WebSpaces/Google/Drive/Project X/file.pdf",
        )
        .expect("indexed file should resolve");
        match file {
            ResolvedPath::Handle { handle } => {
                assert_eq!(handle.kind, "indexed-file");
                assert_eq!(handle.resolver_state, "indexed");
                assert!(!handle.traversable);
                assert_eq!(
                    handle.target_uri.as_deref(),
                    Some("google://drive/Drive/Project X/file.pdf")
                );
                assert_eq!(handle.resolver, "google-drive");
            }
            _ => panic!("expected indexed file handle"),
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn refresh_replaces_index_and_persists_refreshed_head() {
        let base = temp_base("refresh");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        state
            .replace_index(
                "Google",
                vec![WebSpaceIndexInput {
                    path: "Old/file.txt".to_string(),
                    kind: "file".to_string(),
                    target_uri: None,
                    resolver_state: Some("stale".to_string()),
                    readonly: None,
                    description: None,
                }],
            )
            .expect("initial index should persist");

        let receipt = state
            .refresh_handle(
                "localhost://WebSpaces/Google".to_string(),
                Some(vec![
                    WebSpaceIndexInput {
                        path: "Drive".to_string(),
                        kind: "directory".to_string(),
                        target_uri: None,
                        resolver_state: Some("refreshed".to_string()),
                        readonly: None,
                        description: Some("Drive folder refreshed from resolver.".to_string()),
                    },
                    WebSpaceIndexInput {
                        path: "Drive/Project X/file.pdf".to_string(),
                        kind: "file".to_string(),
                        target_uri: None,
                        resolver_state: Some("refreshed".to_string()),
                        readonly: None,
                        description: Some("Project file refreshed from resolver.".to_string()),
                    },
                ]),
            )
            .expect("refresh should persist index and head");

        assert_eq!(receipt["schema"], "elastos.webspace.refresh-receipt/v1");
        assert_eq!(receipt["index_entry_count"], 2);
        assert_eq!(receipt["head"]["status"], "resolver_refreshed");
        assert_eq!(receipt["head"]["cache_state"], "metadata_cached");
        assert!(receipt["head"]["last_cached_at"].as_u64().unwrap_or(0) > 0);
        assert_eq!(state.index_entries.len(), 2);
        assert!(!state
            .index_entries
            .iter()
            .any(|entry| entry.path == "Old/file.txt"));

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        assert_eq!(reloaded.index_entries.len(), 2);
        assert_eq!(reloaded.heads.len(), 1);
        assert_eq!(reloaded.heads[0].status, "resolver_refreshed");

        let project = resolve_path(&reloaded, "localhost://WebSpaces/Google/Drive/Project X")
            .expect("refreshed project folder should resolve");
        let entries = list_for(&reloaded, &project).expect("project folder should list");
        let file = entries
            .iter()
            .find(|entry| entry.name == "file.pdf")
            .expect("refreshed file should be listed");
        assert_eq!(file.resolver_state, "refreshed");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn health_reports_external_resolver_attention_and_metadata_readiness() {
        let base = temp_base("health");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");

        let attention = state.health_report(None).expect("health should report");
        assert_eq!(attention["schema"], "elastos.webspace.health/v1");
        assert_eq!(attention["state"], "attention");
        assert_eq!(attention["live_adapter_count"], 1);
        let google = attention["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["moniker"] == "Google")
            .expect("Google health should be listed");
        assert_eq!(google["state"], "mounted_no_index");
        assert_eq!(google["live_adapter"], false);
        assert_eq!(google["adapter_state"], "not_registered");

        state
            .refresh_handle(
                "Google".to_string(),
                Some(vec![WebSpaceIndexInput {
                    path: "Drive/file.pdf".to_string(),
                    kind: "file".to_string(),
                    target_uri: None,
                    resolver_state: Some("refreshed".to_string()),
                    readonly: None,
                    description: None,
                }]),
            )
            .expect("refresh should make metadata ready");
        let ready = state
            .health_report(Some("Google".to_string()))
            .expect("filtered health should report");
        assert_eq!(ready["state"], "metadata_ready");
        assert_eq!(ready["mount_count"], 1);
        assert_eq!(ready["user_mount_count"], 1);
        assert_eq!(ready["index_entry_count"], 1);
        assert_eq!(ready["head_count"], 1);
        assert_eq!(ready["dirty_head_count"], 0);
        assert_eq!(ready["mounts"][0]["state"], "metadata_ready");
        assert_eq!(ready["mounts"][0]["index_entry_count"], 1);

        state
            .fork_mount(
                "localhost://WebSpaces/Google/Drive/file.pdf".to_string(),
                "ProjectFork".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("fork should create a dirty mutable head");
        let dirty = state
            .health_report(None)
            .expect("dirty fork health should report");
        assert_eq!(dirty["state"], "attention");
        assert_eq!(dirty["dirty_head_count"], 1);
        let fork = dirty["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["moniker"] == "ProjectFork")
            .expect("fork health should be listed");
        assert_eq!(fork["state"], "dirty");
        assert_eq!(fork["dirty_head_count"], 1);

        let google_still_ready = state
            .health_report(Some("Google".to_string()))
            .expect("filtered Google health should ignore fork heads");
        assert_eq!(google_still_ready["state"], "metadata_ready");
        assert_eq!(google_still_ready["dirty_head_count"], 0);

        state
            .sync_handle("localhost://WebSpaces/ProjectFork".to_string())
            .expect("sync should clear the dirty fork head");
        let synced = state
            .health_report(Some("ProjectFork".to_string()))
            .expect("synced fork health should report");
        assert_eq!(synced["state"], "metadata_ready");
        assert_eq!(synced["dirty_head_count"], 0);
        assert_eq!(synced["mounts"][0]["state"], "metadata_ready");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn adapter_registry_persists_and_informs_health() {
        let base = temp_base("adapter_registry");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        let adapter = state
            .upsert_adapter(
                "google-drive".to_string(),
                Some("Google Drive".to_string()),
                Some("https://token:secret@example.test/drive".to_string()),
                Some("google-drive-provider".to_string()),
                Some("connected".to_string()),
                vec!["metadata_index".to_string(), "read_bytes".to_string()],
                Some(true),
                Some("Google Drive resolver adapter.".to_string()),
            )
            .expect("adapter should register");
        assert_eq!(adapter.resolver, "google-drive");
        assert_eq!(adapter.state, "connected");
        assert_eq!(adapter.capabilities, vec!["metadata_index", "read_bytes"]);

        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                None,
                Some(true),
                None,
                None,
                None,
            )
            .expect("mount should persist");

        let health = state.health_report(None).expect("health should report");
        assert_eq!(health["live_adapter_count"], 2);
        assert_eq!(health["connected_adapter_count"], 1);
        assert_eq!(health["checked_adapter_count"], 0);
        let google = health["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|mount| mount["moniker"] == "Google")
            .expect("Google mount should be listed");
        assert_eq!(google["live_adapter"], true);
        assert_eq!(google["adapter_state"], "connected");
        assert_eq!(google["adapter"]["provider"], "google-drive-provider");
        assert_eq!(
            google["adapter"]["health"]["status"],
            "connected_unverified"
        );
        assert_eq!(
            google["adapter"]["endpoint_uri"],
            "https://redacted@example.test/drive"
        );

        let failed = state
            .check_adapter(
                "google-drive".to_string(),
                Some("failed".to_string()),
                None,
                Some("upstream_timeout".to_string()),
                Vec::new(),
            )
            .expect("adapter health check should persist");
        assert_eq!(
            failed["schema"],
            "elastos.webspace.adapter-health-receipt/v1"
        );
        assert_eq!(failed["adapter"]["state"], "unavailable");
        assert_eq!(failed["adapter"]["health"]["status"], "unavailable");
        assert_eq!(
            failed["adapter"]["health"]["last_error_code"],
            "upstream_timeout"
        );

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("reload should configure persistent path");
        assert_eq!(reloaded.adapters.len(), 1);
        assert_eq!(reloaded.adapters[0].resolver, "google-drive");
        assert_eq!(reloaded.adapters[0].state, "unavailable");
        assert_eq!(
            reloaded.adapters[0].last_check_error_code.as_deref(),
            Some("upstream_timeout")
        );
        let checked_health = reloaded
            .health_report(Some("Google".to_string()))
            .expect("checked health should report");
        assert_eq!(checked_health["connected_adapter_count"], 0);
        assert_eq!(checked_health["checked_adapter_count"], 1);
        assert_eq!(checked_health["mounts"][0]["adapter_state"], "unavailable");
        assert_eq!(
            checked_health["mounts"][0]["adapter"]["health"]["status"],
            "unavailable"
        );

        let removed = reloaded
            .unregister_adapter("google-drive")
            .expect("adapter should unregister");
        assert_eq!(removed.resolver, "google-drive");
        assert!(reloaded.adapters.is_empty());
        let health = reloaded
            .health_report(Some("Google".to_string()))
            .expect("health should report after unregister");
        assert_eq!(health["mounts"][0]["adapter_state"], "not_registered");
        assert_eq!(health["mounts"][0]["live_adapter"], false);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn unmount_removes_index_and_head_state() {
        let base = temp_base("unmount-index");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        state
            .replace_index(
                "Google",
                vec![WebSpaceIndexInput {
                    path: "Drive/file.pdf".to_string(),
                    kind: "file".to_string(),
                    target_uri: None,
                    resolver_state: Some("indexed".to_string()),
                    readonly: None,
                    description: None,
                }],
            )
            .expect("index should persist");
        let handle = handle_from_resolved_path(
            resolve_path(&state, "localhost://WebSpaces/Google/Drive/file.pdf")
                .expect("indexed file should resolve"),
        )
        .expect("resolved path should be a handle");
        state
            .upsert_head_for_handle(&handle, "metadata_only", false)
            .expect("head should persist");
        assert_eq!(state.index_entries.len(), 1);
        assert_eq!(state.heads.len(), 1);

        state.unmount("Google").expect("unmount should succeed");
        assert!(state.index_entries.is_empty());
        assert!(state.heads.is_empty());

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        assert!(reloaded.index_entries.is_empty());
        assert!(reloaded.heads.is_empty());
        let root = resolve_path(&reloaded, "localhost://WebSpaces").expect("root should resolve");
        let entries = list_for(&reloaded, &root).expect("root should list");
        assert!(!entries.iter().any(|entry| entry.name == "Google"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn fork_creates_mutable_mount_and_dirty_head() {
        let base = temp_base("fork");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");

        let (record, head) = state
            .fork_mount(
                "localhost://WebSpaces/Google/Drive/Project X/file.pdf".to_string(),
                "ProjectFork".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("fork should create mutable WebSpace mount");

        assert_eq!(record.moniker, "ProjectFork");
        assert!(!record.readonly);
        assert_eq!(
            record.forked_from.as_deref(),
            Some("localhost://WebSpaces/Google/Drive/Project X/file.pdf")
        );
        assert_eq!(head.status, "forked_metadata_only");
        assert!(head.dirty);
        assert_eq!(head.sync_state, "manual_pending");
        assert_eq!(head.handle_uri, "localhost://WebSpaces/ProjectFork");

        let fork = handle_from_resolved_path(
            resolve_path(&state, "localhost://WebSpaces/ProjectFork")
                .expect("forked mount should resolve"),
        )
        .expect("forked mount should be a handle");
        assert_eq!(fork.resolver_state, "mounted-mutable");
        assert_eq!(
            fork.forked_from.as_deref(),
            Some("localhost://WebSpaces/Google/Drive/Project X/file.pdf")
        );
        let cache = cache_status_from_head(&head);
        assert_eq!(cache["content_cached"], false);
        let sync = sync_status_from_head(&head);
        assert_eq!(sync["dirty"], true);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn sync_clears_dirty_fork_head_without_claiming_byte_sync() {
        let base = temp_base("sync-fork");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-thumbnails".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        let (_record, forked_head) = state
            .fork_mount(
                "localhost://WebSpaces/Google/Drive/Project X/file.pdf".to_string(),
                "ProjectFork".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("fork should create dirty head");
        assert!(forked_head.dirty);

        let receipt = state
            .sync_handle("localhost://WebSpaces/ProjectFork".to_string())
            .expect("sync should persist a clean metadata head");
        assert_eq!(receipt["schema"], "elastos.webspace.sync-receipt/v1");
        assert_eq!(receipt["content_synced"], false);
        assert_eq!(receipt["head"]["status"], "metadata_synced");
        assert_eq!(receipt["head"]["dirty"], false);
        assert_eq!(receipt["head"]["sync_state"], "manual_synced");
        assert!(receipt["head"]["last_synced_at"].as_u64().unwrap_or(0) > 0);

        let health = state
            .health_report(Some("ProjectFork".to_string()))
            .expect("fork health should report");
        assert_eq!(health["state"], "metadata_ready");
        assert_eq!(health["mounts"][0]["dirty_head_count"], 0);

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        let synced = reloaded
            .heads
            .iter()
            .find(|head| head.handle_uri == "localhost://WebSpaces/ProjectFork")
            .expect("synced fork head should persist");
        assert!(!synced.dirty);
        assert_eq!(synced.status, "metadata_synced");
        assert_eq!(synced.sync_state, "manual_synced");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn mutable_mount_materializes_objects_and_persists_them() {
        let base = temp_base("materialized");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        let record = state
            .upsert_mount(
                "Project".to_string(),
                "local://project".to_string(),
                None,
                Some("local-materialized".to_string()),
                Some("Mutable project WebSpace.".to_string()),
                Some(false),
                Some("metadata-and-bytes".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mutable mount should persist");
        assert!(!record.readonly);
        assert_eq!(record.access_policy, DEFAULT_MUTABLE_ACCESS_POLICY);

        let mkdir = state
            .mkdir_handle("localhost://WebSpaces/Project/Docs".to_string(), false)
            .expect("mkdir should materialize a local directory");
        assert_eq!(mkdir["schema"], "elastos.webspace.mkdir-receipt/v1");
        assert_eq!(mkdir["object"]["kind"], "materialized-directory");

        let write = state
            .write_handle(
                "localhost://WebSpaces/Project/Docs/notes.txt".to_string(),
                b"hello".to_vec(),
                false,
            )
            .expect("write should materialize local bytes");
        assert_eq!(write["schema"], "elastos.webspace.write-receipt/v1");
        assert_eq!(write["object"]["kind"], "materialized-file");
        assert_eq!(write["object"]["size"], 5);
        assert_eq!(write["head"]["status"], "materialized_local");
        assert_eq!(
            cache_status_from_head(
                &serde_json::from_value::<WebSpaceHead>(write["head"].clone())
                    .expect("write head should deserialize")
            )["content_cached"],
            true
        );

        state
            .write_handle(
                "localhost://WebSpaces/Project/Docs/notes.txt".to_string(),
                b" world".to_vec(),
                true,
            )
            .expect("append should update local bytes");
        let resolved = resolve_path(&state, "localhost://WebSpaces/Project/Docs/notes.txt")
            .expect("materialized file should resolve");
        let handle = handle_from_resolved_path(resolved).expect("resolved path should be a handle");
        assert_eq!(handle.kind, "materialized-file");
        assert_eq!(handle.size, 11);
        let object = state
            .object_for_handle(&handle)
            .expect("materialized object should be stored");
        assert_eq!(object.content, b"hello world");

        let docs = resolve_path(&state, "localhost://WebSpaces/Project/Docs")
            .expect("materialized directory should resolve");
        let entries = list_for(&state, &docs).expect("materialized directory should list");
        assert!(entries.iter().any(|entry| entry.name == "notes.txt"));

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent object table");
        let persisted = handle_from_resolved_path(
            resolve_path(&reloaded, "localhost://WebSpaces/Project/Docs/notes.txt")
                .expect("persisted materialized file should resolve"),
        )
        .expect("persisted path should be a handle");
        assert_eq!(persisted.kind, "materialized-file");
        assert_eq!(persisted.size, 11);

        let delete = reloaded
            .delete_handle(
                "localhost://WebSpaces/Project/Docs/notes.txt".to_string(),
                false,
            )
            .expect("delete should remove local file");
        assert_eq!(delete["schema"], "elastos.webspace.delete-receipt/v1");
        assert_eq!(delete["removed_count"], 1);
        let entries = list_for(
            &reloaded,
            &resolve_path(&reloaded, "localhost://WebSpaces/Project/Docs")
                .expect("directory should still resolve"),
        )
        .expect("directory should list after delete");
        assert!(!entries.iter().any(|entry| entry.name == "notes.txt"));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn cache_handle_materializes_adapter_bytes_without_dirty_sync_debt() {
        let base = temp_base("cache-bytes");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_adapter(
                "google-drive".to_string(),
                Some("Google Drive".to_string()),
                Some("provider:google-drive-adapter".to_string()),
                Some("google-drive-adapter".to_string()),
                Some("connected".to_string()),
                vec!["metadata_index".to_string(), "read_bytes".to_string()],
                Some(true),
                Some("Google Drive adapter.".to_string()),
            )
            .expect("adapter should persist");
        state
            .upsert_mount(
                "Google".to_string(),
                "google://drive".to_string(),
                None,
                Some("google-drive".to_string()),
                Some("Google Drive WebSpace mount.".to_string()),
                Some(true),
                Some("metadata-and-bytes".to_string()),
                Some("manual".to_string()),
                None,
            )
            .expect("mount should persist");
        state
            .replace_index(
                "Google",
                vec![WebSpaceIndexInput {
                    path: "Drive/Project X/file.pdf".to_string(),
                    kind: "file".to_string(),
                    target_uri: Some("google://drive/Drive/Project X/file.pdf".to_string()),
                    resolver_state: Some("indexed".to_string()),
                    readonly: Some(true),
                    description: Some("Indexed file.".to_string()),
                }],
            )
            .expect("index should persist");

        let receipt = state
            .cache_handle(
                "localhost://WebSpaces/Google/Drive/Project X/file.pdf".to_string(),
                Some(b"adapter bytes".to_vec()),
                Some("application/pdf".to_string()),
                Some(serde_json::json!({
                    "schema": "elastos.webspace.adapter-cache-source/v1",
                    "provider": "google-drive-adapter"
                })),
            )
            .expect("cache should materialize adapter bytes");
        assert_eq!(receipt["schema"], "elastos.webspace.cache-receipt/v1");
        assert_eq!(receipt["action"], "content_cached");
        assert_eq!(receipt["content_cached"], true);
        assert_eq!(receipt["dirty"], false);
        assert_eq!(receipt["object"]["kind"], "materialized-file");
        assert_eq!(receipt["object"]["size"], 13);
        assert_eq!(receipt["head"]["status"], "materialized_cached");
        assert_eq!(receipt["head"]["dirty"], false);
        assert_eq!(receipt["head"]["cache_state"], "content_cached");

        let resolved = handle_from_resolved_path(
            resolve_path(
                &state,
                "localhost://WebSpaces/Google/Drive/Project X/file.pdf",
            )
            .expect("cached path should resolve"),
        )
        .expect("cached path should be a handle");
        assert_eq!(resolved.kind, "materialized-file");
        let object = state
            .object_for_handle(&resolved)
            .expect("cached object should be stored");
        assert_eq!(object.content, b"adapter bytes");
        assert!(!object.dirty);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn custom_mount_rejects_reserved_elastos_moniker() {
        let mut state = ProviderState::default();
        let err = state
            .upsert_mount(
                "Elastos".to_string(),
                "elastos://override".to_string(),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("built-in moniker should be reserved");
        assert!(err.contains("built-in WebSpace"));
    }

    #[test]
    fn unmount_removes_persisted_mount() {
        let base = temp_base("unmount");
        let mut state = ProviderState::default();
        state
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should configure persistent path");
        state
            .upsert_mount(
                "Docs".to_string(),
                "https://example.com/docs".to_string(),
                None,
                Some("https".to_string()),
                None,
                Some(true),
                None,
                None,
                None,
            )
            .expect("mount should persist");
        state.unmount("Docs").expect("unmount should persist");

        let mut reloaded = ProviderState::default();
        reloaded
            .configure(serde_json::json!({ "base_path": base.display().to_string() }))
            .expect("init should reload persistent path");
        let err = resolve_path(&reloaded, "localhost://WebSpaces/Docs")
            .expect_err("unmounted WebSpace should be gone");
        assert!(err.contains("unknown WebSpace moniker"));
        let _ = fs::remove_dir_all(base);
    }
}
