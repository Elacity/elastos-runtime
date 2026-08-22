use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Cursor, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context as _};
use base64::Engine as _;
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_common::protected_content::{
    DecryptSessionRequestV1, KeyReleaseRequestV1, ReleaseReceiptV1, RightsDecisionReceiptV1,
    SealedObjectV1, DECRYPT_SESSION_REQUEST_SCHEMA, DECRYPT_SESSION_SCHEMA,
    KEY_RELEASE_REQUEST_SCHEMA, RELEASE_RECEIPT_SCHEMA, RIGHTS_DECISION_RECEIPT_SCHEMA,
};
use elastos_protected_content_provider_contracts::{
    ValidatedClearFmp4MediaSessionLayoutV1, MAX_PROTECT_MEDIA_PART_BYTES_V1,
    MAX_PROTECT_MEDIA_SEGMENTS_V1,
};
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderInvocation, ProviderInvocationTransport, ProviderRegistry,
    ProviderTransfer, ResourceRequest, ResourceResponse,
};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use zip::{ZipArchive, ZipWriter};

const LIBRARY_OBJECT_SCHEMA: &str = "elastos.library.object/v1";
const LIBRARY_ROOT_SCHEMA: &str = "elastos.library.root/v1";
const LIBRARY_EVENT_SCHEMA: &str = "elastos.library.event/v1";
const LIBRARY_ARCHIVE_ENTRIES_SCHEMA: &str = "elastos.library.archive-entries/v1";
const LIBRARY_ARCHIVE_EXTRACT_ENTRIES_SCHEMA: &str = "elastos.library.archive-extract-entries/v1";
const LIBRARY_ARCHIVE_PREVIEW_ENTRY_SCHEMA: &str = "elastos.library.archive-preview-entry/v1";
const LIBRARY_VISIBILITY_SCHEMA: &str = "elastos.library.visibility/v1";
const LIBRARY_TRASH_RECORD_SCHEMA: &str = "elastos.library.trash-record/v1";
const MAX_LIBRARY_EVENTS: usize = 256;
const MAX_ARCHIVE_LIST_ENTRIES: usize = 512;
const MAX_ARCHIVE_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_LIBRARY_PROTECTED_MEDIA_DECLARATION_BYTES: usize = 255;
const RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE: &str = "Runtime custody publish input invalid";
const RUNTIME_CUSTODY_PUBLISH_INACTIVE_MESSAGE: &str =
    "Runtime custody publishing is not active yet";

static LIBRARY_EVENT_NOTIFY: OnceLock<tokio::sync::Notify> = OnceLock::new();

pub(crate) fn library_event_notifier() -> &'static tokio::sync::Notify {
    LIBRARY_EVENT_NOTIFY.get_or_init(tokio::sync::Notify::new)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryObject {
    schema: &'static str,
    uri: String,
    name: String,
    kind: &'static str,
    mime: String,
    size: u64,
    created_at: u64,
    modified_at: u64,
    revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    viewers: Vec<LibraryViewerOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thumbnail_uri: Option<String>,
    availability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published_cid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
    published: bool,
    shared: bool,
    capabilities: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryViewerOption {
    id: String,
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    default: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LibraryRoot {
    schema: &'static str,
    id: &'static str,
    label: &'static str,
    uri: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryPublishRecord {
    schema: String,
    object_uri: String,
    cid: String,
    published_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unpublished_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    share_grants: Vec<LibraryShareGrant>,
    #[serde(default = "default_publish_content_security")]
    content_security: Value,
    receipt: Value,
    availability: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryShareGrant {
    schema: String,
    grant_id: String,
    recipient: String,
    uri: String,
    cid: String,
    policy: String,
    #[serde(default = "default_share_key_release")]
    key_release: Value,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryTrashRecord {
    schema: String,
    trash_uri: String,
    original_uri: String,
    original_name: String,
    trashed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryEvent {
    schema: String,
    event_id: String,
    op: String,
    uri: String,
    at: u64,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    details: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ObjectProviderRequest {
    Roots {
        principal_id: String,
    },
    List {
        principal_id: String,
        #[serde(default)]
        uri: Option<String>,
    },
    Stat {
        principal_id: String,
        uri: String,
    },
    Read {
        principal_id: String,
        uri: String,
    },
    Download {
        principal_id: String,
        uri: String,
    },
    ExtractArchive {
        principal_id: String,
        uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    ArchiveEntries {
        principal_id: String,
        uri: String,
    },
    ArchivePreviewEntry {
        principal_id: String,
        uri: String,
        entry: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    ArchiveExtractEntries {
        principal_id: String,
        uri: String,
        destination_uri: String,
        #[serde(default)]
        entries: Vec<String>,
        #[serde(default)]
        conflict_policy: Option<String>,
        #[serde(default)]
        if_revision: Option<String>,
        #[serde(default)]
        cancel: bool,
    },
    CompressArchive {
        principal_id: String,
        #[serde(default)]
        uri: Option<String>,
        #[serde(default)]
        uris: Vec<String>,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Write {
        principal_id: String,
        uri: String,
        data: String,
        #[serde(default)]
        mime: Option<String>,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Mkdir {
        principal_id: String,
        parent_uri: String,
        name: String,
    },
    Rename {
        principal_id: String,
        uri: String,
        name: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Move {
        principal_id: String,
        uri: String,
        target_parent_uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Copy {
        principal_id: String,
        uri: String,
        target_parent_uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Trash {
        principal_id: String,
        uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Restore {
        principal_id: String,
        uri: String,
        #[serde(default)]
        target_uri: Option<String>,
        #[serde(default)]
        if_revision: Option<String>,
    },
    DeletePermanently {
        principal_id: String,
        uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    EmptyTrash {
        principal_id: String,
    },
    Status {
        principal_id: String,
        uri: String,
    },
    Sync {
        principal_id: String,
        uri: String,
    },
    Events {
        principal_id: String,
        #[serde(default)]
        uri: Option<String>,
        #[serde(default)]
        since: Option<u64>,
        #[serde(default)]
        limit: Option<usize>,
    },
    Publish {
        principal_id: String,
        uri: String,
        #[serde(default)]
        if_revision: Option<String>,
        #[serde(default)]
        protection: Option<LibraryPublishProtectionRequest>,
    },
    Unpublish {
        principal_id: String,
        uri: String,
        #[serde(default)]
        if_revision: Option<String>,
    },
    Repair {
        principal_id: String,
        uri: String,
    },
    Share {
        principal_id: String,
        uri: String,
        #[serde(default)]
        recipients: Vec<String>,
        #[serde(default)]
        policy: Option<String>,
        #[serde(default)]
        key_release_policy: Option<String>,
    },
    SharedAccess {
        principal_id: String,
        uri: String,
        recipient: String,
        #[serde(default)]
        recipient_proof: Option<Value>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum LibraryPublishProtectionRequest {
    RuntimeCustody { mime_type: String, codecs: String },
}

struct LoadedRuntimeCustodyPublishInput {
    mime_type: String,
    codecs: String,
    clear_init_segment: Vec<u8>,
    clear_segments: Vec<Vec<u8>>,
}

pub struct ObjectProvider {
    data_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl ObjectProvider {
    pub fn new(data_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self { data_dir, registry }
    }
}

#[async_trait::async_trait]
impl Provider for ObjectProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "object provider does not support URI resource routing; use raw operations".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["object"]
    }

    fn name(&self) -> &'static str {
        "object-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let request = match serde_json::from_value::<ObjectProviderRequest>(request.clone()) {
            Ok(request) => request,
            Err(err) => return Ok(provider_error("invalid_request", &err.to_string())),
        };

        let data_dir = self.data_dir.clone();
        let result = match request {
            ObjectProviderRequest::Publish {
                principal_id,
                uri,
                if_revision,
                protection,
            } => {
                let Some(registry) = self.registry.upgrade() else {
                    return Ok(provider_error(
                        "library_error",
                        "object provider registry unavailable",
                    ));
                };
                library_publish(
                    &data_dir,
                    registry,
                    &principal_id,
                    &uri,
                    if_revision.as_deref(),
                    protection,
                )
                .await
            }
            ObjectProviderRequest::Unpublish {
                principal_id,
                uri,
                if_revision,
            } => {
                let Some(registry) = self.registry.upgrade() else {
                    return Ok(provider_error(
                        "library_error",
                        "object provider registry unavailable",
                    ));
                };
                library_unpublish(
                    &data_dir,
                    registry,
                    &principal_id,
                    &uri,
                    if_revision.as_deref(),
                )
                .await
            }
            ObjectProviderRequest::Repair { principal_id, uri } => {
                let Some(registry) = self.registry.upgrade() else {
                    return Ok(provider_error(
                        "library_error",
                        "object provider registry unavailable",
                    ));
                };
                library_repair(&data_dir, registry, &principal_id, &uri).await
            }
            request @ (ObjectProviderRequest::Status { .. }
            | ObjectProviderRequest::Share { .. }
            | ObjectProviderRequest::SharedAccess { .. }) => {
                handle_library_request_with_protected_content_status(
                    data_dir,
                    request,
                    self.registry.upgrade(),
                )
                .await
            }
            request => {
                tokio::task::spawn_blocking(move || handle_library_request(&data_dir, request))
                    .await
                    .map_err(|err| anyhow!("object provider task failed: {err}"))
                    .and_then(|result| result)
            }
        };

        Ok(match result {
            Ok(data) => provider_ok(data),
            Err(err) => provider_error("library_error", &err.to_string()),
        })
    }
}

/// Handle one raw object provider request inside an isolated provider process.
///
/// The standalone provider owns principal-root object storage and Library event
/// state. Content publish/unpublish/repair are coordinated by Runtime for now,
/// because the current stdio provider ABI has no provider-to-provider
/// invocation channel.
pub fn handle_object_provider_raw_request(data_dir: &Path, request: &Value) -> Value {
    let request = match serde_json::from_value::<ObjectProviderRequest>(request.clone()) {
        Ok(request) => request,
        Err(err) => return provider_error("invalid_request", &err.to_string()),
    };

    let result = match request {
        ObjectProviderRequest::Publish { .. }
        | ObjectProviderRequest::Unpublish { .. }
        | ObjectProviderRequest::Repair { .. } => Err(anyhow!(
            "library content operation requires Runtime content coordinator"
        )),
        request => handle_library_request(data_dir, request),
    };

    match result {
        Ok(data) => provider_ok(data),
        Err(err) => provider_error("library_error", &err.to_string()),
    }
}

pub fn handle_library_upload_bytes(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    mime: Option<&str>,
    if_revision: Option<&str>,
    bytes: &[u8],
) -> anyhow::Result<Value> {
    let object = write_library_file_bytes(data_dir, principal_id, uri, mime, if_revision, bytes)?;
    Ok(provider_ok(json!({
        "object": object,
        "transport": "raw-body",
    })))
}

pub(crate) async fn handle_library_upload_bytes_runtime(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    uri: &str,
    mime: Option<&str>,
    if_revision: Option<&str>,
    bytes: &[u8],
) -> anyhow::Result<Value> {
    if is_webspace_uri(uri) {
        let uri = clean_webspace_uri(uri)?;
        let receipt = webspace_write_bytes(&registry, &uri, bytes).await?;
        let object = webspace_stat_object(data_dir, &registry, &uri).await?;
        return Ok(provider_ok(json!({
            "object": object,
            "transport": "raw-body",
            "provider_receipt": receipt,
        })));
    }
    handle_library_upload_bytes(data_dir, principal_id, uri, mime, if_revision, bytes)
}

pub(crate) struct LibraryDownloadBytes {
    pub(crate) filename: String,
    pub(crate) mime: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LibraryArchiveFormat {
    TarGz,
    Zip,
}

impl LibraryArchiveFormat {
    pub(crate) fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("tar.gz")
            .to_ascii_lowercase()
            .as_str()
        {
            "tar.gz" | "tgz" | "gzip" | "gz" => Ok(Self::TarGz),
            "zip" => Ok(Self::Zip),
            other => bail!("unsupported Library archive format: {other}"),
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::TarGz => "application/gzip",
            Self::Zip => "application/zip",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

pub(crate) fn handle_library_download_bytes_with_format(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<LibraryDownloadBytes> {
    let (object, filename, bytes) =
        library_download_object(data_dir, principal_id, uri, archive_format)?;
    Ok(LibraryDownloadBytes {
        filename,
        mime: object.mime,
        bytes,
    })
}

pub(crate) fn handle_library_download_selection_bytes_with_format(
    data_dir: &Path,
    principal_id: &str,
    uris: &[String],
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<LibraryDownloadBytes> {
    let (filename, bytes) =
        archive_library_selection(data_dir, principal_id, uris, archive_format)?;
    Ok(LibraryDownloadBytes {
        filename,
        mime: archive_format.mime().to_string(),
        bytes,
    })
}

pub(crate) async fn handle_library_download_bytes_runtime(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    uri: &str,
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<LibraryDownloadBytes> {
    if is_webspace_uri(uri) {
        return webspace_download_bytes(data_dir, &registry, uri).await;
    }
    handle_library_download_bytes_with_format(data_dir, principal_id, uri, archive_format)
}

pub(crate) async fn handle_library_download_selection_bytes_runtime(
    data_dir: &Path,
    principal_id: &str,
    uris: &[String],
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<LibraryDownloadBytes> {
    if uris.iter().any(|uri| is_webspace_uri(uri)) {
        bail!("Spaces selections cannot be archived from Library yet");
    }
    handle_library_download_selection_bytes_with_format(
        data_dir,
        principal_id,
        uris,
        archive_format,
    )
}

/// Handle one Library request with Runtime coordination available.
///
/// This bridge keeps publish/share/status content effects Runtime-mediated so
/// Library never gains raw content, Carrier, Kubo/IPFS, or backend authority.
pub async fn handle_object_provider_runtime_request(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    request: &Value,
) -> Value {
    let request = match serde_json::from_value::<ObjectProviderRequest>(request.clone()) {
        Ok(request) => request,
        Err(err) => return provider_error("invalid_request", &err.to_string()),
    };

    let data_dir = data_dir.to_path_buf();
    if library_request_touches_webspace(&request) {
        let result = handle_library_webspace_request(&data_dir, &registry, request).await;
        return match result {
            Ok(data) => provider_ok(data),
            Err(err) => provider_error("library_error", &err.to_string()),
        };
    }

    let result = match request {
        ObjectProviderRequest::Publish {
            principal_id,
            uri,
            if_revision,
            protection,
        } => {
            library_publish(
                &data_dir,
                registry,
                &principal_id,
                &uri,
                if_revision.as_deref(),
                protection,
            )
            .await
        }
        ObjectProviderRequest::Unpublish {
            principal_id,
            uri,
            if_revision,
        } => {
            library_unpublish(
                &data_dir,
                registry,
                &principal_id,
                &uri,
                if_revision.as_deref(),
            )
            .await
        }
        ObjectProviderRequest::Repair { principal_id, uri } => {
            library_repair(&data_dir, registry, &principal_id, &uri).await
        }
        request @ (ObjectProviderRequest::Status { .. }
        | ObjectProviderRequest::Share { .. }
        | ObjectProviderRequest::SharedAccess { .. }) => {
            handle_library_request_with_protected_content_status(data_dir, request, Some(registry))
                .await
        }
        request => tokio::task::spawn_blocking(move || handle_library_request(&data_dir, request))
            .await
            .map_err(|err| anyhow!("object provider task failed: {err}"))
            .and_then(|result| result),
    };

    match result {
        Ok(data) => provider_ok(data),
        Err(err) => provider_error("library_error", &err.to_string()),
    }
}

async fn handle_library_request_with_protected_content_status(
    data_dir: PathBuf,
    request: ObjectProviderRequest,
    registry: Option<Arc<ProviderRegistry>>,
) -> anyhow::Result<Value> {
    let request_is_shared_access = matches!(request, ObjectProviderRequest::SharedAccess { .. });
    let mut data = tokio::task::spawn_blocking(move || handle_library_request(&data_dir, request))
        .await
        .map_err(|err| anyhow!("object provider task failed: {err}"))
        .and_then(|result| result)?;
    if let Some(registry) = registry {
        if request_is_shared_access {
            attach_protected_content_open_chain(&registry, &mut data).await?;
        }
        attach_protected_content_provider_status(&registry, &mut data).await;
    }
    Ok(data)
}

async fn attach_protected_content_open_chain(
    registry: &ProviderRegistry,
    data: &mut Value,
) -> anyhow::Result<()> {
    let object_cid = data
        .get("cid")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("protected shared_access missing cid"))?
        .to_string();
    let Some(access) = data.get_mut("access") else {
        return Ok(());
    };
    let key_release_required = access
        .get("key_release")
        .and_then(|value| value.get("required"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !key_release_required {
        return Ok(());
    }

    let content_security = access
        .get("content_security")
        .cloned()
        .ok_or_else(|| anyhow!("protected shared_access missing content_security"))?;
    let sealed_object = protected_content_sealed_object_from_security(&content_security)?;
    let recipient_proof = access
        .get("recipient_proof")
        .cloned()
        .ok_or_else(|| anyhow!("protected shared_access missing recipient_proof"))?;
    let principal_id = recipient_proof
        .get("recipient")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("protected shared_access recipient proof missing recipient"))?
        .to_string();
    let session_id = recipient_proof
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("protected shared_access recipient proof missing session_id"))?
        .to_string();
    let now = now_ts();
    let expires_at = now.saturating_add(900);
    let action = "view";
    let reason = "library protected shared_access open";

    let drm_receipt = protected_provider_data(
        registry,
        "drm",
        "open",
        &json!({
            "op": "open",
            "request": {
                "object": sealed_object,
                "principal_id": principal_id,
                "session_id": session_id,
                "action": action,
                "reason": reason
            }
        }),
    )
    .await?;
    reject_forbidden_protected_content_fields(&drm_receipt)?;

    let rights_receipt_value = protected_provider_data(
        registry,
        "rights",
        "has_access_by_content_id",
        &json!({
            "op": "has_access_by_content_id",
            "request": {
                "principal_id": principal_id,
                "session_id": session_id,
                "content_id": object_cid,
                "right": action,
                "reason": reason,
                "policy_ref": sealed_object.rights_policy_cid
            }
        }),
    )
    .await?;
    reject_forbidden_protected_content_fields(&rights_receipt_value)?;
    let rights_receipt: RightsDecisionReceiptV1 =
        serde_json::from_value(rights_receipt_value.clone())
            .context("rights provider returned invalid protected-content receipt")?;
    if rights_receipt.schema != RIGHTS_DECISION_RECEIPT_SCHEMA || !rights_receipt.allowed {
        bail!("rights provider did not allow protected shared_access");
    }

    let key_release_request = KeyReleaseRequestV1 {
        schema: KEY_RELEASE_REQUEST_SCHEMA.to_string(),
        request_id: protected_request_id("key-release", &object_cid, &principal_id, now),
        principal_id: principal_id.clone(),
        session_id: session_id.clone(),
        object_cid: object_cid.clone(),
        action: action.to_string(),
        rights_receipt,
        key_envelope: sealed_object.key_envelope.clone(),
        reason: reason.to_string(),
        expires_at,
    };
    let release_receipt_value = protected_provider_data(
        registry,
        "key",
        "release",
        &json!({
            "op": "release",
            "request": key_release_request
        }),
    )
    .await?;
    reject_forbidden_protected_content_fields(&release_receipt_value)?;
    let release_receipt: ReleaseReceiptV1 =
        serde_json::from_value(release_receipt_value.clone())
            .context("key provider returned invalid release receipt")?;
    if release_receipt.schema != RELEASE_RECEIPT_SCHEMA {
        bail!("key provider returned unsupported release receipt schema");
    }

    let decrypt_request = DecryptSessionRequestV1 {
        schema: DECRYPT_SESSION_REQUEST_SCHEMA.to_string(),
        request_id: protected_request_id("decrypt-session", &object_cid, &principal_id, now),
        principal_id: principal_id.clone(),
        session_id: session_id.clone(),
        object_cid: object_cid.clone(),
        action: action.to_string(),
        viewer_interface: sealed_object.viewer.required_interface.clone(),
        release_receipt,
        output_kind: "rendered".to_string(),
        reason: reason.to_string(),
        expires_at,
    };
    let decrypt_session_value = protected_provider_data(
        registry,
        "decrypt",
        "open_session",
        &json!({
            "op": "open_session",
            "request": decrypt_request
        }),
    )
    .await?;
    reject_forbidden_protected_content_fields(&decrypt_session_value)?;
    if decrypt_session_value.get("schema").and_then(Value::as_str) != Some(DECRYPT_SESSION_SCHEMA) {
        bail!("decrypt provider returned unsupported decrypt session schema");
    }

    if let Some(open) = access.get_mut("open").and_then(Value::as_object_mut) {
        open.insert(
            "provider".to_string(),
            Value::String("decrypt-provider".to_string()),
        );
        open.insert(
            "transport".to_string(),
            Value::String("runtime-protected-provider-chain".to_string()),
        );
        open.insert(
            "status".to_string(),
            Value::String("ready_for_protected_viewer_session".to_string()),
        );
        open.insert(
            "protected_content".to_string(),
            json!({
                "schema": "elastos.library.protected-open/v1",
                "action": action,
                "provider_chain": ["drm-provider.open", "rights-provider.has_access_by_content_id", "key-provider.release", "decrypt-provider.open_session"],
                "drm_receipt": drm_receipt,
                "rights_receipt": rights_receipt_value,
                "key_release_receipt": release_receipt_value,
                "decrypt_session": decrypt_session_value,
                "viewer": {
                    "required_interface": sealed_object.viewer.required_interface,
                    "handoff": "viewer_capsule_session"
                },
                "raw_cek_exposed": false,
                "raw_plaintext_exposed": false
            }),
        );
    }
    Ok(())
}

async fn protected_provider_data(
    registry: &ProviderRegistry,
    scheme: &str,
    op: &str,
    request: &Value,
) -> anyhow::Result<Value> {
    let response = registry
        .send_raw(scheme, request)
        .await
        .map_err(|err| anyhow!("{scheme} provider unavailable for protected {op}: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider returned error");
        bail!("{scheme} provider rejected protected {op}: {message}");
    }
    response
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("{scheme} provider protected {op} response missing data"))
}

fn protected_request_id(kind: &str, object_cid: &str, principal_id: &str, now: u64) -> String {
    let digest = Sha256::digest(format!("{kind}:{object_cid}:{principal_id}:{now}"));
    format!("{kind}:{}", hex::encode(&digest[..16]))
}

fn reject_forbidden_protected_content_fields(value: &Value) -> anyhow::Result<()> {
    const FORBIDDEN: &[&str] = &[
        "raw_cek",
        "cek",
        "raw_plaintext",
        "plaintext",
        "private_key",
        "provider_credentials",
        "kms_node_credentials",
        "wallet_rpc",
        "chain_rpc",
        "kubo_api",
        "kubo_api_url",
        "ipfs_api",
        "ipfs_api_url",
        "elacity_sdk",
        "elacity_sdk_token",
        "contract_sdk",
        "key_backend_sdk",
    ];
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if FORBIDDEN.contains(&key.as_str()) {
                        bail!("protected provider response exposed forbidden field: {key}");
                    }
                    stack.push(value);
                }
            }
            Value::Array(values) => stack.extend(values),
            _ => {}
        }
    }
    Ok(())
}

async fn handle_library_webspace_request(
    data_dir: &Path,
    registry: &ProviderRegistry,
    request: ObjectProviderRequest,
) -> anyhow::Result<Value> {
    match request {
        ObjectProviderRequest::List { principal_id, uri } => {
            let uri = clean_webspace_uri(uri.as_deref().unwrap_or("localhost://WebSpaces"))?;
            webspace_try_refresh_index_from_adapter(data_dir, registry, &uri).await?;
            let data = webspace_provider_data(
                registry,
                json!({
                    "op": "list",
                    "path": uri,
                    "token": "",
                }),
                "list",
            )
            .await?;
            let entries: Vec<WebSpaceDirEntry> = serde_json::from_value(data)
                .context("webspace-provider list response has invalid entries")?;
            let mut objects = entries
                .into_iter()
                .map(|entry| webspace_entry_object(data_dir, &uri, entry))
                .collect::<anyhow::Result<Vec<_>>>()?;
            if uri == "localhost://WebSpaces" {
                objects.push(localhost_space_pointer_object(data_dir, &principal_id)?);
                sort_spaces_root_objects(&mut objects);
            }
            let object = webspace_stat_object(data_dir, registry, &uri).await?;
            Ok(json!({
                "uri": uri,
                "object": object,
                "objects": objects,
            }))
        }
        ObjectProviderRequest::Stat { uri, .. } => {
            let uri = clean_webspace_uri(&uri)?;
            Ok(json!({
                "object": webspace_stat_object(data_dir, registry, &uri).await?,
            }))
        }
        ObjectProviderRequest::Read { uri, .. } | ObjectProviderRequest::Download { uri, .. } => {
            let uri = clean_webspace_uri(&uri)?;
            let (object, content) = webspace_read_bytes(data_dir, registry, &uri).await?;
            Ok(json!({
                "object": object,
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(content),
            }))
        }
        ObjectProviderRequest::ArchiveEntries { uri, .. } => {
            let uri = clean_webspace_uri(&uri)?;
            let (object, bytes) = webspace_read_bytes(data_dir, registry, &uri).await?;
            let archive_name = object.name.clone();
            archive_entries_for_object(webspace_archive_object(object), &uri, &archive_name, bytes)
        }
        ObjectProviderRequest::ArchivePreviewEntry {
            uri,
            entry,
            if_revision,
            ..
        } => {
            let uri = clean_webspace_uri(&uri)?;
            let (object, bytes) = webspace_read_bytes(data_dir, registry, &uri).await?;
            check_object_revision(&object, if_revision.as_deref())?;
            let archive_name = object.name.clone();
            archive_preview_entry_for_object(
                data_dir,
                webspace_archive_object(object),
                &uri,
                &archive_name,
                bytes,
                &entry,
            )
        }
        ObjectProviderRequest::ArchiveExtractEntries {
            principal_id,
            uri,
            destination_uri,
            entries,
            conflict_policy,
            if_revision,
            cancel,
        } => {
            let (source_uri, archive_name, bytes) = if is_webspace_uri(uri.as_str()) {
                let uri = clean_webspace_uri(&uri)?;
                let (object, bytes) = webspace_read_bytes(data_dir, registry, &uri).await?;
                check_object_revision(&object, if_revision.as_deref())?;
                let archive_name = object.name.clone();
                (uri, archive_name, bytes)
            } else {
                let target = library_target(data_dir, &principal_id, &uri)?;
                check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
                let archive_name = library_archive_name(&target, "selected extraction")?;
                let bytes = read_library_file_bytes(data_dir, &principal_id, &target)?;
                (target.uri.clone(), archive_name, bytes)
            };
            let request = ArchiveExtractRequest {
                source_uri: &source_uri,
                archive_name: &archive_name,
                destination_uri: &destination_uri,
                entries: &entries,
                conflict_policy: conflict_policy.as_deref(),
                cancel,
            };
            if is_webspace_uri(destination_uri.as_str()) {
                extract_archive_entries_to_webspace_destination(
                    data_dir,
                    registry,
                    &principal_id,
                    bytes,
                    request,
                )
                .await
            } else {
                extract_archive_entries_to_local_destination(
                    data_dir,
                    &principal_id,
                    bytes,
                    request,
                )
            }
        }
        ObjectProviderRequest::Sync { principal_id, uri } => {
            let _ = principal_id;
            let uri = clean_webspace_uri(&uri)?;
            let receipt = webspace_sync_bytes(data_dir, registry, &uri).await?;
            Ok(json!({
                "object": receipt.get("object").cloned().unwrap_or(Value::Null),
                "receipt": receipt,
            }))
        }
        ObjectProviderRequest::Status { uri, .. } => {
            let uri = clean_webspace_uri(&uri)?;
            Ok(json!({
                "object": webspace_stat_object(data_dir, registry, &uri).await?,
                "published": null,
            }))
        }
        ObjectProviderRequest::Write {
            uri,
            data,
            if_revision: _,
            ..
        } => {
            let uri = clean_webspace_uri(&uri)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .context("library WebSpace write data must be base64")?;
            let receipt = webspace_write_bytes(registry, &uri, &bytes).await?;
            Ok(json!({
                "object": webspace_stat_object(data_dir, registry, &uri).await?,
                "receipt": receipt,
            }))
        }
        ObjectProviderRequest::Mkdir {
            parent_uri, name, ..
        } => {
            let parent_uri = clean_webspace_uri(&parent_uri)?;
            let uri = child_uri(&parent_uri, &name)?;
            let receipt = webspace_mkdir(registry, &uri).await?;
            Ok(json!({
                "object": webspace_stat_object(data_dir, registry, &uri).await?,
                "receipt": receipt,
            }))
        }
        ObjectProviderRequest::DeletePermanently { uri, .. } => {
            let uri = clean_webspace_uri(&uri)?;
            let receipt = webspace_delete_permanently(registry, &uri).await?;
            Ok(json!({
                "deleted_uri": uri,
                "receipt": receipt,
            }))
        }
        _ => Err(anyhow!(
            "Spaces operation is not supported by the current resolver lifecycle model"
        )),
    }
}

fn handle_library_request(
    data_dir: &Path,
    request: ObjectProviderRequest,
) -> anyhow::Result<Value> {
    match request {
        ObjectProviderRequest::Roots { principal_id } => {
            Ok(json!({ "roots": library_roots(data_dir, &principal_id) }))
        }
        ObjectProviderRequest::List { principal_id, uri } => {
            let root = crate::auth::principal_localhost_root(&principal_id);
            let uri = uri.unwrap_or_else(|| root.clone());
            let target = library_target(data_dir, &principal_id, &uri)?;
            if !target.path.exists() {
                return Ok(json!({ "uri": target.uri, "objects": [] }));
            }
            if !target.path.is_dir() {
                bail!("library list target must be a directory");
            }
            let mut objects = Vec::new();
            for entry in fs::read_dir(&target.path)
                .with_context(|| format!("failed to list {:?}", target.path))?
            {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if target.uri == root && (name == ".AppData" || name == ".Trash") {
                    continue;
                }
                let child_uri = format!("{}/{}", target.uri.trim_end_matches('/'), name);
                objects.push(library_object(data_dir, &principal_id, &child_uri)?);
            }
            objects.sort_by(|a, b| {
                a.kind
                    .cmp(b.kind)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(json!({ "uri": target.uri, "objects": objects }))
        }
        ObjectProviderRequest::Stat { principal_id, uri } => {
            Ok(json!({ "object": library_object(data_dir, &principal_id, &uri)? }))
        }
        ObjectProviderRequest::Read { principal_id, uri } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            if !target.path.is_file() {
                bail!("library read target must be a file");
            }
            let bytes = read_library_file_bytes(data_dir, &principal_id, &target)?;
            Ok(json!({
                "object": library_object(data_dir, &principal_id, &target.uri)?,
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        }
        ObjectProviderRequest::Download { principal_id, uri } => {
            let (object, filename, bytes) = library_download_object(
                data_dir,
                &principal_id,
                &uri,
                LibraryArchiveFormat::TarGz,
            )?;
            Ok(json!({
                "object": object,
                "encoding": "base64",
                "filename": filename,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        }
        ObjectProviderRequest::ExtractArchive {
            principal_id,
            uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            let extracted_uri = extract_library_archive(data_dir, &principal_id, &target)?;
            let object = library_object(data_dir, &principal_id, &extracted_uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "extract_archive",
                &extracted_uri,
                json!({
                    "source_uri": target.uri,
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({
                "object": object,
                "source_uri": target.uri,
            }))
        }
        ObjectProviderRequest::ArchiveEntries { principal_id, uri } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            Ok(library_archive_entries(data_dir, &principal_id, &target)?)
        }
        ObjectProviderRequest::ArchivePreviewEntry {
            principal_id,
            uri,
            entry,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            archive_preview_entry(data_dir, &principal_id, &target, &entry)
        }
        ObjectProviderRequest::ArchiveExtractEntries {
            principal_id,
            uri,
            destination_uri,
            entries,
            conflict_policy,
            if_revision,
            cancel,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            extract_library_archive_entries(
                data_dir,
                &principal_id,
                &target,
                &destination_uri,
                &entries,
                conflict_policy.as_deref(),
                cancel,
            )
        }
        ObjectProviderRequest::CompressArchive {
            principal_id,
            uri,
            uris,
            if_revision,
        } => {
            let object = compress_library_archive(
                data_dir,
                &principal_id,
                uri.as_deref(),
                &uris,
                if_revision.as_deref(),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Write {
            principal_id,
            uri,
            data,
            mime,
            if_revision,
        } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data.trim())
                .context("library write data must be base64")?;
            let target = library_target(data_dir, &principal_id, &uri)?;
            if is_trash_uri(&target.localhost_root, &target.uri) {
                bail!("library Trash accepts objects only through delete");
            }
            let object = write_library_file_bytes(
                data_dir,
                &principal_id,
                &target.uri,
                mime.as_deref(),
                if_revision.as_deref(),
                &bytes,
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Mkdir {
            principal_id,
            parent_uri,
            name,
        } => {
            let parent = library_target(data_dir, &principal_id, &parent_uri)?;
            if parent.path.exists() && !parent.path.is_dir() {
                bail!("library mkdir parent must be a directory");
            }
            if is_trash_uri(&parent.localhost_root, &parent.uri) {
                bail!("library Trash accepts objects only through delete");
            }
            let child_uri = child_uri(&parent.uri, &name)?;
            let child = library_target(data_dir, &principal_id, &child_uri)?;
            fs::create_dir_all(&child.path)?;
            let object = library_object(data_dir, &principal_id, &child.uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "mkdir",
                &child.uri,
                json!({
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Rename {
            principal_id,
            uri,
            name,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            if is_trash_uri(&target.localhost_root, &target.uri) {
                bail!("library Trash objects can be restored or deleted permanently");
            }
            let parent_uri = target
                .uri
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .ok_or_else(|| anyhow!("library rename target has no parent"))?;
            let new_uri = child_uri(parent_uri, &name)?;
            move_library_object(data_dir, &principal_id, &target.uri, &new_uri)?;
            let object = library_object(data_dir, &principal_id, &new_uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "rename",
                &new_uri,
                json!({
                    "old_uri": target.uri,
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Move {
            principal_id,
            uri,
            target_parent_uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            let parent = library_target(data_dir, &principal_id, &target_parent_uri)?;
            if parent.path.exists() && !parent.path.is_dir() {
                bail!("library move target parent must be a directory");
            }
            if is_trash_uri(&target.localhost_root, &target.uri)
                || is_trash_uri(&parent.localhost_root, &parent.uri)
            {
                bail!("library Trash objects can be moved only through trash or restore");
            }
            if parent.uri == target.uri || parent.uri.starts_with(&(target.uri.clone() + "/")) {
                bail!("library object cannot be moved inside itself");
            }
            let name = target
                .uri
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow!("library move target has no name"))?;
            let new_uri = unique_child_uri(data_dir, &principal_id, &parent.uri, name)?;
            move_library_object(data_dir, &principal_id, &target.uri, &new_uri)?;
            let object = library_object(data_dir, &principal_id, &new_uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "move",
                &new_uri,
                json!({
                    "old_uri": target.uri,
                    "target_uri": new_uri,
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Copy {
            principal_id,
            uri,
            target_parent_uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            let parent = library_target(data_dir, &principal_id, &target_parent_uri)?;
            if parent.path.exists() && !parent.path.is_dir() {
                bail!("library copy target parent must be a directory");
            }
            if is_trash_uri(&target.localhost_root, &target.uri)
                || is_trash_uri(&parent.localhost_root, &parent.uri)
            {
                bail!("library Trash objects cannot be copied directly");
            }
            if parent.uri == target.uri || parent.uri.starts_with(&(target.uri.clone() + "/")) {
                bail!("library object cannot be copied inside itself");
            }
            let name = target
                .uri
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow!("library copy target has no name"))?;
            let new_uri = unique_child_uri(data_dir, &principal_id, &parent.uri, name)?;
            copy_library_object(data_dir, &principal_id, &target.uri, &new_uri)?;
            let object = library_object(data_dir, &principal_id, &new_uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "copy",
                &new_uri,
                json!({
                    "source_uri": target.uri,
                    "target_uri": new_uri,
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::Trash {
            principal_id,
            uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            if is_trash_uri(&target.localhost_root, &target.uri) {
                bail!("library object is already in Trash");
            }
            if is_runtime_private_uri(&target.localhost_root, &target.uri) {
                bail!("runtime-private Library objects cannot be moved to Trash");
            }
            let name = target
                .uri
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow!("library trash target has no name"))?;
            let trash_root = format!("{}/.Trash", target.localhost_root);
            let trash_uri = unique_child_uri(data_dir, &principal_id, &trash_root, name)?;
            move_library_object(data_dir, &principal_id, &target.uri, &trash_uri)?;
            write_trash_record(
                data_dir,
                &principal_id,
                &LibraryTrashRecord {
                    schema: LIBRARY_TRASH_RECORD_SCHEMA.to_string(),
                    trash_uri: trash_uri.clone(),
                    original_uri: target.uri.clone(),
                    original_name: name.to_string(),
                    trashed_at: now_ts(),
                },
            )?;
            let object = library_object(data_dir, &principal_id, &trash_uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "trash",
                &trash_uri,
                json!({
                    "original_uri": target.uri,
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({
                "object": object,
                "original_uri": target.uri,
            }))
        }
        ObjectProviderRequest::Restore {
            principal_id,
            uri,
            target_uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            if !is_trash_child_uri(&target.localhost_root, &target.uri) {
                bail!("library restore target must be in Trash");
            }
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            let trash_record = read_trash_record(data_dir, &principal_id, &target.uri).ok();
            let restore_uri = target_uri
                .as_deref()
                .filter(|uri| !uri.trim().is_empty())
                .map(|uri| clean_library_uri(&target.localhost_root, uri))
                .transpose()?
                .map(Ok)
                .unwrap_or_else(|| {
                    restore_uri_from_trash_record(
                        data_dir,
                        &principal_id,
                        &target,
                        trash_record.as_ref(),
                    )
                })?;
            let restore_target = library_target(data_dir, &principal_id, &restore_uri)?;
            if is_trash_uri(&restore_target.localhost_root, &restore_target.uri) {
                bail!("library restore target cannot be inside Trash");
            }
            move_library_object(data_dir, &principal_id, &target.uri, &restore_target.uri)?;
            remove_trash_record(data_dir, &principal_id, &target.uri)?;
            let object = library_object(data_dir, &principal_id, &restore_target.uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "restore",
                &restore_target.uri,
                json!({
                    "trash_uri": target.uri,
                    "original_uri": trash_record.as_ref().map(|record| record.original_uri.clone()),
                    "object": object.clone(),
                }),
            )?;
            Ok(json!({ "object": object }))
        }
        ObjectProviderRequest::DeletePermanently {
            principal_id,
            uri,
            if_revision,
        } => {
            let target = library_target(data_dir, &principal_id, &uri)?;
            if !is_trash_child_uri(&target.localhost_root, &target.uri) {
                bail!("library delete_permanently target must be in Trash");
            }
            check_revision(data_dir, &principal_id, &target.uri, if_revision.as_deref())?;
            if target.path.is_dir() {
                fs::remove_dir_all(&target.path)?;
            } else {
                fs::remove_file(&target.path)?;
            }
            remove_trash_record(data_dir, &principal_id, &target.uri)?;
            append_library_event(
                data_dir,
                &principal_id,
                "delete_permanently",
                &target.uri,
                json!({}),
            )?;
            Ok(json!({ "deleted_uri": target.uri }))
        }
        ObjectProviderRequest::EmptyTrash { principal_id } => {
            let root = crate::auth::principal_localhost_root(&principal_id);
            let trash_root = format!("{root}/.Trash");
            let target = library_target(data_dir, &principal_id, &trash_root)?;
            let mut deleted_uris = Vec::new();
            if target.path.exists() {
                if !target.path.is_dir() {
                    bail!("library Trash root must be a directory");
                }
                for entry in fs::read_dir(&target.path)
                    .with_context(|| format!("failed to list {:?}", target.path))?
                {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().to_string();
                    let child_uri = format!("{}/{}", target.uri, name);
                    let child = library_target(data_dir, &principal_id, &child_uri)?;
                    if child.path.is_dir() {
                        fs::remove_dir_all(&child.path)?;
                    } else {
                        fs::remove_file(&child.path)?;
                    }
                    remove_trash_record(data_dir, &principal_id, &child.uri)?;
                    deleted_uris.push(child.uri);
                }
            }
            let deleted_count = deleted_uris.len();
            append_library_event(
                data_dir,
                &principal_id,
                "empty_trash",
                &target.uri,
                json!({
                    "deleted_count": deleted_uris.len(),
                    "deleted_uris": deleted_uris,
                }),
            )?;
            Ok(json!({ "deleted_count": deleted_count }))
        }
        ObjectProviderRequest::Status { principal_id, uri } => {
            let object = library_object(data_dir, &principal_id, &uri)?;
            let record = read_publish_record(data_dir, &principal_id, &uri).ok();
            Ok(json!({
                "object": object,
                "published": record,
            }))
        }
        ObjectProviderRequest::Sync { principal_id, .. } => {
            let _ = principal_id;
            bail!("library sync is only supported for Spaces objects")
        }
        ObjectProviderRequest::Events {
            principal_id,
            uri,
            since,
            limit,
        } => Ok(json!({
            "schema": "elastos.library.events/v1",
            "events": library_events(data_dir, &principal_id, uri.as_deref(), since, limit)?,
        })),
        ObjectProviderRequest::Share {
            principal_id,
            uri,
            recipients,
            policy,
            key_release_policy,
        } => {
            let object = library_object(data_dir, &principal_id, &uri)?;
            let mut record = read_publish_record(data_dir, &principal_id, &uri)
                .context("library share requires a published object")?;
            if !record_is_published(&record) {
                bail!("library share requires an actively published object");
            }
            let shared_at = now_ts();
            let recipients = normalized_share_recipients(&recipients)?;
            let share_policy = normalized_share_policy(policy.as_deref(), recipients.is_empty())?;
            let key_release = normalized_key_release_policy(
                key_release_policy.as_deref(),
                &record.content_security,
            )?;
            let remote_enforcement = share_remote_enforcement_contract(&share_policy, &key_release);
            record.shared_at = Some(shared_at);
            record.share_policy = Some(share_policy.clone());
            record.share_grants = recipients
                .iter()
                .map(|recipient| {
                    share_grant(
                        recipient,
                        &record.cid,
                        &format!("elastos://{}", record.cid),
                        &share_policy,
                        key_release.clone(),
                        shared_at,
                    )
                })
                .collect();
            write_publish_record(data_dir, &principal_id, &record)?;
            append_library_event(
                data_dir,
                &principal_id,
                "share",
                &uri,
                json!({
                    "cid": record.cid,
                    "policy": share_policy,
                    "key_release": key_release.clone(),
                    "remote_enforcement": remote_enforcement.clone(),
                    "recipients": recipients,
                    "shared_at": record.shared_at,
                }),
            )?;
            Ok(json!({
                "schema": "elastos.library.share/v1",
                "object": library_object(data_dir, &principal_id, &uri)?,
                "uri": format!("elastos://{}", record.cid),
                "cid": record.cid,
                "policy": record.share_policy,
                "content_security": record.content_security,
                "key_release": key_release.clone(),
                "remote_enforcement": remote_enforcement,
                "recipients": recipients,
                "grants": record.share_grants,
                "availability": record.availability,
                "shared_at": record.shared_at,
                "object_uri": object.uri,
            }))
        }
        ObjectProviderRequest::SharedAccess {
            principal_id,
            uri,
            recipient,
            recipient_proof,
        } => {
            let object = library_object(data_dir, &principal_id, &uri)?;
            let record = read_publish_record(data_dir, &principal_id, &uri)
                .context("library shared_access requires a published object")?;
            if !record_is_published(&record) || record.shared_at.is_none() {
                bail!("library shared_access requires an actively shared object");
            }
            let access = match shared_access_receipt(&record, &recipient, recipient_proof.as_ref())
            {
                Ok(access) => access,
                Err(err) => {
                    append_library_event(
                        data_dir,
                        &principal_id,
                        "shared_access",
                        &uri,
                        json!({
                            "cid": record.cid,
                            "recipient": recipient,
                            "policy": record.share_policy,
                            "allowed": false,
                            "reason": err.to_string(),
                        }),
                    )?;
                    return Err(err);
                }
            };
            append_library_event(
                data_dir,
                &principal_id,
                "shared_access",
                &uri,
                json!({
                    "cid": record.cid,
                    "recipient": recipient,
                    "policy": record.share_policy,
                    "allowed": true,
                    "decision": access.get("decision").cloned().unwrap_or(Value::Null),
                    "open": access.get("open").cloned().unwrap_or(Value::Null),
                    "key_release": access.get("key_release").cloned().unwrap_or(Value::Null),
                }),
            )?;
            Ok(json!({
                "schema": "elastos.library.shared-access/v1",
                "object": object,
                "uri": format!("elastos://{}", record.cid),
                "cid": record.cid,
                "access": access,
                "availability": record.availability,
            }))
        }
        ObjectProviderRequest::Publish { .. }
        | ObjectProviderRequest::Unpublish { .. }
        | ObjectProviderRequest::Repair { .. } => {
            unreachable!("publish/unpublish/repair handled asynchronously")
        }
    }
}

async fn library_publish(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    uri: &str,
    if_revision: Option<&str>,
    protection: Option<LibraryPublishProtectionRequest>,
) -> anyhow::Result<Value> {
    let target = library_target(data_dir, principal_id, uri)?;
    check_revision(data_dir, principal_id, &target.uri, if_revision)?;
    if let Some(protection) = protection {
        let _loaded_input =
            validate_runtime_custody_publish_input(data_dir, principal_id, &target, protection)?;
        let LoadedRuntimeCustodyPublishInput {
            mime_type: _,
            codecs: _,
            clear_init_segment: _,
            clear_segments: _,
        } = _loaded_input;
        bail!(RUNTIME_CUSTODY_PUBLISH_INACTIVE_MESSAGE);
    }
    if !target.path.is_file() {
        bail!("library publish currently supports files only");
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, &target)?;
    let filename = target
        .uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("object.bin");
    let publish_request = json!({
        "op": "publish",
        "kind": "file",
        "filename": filename,
        "mime": mime_for_name(filename),
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "pin": true,
        "publisher_did": principal_id,
    });
    let response = registry
        .send_raw("content", &publish_request)
        .await
        .map_err(|err| anyhow!("content provider unavailable: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("content publish failed");
        bail!("content publish failed: {message}");
    }
    let data = response
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("content publish response missing data"))?;
    let payload_cid = data
        .get("cid")
        .and_then(Value::as_str)
        .filter(|cid| !cid.trim().is_empty())
        .ok_or_else(|| anyhow!("content publish response missing cid"))?
        .to_string();
    let cid = payload_cid;
    let receipt = data
        .get("receipt")
        .cloned()
        .unwrap_or_else(|| json!({"status": "not_provided"}));
    let availability = data
        .get("availability")
        .cloned()
        .unwrap_or_else(|| json!({"status": "unknown"}));
    let content_security = published_content_security(data_dir, principal_id, &target)?;
    let record = LibraryPublishRecord {
        schema: "elastos.library.publish-record/v1".to_string(),
        object_uri: target.uri.clone(),
        cid: cid.clone(),
        published_at: now_ts(),
        unpublished_at: None,
        shared_at: None,
        share_policy: None,
        share_grants: Vec::new(),
        content_security,
        receipt,
        availability,
    };
    write_publish_record(data_dir, principal_id, &record)?;
    let object = library_object(data_dir, principal_id, &target.uri)?;
    append_library_event(
        data_dir,
        principal_id,
        "publish",
        &target.uri,
        json!({
            "cid": cid,
            "availability": record.availability,
            "object": object,
        }),
    )?;
    Ok(json!({
        "object": object,
        "uri": format!("elastos://{}", record.cid),
        "cid": record.cid,
        "receipt": record.receipt,
        "availability": record.availability,
        "content_security": record.content_security,
        "published_at": record.published_at,
    }))
}

async fn library_unpublish(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    uri: &str,
    if_revision: Option<&str>,
) -> anyhow::Result<Value> {
    let target = library_target(data_dir, principal_id, uri)?;
    check_revision(data_dir, principal_id, &target.uri, if_revision)?;
    let mut record = read_publish_record(data_dir, principal_id, &target.uri)
        .context("library unpublish requires a published object")?;
    if !record_is_published(&record) {
        bail!("library object is not actively published");
    }
    let response = registry
        .send_raw(
            "content",
            &json!({
                "op": "unpublish",
                "cid": record.cid,
                "object_did": target.uri,
                "publisher_did": principal_id,
            }),
        )
        .await
        .map_err(|err| anyhow!("content provider unavailable: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("content unpublish failed");
        bail!("content unpublish failed: {message}");
    }
    let data = response
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("content unpublish response missing data"))?;
    record.unpublished_at = Some(now_ts());
    record.shared_at = None;
    record.receipt = data
        .get("receipt")
        .cloned()
        .unwrap_or_else(|| json!({"status": "not_provided"}));
    record.availability = data
        .get("availability")
        .cloned()
        .unwrap_or_else(|| json!({"status": "local_unpinned"}));
    write_publish_record(data_dir, principal_id, &record)?;
    let object = library_object(data_dir, principal_id, &target.uri)?;
    append_library_event(
        data_dir,
        principal_id,
        "unpublish",
        &target.uri,
        json!({
            "cid": record.cid,
            "availability": record.availability,
            "object": object,
        }),
    )?;
    Ok(json!({
        "object": object,
        "uri": format!("elastos://{}", record.cid),
        "cid": record.cid,
        "receipt": record.receipt,
        "availability": record.availability,
        "unpublished_at": record.unpublished_at,
    }))
}

async fn library_repair(
    data_dir: &Path,
    registry: Arc<ProviderRegistry>,
    principal_id: &str,
    uri: &str,
) -> anyhow::Result<Value> {
    let target = library_target(data_dir, principal_id, uri)?;
    let mut record = read_publish_record(data_dir, principal_id, &target.uri)
        .context("library repair requires a published object")?;
    let response = registry
        .send_raw(
            "content",
            &json!({
                "op": "repair",
                "cid": record.cid,
                "object_did": target.uri,
                "publisher_did": principal_id,
            }),
        )
        .await
        .map_err(|err| anyhow!("content provider unavailable: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("content repair failed");
        bail!("content repair failed: {message}");
    }
    let data = response
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("content repair response missing data"))?;
    record.unpublished_at = None;
    record.receipt = data
        .get("receipt")
        .cloned()
        .unwrap_or_else(|| json!({"status": "not_provided"}));
    record.availability = data
        .get("availability")
        .cloned()
        .unwrap_or_else(|| json!({"status": "unknown"}));
    write_publish_record(data_dir, principal_id, &record)?;
    let object = library_object(data_dir, principal_id, &target.uri)?;
    append_library_event(
        data_dir,
        principal_id,
        "repair",
        &target.uri,
        json!({
            "cid": record.cid,
            "availability": record.availability,
            "object": object,
        }),
    )?;
    Ok(json!({
        "object": object,
        "uri": format!("elastos://{}", record.cid),
        "cid": record.cid,
        "receipt": record.receipt,
        "availability": record.availability,
    }))
}

struct LibraryTarget {
    localhost_root: String,
    uri: String,
    path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct WebSpaceDirEntry {
    name: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    resolver_state: Option<String>,
    #[serde(default)]
    resolver: Option<String>,
    #[serde(default)]
    cache_policy: Option<String>,
    #[serde(default)]
    sync_policy: Option<String>,
    #[serde(default, rename = "kind")]
    webspace_kind: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    head_id: Option<String>,
    #[serde(default)]
    cache_state: Option<String>,
    #[serde(default)]
    sync_state: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    readonly: Option<bool>,
    #[serde(default)]
    access_policy: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebSpaceFileStat {
    path: String,
    is_file: bool,
    is_dir: bool,
    size: u64,
    #[serde(default)]
    modified: Option<u64>,
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    target_uri: Option<String>,
    #[serde(default)]
    resolver_state: Option<String>,
    #[serde(default)]
    resolver: Option<String>,
    #[serde(default)]
    cache_policy: Option<String>,
    #[serde(default)]
    sync_policy: Option<String>,
    #[serde(default, rename = "kind")]
    webspace_kind: Option<String>,
    #[serde(default)]
    object_id: Option<String>,
    #[serde(default)]
    head_id: Option<String>,
    #[serde(default)]
    cache_state: Option<String>,
    #[serde(default)]
    sync_state: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    readonly: Option<bool>,
    #[serde(default)]
    access_policy: Option<String>,
}

async fn webspace_provider_data(
    registry: &ProviderRegistry,
    request: Value,
    op: &str,
) -> anyhow::Result<Value> {
    let response = registry
        .send_raw("webspace", &request)
        .await
        .map_err(|err| anyhow!("webspace-provider unavailable: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let code = response
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("webspace_error");
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("webspace-provider request failed");
        bail!("webspace-provider {op} failed [{code}]: {message}");
    }
    response
        .get("data")
        .cloned()
        .ok_or_else(|| anyhow!("webspace-provider {op} response missing data"))
}

async fn webspace_stat_object(
    data_dir: &Path,
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<LibraryObject> {
    let data = webspace_provider_data(
        registry,
        json!({
            "op": "stat",
            "path": uri,
            "token": "",
        }),
        "stat",
    )
    .await?;
    let stat: WebSpaceFileStat =
        serde_json::from_value(data).context("webspace-provider stat response is invalid")?;
    webspace_stat_to_object(data_dir, uri, stat)
}

async fn webspace_download_bytes(
    data_dir: &Path,
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<LibraryDownloadBytes> {
    let uri = clean_webspace_uri(uri)?;
    let (object, bytes) = webspace_read_bytes(data_dir, registry, &uri).await?;
    Ok(LibraryDownloadBytes {
        filename: object.name,
        mime: object.mime,
        bytes,
    })
}

async fn webspace_read_bytes(
    data_dir: &Path,
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<(LibraryObject, Vec<u8>)> {
    let object = webspace_stat_object(data_dir, registry, uri).await?;
    if object.kind != "file" {
        bail!("library read target must be a file");
    }
    if let Some((object, bytes)) =
        webspace_try_cache_bytes_from_adapter(data_dir, registry, &object).await?
    {
        return Ok((object, bytes));
    }
    let content = webspace_read_provider_bytes(registry, uri).await?;
    Ok((object, content))
}

async fn webspace_sync_bytes(
    data_dir: &Path,
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<Value> {
    let object = webspace_stat_object(data_dir, registry, uri).await?;
    if object.kind != "file" {
        bail!("Spaces byte sync target must be a file");
    }
    if webspace_metadata_str(&object, "sync_state") == Some("manual_pending")
        || (webspace_metadata_bool(&object, "readonly") == Some(false)
            && webspace_metadata_str(&object, "sync_state") != Some("manual_synced"))
    {
        return webspace_sync_mutable_file_to_resolver(data_dir, registry, object).await;
    }
    if webspace_metadata_str(&object, "cache_state") == Some("content_cached")
        || webspace_metadata_str(&object, "webspace_kind") == Some("materialized-file")
    {
        let availability_hint = webspace_object_availability_hint(&object);
        return Ok(json!({
            "schema": "elastos.webspace.byte-sync-receipt/v1",
            "action": "already_content_cached",
            "handle_uri": object.uri,
            "content_synced": true,
            "foreground_read": false,
            "bytes_exposed": false,
            "availability_hint": availability_hint,
            "object": object,
            "note": "Resolver bytes are already present in the provider-owned WebSpace cache."
        }));
    }
    let Some((cached_object, bytes)) =
        webspace_try_cache_bytes_from_adapter(data_dir, registry, &object).await?
    else {
        bail!("Spaces byte sync requires a connected resolver adapter with read_bytes");
    };
    let availability_hint = webspace_object_availability_hint(&cached_object);
    Ok(json!({
        "schema": "elastos.webspace.byte-sync-receipt/v1",
        "action": "bytes_cached_from_adapter",
        "handle_uri": cached_object.uri,
        "content_synced": true,
        "foreground_read": false,
        "bytes_exposed": false,
        "bytes_cached": bytes.len(),
        "availability_hint": availability_hint,
        "object": cached_object,
        "note": "Runtime invoked the resolver adapter and stored bytes in the provider-owned WebSpace cache without returning bytes to the caller."
    }))
}

async fn webspace_sync_mutable_file_to_resolver(
    data_dir: &Path,
    registry: &ProviderRegistry,
    object: LibraryObject,
) -> anyhow::Result<Value> {
    let bytes = webspace_read_provider_bytes(registry, &object.uri).await?;
    let Some(adapter) = webspace_adapter_target(registry, &object).await? else {
        return Ok(webspace_resolver_sync_failed_receipt(
            &object,
            "resolver_write_unavailable",
            "No connected resolver adapter is available for this mutable WebSpace object.",
            None,
            None,
        ));
    };
    if !adapter.capabilities.contains("write_bytes") {
        return Ok(webspace_resolver_sync_failed_receipt(
            &object,
            "resolver_write_unavailable",
            "The connected resolver adapter does not advertise write_bytes.",
            Some(adapter.provider),
            None,
        ));
    }
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "webspace-provider".to_string(),
            target: adapter.provider.clone(),
            op: "write_bytes".to_string(),
            request: json!({
                "op": "write_bytes",
                "schema": "elastos.webspace.adapter.write-bytes-request/v1",
                "mount": adapter.mount.clone(),
                "resolver": adapter.resolver.clone(),
                "handle_uri": object.uri.clone(),
                "target_uri": adapter.target_uri.clone(),
                "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "if_head": webspace_metadata_str(&object, "head_id"),
            }),
            transfer: ProviderTransfer::Bytes,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| anyhow!("WebSpace adapter write_bytes failed: {err}"))?;
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let code = response
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("adapter_write_failed")
            .to_string();
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("resolver adapter write_bytes failed")
            .to_string();
        return Ok(webspace_resolver_sync_failed_receipt(
            &object,
            if code == "conflict" {
                "resolver_write_conflict"
            } else {
                "resolver_write_failed"
            },
            &message,
            Some(adapter.provider),
            Some(response),
        ));
    }
    let data = response_data_object(&response, "write_bytes")?;
    if data.get("schema").and_then(Value::as_str) != Some("elastos.webspace.adapter.write-bytes/v1")
    {
        bail!("WebSpace adapter write_bytes response schema mismatch");
    }
    let provider_sync_receipt = webspace_provider_data(
        registry,
        json!({
            "op": "sync",
            "path": object.uri.clone(),
            "token": "",
        }),
        "sync",
    )
    .await?;
    let synced_object = webspace_stat_object(data_dir, registry, &object.uri).await?;
    let availability_hint = webspace_object_availability_hint(&synced_object);
    Ok(json!({
        "schema": "elastos.webspace.resolver-sync-receipt/v1",
        "action": "resolver_write_synced",
        "handle_uri": synced_object.uri.clone(),
        "target_uri": adapter.target_uri,
        "resolver": adapter.resolver,
        "provider": adapter.provider,
        "resolver_synced": true,
        "content_synced": true,
        "fail_closed": false,
        "conflict": false,
        "bytes_exposed": false,
        "bytes_synced": bytes.len(),
        "availability_hint": availability_hint,
        "object": synced_object,
        "provider_sync_receipt": provider_sync_receipt,
        "adapter_receipt": data.get("receipt").cloned(),
        "runtime_transfer": response.get("_runtime_transfer").cloned(),
    }))
}

fn webspace_resolver_sync_failed_receipt(
    object: &LibraryObject,
    action: &str,
    message: &str,
    provider: Option<String>,
    adapter_response: Option<Value>,
) -> Value {
    json!({
        "schema": "elastos.webspace.resolver-sync-receipt/v1",
        "action": action,
        "handle_uri": object.uri.clone(),
        "target_uri": webspace_metadata_str(object, "target_uri"),
        "resolver": webspace_metadata_str(object, "resolver"),
        "provider": provider,
        "resolver_synced": false,
        "content_synced": false,
        "fail_closed": true,
        "conflict": action == "resolver_write_conflict",
        "bytes_exposed": false,
        "object": object,
        "adapter_response": adapter_response,
        "message": message,
    })
}

#[derive(Debug)]
struct WebSpaceAdapterTarget {
    mount: String,
    resolver: String,
    target_uri: String,
    provider: String,
    capabilities: BTreeSet<String>,
}

async fn webspace_try_refresh_index_from_adapter(
    data_dir: &Path,
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<()> {
    let object = match webspace_stat_object(data_dir, registry, uri).await {
        Ok(object) => object,
        Err(_) => return Ok(()),
    };
    let Some(adapter) = webspace_adapter_target(registry, &object).await? else {
        return Ok(());
    };
    if !adapter.capabilities.contains("metadata_index") {
        return Ok(());
    }
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "webspace-provider".to_string(),
            target: adapter.provider.clone(),
            op: "metadata_index".to_string(),
            request: json!({
                "op": "metadata_index",
                "schema": "elastos.webspace.adapter.metadata-index-request/v1",
                "mount": adapter.mount.clone(),
                "resolver": adapter.resolver.clone(),
                "handle_uri": object.uri,
                "target_uri": adapter.target_uri.clone(),
            }),
            transfer: ProviderTransfer::Json,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| anyhow!("WebSpace adapter metadata_index failed: {err}"))?;
    let data = response_data_object(&response, "metadata_index")?;
    if data.get("schema").and_then(Value::as_str)
        != Some("elastos.webspace.adapter.metadata-index/v1")
    {
        bail!("WebSpace adapter metadata_index response schema mismatch");
    }
    let entries = data
        .get("entries")
        .cloned()
        .ok_or_else(|| anyhow!("WebSpace adapter metadata_index response missing entries"))?;
    webspace_provider_data(
        registry,
        json!({
            "op": "refresh",
            "path": format!("localhost://WebSpaces/{}", adapter.mount),
            "entries": entries,
            "token": "",
        }),
        "refresh",
    )
    .await?;
    Ok(())
}

async fn webspace_try_cache_bytes_from_adapter(
    data_dir: &Path,
    registry: &ProviderRegistry,
    object: &LibraryObject,
) -> anyhow::Result<Option<(LibraryObject, Vec<u8>)>> {
    if webspace_metadata_str(object, "cache_state") == Some("content_cached")
        || webspace_metadata_str(object, "webspace_kind") == Some("materialized-file")
    {
        return Ok(None);
    }
    let Some(adapter) = webspace_adapter_target(registry, object).await? else {
        return Ok(None);
    };
    if !adapter.capabilities.contains("read_bytes") {
        return Ok(None);
    }
    let response = registry
        .invoke_provider(ProviderInvocation {
            source: "webspace-provider".to_string(),
            target: adapter.provider.clone(),
            op: "read_bytes".to_string(),
            request: json!({
                "op": "read_bytes",
                "schema": "elastos.webspace.adapter.read-bytes-request/v1",
                "mount": adapter.mount.clone(),
                "resolver": adapter.resolver.clone(),
                "handle_uri": object.uri,
                "target_uri": adapter.target_uri.clone(),
            }),
            transfer: ProviderTransfer::Bytes,
            range: None,
            progress: None,
            transport: ProviderInvocationTransport::Local,
        })
        .await
        .map_err(|err| anyhow!("WebSpace adapter read_bytes failed: {err}"))?;
    let data = response_data_object(&response, "read_bytes")?;
    if data.get("schema").and_then(Value::as_str) != Some("elastos.webspace.adapter.read-bytes/v1")
    {
        bail!("WebSpace adapter read_bytes response schema mismatch");
    }
    let encoded = data
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("WebSpace adapter read_bytes response missing data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("WebSpace adapter read_bytes response has invalid base64 data")?;
    let mime = data
        .get("mime")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
    let source_receipt = json!({
        "schema": "elastos.webspace.adapter-cache-source/v1",
        "provider": adapter.provider.clone(),
        "resolver": adapter.resolver.clone(),
        "target_uri": adapter.target_uri.clone(),
        "runtime_transfer": response.get("_runtime_transfer").cloned(),
        "adapter_receipt": data.get("receipt").cloned(),
    });
    webspace_provider_data(
        registry,
        json!({
            "op": "cache",
            "path": object.uri,
            "content": bytes,
            "mime": mime,
            "source_receipt": source_receipt,
            "token": "",
        }),
        "cache",
    )
    .await?;
    let cached_object = webspace_stat_object(data_dir, registry, &object.uri).await?;
    Ok(Some((cached_object, bytes)))
}

async fn webspace_adapter_target(
    registry: &ProviderRegistry,
    object: &LibraryObject,
) -> anyhow::Result<Option<WebSpaceAdapterTarget>> {
    let Some(mount) = webspace_metadata_string(object, "mount") else {
        return Ok(None);
    };
    let Some(resolver) = webspace_metadata_string(object, "resolver") else {
        return Ok(None);
    };
    if resolver == "builtin" {
        return Ok(None);
    }
    let Some(target_uri) = webspace_metadata_string(object, "target_uri") else {
        return Ok(None);
    };
    let health = match webspace_provider_data(
        registry,
        json!({
            "op": "health",
            "moniker": mount.clone(),
            "token": "",
        }),
        "health",
    )
    .await
    {
        Ok(health) => health,
        Err(_) => return Ok(None),
    };
    let adapter = health
        .get("mounts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|mount_health| {
            mount_health.get("moniker").and_then(Value::as_str)
                == webspace_metadata_str(object, "mount")
        })
        .and_then(|mount_health| mount_health.get("adapter"))
        .and_then(Value::as_object);
    let Some(adapter) = adapter else {
        return Ok(None);
    };
    if adapter.get("live").and_then(Value::as_bool) != Some(true)
        || adapter.get("state").and_then(Value::as_str) != Some("connected")
    {
        return Ok(None);
    }
    let Some(provider) = adapter.get("provider").and_then(Value::as_str) else {
        return Ok(None);
    };
    let capabilities = adapter
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    Ok(Some(WebSpaceAdapterTarget {
        mount,
        resolver,
        target_uri,
        provider: provider.to_string(),
        capabilities,
    }))
}

fn response_data_object<'a>(response: &'a Value, op: &str) -> anyhow::Result<&'a Value> {
    if response.get("status").and_then(Value::as_str) == Some("error") {
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider request failed");
        bail!("WebSpace adapter {op} failed: {message}");
    }
    response
        .get("data")
        .ok_or_else(|| anyhow!("WebSpace adapter {op} response missing data"))
}

fn webspace_metadata_string(object: &LibraryObject, key: &str) -> Option<String> {
    webspace_metadata_str(object, key).map(str::to_string)
}

fn webspace_metadata_str<'a>(object: &'a LibraryObject, key: &str) -> Option<&'a str> {
    object
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(Value::as_str)
}

fn webspace_metadata_bool(object: &LibraryObject, key: &str) -> Option<bool> {
    object
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(key))
        .and_then(Value::as_bool)
}

async fn webspace_read_provider_bytes(
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<Vec<u8>> {
    let data = webspace_provider_data(
        registry,
        json!({
            "op": "read",
            "path": uri,
            "token": "",
            "offset": null,
            "length": null,
        }),
        "read",
    )
    .await?;
    data.get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("webspace-provider read response missing content"))?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .filter(|byte| *byte <= u8::MAX as u64)
                .map(|byte| byte as u8)
                .ok_or_else(|| anyhow!("webspace-provider read response has invalid byte"))
        })
        .collect()
}

async fn webspace_write_bytes(
    registry: &ProviderRegistry,
    uri: &str,
    bytes: &[u8],
) -> anyhow::Result<Value> {
    webspace_provider_data(
        registry,
        json!({
            "op": "write",
            "path": uri,
            "token": "",
            "content": bytes,
            "append": false,
        }),
        "write",
    )
    .await
}

async fn webspace_mkdir(registry: &ProviderRegistry, uri: &str) -> anyhow::Result<Value> {
    webspace_provider_data(
        registry,
        json!({
            "op": "mkdir",
            "path": uri,
            "token": "",
            "parents": false,
        }),
        "mkdir",
    )
    .await
}

async fn webspace_delete_permanently(
    registry: &ProviderRegistry,
    uri: &str,
) -> anyhow::Result<Value> {
    webspace_provider_data(
        registry,
        json!({
            "op": "delete",
            "path": uri,
            "token": "",
            "recursive": true,
        }),
        "delete",
    )
    .await
}

fn webspace_entry_object(
    data_dir: &Path,
    parent_uri: &str,
    entry: WebSpaceDirEntry,
) -> anyhow::Result<LibraryObject> {
    let uri = child_uri(parent_uri, &entry.name)?;
    webspace_stat_to_object(
        data_dir,
        &uri,
        WebSpaceFileStat {
            path: uri.clone(),
            is_file: entry.is_file,
            is_dir: entry.is_dir,
            size: entry.size,
            modified: None,
            created: None,
            target_uri: entry.target_uri,
            resolver_state: entry.resolver_state,
            resolver: entry.resolver,
            cache_policy: entry.cache_policy,
            sync_policy: entry.sync_policy,
            webspace_kind: entry.webspace_kind,
            object_id: entry.object_id,
            head_id: entry.head_id,
            cache_state: entry.cache_state,
            sync_state: entry.sync_state,
            provider: entry.provider,
            readonly: entry.readonly,
            access_policy: entry.access_policy,
        },
    )
}

fn webspace_stat_to_object(
    data_dir: &Path,
    uri: &str,
    stat: WebSpaceFileStat,
) -> anyhow::Result<LibraryObject> {
    let uri = clean_webspace_uri(uri)?;
    let is_dir = stat.is_dir && !stat.is_file;
    let name = if uri == "localhost://WebSpaces" {
        "Spaces".to_string()
    } else {
        uri.rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Space")
            .to_string()
    };
    let revision = format!(
        "rev:webspace:{}",
        hex::encode(Sha256::digest(format!(
            "{}:{}:{}",
            stat.path,
            stat.size,
            stat.modified.unwrap_or(0)
        )))
    );
    let metadata = webspace_object_metadata(&uri, &stat);
    let availability = webspace_availability_label(&metadata);
    let readonly = stat.readonly.unwrap_or(true);
    let viewers = if is_dir {
        Vec::new()
    } else {
        viewer_options_for_name(data_dir, &name)
    };
    let viewer = viewers.first().map(|viewer| viewer.id.clone());
    let mime = if is_dir {
        "inode/directory".to_string()
    } else if stat.webspace_kind.as_deref() == Some("file-endpoint") {
        "application/json".to_string()
    } else {
        mime_for_name(&name).to_string()
    };
    let capabilities = if is_dir {
        if readonly {
            vec!["open", "list", "properties"]
        } else {
            vec!["open", "list", "new_folder", "write", "properties"]
        }
    } else if readonly {
        vec!["open", "read", "download", "properties"]
    } else {
        vec![
            "open",
            "read",
            "download",
            "write",
            "delete_permanently",
            "properties",
        ]
    };
    let content_cid = if is_dir {
        None
    } else {
        webspace_content_cid_from_metadata(&metadata)
    };
    Ok(LibraryObject {
        schema: LIBRARY_OBJECT_SCHEMA,
        uri,
        name,
        kind: if is_dir { "directory" } else { "file" },
        mime,
        size: stat.size,
        created_at: stat.created.unwrap_or(0),
        modified_at: stat.modified.unwrap_or(0),
        revision,
        viewer,
        viewers,
        thumbnail_uri: None,
        availability,
        blocked_reason: None,
        content_cid,
        published_cid: None,
        metadata: Some(metadata),
        published: false,
        shared: false,
        capabilities,
    })
}

fn localhost_space_pointer_object(
    data_dir: &Path,
    principal_id: &str,
) -> anyhow::Result<LibraryObject> {
    let uri = crate::auth::principal_localhost_root(principal_id);
    let target = library_target(data_dir, principal_id, &uri)?;
    let metadata = fs::metadata(&target.path).ok();
    let modified_at = metadata
        .as_ref()
        .and_then(|metadata| system_time_secs(metadata.modified().ok()))
        .unwrap_or_else(now_ts);
    let created_at = metadata
        .as_ref()
        .and_then(|metadata| system_time_secs(metadata.created().ok()))
        .unwrap_or(modified_at);
    Ok(LibraryObject {
        schema: LIBRARY_OBJECT_SCHEMA,
        uri: uri.clone(),
        name: "Localhost".to_string(),
        kind: "directory",
        mime: "inode/directory".to_string(),
        size: 0,
        created_at,
        modified_at,
        revision: format!("rev:space:{}", hex::encode(Sha256::digest(uri.as_bytes()))),
        viewer: None,
        viewers: Vec::new(),
        thumbnail_uri: None,
        availability: "local-principal".to_string(),
        blocked_reason: None,
        content_cid: None,
        published_cid: None,
        metadata: Some(json!({
            "schema": "elastos.library.space-pointer/v1",
            "space": "localhost",
            "label": "Localhost",
            "target_uri": uri,
            "provider": "object-provider",
            "authority": "signed-principal-root",
            "writable": true,
            "note": "This opens the signed principal's mutable localhost object space. It is not a broad host filesystem grant."
        })),
        published: false,
        shared: false,
        capabilities: vec!["open", "list", "properties"],
    })
}

fn sort_spaces_root_objects(objects: &mut [LibraryObject]) {
    objects.sort_by(|left, right| {
        spaces_root_rank(left)
            .cmp(&spaces_root_rank(right))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
}

fn spaces_root_rank(object: &LibraryObject) -> u8 {
    match object.name.as_str() {
        "Localhost" => 0,
        "Elastos" => 1,
        _ => 2,
    }
}

fn webspace_content_cid_from_metadata(metadata: &Value) -> Option<String> {
    metadata
        .get("target_uri")
        .and_then(Value::as_str)
        .and_then(|target| target.strip_prefix("elastos://"))
        .map(str::trim)
        .filter(|cid| cid::Cid::try_from(*cid).is_ok())
        .map(str::to_string)
}

fn webspace_object_metadata(uri: &str, stat: &WebSpaceFileStat) -> Value {
    let mount = uri
        .strip_prefix("localhost://WebSpaces/")
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty());
    let target_uri = stat
        .target_uri
        .clone()
        .or_else(|| inferred_webspace_target_uri(uri));
    let mut metadata = json!({
        "schema": "elastos.library.webspace-object/v1",
        "handle_uri": uri,
        "mount": mount,
        "resolver_state": stat.resolver_state.as_deref().unwrap_or("resolved"),
        "resolver": stat.resolver.as_deref().unwrap_or("builtin"),
        "cache_policy": stat.cache_policy.as_deref().unwrap_or("metadata-only"),
        "sync_policy": stat.sync_policy.as_deref().unwrap_or("manual"),
        "cache_state": stat.cache_state.as_deref().unwrap_or("metadata_cached"),
        "sync_state": stat.sync_state.as_deref().unwrap_or("manual_idle"),
        "webspace_kind": stat.webspace_kind.as_deref().unwrap_or("webspace-object"),
        "readonly": stat.readonly.unwrap_or(true),
        "access_policy": stat.access_policy.as_deref().unwrap_or("resolver-readonly"),
        "provider": stat.provider.as_deref().unwrap_or("webspace-provider"),
    });
    if let Some(object_id) = stat.object_id.as_deref() {
        metadata["object_id"] = Value::String(object_id.to_string());
    }
    if let Some(head_id) = stat.head_id.as_deref() {
        metadata["head_id"] = Value::String(head_id.to_string());
    }
    if let Some(target_uri) = target_uri {
        metadata["target_uri"] = Value::String(target_uri);
    }
    if let Some(hint) = webspace_availability_hint(uri, stat, metadata.get("target_uri")) {
        metadata["availability_hint"] = hint;
    }
    metadata
}

fn webspace_availability_hint(
    uri: &str,
    stat: &WebSpaceFileStat,
    target_uri: Option<&Value>,
) -> Option<Value> {
    if !stat.is_file {
        return None;
    }
    let resolver = stat.resolver.as_deref().unwrap_or("builtin");
    if resolver == "builtin" || resolver == "local-materialized" {
        return None;
    }
    let target_uri = target_uri.and_then(Value::as_str)?;
    let webspace_kind = stat.webspace_kind.as_deref().unwrap_or("webspace-object");
    let cache_state = stat.cache_state.as_deref().unwrap_or("metadata_cached");
    let sync_state = stat.sync_state.as_deref().unwrap_or("manual_idle");
    let readonly = stat.readonly.unwrap_or(true);
    let status = if !readonly && sync_state == "manual_synced" {
        "resolver_synced"
    } else if readonly && webspace_kind == "materialized-file" && cache_state == "content_cached" {
        "resolver_cached"
    } else {
        return None;
    };
    Some(json!({
        "schema": "elastos.webspace.availability-hint/v1",
        "scope": "resolver",
        "status": status,
        "handle_uri": uri,
        "target_uri": target_uri,
        "resolver": resolver,
        "cache_state": cache_state,
        "sync_state": sync_state,
        "not_content_availability": true,
        "note": "This is a resolver/cache hint only. It is not a SmartWeb content availability receipt and does not prove CID replication."
    }))
}

fn webspace_availability_label(metadata: &Value) -> String {
    match metadata
        .get("availability_hint")
        .and_then(|hint| hint.get("status"))
        .and_then(Value::as_str)
    {
        Some("resolver_synced") => "resolver-synced",
        Some("resolver_cached") => "resolver-cached",
        _ => "resolver-owned",
    }
    .to_string()
}

fn webspace_object_availability_hint(object: &LibraryObject) -> Option<Value> {
    object
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("availability_hint"))
        .cloned()
}

fn inferred_webspace_target_uri(uri: &str) -> Option<String> {
    let cid = uri.strip_prefix("localhost://WebSpaces/Elastos/content/")?;
    let cid = cid.split('/').next()?.trim();
    if cid.is_empty() {
        None
    } else {
        Some(format!("elastos://{cid}"))
    }
}

fn library_target(data_dir: &Path, principal_id: &str, uri: &str) -> anyhow::Result<LibraryTarget> {
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let uri = clean_library_uri(&localhost_root, uri)?;
    let rooted = uri
        .strip_prefix("localhost://")
        .ok_or_else(|| anyhow!("library object URI must be localhost://"))?;
    let path = rooted_localhost_fs_path(data_dir, rooted)
        .ok_or_else(|| anyhow!("invalid library object path"))?;
    Ok(LibraryTarget {
        localhost_root,
        uri,
        path,
    })
}

fn clean_library_uri(localhost_root: &str, uri: &str) -> anyhow::Result<String> {
    let uri = uri.trim().trim_end_matches('/').to_string();
    if !uri.starts_with("localhost://") {
        bail!("library object URI must be localhost://");
    }
    let under_root = uri == localhost_root
        || uri
            .strip_prefix(localhost_root)
            .is_some_and(|rest| rest.starts_with('/'));
    if !under_root {
        bail!("library object URI is outside the active principal root");
    }
    if uri.split('/').any(|part| part == ".." || part == ".") {
        bail!("library object URI must not contain traversal segments");
    }
    Ok(uri)
}

fn is_webspace_uri(uri: &str) -> bool {
    uri == "localhost://WebSpaces"
        || uri
            .strip_prefix("localhost://WebSpaces/")
            .is_some_and(|rest| !rest.is_empty())
}

fn clean_webspace_uri(uri: &str) -> anyhow::Result<String> {
    let uri = uri.trim().trim_end_matches('/').to_string();
    if !is_webspace_uri(&uri) {
        bail!("Library Spaces URI must be under localhost://WebSpaces");
    }
    if uri.split('/').any(|part| part == ".." || part == ".") {
        bail!("Library Spaces URI must not contain traversal segments");
    }
    Ok(uri)
}

fn library_request_touches_webspace(request: &ObjectProviderRequest) -> bool {
    fn any_webspace(values: &[&str]) -> bool {
        values
            .iter()
            .any(|value| is_webspace_uri(value.trim_end_matches('/')))
    }

    match request {
        ObjectProviderRequest::List { uri: Some(uri), .. } => any_webspace(&[uri]),
        ObjectProviderRequest::Stat { uri, .. }
        | ObjectProviderRequest::Read { uri, .. }
        | ObjectProviderRequest::Download { uri, .. }
        | ObjectProviderRequest::ExtractArchive { uri, .. }
        | ObjectProviderRequest::ArchiveEntries { uri, .. }
        | ObjectProviderRequest::ArchivePreviewEntry { uri, .. }
        | ObjectProviderRequest::Write { uri, .. }
        | ObjectProviderRequest::Rename { uri, .. }
        | ObjectProviderRequest::Trash { uri, .. }
        | ObjectProviderRequest::DeletePermanently { uri, .. }
        | ObjectProviderRequest::Status { uri, .. }
        | ObjectProviderRequest::Sync { uri, .. }
        | ObjectProviderRequest::Publish { uri, .. }
        | ObjectProviderRequest::Unpublish { uri, .. }
        | ObjectProviderRequest::Repair { uri, .. }
        | ObjectProviderRequest::Share { uri, .. }
        | ObjectProviderRequest::SharedAccess { uri, .. } => any_webspace(&[uri]),
        ObjectProviderRequest::CompressArchive { uri, uris, .. } => {
            uri.as_deref().is_some_and(|uri| any_webspace(&[uri]))
                || uris
                    .iter()
                    .any(|uri| is_webspace_uri(uri.trim_end_matches('/')))
        }
        ObjectProviderRequest::ArchiveExtractEntries {
            uri,
            destination_uri,
            ..
        } => any_webspace(&[uri, destination_uri]),
        ObjectProviderRequest::Mkdir { parent_uri, .. } => any_webspace(&[parent_uri]),
        ObjectProviderRequest::Move {
            uri,
            target_parent_uri,
            ..
        }
        | ObjectProviderRequest::Copy {
            uri,
            target_parent_uri,
            ..
        } => any_webspace(&[uri, target_parent_uri]),
        ObjectProviderRequest::Restore {
            uri, target_uri, ..
        } => target_uri
            .as_deref()
            .map(|target_uri| any_webspace(&[uri, target_uri]))
            .unwrap_or_else(|| any_webspace(&[uri])),
        ObjectProviderRequest::Roots { .. }
        | ObjectProviderRequest::List { uri: None, .. }
        | ObjectProviderRequest::EmptyTrash { .. }
        | ObjectProviderRequest::Events { .. } => false,
    }
}

fn library_object(data_dir: &Path, principal_id: &str, uri: &str) -> anyhow::Result<LibraryObject> {
    let target = library_target(data_dir, principal_id, uri)?;
    let metadata = fs::metadata(&target.path)?;
    let is_dir = metadata.is_dir();
    let name = if target.uri == target.localhost_root {
        "Home".to_string()
    } else {
        target
            .uri
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("object")
            .to_string()
    };
    let modified_at = system_time_secs(metadata.modified().ok()).unwrap_or_else(now_ts);
    let created_at = system_time_secs(metadata.created().ok()).unwrap_or(modified_at);
    let mut blocked_reason = None;
    let (size, revision, content_cid) = if is_dir {
        (0, directory_revision(&target.path, &target.uri)?, None)
    } else {
        match read_library_file_bytes(data_dir, principal_id, &target) {
            Ok(bytes) => {
                let revision = format!("rev:{}", hex::encode(Sha256::digest(&bytes)));
                let content_cid = raw_sha256_cid(&bytes)?;
                (bytes.len() as u64, revision, Some(content_cid))
            }
            Err(err) if is_unencrypted_principal_root_object(&err) => {
                blocked_reason = Some("protected_principal_root_object_not_encrypted".to_string());
                let revision_input = format!("{}:{}:{}", target.uri, metadata.len(), modified_at);
                (
                    metadata.len(),
                    format!(
                        "rev:blocked:{}",
                        hex::encode(Sha256::digest(revision_input))
                    ),
                    None,
                )
            }
            Err(err) => return Err(err),
        }
    };
    let record = read_publish_record(data_dir, principal_id, &target.uri).ok();
    let active_record = record.as_ref().filter(|record| record_is_published(record));
    let published_cid = active_record.map(|record| record.cid.clone());
    let is_trash_root = is_trash_root_uri(&target.localhost_root, &target.uri);
    let in_trash = is_trash_uri(&target.localhost_root, &target.uri);
    let visibility = library_visibility_metadata(
        &target.localhost_root,
        &target.uri,
        is_dir,
        blocked_reason.as_deref(),
        active_record,
    );
    let mut capabilities = if is_dir {
        let mut capabilities = vec![
            "open",
            "list",
            "rename",
            "move",
            "copy",
            "trash",
            "properties",
        ];
        if target.uri != target.localhost_root
            && !is_runtime_private_uri(&target.localhost_root, &target.uri)
        {
            capabilities.push("download");
            capabilities.push("compress_archive");
        }
        capabilities
    } else {
        let mut capabilities = vec![
            "open",
            "read",
            "download",
            "rename",
            "move",
            "copy",
            "publish",
            "trash",
            "properties",
        ];
        if !is_runtime_private_uri(&target.localhost_root, &target.uri) {
            capabilities.push("compress_archive");
        }
        if active_record.is_some() {
            capabilities.push("unpublish");
            capabilities.push("repair");
            capabilities.push("share");
        }
        if is_extractable_archive_name(&name) {
            capabilities.push("extract_archive");
        }
        capabilities
    };
    if is_trash_root {
        capabilities = vec!["open", "list", "empty_trash", "properties"];
    } else if in_trash {
        capabilities = vec!["restore", "delete_permanently", "properties"];
    }
    if blocked_reason.is_some() {
        capabilities = vec!["properties"];
    }
    let mut local_metadata = if is_dir {
        json!({
            "schema": "elastos.library.object-metadata/v1",
            "visibility": visibility,
        })
    } else {
        let mut metadata = json!({
            "schema": "elastos.library.object-metadata/v1",
            "visibility": visibility,
            "content_identity": {
                "schema": "elastos.library.content-identity/v1",
                "current_cid": content_cid.clone(),
                "scope": "local-object-head",
                "published_cid": published_cid.clone(),
                "note": "The current CID is the immutable raw-byte CID for this mutable object head. The published CID is set only after content-provider publish."
            }
        });
        if let Some(archive_support) = archive_support_for_name(&name) {
            metadata["archive_support"] = archive_support;
        }
        metadata
    };
    if in_trash && !is_trash_root {
        if let Ok(record) = read_trash_record(data_dir, principal_id, &target.uri) {
            local_metadata["trash"] = json!({
                "schema": LIBRARY_TRASH_RECORD_SCHEMA,
                "trash_uri": record.trash_uri,
                "original_uri": record.original_uri,
                "original_name": record.original_name,
                "trashed_at": record.trashed_at,
            });
        }
    }
    let local_metadata = Some(local_metadata);
    let viewers = viewer_options_for_name(data_dir, uri);
    let viewer = viewers.first().map(|viewer| viewer.id.clone());
    let availability = if blocked_reason.is_some() {
        "blocked".to_string()
    } else {
        record_availability_label(record.as_ref())
    };
    Ok(LibraryObject {
        schema: LIBRARY_OBJECT_SCHEMA,
        uri: target.uri,
        name,
        kind: if is_dir { "directory" } else { "file" },
        mime: if is_dir {
            "inode/directory".to_string()
        } else {
            mime_for_name(uri).to_string()
        },
        size,
        created_at,
        modified_at,
        revision,
        viewer,
        viewers,
        thumbnail_uri: None,
        availability,
        blocked_reason,
        content_cid,
        published_cid,
        metadata: local_metadata,
        published: active_record.is_some(),
        shared: active_record.is_some_and(|record| record.shared_at.is_some()),
        capabilities,
    })
}

fn library_visibility_metadata(
    localhost_root: &str,
    uri: &str,
    is_dir: bool,
    blocked_reason: Option<&str>,
    active_record: Option<&LibraryPublishRecord>,
) -> Value {
    let placement = if is_trash_uri(localhost_root, uri) {
        "trash"
    } else if is_runtime_private_uri(localhost_root, uri) {
        "runtime_private"
    } else if is_public_uri(localhost_root, uri) {
        "public_folder"
    } else {
        "private_folder"
    };
    let share_policy = active_record
        .and_then(|record| record.share_policy.as_deref())
        .unwrap_or("not_shared");
    let effective_access = if blocked_reason.is_some() {
        "blocked"
    } else if active_record.is_some_and(|record| record.shared_at.is_some()) {
        match share_policy {
            "recipient_scoped" => "recipient_scoped_link",
            _ => "public_content_link",
        }
    } else if active_record.is_some() {
        "public_content_link"
    } else {
        "principal_private"
    };
    let published_cid = active_record.map(|record| record.cid.clone());

    json!({
        "schema": LIBRARY_VISIBILITY_SCHEMA,
        "placement": placement,
        "placement_label": match placement {
            "public_folder" => "Public folder",
            "trash" => "Trash",
            "runtime_private" => "Runtime private area",
            _ => "Private folder",
        },
        "effective_access": effective_access,
        "published": active_record.is_some(),
        "published_cid": published_cid.clone(),
        "published_link": published_cid.map(|cid| format!("elastos://{cid}")),
        "shared": active_record.is_some_and(|record| record.shared_at.is_some()),
        "share_policy": share_policy,
        "public_folder_policy": "placement_only",
        "publish_required_for_public_link": !is_dir && active_record.is_none(),
        "note": "Public folder placement is a user-facing Library projection. Public network access requires an explicit content-provider publish receipt."
    })
}

fn is_public_uri(localhost_root: &str, uri: &str) -> bool {
    let public_root = format!("{}/Public", localhost_root.trim_end_matches('/'));
    uri == public_root || uri.starts_with(&format!("{public_root}/"))
}

fn raw_sha256_cid(bytes: &[u8]) -> anyhow::Result<String> {
    let digest = Sha256::digest(bytes);
    let multihash = cid::multihash::Multihash::<64>::wrap(0x12, &digest)
        .map_err(|err| anyhow!("failed to build raw content CID: {err}"))?;
    Ok(cid::Cid::new_v1(0x55, multihash).to_string())
}

fn is_unencrypted_principal_root_object(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .to_string()
            .contains(crate::auth::PROTECTED_PRINCIPAL_ROOT_OBJECT_NOT_ENCRYPTED)
    })
}

fn read_library_file_bytes(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Vec<u8>> {
    match crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &target.path,
    ) {
        Ok(bytes) => Ok(bytes),
        Err(err) if is_unencrypted_principal_root_object(&err) => {
            protect_legacy_plaintext_library_file(data_dir, principal_id, target)
        }
        Err(err) => Err(err),
    }
}

fn write_library_file_bytes(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    _mime: Option<&str>,
    if_revision: Option<&str>,
    bytes: &[u8],
) -> anyhow::Result<LibraryObject> {
    let target = library_target(data_dir, principal_id, uri)?;
    if is_trash_uri(&target.localhost_root, &target.uri) {
        bail!("library Trash accepts objects only through delete");
    }
    check_revision(data_dir, principal_id, &target.uri, if_revision)?;
    if let Some(parent) = target.path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &target.path,
        bytes,
    )?;
    let object = library_object(data_dir, principal_id, &target.uri)?;
    append_library_event(
        data_dir,
        principal_id,
        "write",
        &target.uri,
        json!({
            "object": object.clone(),
        }),
    )?;
    Ok(object)
}

fn library_download_object(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<(LibraryObject, String, Vec<u8>)> {
    let target = library_target(data_dir, principal_id, uri)?;
    let object = library_object(data_dir, principal_id, &target.uri)?;
    if target.path.is_file() {
        let bytes = read_library_file_bytes(data_dir, principal_id, &target)?;
        let filename = object.name.clone();
        return Ok((object, filename, bytes));
    }
    if !target.path.is_dir() {
        bail!("library download target must be a file or directory");
    }
    let bytes = archive_library_directory(data_dir, principal_id, &target, archive_format)?;
    let filename = format!(
        "{}.{}",
        safe_archive_name(&object.name),
        archive_format.extension()
    );
    let mut archive_object = object;
    archive_object.mime = archive_format.mime().to_string();
    archive_object.size = bytes.len() as u64;
    Ok((archive_object, filename, bytes))
}

fn compress_library_archive(
    data_dir: &Path,
    principal_id: &str,
    uri: Option<&str>,
    uris: &[String],
    if_revision: Option<&str>,
) -> anyhow::Result<LibraryObject> {
    if !uris.is_empty() {
        if uri.is_some() {
            bail!("library compress_archive accepts either uri or uris, not both");
        }
        if if_revision.is_some() {
            bail!("library selected compress_archive does not accept if_revision");
        }
        let (parent_uri, targets) =
            library_selection_archive_targets(data_dir, principal_id, uris)?;
        let bytes = archive_library_selection_zip(data_dir, principal_id, &targets)?;
        let filename = library_selection_archive_filename(&parent_uri, LibraryArchiveFormat::Zip);
        let archive_uri = unique_child_uri(data_dir, principal_id, &parent_uri, &filename)?;
        let object = write_library_file_bytes(
            data_dir,
            principal_id,
            &archive_uri,
            Some(LibraryArchiveFormat::Zip.mime()),
            None,
            &bytes,
        )?;
        append_library_event(
            data_dir,
            principal_id,
            "compress_archive",
            &archive_uri,
            json!({
                "uris": targets.iter().map(|target| target.uri.clone()).collect::<Vec<_>>(),
                "object": object.clone(),
            }),
        )?;
        return Ok(object);
    }

    let uri = uri.ok_or_else(|| anyhow!("library compress_archive requires uri or uris"))?;
    let target = library_target(data_dir, principal_id, uri)?;
    check_revision(data_dir, principal_id, &target.uri, if_revision)?;
    let parent_uri = target
        .uri
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .ok_or_else(|| anyhow!("library compress_archive target has no parent"))?;
    let name = target
        .uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Library");
    let filename = format!("{}.zip", safe_archive_name(name));
    let bytes = archive_library_single_zip(data_dir, principal_id, &target)?;
    let archive_uri = unique_child_uri(data_dir, principal_id, &parent_uri, &filename)?;
    let object = write_library_file_bytes(
        data_dir,
        principal_id,
        &archive_uri,
        Some(LibraryArchiveFormat::Zip.mime()),
        None,
        &bytes,
    )?;
    append_library_event(
        data_dir,
        principal_id,
        "compress_archive",
        &archive_uri,
        json!({
            "source_uri": target.uri,
            "object": object.clone(),
        }),
    )?;
    Ok(object)
}

fn protect_legacy_plaintext_library_file(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Vec<u8>> {
    let bytes = fs::read(&target.path).with_context(|| {
        format!(
            "failed to read legacy plaintext library object {:?}",
            target.path
        )
    })?;
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &target.path,
        &bytes,
    )?;
    Ok(bytes)
}

fn archive_library_directory(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<Vec<u8>> {
    if target.uri == target.localhost_root {
        bail!("library root download is not supported; use Recovery Kit for full backups");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library folders cannot be downloaded");
    }
    match archive_format {
        LibraryArchiveFormat::TarGz => {
            archive_library_directory_tar_gz(data_dir, principal_id, target)
        }
        LibraryArchiveFormat::Zip => archive_library_directory_zip(data_dir, principal_id, target),
    }
}

fn archive_library_directory_tar_gz(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let archive_root = PathBuf::from(safe_archive_name(
        target
            .uri
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Library"),
    ));
    append_library_archive_entry(&mut builder, data_dir, principal_id, target, &archive_root)?;
    let encoder = builder.into_inner()?;
    let bytes = encoder.finish()?;
    Ok(bytes)
}

fn archive_library_directory_zip(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let archive_root = PathBuf::from(safe_archive_name(
        target
            .uri
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Library"),
    ));
    append_library_zip_entry(&mut writer, data_dir, principal_id, target, &archive_root)?;
    Ok(writer.finish()?.into_inner())
}

fn archive_library_selection(
    data_dir: &Path,
    principal_id: &str,
    uris: &[String],
    archive_format: LibraryArchiveFormat,
) -> anyhow::Result<(String, Vec<u8>)> {
    let (parent_uri, targets) = library_selection_archive_targets(data_dir, principal_id, uris)?;
    let bytes = match archive_format {
        LibraryArchiveFormat::TarGz => {
            archive_library_selection_tar_gz(data_dir, principal_id, &targets)?
        }
        LibraryArchiveFormat::Zip => {
            archive_library_selection_zip(data_dir, principal_id, &targets)?
        }
    };
    Ok((
        library_selection_archive_filename(&parent_uri, archive_format),
        bytes,
    ))
}

fn library_selection_archive_targets(
    data_dir: &Path,
    principal_id: &str,
    uris: &[String],
) -> anyhow::Result<(String, Vec<LibraryTarget>)> {
    if uris.len() < 2 {
        bail!("library selected archive requires at least two objects");
    }
    let mut seen = BTreeSet::new();
    let mut parent_uri: Option<String> = None;
    let mut targets = Vec::new();
    for uri in uris {
        let target = library_target(data_dir, principal_id, uri)?;
        if !seen.insert(target.uri.clone()) {
            continue;
        }
        if target.uri == target.localhost_root {
            bail!("library root download is not supported; use Recovery Kit for full backups");
        }
        if is_runtime_private_uri(&target.localhost_root, &target.uri) {
            bail!("runtime-private Library folders cannot be downloaded");
        }
        if !target.path.is_file() && !target.path.is_dir() {
            bail!("library selected archive entries must be files or directories");
        }
        let parent = target
            .uri
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .ok_or_else(|| anyhow!("library selected archive entry has no parent"))?;
        if parent_uri
            .as_deref()
            .is_some_and(|expected| expected != parent)
        {
            bail!("library selected archive entries must share one parent folder");
        }
        parent_uri = Some(parent);
        targets.push(target);
    }
    if targets.len() < 2 {
        bail!("library selected archive requires at least two unique objects");
    }
    let parent_uri = parent_uri.ok_or_else(|| anyhow!("library selected archive has no parent"))?;
    Ok((parent_uri, targets))
}

fn library_selection_archive_filename(
    parent_uri: &str,
    archive_format: LibraryArchiveFormat,
) -> String {
    let parent_name = parent_uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Library");
    format!(
        "{} Selection.{}",
        safe_archive_name(parent_name),
        archive_format.extension()
    )
}

fn archive_library_selection_tar_gz(
    data_dir: &Path,
    principal_id: &str,
    targets: &[LibraryTarget],
) -> anyhow::Result<Vec<u8>> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for target in targets {
        let name = target
            .uri
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Library");
        append_library_archive_entry(
            &mut builder,
            data_dir,
            principal_id,
            target,
            &PathBuf::from(safe_archive_name(name)),
        )?;
    }
    let encoder = builder.into_inner()?;
    Ok(encoder.finish()?)
}

fn archive_library_selection_zip(
    data_dir: &Path,
    principal_id: &str,
    targets: &[LibraryTarget],
) -> anyhow::Result<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for target in targets {
        let name = target
            .uri
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("Library");
        append_library_zip_entry(
            &mut writer,
            data_dir,
            principal_id,
            target,
            &PathBuf::from(safe_archive_name(name)),
        )?;
    }
    Ok(writer.finish()?.into_inner())
}

fn archive_library_single_zip(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Vec<u8>> {
    if target.uri == target.localhost_root {
        bail!("library root compress is not supported; use Recovery Kit for full backups");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library folders cannot be compressed");
    }
    if !target.path.is_file() && !target.path.is_dir() {
        bail!("library compress_archive target must be a file or directory");
    }
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let name = target
        .uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Library");
    append_library_zip_entry(
        &mut writer,
        data_dir,
        principal_id,
        target,
        &PathBuf::from(safe_archive_name(name)),
    )?;
    Ok(writer.finish()?.into_inner())
}

fn library_archive_entries(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Value> {
    if !target.path.is_file() {
        bail!("library archive listing target must be a file");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library archives cannot be listed");
    }
    let name = library_archive_name(target, "listing")?;
    if !is_extractable_archive_name(&name) {
        bail!("library archive listing only supports .tar, .tar.gz, .tgz, and .zip archives");
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    archive_entries_for_object(
        library_object(data_dir, principal_id, &target.uri)?,
        &target.uri,
        &name,
        bytes,
    )
}

fn archive_preview_entry(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    entry: &str,
) -> anyhow::Result<Value> {
    if !target.path.is_file() {
        bail!("library archive preview target must be a file");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library archives cannot be previewed");
    }
    let name = library_archive_name(target, "preview")?;
    if !is_extractable_archive_name(&name) {
        bail!("library archive preview only supports .tar, .tar.gz, .tgz, and .zip archives");
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    archive_preview_entry_for_object(
        data_dir,
        library_object(data_dir, principal_id, &target.uri)?,
        &target.uri,
        &name,
        bytes,
        entry,
    )
}

fn archive_entries_for_object(
    object: LibraryObject,
    uri: &str,
    archive_name: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<Value> {
    if !is_extractable_archive_name(archive_name) {
        bail!("library archive listing only supports .tar, .tar.gz, .tgz, and .zip archives");
    }
    let lower_name = archive_name.to_ascii_lowercase();
    let (entries, truncated) = if lower_name.ends_with(".zip") {
        list_zip_archive_entries(bytes)?
    } else {
        list_tar_archive_entries(archive_name, bytes)?
    };
    let returned_entries = entries.len();
    Ok(json!({
        "schema": LIBRARY_ARCHIVE_ENTRIES_SCHEMA,
        "object": object,
        "uri": uri,
        "family": archive_family_for_name(archive_name).unwrap_or("archive"),
        "entries": entries,
        "limits": {
            "max_entries": MAX_ARCHIVE_LIST_ENTRIES,
            "returned_entries": returned_entries,
            "truncated": truncated,
        },
    }))
}

fn archive_preview_entry_for_object(
    data_dir: &Path,
    object: LibraryObject,
    uri: &str,
    archive_name: &str,
    bytes: Vec<u8>,
    entry: &str,
) -> anyhow::Result<Value> {
    if !is_extractable_archive_name(archive_name) {
        bail!("library archive preview only supports .tar, .tar.gz, .tgz, and .zip archives");
    }
    let normalized = normalized_archive_entry_path(Path::new(entry))?;
    let preview = if archive_name.to_ascii_lowercase().ends_with(".zip") {
        preview_zip_archive_entry(bytes, &normalized)?
    } else {
        preview_tar_archive_entry(archive_name, bytes, &normalized)?
    };
    let mime = mime_for_name(&normalized);
    let text = if is_archive_preview_text_mime(mime) {
        Some(String::from_utf8_lossy(&preview.bytes).to_string())
    } else {
        None
    };
    let entry_name = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_string();
    let viewers = viewer_options_for_name(data_dir, &normalized);
    Ok(json!({
        "schema": LIBRARY_ARCHIVE_PREVIEW_ENTRY_SCHEMA,
        "object": object,
        "uri": uri,
        "family": archive_family_for_name(archive_name).unwrap_or("archive"),
        "entry": {
            "path": normalized,
            "name": entry_name,
            "kind": "file",
            "size": preview.size,
            "compressed_size": preview.compressed_size,
            "modified_at": preview.modified_at,
            "mime": mime,
            "safety": {
                "status": "safe",
                "reason": Value::Null,
            },
            "viewers": viewers,
        },
        "preview": {
            "encoding": "base64",
            "data": base64::engine::general_purpose::STANDARD.encode(&preview.bytes),
            "text": text,
            "truncated": preview.truncated,
            "max_bytes": MAX_ARCHIVE_PREVIEW_BYTES,
            "mode": "provider_bounded_safe_entry_preview",
        },
    }))
}

struct ArchiveEntryPreview {
    bytes: Vec<u8>,
    size: Option<u64>,
    compressed_size: Option<u64>,
    modified_at: Option<u64>,
    truncated: bool,
}

fn preview_zip_archive_entry(
    bytes: Vec<u8>,
    selected: &str,
) -> anyhow::Result<ArchiveEntryPreview> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| anyhow!("invalid ZIP archive: {err}"))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| anyhow!("invalid ZIP archive entry: {err}"))?;
        let raw_name = file.name().to_string();
        let Ok(normalized) = normalized_archive_entry_path(Path::new(&raw_name)) else {
            continue;
        };
        if normalized != selected {
            continue;
        }
        if file.is_dir() || !file.is_file() {
            bail!("library archive preview only supports safe file entries");
        }
        let declared_size = file.size();
        let mut preview_bytes = Vec::new();
        file.by_ref()
            .take(MAX_ARCHIVE_PREVIEW_BYTES as u64 + 1)
            .read_to_end(&mut preview_bytes)?;
        let truncated = preview_bytes.len() > MAX_ARCHIVE_PREVIEW_BYTES
            || declared_size > MAX_ARCHIVE_PREVIEW_BYTES as u64;
        preview_bytes.truncate(MAX_ARCHIVE_PREVIEW_BYTES);
        return Ok(ArchiveEntryPreview {
            bytes: preview_bytes,
            size: Some(declared_size),
            compressed_size: Some(file.compressed_size()),
            modified_at: None,
            truncated,
        });
    }
    bail!("library archive preview entry not found");
}

fn preview_tar_archive_entry(
    name: &str,
    bytes: Vec<u8>,
    selected: &str,
) -> anyhow::Result<ArchiveEntryPreview> {
    let reader: Box<dyn std::io::Read> = if name.to_ascii_lowercase().ends_with(".tar") {
        Box::new(Cursor::new(bytes))
    } else {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    };
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = match entry.path() {
            Ok(path) => path.to_path_buf(),
            Err(_) => continue,
        };
        let Ok(normalized) = normalized_archive_entry_path(&path) else {
            continue;
        };
        if normalized != selected {
            continue;
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() || !entry_type.is_file() {
            bail!("library archive preview only supports safe file entries");
        }
        let declared_size = entry.header().size().ok();
        let modified_at = entry.header().mtime().ok();
        let mut preview_bytes = Vec::new();
        entry
            .by_ref()
            .take(MAX_ARCHIVE_PREVIEW_BYTES as u64 + 1)
            .read_to_end(&mut preview_bytes)?;
        let truncated = preview_bytes.len() > MAX_ARCHIVE_PREVIEW_BYTES
            || declared_size.is_some_and(|size| size > MAX_ARCHIVE_PREVIEW_BYTES as u64);
        preview_bytes.truncate(MAX_ARCHIVE_PREVIEW_BYTES);
        return Ok(ArchiveEntryPreview {
            bytes: preview_bytes,
            size: declared_size,
            compressed_size: None,
            modified_at,
            truncated,
        });
    }
    bail!("library archive preview entry not found");
}

fn is_archive_preview_text_mime(mime: &str) -> bool {
    mime == "text/plain" || mime == "application/json" || mime == "text/html"
}

fn library_archive_name(target: &LibraryTarget, action: &str) -> anyhow::Result<String> {
    target
        .uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("library archive {action} target has no name"))
}

fn check_object_revision(object: &LibraryObject, if_revision: Option<&str>) -> anyhow::Result<()> {
    if let Some(expected) = if_revision {
        if expected != object.revision {
            bail!("library object revision mismatch");
        }
    }
    Ok(())
}

fn webspace_archive_object(mut object: LibraryObject) -> LibraryObject {
    if let Some(metadata) = object.metadata.as_mut() {
        redact_resolver_private_fields(metadata);
        if let Some(map) = metadata.as_object_mut() {
            map.insert("resolver_target_redacted".to_string(), Value::Bool(true));
        }
    }
    object
}

fn redact_resolver_private_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("target_uri");
            map.remove("provider_credentials");
            map.remove("endpoint_credentials");
            map.remove("credentials");
            for child in map.values_mut() {
                redact_resolver_private_fields(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_resolver_private_fields(child);
            }
        }
        _ => {}
    }
}

fn redacted_resolver_private_value(mut value: Value) -> Value {
    redact_resolver_private_fields(&mut value);
    value
}

fn list_tar_archive_entries(name: &str, bytes: Vec<u8>) -> anyhow::Result<(Vec<Value>, bool)> {
    let reader: Box<dyn std::io::Read> = if name.to_ascii_lowercase().ends_with(".tar") {
        Box::new(Cursor::new(bytes))
    } else {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    };
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, entry) in archive.entries()?.enumerate() {
        if entries.len() >= MAX_ARCHIVE_LIST_ENTRIES {
            truncated = true;
            break;
        }
        let entry = entry?;
        let entry_type = entry.header().entry_type();
        let size = entry.header().size().ok();
        let modified_at = entry.header().mtime().ok();
        let path = match entry.path() {
            Ok(path) => path.to_path_buf(),
            Err(err) => {
                entries.push(blocked_archive_entry_listing(
                    index,
                    format!("entry-{index}"),
                    None,
                    None,
                    modified_at,
                    format!("invalid archive entry path: {err}"),
                ));
                continue;
            }
        };
        let display_path = archive_entry_display_path(&path, index);
        let normalized = match normalized_archive_entry_path(&path) {
            Ok(path) => path,
            Err(err) => {
                entries.push(blocked_archive_entry_listing(
                    index,
                    display_path,
                    size,
                    None,
                    modified_at,
                    err.to_string(),
                ));
                continue;
            }
        };
        let kind = if entry_type.is_dir() {
            "directory"
        } else if entry_type.is_file() {
            "file"
        } else {
            entries.push(blocked_archive_entry_listing(
                index,
                normalized,
                size,
                None,
                modified_at,
                "library archive listing rejects non-file archive entries".to_string(),
            ));
            continue;
        };
        entries.push(safe_archive_entry_listing(
            index,
            normalized,
            kind,
            size,
            None,
            modified_at,
        ));
    }
    Ok((entries, truncated))
}

fn list_zip_archive_entries(bytes: Vec<u8>) -> anyhow::Result<(Vec<Value>, bool)> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| anyhow!("invalid ZIP archive: {err}"))?;
    let mut entries = Vec::new();
    let truncated = archive.len() > MAX_ARCHIVE_LIST_ENTRIES;
    for index in 0..archive.len().min(MAX_ARCHIVE_LIST_ENTRIES) {
        let file = archive
            .by_index(index)
            .map_err(|err| anyhow!("invalid ZIP archive entry: {err}"))?;
        let raw_name = file.name().to_string();
        let path = Path::new(&raw_name);
        let normalized = match normalized_archive_entry_path(path) {
            Ok(path) => path,
            Err(err) => {
                entries.push(blocked_archive_entry_listing(
                    index,
                    archive_entry_display_name(&raw_name, index),
                    Some(file.size()),
                    Some(file.compressed_size()),
                    None,
                    err.to_string(),
                ));
                continue;
            }
        };
        let kind = if file.is_dir() {
            "directory"
        } else if file.is_file() {
            "file"
        } else {
            entries.push(blocked_archive_entry_listing(
                index,
                normalized,
                Some(file.size()),
                Some(file.compressed_size()),
                None,
                "library archive listing rejects non-file archive entries".to_string(),
            ));
            continue;
        };
        entries.push(safe_archive_entry_listing(
            index,
            normalized,
            kind,
            Some(file.size()),
            Some(file.compressed_size()),
            None,
        ));
    }
    Ok((entries, truncated))
}

fn normalized_archive_entry_path(path: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| anyhow!("library archive entry path must be UTF-8"))?;
                if name.is_empty()
                    || name.contains('/')
                    || name.contains('\\')
                    || name.contains('\0')
                    || name == "."
                    || name == ".."
                {
                    bail!("library archive entry path must be relative and safe");
                }
                parts.push(name);
            }
            _ => bail!("library archive entry path must be relative and safe"),
        }
    }
    if parts.is_empty() {
        bail!("library archive entry path must not be empty");
    }
    Ok(parts.join("/"))
}

fn archive_entry_display_path(path: &Path, index: usize) -> String {
    let display = path.to_string_lossy().trim().to_string();
    if display.is_empty() {
        format!("entry-{index}")
    } else {
        display
    }
}

fn archive_entry_display_name(name: &str, index: usize) -> String {
    let display = name.trim();
    if display.is_empty() {
        format!("entry-{index}")
    } else {
        display.to_string()
    }
}

struct ArchiveEntryListing {
    index: usize,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    compressed_size: Option<u64>,
    modified_at: Option<u64>,
    safety_status: &'static str,
    safety_reason: Option<String>,
}

fn safe_archive_entry_listing(
    index: usize,
    path: String,
    kind: &'static str,
    size: Option<u64>,
    compressed_size: Option<u64>,
    modified_at: Option<u64>,
) -> Value {
    archive_entry_listing(ArchiveEntryListing {
        index,
        path,
        kind,
        size,
        compressed_size,
        modified_at,
        safety_status: "safe",
        safety_reason: None,
    })
}

fn blocked_archive_entry_listing(
    index: usize,
    path: String,
    size: Option<u64>,
    compressed_size: Option<u64>,
    modified_at: Option<u64>,
    safety_reason: String,
) -> Value {
    archive_entry_listing(ArchiveEntryListing {
        index,
        path,
        kind: "blocked",
        size,
        compressed_size,
        modified_at,
        safety_status: "blocked",
        safety_reason: Some(safety_reason),
    })
}

fn archive_entry_listing(entry: ArchiveEntryListing) -> Value {
    let name = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&entry.path)
        .to_string();
    json!({
        "id": format!("entry:{}", entry.index),
        "path": entry.path,
        "name": name,
        "kind": entry.kind,
        "size": entry.size,
        "compressed_size": entry.compressed_size,
        "modified_at": entry.modified_at,
        "safety": {
            "status": entry.safety_status,
            "reason": entry.safety_reason,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveConflictPolicy {
    KeepBoth,
    Replace,
    Skip,
}

impl ArchiveConflictPolicy {
    fn parse(value: Option<&str>) -> anyhow::Result<Self> {
        match value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("keep_both")
            .to_ascii_lowercase()
            .replace(' ', "_")
            .as_str()
        {
            "keep_both" => Ok(Self::KeepBoth),
            "replace" => Ok(Self::Replace),
            "skip" => Ok(Self::Skip),
            other => bail!("unsupported archive conflict policy: {other}"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::KeepBoth => "keep_both",
            Self::Replace => "replace",
            Self::Skip => "skip",
        }
    }
}

#[derive(Debug)]
struct ArchiveExtractOutcome {
    requested: BTreeSet<String>,
    matched: BTreeSet<String>,
    written: Vec<Value>,
    skipped: Vec<Value>,
    blocked: Vec<Value>,
    processed_entries: usize,
}

impl ArchiveExtractOutcome {
    fn new(requested: BTreeSet<String>) -> Self {
        Self {
            requested,
            matched: BTreeSet::new(),
            written: Vec::new(),
            skipped: Vec::new(),
            blocked: Vec::new(),
            processed_entries: 0,
        }
    }

    fn mark_matched(&mut self, normalized_path: &str) {
        for selected in &self.requested {
            if normalized_path == selected
                || normalized_path
                    .strip_prefix(selected)
                    .is_some_and(|rest| rest.starts_with('/'))
            {
                self.matched.insert(selected.clone());
            }
        }
    }

    fn is_selected(&self, normalized_path: &str) -> bool {
        self.requested.iter().any(|selected| {
            normalized_path == selected
                || normalized_path
                    .strip_prefix(selected)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    fn finish_missing(&mut self) {
        for entry in self.requested.difference(&self.matched) {
            self.skipped.push(json!({
                "path": entry,
                "reason": "entry_not_found",
            }));
        }
    }
}

#[derive(Debug)]
enum ArchiveExtractWrite {
    Directory { path: String },
    File { path: String, bytes: Vec<u8> },
}

struct ArchiveExtractRequest<'a> {
    source_uri: &'a str,
    archive_name: &'a str,
    destination_uri: &'a str,
    entries: &'a [String],
    conflict_policy: Option<&'a str>,
    cancel: bool,
}

struct ArchiveExtractResponseInput<'a> {
    destination_object: LibraryObject,
    source_uri: &'a str,
    destination_uri: &'a str,
    archive_name: &'a str,
    policy: ArchiveConflictPolicy,
    status: &'a str,
    outcome: ArchiveExtractOutcome,
    cancel_requested: bool,
}

fn extract_library_archive_entries(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    destination_uri: &str,
    entries: &[String],
    conflict_policy: Option<&str>,
    cancel: bool,
) -> anyhow::Result<Value> {
    if !target.path.is_file() {
        bail!("library archive selected extraction target must be a file");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library archives cannot be selectively extracted");
    }
    let name = library_archive_name(target, "selected extraction")?;
    if !is_extractable_archive_name(&name) {
        bail!(
            "library archive selected extraction only supports .tar, .tar.gz, .tgz, and .zip archives"
        );
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    let request = ArchiveExtractRequest {
        source_uri: &target.uri,
        archive_name: &name,
        destination_uri,
        entries,
        conflict_policy,
        cancel,
    };
    extract_archive_entries_to_local_destination(data_dir, principal_id, bytes, request)
}

fn extract_archive_entries_to_local_destination(
    data_dir: &Path,
    principal_id: &str,
    bytes: Vec<u8>,
    request: ArchiveExtractRequest<'_>,
) -> anyhow::Result<Value> {
    if !is_extractable_archive_name(request.archive_name) {
        bail!(
            "library archive selected extraction only supports .tar, .tar.gz, .tgz, and .zip archives"
        );
    }
    let destination = library_target(data_dir, principal_id, request.destination_uri)?;
    if destination.path.exists() && !destination.path.is_dir() {
        bail!("library archive selected extraction destination must be a directory");
    }
    let policy = ArchiveConflictPolicy::parse(request.conflict_policy)?;
    let selected_entries = selected_archive_entries(request.entries)?;
    let mut outcome = ArchiveExtractOutcome::new(selected_entries);
    if request.cancel {
        let destination_object = library_object(data_dir, principal_id, &destination.uri)?;
        return Ok(archive_extract_entries_response(
            ArchiveExtractResponseInput {
                destination_object,
                source_uri: request.source_uri,
                destination_uri: &destination.uri,
                archive_name: request.archive_name,
                policy,
                status: "cancelled",
                outcome,
                cancel_requested: true,
            },
        ));
    }

    fs::create_dir_all(&destination.path)?;
    let writes = collect_selected_archive_writes(request.archive_name, bytes, &mut outcome)?;
    apply_selected_archive_writes_to_local(
        data_dir,
        principal_id,
        &destination,
        writes,
        policy,
        &mut outcome,
    )?;
    outcome.finish_missing();
    let status = if outcome.blocked.is_empty() {
        "completed"
    } else {
        "completed_with_blocked_entries"
    };
    let destination_object = library_object(data_dir, principal_id, &destination.uri)?;
    let response = archive_extract_entries_response(ArchiveExtractResponseInput {
        destination_object,
        source_uri: request.source_uri,
        destination_uri: &destination.uri,
        archive_name: request.archive_name,
        policy,
        status,
        outcome,
        cancel_requested: false,
    });
    append_library_event(
        data_dir,
        principal_id,
        "archive_extract_entries",
        &destination.uri,
        json!({
            "source_uri": request.source_uri,
            "destination_uri": destination.uri,
            "receipt": response.get("receipt").cloned().unwrap_or(Value::Null),
        }),
    )?;
    Ok(response)
}

fn selected_archive_entries(entries: &[String]) -> anyhow::Result<BTreeSet<String>> {
    let mut selected = BTreeSet::new();
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        selected.insert(normalized_archive_entry_path(Path::new(entry))?);
    }
    if selected.is_empty() {
        bail!("library archive selected extraction requires at least one entry");
    }
    Ok(selected)
}

fn collect_selected_archive_writes(
    name: &str,
    bytes: Vec<u8>,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<Vec<ArchiveExtractWrite>> {
    if name.to_ascii_lowercase().ends_with(".zip") {
        collect_selected_zip_writes(bytes, outcome)
    } else {
        collect_selected_tar_writes(name, bytes, outcome)
    }
}

fn collect_selected_tar_writes(
    name: &str,
    bytes: Vec<u8>,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<Vec<ArchiveExtractWrite>> {
    let reader: Box<dyn std::io::Read> = if name.to_ascii_lowercase().ends_with(".tar") {
        Box::new(Cursor::new(bytes))
    } else {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    };
    let mut archive = tar::Archive::new(reader);
    let mut writes = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = match entry.path() {
            Ok(path) => path.to_path_buf(),
            Err(err) => {
                outcome.blocked.push(json!({
                    "path": "invalid-entry-path",
                    "reason": format!("invalid archive entry path: {err}"),
                }));
                continue;
            }
        };
        let normalized = match normalized_archive_entry_path(&path) {
            Ok(path) => path,
            Err(err) => {
                outcome.blocked.push(json!({
                    "path": archive_entry_display_path(&path, outcome.processed_entries),
                    "reason": err.to_string(),
                }));
                continue;
            }
        };
        if !outcome.is_selected(&normalized) {
            continue;
        }
        outcome.mark_matched(&normalized);
        outcome.processed_entries += 1;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            writes.push(ArchiveExtractWrite::Directory { path: normalized });
            continue;
        }
        if !entry_type.is_file() {
            outcome.blocked.push(json!({
                "path": normalized,
                "reason": "library archive selected extraction rejects non-file archive entries",
            }));
            continue;
        }
        let mut entry_bytes = Vec::new();
        entry.read_to_end(&mut entry_bytes)?;
        writes.push(ArchiveExtractWrite::File {
            path: normalized,
            bytes: entry_bytes,
        });
    }
    Ok(writes)
}

fn collect_selected_zip_writes(
    bytes: Vec<u8>,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<Vec<ArchiveExtractWrite>> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| anyhow!("invalid ZIP archive: {err}"))?;
    let mut writes = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| anyhow!("invalid ZIP archive entry: {err}"))?;
        let raw_name = file.name().to_string();
        let normalized = match normalized_archive_entry_path(Path::new(&raw_name)) {
            Ok(path) => path,
            Err(err) => {
                outcome.blocked.push(json!({
                    "path": archive_entry_display_name(&raw_name, index),
                    "reason": err.to_string(),
                }));
                continue;
            }
        };
        if !outcome.is_selected(&normalized) {
            continue;
        }
        outcome.mark_matched(&normalized);
        outcome.processed_entries += 1;
        if file.is_dir() {
            writes.push(ArchiveExtractWrite::Directory { path: normalized });
            continue;
        }
        if !file.is_file() {
            outcome.blocked.push(json!({
                "path": normalized,
                "reason": "library archive selected extraction rejects non-file archive entries",
            }));
            continue;
        }
        let mut entry_bytes = Vec::new();
        file.read_to_end(&mut entry_bytes)?;
        writes.push(ArchiveExtractWrite::File {
            path: normalized,
            bytes: entry_bytes,
        });
    }
    Ok(writes)
}

fn apply_selected_archive_writes_to_local(
    data_dir: &Path,
    principal_id: &str,
    destination: &LibraryTarget,
    writes: Vec<ArchiveExtractWrite>,
    policy: ArchiveConflictPolicy,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<()> {
    for write in writes {
        match write {
            ArchiveExtractWrite::Directory { path } => {
                create_selected_archive_directory(data_dir, principal_id, destination, &path)?;
                outcome.written.push(json!({
                    "path": path,
                    "uri": archive_entry_uri(&destination.uri, Path::new(&path))?,
                    "kind": "directory",
                }));
            }
            ArchiveExtractWrite::File { path, bytes } => {
                write_selected_archive_file(
                    data_dir,
                    principal_id,
                    destination,
                    &path,
                    &bytes,
                    policy,
                    outcome,
                )?;
            }
        }
    }
    Ok(())
}

async fn extract_archive_entries_to_webspace_destination(
    data_dir: &Path,
    registry: &ProviderRegistry,
    principal_id: &str,
    bytes: Vec<u8>,
    request: ArchiveExtractRequest<'_>,
) -> anyhow::Result<Value> {
    if !is_extractable_archive_name(request.archive_name) {
        bail!(
            "library archive selected extraction only supports .tar, .tar.gz, .tgz, and .zip archives"
        );
    }
    let destination_uri = clean_webspace_uri(request.destination_uri)?;
    let destination_object = webspace_stat_object(data_dir, registry, &destination_uri).await?;
    if destination_object.kind != "directory" {
        bail!("library archive selected extraction WebSpace destination must be a folder");
    }
    ensure_webspace_archive_write_allowed(registry, &destination_object).await?;
    let policy = ArchiveConflictPolicy::parse(request.conflict_policy)?;
    if policy != ArchiveConflictPolicy::Replace {
        bail!("Spaces archive write-back requires conflict_policy=replace until resolver adapters expose existence/conflict APIs");
    }
    let selected_entries = selected_archive_entries(request.entries)?;
    let mut outcome = ArchiveExtractOutcome::new(selected_entries);
    if request.cancel {
        return Ok(archive_extract_entries_response(
            ArchiveExtractResponseInput {
                destination_object: webspace_archive_object(destination_object),
                source_uri: request.source_uri,
                destination_uri: &destination_uri,
                archive_name: request.archive_name,
                policy,
                status: "cancelled",
                outcome,
                cancel_requested: true,
            },
        ));
    }
    let writes = collect_selected_archive_writes(request.archive_name, bytes, &mut outcome)?;
    apply_selected_archive_writes_to_webspace(
        data_dir,
        registry,
        &destination_uri,
        writes,
        &mut outcome,
    )
    .await?;
    outcome.finish_missing();
    let status = if outcome.blocked.is_empty() {
        "completed"
    } else {
        "completed_with_blocked_entries"
    };
    let destination_object = webspace_stat_object(data_dir, registry, &destination_uri).await?;
    let response = archive_extract_entries_response(ArchiveExtractResponseInput {
        destination_object: webspace_archive_object(destination_object),
        source_uri: request.source_uri,
        destination_uri: &destination_uri,
        archive_name: request.archive_name,
        policy,
        status,
        outcome,
        cancel_requested: false,
    });
    append_library_event(
        data_dir,
        principal_id,
        "archive_extract_entries",
        &destination_uri,
        json!({
            "source_uri": request.source_uri,
            "destination_uri": destination_uri,
            "receipt": response.get("receipt").cloned().unwrap_or(Value::Null),
        }),
    )?;
    Ok(response)
}

async fn ensure_webspace_archive_write_allowed(
    registry: &ProviderRegistry,
    destination: &LibraryObject,
) -> anyhow::Result<()> {
    if webspace_metadata_bool(destination, "readonly") != Some(false) {
        bail!("Spaces archive write-back requires a mutable destination Space");
    }
    let Some(adapter) = webspace_adapter_target(registry, destination).await? else {
        bail!("Spaces archive write-back requires a connected resolver adapter");
    };
    if !adapter.capabilities.contains("write_bytes") {
        bail!("Spaces archive write-back requires an adapter with write_bytes capability");
    }
    Ok(())
}

async fn apply_selected_archive_writes_to_webspace(
    data_dir: &Path,
    registry: &ProviderRegistry,
    destination_uri: &str,
    writes: Vec<ArchiveExtractWrite>,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<()> {
    for write in writes {
        match write {
            ArchiveExtractWrite::Directory { path } => {
                let uri = archive_entry_uri(destination_uri, Path::new(&path))?;
                let mut receipt = webspace_mkdir(registry, &uri).await?;
                redact_resolver_private_fields(&mut receipt);
                outcome.written.push(json!({
                    "path": path,
                    "uri": uri,
                    "kind": "directory",
                    "webspace": {
                        "write_back": "materialized_directory_handle",
                        "provider_receipt": receipt,
                    },
                }));
            }
            ArchiveExtractWrite::File { path, bytes } => {
                write_selected_archive_webspace_file(
                    data_dir,
                    registry,
                    destination_uri,
                    &path,
                    &bytes,
                    outcome,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn write_selected_archive_webspace_file(
    data_dir: &Path,
    registry: &ProviderRegistry,
    destination_uri: &str,
    normalized_path: &str,
    bytes: &[u8],
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<()> {
    ensure_webspace_archive_parent_dirs(registry, destination_uri, normalized_path).await?;
    let uri = archive_entry_uri(destination_uri, Path::new(normalized_path))?;
    let mut provider_receipt = webspace_write_bytes(registry, &uri, bytes).await?;
    redact_resolver_private_fields(&mut provider_receipt);
    let sync_receipt = webspace_sync_bytes(data_dir, registry, &uri).await?;
    if sync_receipt
        .get("fail_closed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        bail!("Spaces archive write-back failed to sync resolver bytes");
    }
    outcome.written.push(json!({
        "path": normalized_path,
        "uri": uri,
        "kind": "file",
        "size": bytes.len(),
        "webspace": {
            "write_back": "resolver_synced",
            "provider_receipt": provider_receipt,
            "sync_receipt": redacted_resolver_private_value(sync_receipt),
        },
    }));
    Ok(())
}

async fn ensure_webspace_archive_parent_dirs(
    registry: &ProviderRegistry,
    destination_uri: &str,
    normalized_path: &str,
) -> anyhow::Result<()> {
    let mut uri = destination_uri.trim_end_matches('/').to_string();
    let mut components = normalized_path.split('/').peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        uri = child_uri(&uri, component)?;
        let _ = webspace_mkdir(registry, &uri).await?;
    }
    Ok(())
}

fn create_selected_archive_directory(
    data_dir: &Path,
    principal_id: &str,
    destination: &LibraryTarget,
    normalized_path: &str,
) -> anyhow::Result<()> {
    let uri = archive_entry_uri(&destination.uri, Path::new(normalized_path))?;
    let target = library_target(data_dir, principal_id, &uri)?;
    fs::create_dir_all(&target.path)?;
    Ok(())
}

fn write_selected_archive_file(
    data_dir: &Path,
    principal_id: &str,
    destination: &LibraryTarget,
    normalized_path: &str,
    bytes: &[u8],
    policy: ArchiveConflictPolicy,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<()> {
    let candidate_uri = archive_entry_uri(&destination.uri, Path::new(normalized_path))?;
    let target_uri = selected_archive_conflict_uri(
        data_dir,
        principal_id,
        &candidate_uri,
        normalized_path,
        policy,
        outcome,
    )?;
    let Some(target_uri) = target_uri else {
        return Ok(());
    };
    let target = library_target(data_dir, principal_id, &target_uri)?;
    if let Some(parent) = target.path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &target.localhost_root,
        &target.uri,
        &target.path,
        bytes,
    )?;
    outcome.written.push(json!({
        "path": normalized_path,
        "uri": target.uri,
        "kind": "file",
        "size": bytes.len(),
    }));
    Ok(())
}

fn selected_archive_conflict_uri(
    data_dir: &Path,
    principal_id: &str,
    candidate_uri: &str,
    normalized_path: &str,
    policy: ArchiveConflictPolicy,
    outcome: &mut ArchiveExtractOutcome,
) -> anyhow::Result<Option<String>> {
    let target = library_target(data_dir, principal_id, candidate_uri)?;
    if !target.path.exists() {
        return Ok(Some(candidate_uri.to_string()));
    }
    match policy {
        ArchiveConflictPolicy::Skip => {
            outcome.skipped.push(json!({
                "path": normalized_path,
                "uri": candidate_uri,
                "reason": "conflict_skipped",
            }));
            Ok(None)
        }
        ArchiveConflictPolicy::Replace => {
            if target.path.is_dir() {
                fs::remove_dir_all(&target.path)?;
            } else {
                fs::remove_file(&target.path)?;
            }
            Ok(Some(candidate_uri.to_string()))
        }
        ArchiveConflictPolicy::KeepBoth => {
            let (parent_uri, name) = candidate_uri
                .rsplit_once('/')
                .ok_or_else(|| anyhow!("archive selected extraction target has no parent"))?;
            unique_child_uri(data_dir, principal_id, parent_uri, name).map(Some)
        }
    }
}

fn archive_extract_entries_response(input: ArchiveExtractResponseInput<'_>) -> Value {
    let ArchiveExtractResponseInput {
        destination_object,
        source_uri,
        destination_uri,
        archive_name,
        policy,
        status,
        outcome,
        cancel_requested,
    } = input;
    let requested_entries = outcome.requested.len();
    let written_entries = outcome.written.len();
    let skipped_entries = outcome.skipped.len();
    let blocked_entries = outcome.blocked.len();
    json!({
        "schema": LIBRARY_ARCHIVE_EXTRACT_ENTRIES_SCHEMA,
        "object": destination_object,
        "source_uri": source_uri,
        "destination_uri": destination_uri,
        "family": archive_family_for_name(archive_name).unwrap_or("archive"),
        "conflict_policy": policy.as_str(),
        "written": outcome.written,
        "skipped": outcome.skipped,
        "blocked": outcome.blocked,
        "receipt": {
            "schema": "elastos.library.archive-extract-entries.receipt/v1",
            "status": status,
            "progress": {
                "requested_entries": requested_entries,
                "processed_entries": outcome.processed_entries,
                "written_entries": written_entries,
                "skipped_entries": skipped_entries,
                "blocked_entries": blocked_entries,
            },
            "cancel": {
                "supported": true,
                "requested": cancel_requested,
                "status": if cancel_requested { "cancelled_before_write" } else { "not_requested" },
                "mode": "bounded_synchronous_provider_operation",
            },
        },
    })
}

fn extract_library_archive(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<String> {
    if !target.path.is_file() {
        bail!("library archive extraction target must be a file");
    }
    if is_runtime_private_uri(&target.localhost_root, &target.uri) {
        bail!("runtime-private Library archives cannot be extracted");
    }
    let name = target
        .uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("library archive extraction target has no name"))?;
    if !is_extractable_archive_name(name) {
        bail!("library archive extraction only supports .tar, .tar.gz, .tgz, and .zip archives");
    }
    let parent_uri = target
        .uri
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or_else(|| anyhow!("library archive extraction target has no parent"))?;
    let destination_name = archive_extract_folder_name(name);
    let destination_uri = unique_child_uri(data_dir, principal_id, parent_uri, &destination_name)?;
    let destination = library_target(data_dir, principal_id, &destination_uri)?;
    fs::create_dir_all(&destination.path)?;

    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    let lower_name = name.to_ascii_lowercase();
    if lower_name.ends_with(".zip") {
        extract_zip_archive(data_dir, principal_id, &destination, bytes)?;
    } else {
        extract_tar_archive(data_dir, principal_id, &destination, name, bytes)?;
    }
    Ok(destination.uri)
}

fn extract_tar_archive(
    data_dir: &Path,
    principal_id: &str,
    destination: &LibraryTarget,
    name: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<()> {
    let reader: Box<dyn std::io::Read> = if name.to_ascii_lowercase().ends_with(".tar") {
        Box::new(Cursor::new(bytes))
    } else {
        Box::new(GzDecoder::new(Cursor::new(bytes)))
    };
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();
        let entry_uri = archive_entry_uri(&destination.uri, &entry_path)?;
        if entry_uri == destination.uri {
            continue;
        }
        let entry_target = library_target(data_dir, principal_id, &entry_uri)?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&entry_target.path)?;
            continue;
        }
        if !entry_type.is_file() {
            bail!("library archive extraction rejects non-file archive entries");
        }
        let mut entry_bytes = Vec::new();
        entry.read_to_end(&mut entry_bytes)?;
        if let Some(parent) = entry_target.path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::auth::write_principal_root_object(
            data_dir,
            principal_id,
            &entry_target.localhost_root,
            &entry_target.uri,
            &entry_target.path,
            &entry_bytes,
        )?;
    }
    Ok(())
}

fn extract_zip_archive(
    data_dir: &Path,
    principal_id: &str,
    destination: &LibraryTarget,
    bytes: Vec<u8>,
) -> anyhow::Result<()> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|err| anyhow!("invalid ZIP archive: {err}"))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| anyhow!("invalid ZIP archive entry: {err}"))?;
        let entry_path = file
            .enclosed_name()
            .ok_or_else(|| anyhow!("library archive entry path must be relative and safe"))?;
        let entry_uri = archive_entry_uri(&destination.uri, &entry_path)?;
        if entry_uri == destination.uri {
            continue;
        }
        let entry_target = library_target(data_dir, principal_id, &entry_uri)?;
        if file.is_dir() {
            fs::create_dir_all(&entry_target.path)?;
            continue;
        }
        if !file.is_file() {
            bail!("library archive extraction rejects non-file archive entries");
        }
        let mut entry_bytes = Vec::new();
        file.read_to_end(&mut entry_bytes)?;
        if let Some(parent) = entry_target.path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::auth::write_principal_root_object(
            data_dir,
            principal_id,
            &entry_target.localhost_root,
            &entry_target.uri,
            &entry_target.path,
            &entry_bytes,
        )?;
    }
    Ok(())
}

fn archive_entry_uri(destination_uri: &str, path: &Path) -> anyhow::Result<String> {
    let mut uri = destination_uri.trim_end_matches('/').to_string();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| anyhow!("library archive entry path must be UTF-8"))?;
                uri = child_uri(&uri, name)?;
            }
            _ => bail!("library archive entry path must be relative and safe"),
        }
    }
    Ok(uri)
}

fn is_extractable_archive_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".tar")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".zip")
}

fn archive_family_for_name(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some("tar.gz")
    } else if name.ends_with(".tar") {
        Some("tar")
    } else if name.ends_with(".zip") {
        Some("zip")
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        Some("tar.xz")
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") {
        Some("tar.bz2")
    } else if name.ends_with(".tar.zst") || name.ends_with(".tzst") {
        Some("tar.zst")
    } else if name.ends_with(".rar") {
        Some("rar")
    } else if name.ends_with(".7z") {
        Some("7z")
    } else if name.ends_with(".xz") {
        Some("xz")
    } else if name.ends_with(".bz2") {
        Some("bz2")
    } else if name.ends_with(".zst") {
        Some("zst")
    } else if name.ends_with(".lz4") {
        Some("lz4")
    } else if name.ends_with(".gz") {
        Some("gzip")
    } else {
        None
    }
}

fn archive_support_for_name(name: &str) -> Option<Value> {
    let family = archive_family_for_name(name)?;
    let extractable = is_extractable_archive_name(name);
    Some(json!({
        "schema": "elastos.library.archive-support/v1",
        "family": family,
        "status": if extractable {
            "extractable"
        } else {
            "policy_gated_unsupported_archive_family"
        },
        "implemented": {
            "download_formats": ["zip", "tar.gz"],
            "compress_to_library": ["zip"],
            "extract_formats": ["zip", "tar", "tar.gz", "tgz"],
            "safety": "relative UTF-8 file paths only; non-file archive entries are rejected"
        },
        "policy_gate": if extractable {
            Value::Null
        } else {
            json!({
                "required": true,
                "reason": "generic archive support needs dependency and release-policy review before enabling",
                "blocked_formats": ["7z", "rar", "tar.xz", "tar.bz2", "tar.zst", "xz", "bz2", "zst", "lz4", "gzip"]
            })
        }
    }))
}

fn archive_extract_folder_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let stem = if lower.ends_with(".tar.gz") {
        &name[..name.len().saturating_sub(".tar.gz".len())]
    } else if lower.ends_with(".tgz") {
        &name[..name.len().saturating_sub(".tgz".len())]
    } else if lower.ends_with(".tar") {
        &name[..name.len().saturating_sub(".tar".len())]
    } else if lower.ends_with(".zip") {
        &name[..name.len().saturating_sub(".zip".len())]
    } else {
        name
    };
    let stem = stem.trim().trim_matches('.');
    if stem.is_empty() {
        "Extracted Archive".to_string()
    } else {
        stem.to_string()
    }
}

fn append_library_archive_entry(
    builder: &mut tar::Builder<GzEncoder<Vec<u8>>>,
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    archive_path: &Path,
) -> anyhow::Result<()> {
    let metadata = fs::metadata(&target.path)?;
    if metadata.is_dir() {
        builder.append_dir(archive_path, &target.path)?;
        for entry in fs::read_dir(&target.path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let child_uri = format!("{}/{}", target.uri, name);
            let child_target = library_target(data_dir, principal_id, &child_uri)?;
            let child_archive_path = archive_path.join(safe_archive_name(&name));
            append_library_archive_entry(
                builder,
                data_dir,
                principal_id,
                &child_target,
                &child_archive_path,
            )?;
        }
        return Ok(());
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    if let Some(modified) = system_time_secs(metadata.modified().ok()) {
        header.set_mtime(modified);
    }
    header.set_cksum();
    let mut data = bytes.as_slice();
    builder.append_data(&mut header, archive_path, &mut data)?;
    Ok(())
}

fn append_library_zip_entry(
    writer: &mut ZipWriter<Cursor<Vec<u8>>>,
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    archive_path: &Path,
) -> anyhow::Result<()> {
    let metadata = fs::metadata(&target.path)?;
    if metadata.is_dir() {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.add_directory(zip_archive_entry_name(archive_path, true)?, options)?;
        for entry in fs::read_dir(&target.path)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            let child_uri = format!("{}/{}", target.uri, name);
            let child_target = library_target(data_dir, principal_id, &child_uri)?;
            let child_archive_path = archive_path.join(safe_archive_name(&name));
            append_library_zip_entry(
                writer,
                data_dir,
                principal_id,
                &child_target,
                &child_archive_path,
            )?;
        }
        return Ok(());
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)?;
    let options = zip_file_options_for_entry(archive_path, bytes.len());
    writer.start_file(zip_archive_entry_name(archive_path, false)?, options)?;
    writer.write_all(&bytes)?;
    Ok(())
}

fn zip_file_options_for_entry(path: &Path, size: usize) -> zip::write::SimpleFileOptions {
    let method = if should_store_zip_entry(path, size) {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    };
    zip::write::SimpleFileOptions::default().compression_method(method)
}

fn should_store_zip_entry(path: &Path, size: usize) -> bool {
    if size < 1024 {
        return true;
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "zip"
            | "gz"
            | "tgz"
            | "7z"
            | "rar"
            | "xz"
            | "bz2"
            | "zst"
            | "lz4"
            | "mp4"
            | "mov"
            | "mkv"
            | "webm"
            | "mp3"
            | "aac"
            | "ogg"
            | "flac"
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "avif"
            | "pdf"
    )
}

fn zip_archive_entry_name(path: &Path, directory: bool) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                let name = name
                    .to_str()
                    .ok_or_else(|| anyhow!("library ZIP archive entry path must be UTF-8"))?;
                if !name.is_empty() {
                    parts.push(name);
                }
            }
            Component::CurDir => {}
            _ => bail!("library ZIP archive entry path must be relative and safe"),
        }
    }
    let mut name = parts.join("/");
    if name.is_empty() {
        bail!("library ZIP archive entry path must not be empty");
    }
    if directory && !name.ends_with('/') {
        name.push('/');
    }
    Ok(name)
}

fn safe_archive_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            _ => ch,
        })
        .collect();
    let sanitized = sanitized.trim().trim_matches('.').to_string();
    if sanitized.is_empty() {
        "Library".to_string()
    } else {
        sanitized
    }
}

fn is_runtime_private_uri(localhost_root: &str, uri: &str) -> bool {
    uri == format!("{localhost_root}/.AppData")
        || uri
            .strip_prefix(&format!("{localhost_root}/.AppData/"))
            .is_some()
        || is_trash_uri(localhost_root, uri)
}

pub(crate) fn principal_root_protected_object_inventory(
    localhost_root: &str,
) -> Vec<crate::auth::PrincipalRootProtectedObjectDeclarationV1> {
    [
        "Desktop",
        "Documents",
        "Pictures",
        "Videos",
        "Downloads",
        "Public",
        ".Trash",
        ".AppData/LocalHost/.Runtime/Library",
    ]
    .into_iter()
    .map(|relative| {
        crate::auth::PrincipalRootProtectedObjectDeclarationV1::root(format!(
            "{localhost_root}/{relative}"
        ))
    })
    .collect()
}

fn record_is_published(record: &LibraryPublishRecord) -> bool {
    record.unpublished_at.is_none()
        && record
            .availability
            .get("status")
            .and_then(Value::as_str)
            .map(|status| status != "local_unpinned")
            .unwrap_or(true)
}

fn default_publish_content_security() -> Value {
    json!({
        "schema": "elastos.library.published-content-security/v1",
        "source_storage": "unknown",
        "published_payload": "plain_content",
        "key_release_required": false,
        "status": "not_required_for_plain_published_content",
        "required_providers": protected_content_provider_requirements(false),
    })
}

fn default_share_key_release() -> Value {
    json!({
        "schema": "elastos.library.key-release/v1",
        "required": false,
        "status": "not_required_for_plain_published_content",
        "required_providers": protected_content_provider_requirements(false),
        "next": "Protected encrypted-content sharing requires drm/rights/key/decrypt providers before content is opened."
    })
}

fn protected_content_provider_requirements(required: bool) -> Value {
    let status = if required {
        "required_for_encrypted_recipient_payload"
    } else {
        "not_required_for_plain_published_content"
    };
    json!({
        "schema": "elastos.library.protected-content-provider-requirements/v1",
        "required": required,
        "status": status,
        "providers": [
            {
                "id": "drm-provider",
                "scheme": "drm",
                "role": "protected-content open orchestration",
                "operation": "open",
                "required": required
            },
            {
                "id": "rights-provider",
                "scheme": "rights",
                "role": "recipient rights/ACL decision",
                "operation": "has_access_by_content_id",
                "required": required
            },
            {
                "id": "key-provider",
                "scheme": "key",
                "role": "recipient-scoped key release",
                "operation": "release",
                "required": required
            },
            {
                "id": "decrypt-provider",
                "scheme": "decrypt",
                "role": "viewer-scoped decrypt/render session",
                "operation": "open_session",
                "required": required
            }
        ],
        "authority_boundary": "Library records grants; drm/rights/key/decrypt providers enforce protected-content access without exposing raw CEKs or broad plaintext authority."
    })
}

async fn attach_protected_content_provider_status(registry: &ProviderRegistry, data: &mut Value) {
    let status = protected_content_provider_status(registry).await;
    if let Some(object) = data.as_object_mut() {
        object.insert("protected_content".to_string(), status.clone());
        if let Some(published) = object.get_mut("published").and_then(Value::as_object_mut) {
            published.insert("protected_content".to_string(), status);
        }
    }
}

async fn protected_content_provider_status(registry: &ProviderRegistry) -> Value {
    let providers = [
        protected_content_provider_runtime_status(registry, "drm-provider", "drm", "open").await,
        protected_content_provider_runtime_status(
            registry,
            "rights-provider",
            "rights",
            "has_access_by_content_id",
        )
        .await,
        protected_content_provider_runtime_status(registry, "key-provider", "key", "release").await,
        protected_content_provider_runtime_status(
            registry,
            "decrypt-provider",
            "decrypt",
            "open_session",
        )
        .await,
    ];
    let available_count = providers
        .iter()
        .filter(|provider| {
            provider
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let configured_count = providers
        .iter()
        .filter(|provider| {
            provider
                .get("configured")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .count();
    let provider_chain_ready = configured_count == providers.len();
    json!({
        "schema": "elastos.library.protected-content-provider-status/v1",
        "authority_boundary": "Apps receive provider readiness and receipts only; drm/rights/key/decrypt providers retain protected-content open, dDRM, key-release, and decrypt authority.",
        "available_provider_count": available_count,
        "configured_provider_count": configured_count,
        "required_provider_count": providers.len(),
        "providers": providers,
        "encrypted_recipient_sharing": {
            "schema": "elastos.library.encrypted-recipient-sharing-readiness/v1",
            "providers_ready": provider_chain_ready,
            "production_encrypted_publish_mode_ready": false,
            "status": if provider_chain_ready {
                "provider_chain_ready"
            } else {
                "blocked_until_drm_rights_key_decrypt_providers_configured"
            },
            "required_published_payload": "encrypted_recipient_content",
            "next": if provider_chain_ready {
                "Protected-content provider chain is configured. Runtime custody publish mode remains inactive until the protected publish path is activated."
            } else {
                "Configure drm/rights/key/decrypt providers before protected-content key release can proceed."
            }
        }
    })
}

async fn protected_content_provider_runtime_status(
    registry: &ProviderRegistry,
    id: &str,
    scheme: &str,
    primary_operation: &str,
) -> Value {
    if registry.get(scheme).await.is_none() {
        return json!({
            "id": id,
            "scheme": scheme,
            "primary_operation": primary_operation,
            "available": false,
            "configured": false,
            "status": "provider_not_registered",
            "next": format!("{id} must be installed and registered on the Runtime provider plane.")
        });
    }
    match registry.send_raw(scheme, &json!({ "op": "status" })).await {
        Ok(response) => {
            let data = response
                .get("data")
                .filter(|_| response.get("status").and_then(Value::as_str) == Some("ok"))
                .unwrap_or(&response);
            let configured = data
                .get("configured")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            json!({
                "id": id,
                "scheme": scheme,
                "primary_operation": primary_operation,
                "available": true,
                "configured": configured,
                "provider": data.get("provider").and_then(Value::as_str).unwrap_or(scheme),
                "version": data.get("version").cloned().unwrap_or(Value::Null),
                "contract_schema": data
                    .get("contract")
                    .and_then(|contract| contract.get("schema"))
                    .cloned()
                    .unwrap_or(Value::Null),
                "supported_operations": data
                    .get("supported_operations")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "blocked_authority": data
                    .get("blocked_authority")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
                "status": if configured {
                    "configured"
                } else {
                    "provider_registered_not_configured"
                },
                "next": if configured {
                    "Provider is configured; encrypted publish mode still controls whether Library requests key release."
                } else {
                    "Provider is installed but still fail-closed until its backend policy/key/decrypt configuration is complete."
                }
            })
        }
        Err(err) => json!({
            "id": id,
            "scheme": scheme,
            "primary_operation": primary_operation,
            "available": false,
            "configured": false,
            "status": "provider_status_unavailable",
            "error": err.to_string(),
            "next": format!("Inspect {id} registration and provider health.")
        }),
    }
}

fn published_content_security(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
) -> anyhow::Result<Value> {
    let source_storage = if crate::auth::load_principal_root_protection(
        data_dir,
        principal_id,
        &target.localhost_root,
    )?
    .is_some()
    {
        "protected_principal_root"
    } else {
        "plain_localhost_root"
    };
    Ok(json!({
        "schema": "elastos.library.published-content-security/v1",
        "object_uri": target.uri,
        "source_storage": source_storage,
        "published_payload": "plain_content",
        "key_release_required": false,
        "status": "not_required_for_plain_published_content",
        "required_providers": protected_content_provider_requirements(false),
        "next": "Publishing currently materializes a plain content payload through content-provider. Encrypted recipient payloads require drm/rights/key/decrypt providers and encrypted-content publish mode."
    }))
}

fn validate_runtime_custody_publish_input(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    protection: LibraryPublishProtectionRequest,
) -> anyhow::Result<LoadedRuntimeCustodyPublishInput> {
    let LibraryPublishProtectionRequest::RuntimeCustody { mime_type, codecs } = protection;
    validate_runtime_custody_media_declaration(&mime_type, "mime_type")?;
    validate_runtime_custody_media_declaration(&codecs, "codecs")?;

    let target_metadata = fs::symlink_metadata(&target.path)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
    if target_metadata.file_type().is_symlink() || !target_metadata.is_dir() {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }

    let mut saw_init = false;
    let mut saw_segments = false;
    for entry in fs::read_dir(&target.path)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?
    {
        let entry = entry.map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        if metadata.file_type().is_symlink() {
            bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
        }
        match name.as_str() {
            "init.mp4" if metadata.is_file() => saw_init = true,
            "segments" if metadata.is_dir() => saw_segments = true,
            _ => bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE),
        }
    }
    if !saw_init || !saw_segments {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }

    let init_target = library_target(data_dir, principal_id, &format!("{}/init.mp4", target.uri))?;
    let clear_init =
        read_runtime_custody_publish_part(data_dir, principal_id, &init_target, false)?;
    let session = ValidatedClearFmp4MediaSessionLayoutV1::new(&clear_init)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;

    let segments_target =
        library_target(data_dir, principal_id, &format!("{}/segments", target.uri))?;
    let segments_metadata = fs::symlink_metadata(&segments_target.path)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
    if segments_metadata.file_type().is_symlink() || !segments_metadata.is_dir() {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }

    let mut segment_bytes = BTreeMap::new();
    for entry in fs::read_dir(&segments_target.path)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?
    {
        let entry = entry.map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
        }
        let index = parse_runtime_custody_segment_name(&name)?;
        let segment_target = library_target(
            data_dir,
            principal_id,
            &format!("{}/segments/{name}", target.uri),
        )?;
        let clear_segment =
            read_runtime_custody_publish_part(data_dir, principal_id, &segment_target, true)?;
        session
            .validate_segment(&clear_segment)
            .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
        if segment_bytes.insert(index, clear_segment).is_some() {
            bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
        }
    }
    if segment_bytes.is_empty() || segment_bytes.len() > MAX_PROTECT_MEDIA_SEGMENTS_V1 as usize {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    let mut clear_segments = Vec::with_capacity(segment_bytes.len());
    for (expected, (actual, bytes)) in segment_bytes.into_iter().enumerate() {
        if actual != expected as u32 {
            bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
        }
        clear_segments.push(bytes);
    }
    Ok(LoadedRuntimeCustodyPublishInput {
        mime_type,
        codecs,
        clear_init_segment: clear_init,
        clear_segments,
    })
}

fn validate_runtime_custody_media_declaration(
    value: &str,
    field: &'static str,
) -> anyhow::Result<()> {
    let valid = !value.is_empty()
        && value.len() <= MAX_LIBRARY_PROTECTED_MEDIA_DECLARATION_BYTES
        && value
            .as_bytes()
            .iter()
            .all(|byte| matches!(*byte, 0x21..=0x7e));
    if !valid {
        bail!("invalid protected publish {field}");
    }
    Ok(())
}

fn read_runtime_custody_publish_part(
    data_dir: &Path,
    principal_id: &str,
    target: &LibraryTarget,
    require_non_empty: bool,
) -> anyhow::Result<Vec<u8>> {
    let metadata = fs::metadata(&target.path)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
    if metadata.len() > MAX_PROTECT_MEDIA_PART_BYTES_V1 as u64
        || (require_non_empty && metadata.len() == 0)
    {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    let bytes = read_library_file_bytes(data_dir, principal_id, target)
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
    if bytes.len() > MAX_PROTECT_MEDIA_PART_BYTES_V1 || (require_non_empty && bytes.is_empty()) {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    Ok(bytes)
}

fn parse_runtime_custody_segment_name(name: &str) -> anyhow::Result<u32> {
    if name.len() != "00000000.m4s".len() || !name.ends_with(".m4s") {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    let digits = &name[..8];
    if !digits.as_bytes().iter().all(u8::is_ascii_digit) {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    let index = digits
        .parse::<u32>()
        .map_err(|_| anyhow!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE))?;
    if index >= MAX_PROTECT_MEDIA_SEGMENTS_V1 {
        bail!(RUNTIME_CUSTODY_PUBLISH_INPUT_INVALID_MESSAGE);
    }
    Ok(index)
}

fn protected_content_sealed_object_from_security(
    content_security: &Value,
) -> anyhow::Result<SealedObjectV1> {
    let sealed = content_security
        .get("sealed_object")
        .cloned()
        .ok_or_else(|| anyhow!("protected content security missing sealed_object"))?;
    serde_json::from_value(sealed).context("protected content sealed_object is invalid")
}

fn normalized_key_release_policy(
    policy: Option<&str>,
    content_security: &Value,
) -> anyhow::Result<Value> {
    let policy = policy
        .map(str::trim)
        .filter(|policy| !policy.is_empty())
        .unwrap_or("auto");
    let content_requires_key_release = content_security
        .get("key_release_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match policy {
        "auto" if content_requires_key_release => {
            protected_content_key_release_policy(content_security)
        }
        "auto" | "none" | "plain_published_content" => Ok(json!({
            "schema": "elastos.library.key-release/v1",
            "required": false,
            "status": content_security
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("not_required_for_plain_published_content"),
            "published_payload": content_security
                .get("published_payload")
                .and_then(Value::as_str)
                .unwrap_or("plain_content"),
            "source_storage": content_security
                .get("source_storage")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            "required_providers": protected_content_provider_requirements(false),
            "next": "No recipient key release is needed for the current plain published payload. Encrypted recipient payloads require drm/rights/key/decrypt providers."
        })),
        "recipient_key_release" | "protected_content" | "encrypted_recipient"
            if content_requires_key_release =>
        {
            protected_content_key_release_policy(content_security)
        }
        "recipient_key_release" | "protected_content" | "encrypted_recipient" => bail!(
            "recipient key release requires drm/rights/key/decrypt providers and encrypted-content publish mode"
        ),
        _ => bail!("unsupported library key_release_policy"),
    }
}

fn protected_content_key_release_policy(content_security: &Value) -> anyhow::Result<Value> {
    let sealed_object = protected_content_sealed_object_from_security(content_security)?;
    Ok(json!({
        "schema": "elastos.library.key-release/v1",
        "required": true,
        "status": "provider_receipt_chain_required",
        "published_payload": content_security
            .get("published_payload")
            .and_then(Value::as_str)
            .unwrap_or("protected_content"),
        "payload_cid": sealed_object.payload_cid,
        "sealed_cid": content_security
            .get("sealed_cid")
            .cloned()
            .unwrap_or(Value::Null),
        "viewer_interface": sealed_object.viewer.required_interface,
        "key_envelope": sealed_object.key_envelope,
        "required_providers": protected_content_provider_requirements(true),
        "authority_boundary": "Library records the recipient grant; drm/rights/key/decrypt providers must issue receipts before a viewer opens protected content.",
        "next": "Runtime shared_access must invoke drm, rights, key, and decrypt providers before returning a protected viewer session."
    }))
}

fn record_availability_label(record: Option<&LibraryPublishRecord>) -> String {
    let Some(record) = record else {
        return "local-only".to_string();
    };
    record
        .availability
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or(if record_is_published(record) {
            "published"
        } else {
            "local_unpinned"
        })
        .to_string()
}

fn normalized_share_recipients(recipients: &[String]) -> anyhow::Result<Vec<String>> {
    let mut normalized = BTreeSet::new();
    for recipient in recipients {
        let recipient = recipient.trim();
        if recipient.is_empty() {
            continue;
        }
        if recipient.len() > 256 || recipient.chars().any(char::is_control) {
            bail!("library share recipient is invalid");
        }
        if !(recipient.starts_with("did:")
            || recipient.starts_with("person:")
            || recipient.starts_with("principal:")
            || recipient.contains('@'))
        {
            bail!("library share recipient must be a DID, principal/person id, or address");
        }
        normalized.insert(recipient.to_string());
    }
    Ok(normalized.into_iter().collect())
}

fn normalized_share_policy(policy: Option<&str>, public_link: bool) -> anyhow::Result<String> {
    let default_policy = if public_link {
        "public_link"
    } else {
        "recipient_scoped"
    };
    let policy = policy
        .map(str::trim)
        .filter(|policy| !policy.is_empty())
        .unwrap_or(default_policy);
    match policy {
        "public_link" if public_link => Ok(policy.to_string()),
        "recipient_scoped" if !public_link => Ok(policy.to_string()),
        "public_link" => bail!("public_link share policy must not include recipients"),
        "recipient_scoped" => bail!("recipient_scoped share policy requires recipients"),
        _ => bail!("unsupported library share policy"),
    }
}

fn share_remote_enforcement_contract(policy: &str, key_release: &Value) -> Value {
    let key_release_required = key_release
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let recipient_proof_required = policy == "recipient_scoped";
    json!({
        "schema": "elastos.library.remote-access-policy/v1",
        "policy": policy,
        "provider_gate": "object-provider shared_access",
        "recipient_proof_required": recipient_proof_required,
        "key_release_required": key_release_required,
        "key_release_status": key_release
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        "required_providers": protected_content_provider_requirements(key_release_required),
        "provider_invocation": {
            "drm": "drm-provider.open",
            "rights": "rights-provider.has_access_by_content_id",
            "key": "key-provider.release",
            "decrypt": "decrypt-provider.open_session",
            "transport": "Carrier provider invocation when encrypted payloads are enabled"
        },
        "plain_content_fetch": !key_release_required,
        "status": if key_release_required {
            "blocked_until_drm_rights_key_decrypt_providers"
        } else if recipient_proof_required {
            "recipient_proof_enforced_by_runtime"
        } else {
            "public_link_ready"
        },
        "next": if key_release_required {
            "Attach drm/rights/key/decrypt providers before releasing encrypted payload keys."
        } else if recipient_proof_required {
            "Remote recipients must present a Runtime recipient proof before object-provider returns the shared open contract."
        } else {
            "Published plain content is available to holders of the content URI."
        }
    })
}

fn share_grant(
    recipient: &str,
    cid: &str,
    uri: &str,
    policy: &str,
    key_release: Value,
    created_at: u64,
) -> LibraryShareGrant {
    let digest = Sha256::digest(format!("{recipient}:{cid}:{policy}:{created_at}"));
    LibraryShareGrant {
        schema: "elastos.library.share-grant/v1".to_string(),
        grant_id: format!("share:{}", hex::encode(&digest[..16])),
        recipient: recipient.to_string(),
        uri: uri.to_string(),
        cid: cid.to_string(),
        policy: policy.to_string(),
        key_release,
        created_at,
    }
}

fn shared_access_receipt(
    record: &LibraryPublishRecord,
    recipient: &str,
    recipient_proof: Option<&Value>,
) -> anyhow::Result<Value> {
    let policy = record.share_policy.as_deref().unwrap_or("public_link");
    match policy {
        "public_link" => Ok(json!({
            "schema": "elastos.library.shared-access.receipt/v1",
            "policy": "public_link",
            "recipient": recipient.trim(),
            "grant_id": null,
            "content_security": record.content_security.clone(),
            "key_release": default_share_key_release(),
            "recipient_proof": shared_access_recipient_proof_state(recipient.trim(), false, "not_required_for_public_link", None, None),
            "decision": shared_access_decision("public_link", recipient.trim(), None, true, "public_link"),
            "open": shared_access_open_contract(record, "public_link", recipient.trim(), None, &default_share_key_release(), false)
        })),
        "recipient_scoped" => {
            let normalized = normalized_share_recipients(&[recipient.to_string()])?;
            let recipient = normalized
                .first()
                .ok_or_else(|| anyhow!("library shared_access recipient is required"))?;
            let grant = record
                .share_grants
                .iter()
                .find(|grant| grant.recipient == *recipient && grant.policy == "recipient_scoped")
                .ok_or_else(|| anyhow!("library shared_access recipient is not authorized"))?;
            let proof_state = validate_shared_access_recipient_proof(recipient, recipient_proof)?;
            Ok(json!({
                "schema": "elastos.library.shared-access.receipt/v1",
                "policy": "recipient_scoped",
                "recipient": recipient,
                "grant_id": grant.grant_id,
                "content_security": record.content_security.clone(),
                "key_release": grant.key_release.clone(),
                "recipient_proof": proof_state,
                "decision": shared_access_decision("recipient_scoped", recipient, Some(&grant.grant_id), true, "recipient grant and Runtime recipient proof matched"),
                "open": shared_access_open_contract(record, "recipient_scoped", recipient, Some(&grant.grant_id), &grant.key_release, true)
            }))
        }
        other => bail!("unsupported library share policy: {other}"),
    }
}

fn validate_shared_access_recipient_proof(
    recipient: &str,
    proof: Option<&Value>,
) -> anyhow::Result<Value> {
    let proof = proof.ok_or_else(|| {
        anyhow!(
            "library shared_access requires Runtime recipient_proof for recipient_scoped policy"
        )
    })?;
    let schema = proof
        .get("schema")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("library shared_access recipient_proof requires schema"))?;
    if schema != "elastos.library.recipient-proof/v1" {
        bail!("library shared_access recipient_proof schema is unsupported");
    }
    let source = proof
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if source != "runtime-launch-grant" {
        bail!("library shared_access recipient_proof source is unsupported");
    }
    let proof_binding_id = proof
        .get("proof_binding_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!("library shared_access recipient_proof requires proof_binding_id")
        })?;
    if !proof_binding_id.starts_with("proof:passkey:") {
        bail!("library shared_access recipient_proof requires passkey proof binding");
    }
    let claimed = proof
        .get("recipient")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("library shared_access recipient_proof requires recipient"))?;
    let normalized_requested = normalized_share_recipients(&[recipient.to_string()])?;
    let normalized_claimed = normalized_share_recipients(&[claimed.to_string()])?;
    if normalized_requested.first() != normalized_claimed.first() {
        bail!("library shared_access recipient_proof recipient mismatch");
    }
    let session_id = proof
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    Ok(shared_access_recipient_proof_state(
        normalized_requested
            .first()
            .map(String::as_str)
            .unwrap_or(recipient),
        true,
        source,
        Some(proof_binding_id),
        session_id,
    ))
}

fn shared_access_recipient_proof_state(
    recipient: &str,
    verified: bool,
    source: &str,
    proof_binding_id: Option<&str>,
    session_id: Option<&str>,
) -> Value {
    json!({
        "schema": "elastos.library.recipient-proof-state/v1",
        "recipient": recipient,
        "verified": verified,
        "source": source,
        "proof_binding_id": proof_binding_id,
        "session_id": session_id,
    })
}

fn shared_access_decision(
    policy: &str,
    recipient: &str,
    grant_id: Option<&str>,
    allowed: bool,
    reason: &str,
) -> Value {
    json!({
        "schema": "elastos.library.access-decision/v1",
        "policy": policy,
        "recipient": recipient,
        "grant_id": grant_id,
        "allowed": allowed,
        "reason": reason,
    })
}

fn shared_access_open_contract(
    record: &LibraryPublishRecord,
    policy: &str,
    recipient: &str,
    grant_id: Option<&str>,
    key_release: &Value,
    recipient_proof_verified: bool,
) -> Value {
    let key_release_required = key_release
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    json!({
        "schema": "elastos.library.shared-open/v1",
        "uri": format!("elastos://{}", record.cid),
        "cid": record.cid,
        "policy": policy,
        "recipient": recipient,
        "grant_id": grant_id,
        "provider": "content-provider",
        "transport": "runtime-provider-fetch",
        "published_payload": record
            .content_security
            .get("published_payload")
            .and_then(Value::as_str)
            .unwrap_or("plain_content"),
        "recipient_proof_verified": recipient_proof_verified,
        "key_release_required": key_release_required,
        "drm_provider_required": key_release_required,
        "rights_provider_required": key_release_required,
        "key_provider_required": key_release_required,
        "decrypt_provider_required": key_release_required,
        "required_providers": protected_content_provider_requirements(key_release_required),
        "status": if key_release_required {
            "blocked_until_drm_rights_key_decrypt_providers"
        } else {
            "ready_for_plain_content_fetch"
        },
        "remote_enforcement": share_remote_enforcement_contract(policy, key_release),
    })
}

fn library_roots(data_dir: &Path, principal_id: &str) -> Vec<LibraryRoot> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let mut roots: Vec<_> = [
        ("home", "Home", root.clone(), "principal-root"),
        ("desktop", "Desktop", format!("{root}/Desktop"), "directory"),
        (
            "documents",
            "Documents",
            format!("{root}/Documents"),
            "directory",
        ),
        (
            "pictures",
            "Pictures",
            format!("{root}/Pictures"),
            "directory",
        ),
        ("videos", "Videos", format!("{root}/Videos"), "directory"),
        (
            "downloads",
            "Downloads",
            format!("{root}/Downloads"),
            "directory",
        ),
        ("public", "Public", format!("{root}/Public"), "directory"),
        (
            "webspaces",
            "Spaces",
            "localhost://WebSpaces".to_string(),
            "webspace-root",
        ),
    ]
    .into_iter()
    .map(|(id, label, uri, kind)| LibraryRoot {
        schema: LIBRARY_ROOT_SCHEMA,
        id,
        label,
        uri,
        kind,
        metadata: None,
    })
    .collect();
    let trash_uri = format!("{root}/.Trash");
    let (empty, item_count) = trash_root_state(data_dir, principal_id, &trash_uri);
    roots.push(LibraryRoot {
        schema: LIBRARY_ROOT_SCHEMA,
        id: "trash",
        label: "Trash",
        uri: trash_uri,
        kind: "directory",
        metadata: Some(json!({
            "schema": "elastos.library.trash-root/v1",
            "empty": empty,
            "item_count": item_count,
        })),
    });
    roots
}

fn trash_root_state(data_dir: &Path, principal_id: &str, trash_uri: &str) -> (bool, usize) {
    let Ok(target) = library_target(data_dir, principal_id, trash_uri) else {
        return (true, 0);
    };
    let Ok(entries) = fs::read_dir(&target.path) else {
        return (true, 0);
    };
    let count = entries.filter_map(Result::ok).count();
    (count == 0, count)
}

fn move_library_object(
    data_dir: &Path,
    principal_id: &str,
    from_uri: &str,
    to_uri: &str,
) -> anyhow::Result<()> {
    let from = library_target(data_dir, principal_id, from_uri)?;
    let to = library_target(data_dir, principal_id, to_uri)?;
    if !from.path.exists() {
        bail!("library source object not found");
    }
    if to.path.exists() {
        bail!("library destination already exists");
    }
    if from.path.is_dir() {
        move_library_directory(data_dir, principal_id, &from, &to)?;
    } else {
        move_library_file(data_dir, principal_id, &from, &to)?;
    }
    Ok(())
}

fn copy_library_object(
    data_dir: &Path,
    principal_id: &str,
    from_uri: &str,
    to_uri: &str,
) -> anyhow::Result<()> {
    let from = library_target(data_dir, principal_id, from_uri)?;
    let to = library_target(data_dir, principal_id, to_uri)?;
    if !from.path.exists() {
        bail!("library source object not found");
    }
    if to.path.exists() {
        bail!("library destination already exists");
    }
    if from.path.is_dir() {
        copy_library_directory(data_dir, principal_id, &from, &to)?;
    } else {
        copy_library_file(data_dir, principal_id, &from, &to)?;
    }
    Ok(())
}

fn copy_library_directory(
    data_dir: &Path,
    principal_id: &str,
    from: &LibraryTarget,
    to: &LibraryTarget,
) -> anyhow::Result<()> {
    fs::create_dir_all(&to.path)?;
    for entry in fs::read_dir(&from.path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child_from_uri = format!("{}/{}", from.uri, name);
        let child_to_uri = format!("{}/{}", to.uri, name);
        let child_from = library_target(data_dir, principal_id, &child_from_uri)?;
        let child_to = library_target(data_dir, principal_id, &child_to_uri)?;
        if child_from.path.is_dir() {
            copy_library_directory(data_dir, principal_id, &child_from, &child_to)?;
        } else {
            copy_library_file(data_dir, principal_id, &child_from, &child_to)?;
        }
    }
    Ok(())
}

fn copy_library_file(
    data_dir: &Path,
    principal_id: &str,
    from: &LibraryTarget,
    to: &LibraryTarget,
) -> anyhow::Result<()> {
    let bytes = read_library_file_bytes(data_dir, principal_id, from)?;
    if let Some(parent) = to.path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &to.localhost_root,
        &to.uri,
        &to.path,
        &bytes,
    )?;
    Ok(())
}

fn move_library_directory(
    data_dir: &Path,
    principal_id: &str,
    from: &LibraryTarget,
    to: &LibraryTarget,
) -> anyhow::Result<()> {
    fs::create_dir_all(&to.path)?;
    for entry in fs::read_dir(&from.path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child_from_uri = format!("{}/{}", from.uri, name);
        let child_to_uri = format!("{}/{}", to.uri, name);
        let child_from = library_target(data_dir, principal_id, &child_from_uri)?;
        let child_to = library_target(data_dir, principal_id, &child_to_uri)?;
        if child_from.path.is_dir() {
            move_library_directory(data_dir, principal_id, &child_from, &child_to)?;
        } else {
            move_library_file(data_dir, principal_id, &child_from, &child_to)?;
        }
    }
    fs::remove_dir(&from.path)?;
    Ok(())
}

fn move_library_file(
    data_dir: &Path,
    principal_id: &str,
    from: &LibraryTarget,
    to: &LibraryTarget,
) -> anyhow::Result<()> {
    let bytes = read_library_file_bytes(data_dir, principal_id, from)?;
    if let Some(parent) = to.path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &to.localhost_root,
        &to.uri,
        &to.path,
        &bytes,
    )?;
    fs::remove_file(&from.path)?;
    Ok(())
}

fn check_revision(
    data_dir: &Path,
    principal_id: &str,
    uri: &str,
    expected: Option<&str>,
) -> anyhow::Result<()> {
    let Some(expected) = expected.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    let object = library_object(data_dir, principal_id, uri)?;
    if object.revision != expected {
        bail!("library object revision precondition failed");
    }
    Ok(())
}

fn child_uri(parent_uri: &str, name: &str) -> anyhow::Result<String> {
    let name = clean_object_name(name)?;
    Ok(format!("{}/{}", parent_uri.trim_end_matches('/'), name))
}

fn unique_child_uri(
    data_dir: &Path,
    principal_id: &str,
    parent_uri: &str,
    name: &str,
) -> anyhow::Result<String> {
    let mut candidate = child_uri(parent_uri, name)?;
    if !library_target(data_dir, principal_id, &candidate)?
        .path
        .exists()
    {
        return Ok(candidate);
    }
    let timestamp = now_ts();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    for index in 0..1_000 {
        let suffix = if index == 0 {
            timestamp.to_string()
        } else {
            format!("{timestamp}-{index}")
        };
        let fallback = if ext.is_empty() {
            format!("{stem} ({suffix})")
        } else {
            format!("{stem} ({suffix}).{ext}")
        };
        candidate = child_uri(parent_uri, &fallback)?;
        if !library_target(data_dir, principal_id, &candidate)?
            .path
            .exists()
        {
            return Ok(candidate);
        }
    }
    bail!("failed to allocate unique Library object name")
}

fn clean_object_name(name: &str) -> anyhow::Result<&str> {
    let name = name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name == "."
        || name == ".."
    {
        bail!("invalid library object name");
    }
    Ok(name)
}

fn library_uri_parent(uri: &str) -> anyhow::Result<&str> {
    uri.rsplit_once('/')
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
        .ok_or_else(|| anyhow!("library object URI has no parent"))
}

fn is_trash_uri(localhost_root: &str, uri: &str) -> bool {
    is_trash_root_uri(localhost_root, uri) || is_trash_child_uri(localhost_root, uri)
}

fn is_trash_root_uri(localhost_root: &str, uri: &str) -> bool {
    uri == format!("{localhost_root}/.Trash")
}

fn is_trash_child_uri(localhost_root: &str, uri: &str) -> bool {
    uri.strip_prefix(&format!("{localhost_root}/.Trash/"))
        .is_some_and(|rest| !rest.is_empty())
}

fn restore_uri_from_trash_record(
    data_dir: &Path,
    principal_id: &str,
    trash_target: &LibraryTarget,
    trash_record: Option<&LibraryTrashRecord>,
) -> anyhow::Result<String> {
    let record =
        trash_record.ok_or_else(|| anyhow!("library Trash restore metadata is missing"))?;
    let original_uri = clean_library_uri(&trash_target.localhost_root, &record.original_uri)?;
    let parent_uri = library_uri_parent(&original_uri)?;
    let name = original_uri
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(record.original_name.as_str());
    unique_child_uri(data_dir, principal_id, parent_uri, name)
}

fn trash_record_uri(localhost_root: &str, trash_uri: &str) -> String {
    let digest = hex::encode(Sha256::digest(trash_uri.as_bytes()));
    format!("{localhost_root}/.AppData/LocalHost/.Runtime/Library/Trash/{digest}.json")
}

fn read_trash_record(
    data_dir: &Path,
    principal_id: &str,
    trash_uri: &str,
) -> anyhow::Result<LibraryTrashRecord> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let record_uri = trash_record_uri(&root, trash_uri);
    let record_path = rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or_else(|| anyhow!("invalid library Trash record path"))?;
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &record_uri,
        &record_path,
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_trash_record(
    data_dir: &Path,
    principal_id: &str,
    record: &LibraryTrashRecord,
) -> anyhow::Result<()> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let record_uri = trash_record_uri(&root, &record.trash_uri);
    let record_path = rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or_else(|| anyhow!("invalid library Trash record path"))?;
    if let Some(parent) = record_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(record)?;
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &record_uri,
        &record_path,
        &bytes,
    )
}

fn remove_trash_record(data_dir: &Path, principal_id: &str, trash_uri: &str) -> anyhow::Result<()> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let record_uri = trash_record_uri(&root, trash_uri);
    let record_path = rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or_else(|| anyhow!("invalid library Trash record path"))?;
    match fs::remove_file(record_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn publish_record_uri(localhost_root: &str, object_uri: &str) -> String {
    let digest = hex::encode(Sha256::digest(object_uri.as_bytes()));
    format!("{localhost_root}/.AppData/LocalHost/.Runtime/Library/Published/{digest}.json")
}

fn read_publish_record(
    data_dir: &Path,
    principal_id: &str,
    object_uri: &str,
) -> anyhow::Result<LibraryPublishRecord> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let record_uri = publish_record_uri(&root, object_uri);
    let record_path = rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or_else(|| anyhow!("invalid library publish record path"))?;
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &record_uri,
        &record_path,
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_publish_record(
    data_dir: &Path,
    principal_id: &str,
    record: &LibraryPublishRecord,
) -> anyhow::Result<()> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let record_uri = publish_record_uri(&root, &record.object_uri);
    let record_path = rooted_localhost_fs_path(data_dir, &record_uri)
        .ok_or_else(|| anyhow!("invalid library publish record path"))?;
    if let Some(parent) = record_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(record)?;
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &record_uri,
        &record_path,
        &bytes,
    )
}

fn library_events(
    data_dir: &Path,
    principal_id: &str,
    uri_filter: Option<&str>,
    since: Option<u64>,
    limit: Option<usize>,
) -> anyhow::Result<Vec<LibraryEvent>> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let uri_filter = uri_filter
        .map(|uri| clean_library_uri(&root, uri))
        .transpose()?;
    let limit = limit.unwrap_or(64).clamp(1, MAX_LIBRARY_EVENTS);
    let mut events = read_library_events(data_dir, principal_id)?;
    events.retain(|event| {
        since.map(|since| event.at > since).unwrap_or(true)
            && uri_filter
                .as_deref()
                .map(|uri| library_event_matches_uri(event, uri))
                .unwrap_or(true)
    });
    if events.len() > limit {
        let keep_from = events.len() - limit;
        events.drain(0..keep_from);
    }
    Ok(events)
}

fn append_library_event(
    data_dir: &Path,
    principal_id: &str,
    op: &str,
    uri: &str,
    details: Value,
) -> anyhow::Result<LibraryEvent> {
    let mut events = read_library_events(data_dir, principal_id)?;
    let event = LibraryEvent {
        schema: LIBRARY_EVENT_SCHEMA.to_string(),
        event_id: library_event_id(op, uri, &details),
        op: op.to_string(),
        uri: uri.to_string(),
        at: now_ts(),
        details,
    };
    events.push(event.clone());
    if events.len() > MAX_LIBRARY_EVENTS {
        let keep_from = events.len() - MAX_LIBRARY_EVENTS;
        events.drain(0..keep_from);
    }
    write_library_events(data_dir, principal_id, &events)?;
    library_event_notifier().notify_waiters();
    Ok(event)
}

fn read_library_events(data_dir: &Path, principal_id: &str) -> anyhow::Result<Vec<LibraryEvent>> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let event_uri = library_event_log_uri(&root);
    let event_path = rooted_localhost_fs_path(data_dir, &event_uri)
        .ok_or_else(|| anyhow!("invalid library event log path"))?;
    if !event_path.exists() {
        return Ok(Vec::new());
    }
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &event_uri,
        &event_path,
    )?;
    let text = String::from_utf8(bytes).context("library event log must be utf-8 jsonl")?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("invalid library event entry"))
        .collect()
}

fn write_library_events(
    data_dir: &Path,
    principal_id: &str,
    events: &[LibraryEvent],
) -> anyhow::Result<()> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let event_uri = library_event_log_uri(&root);
    let event_path = rooted_localhost_fs_path(data_dir, &event_uri)
        .ok_or_else(|| anyhow!("invalid library event log path"))?;
    if let Some(parent) = event_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::new();
    for event in events {
        serde_json::to_writer(&mut bytes, event)?;
        bytes.push(b'\n');
    }
    crate::auth::write_principal_root_object(
        data_dir,
        principal_id,
        &root,
        &event_uri,
        &event_path,
        &bytes,
    )
}

fn library_event_log_uri(localhost_root: &str) -> String {
    format!("{localhost_root}/.AppData/LocalHost/.Runtime/Library/events.jsonl")
}

fn library_event_id(op: &str, uri: &str, details: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(now_nanos().to_be_bytes());
    hasher.update(op.as_bytes());
    hasher.update(uri.as_bytes());
    hasher.update(details.to_string().as_bytes());
    let digest = hasher.finalize();
    format!("library:event:{}", hex::encode(&digest[..16]))
}

fn library_event_matches_uri(event: &LibraryEvent, uri: &str) -> bool {
    uri_matches_filter(&event.uri, uri)
        || [
            "old_uri",
            "original_uri",
            "source_uri",
            "trash_uri",
            "target_uri",
        ]
        .into_iter()
        .any(|field| {
            event
                .details
                .get(field)
                .and_then(Value::as_str)
                .map(|value| uri_matches_filter(value, uri))
                .unwrap_or(false)
        })
        || event
            .details
            .get("object")
            .and_then(|object| object.get("uri"))
            .and_then(Value::as_str)
            .map(|value| uri_matches_filter(value, uri))
            .unwrap_or(false)
}

fn uri_matches_filter(uri: &str, filter: &str) -> bool {
    uri == filter
        || uri
            .strip_prefix(filter.trim_end_matches('/'))
            .is_some_and(|rest| rest.starts_with('/'))
}

fn directory_revision(path: &Path, uri: &str) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(uri.as_bytes());
    if path.exists() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            hasher.update(entry.file_name().to_string_lossy().as_bytes());
            let metadata = entry.metadata()?;
            hasher.update(metadata.len().to_be_bytes());
            if let Some(modified) = system_time_secs(metadata.modified().ok()) {
                hasher.update(modified.to_be_bytes());
            }
        }
    }
    Ok(format!("rev:{}", hex::encode(hasher.finalize())))
}

fn system_time_secs(time: Option<SystemTime>) -> Option<u64> {
    time.and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn mime_for_name(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        "text/plain"
    } else if lower.ends_with(".html") {
        "text/html"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else if lower.ends_with(".tar") {
        "application/x-tar"
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        "application/gzip"
    } else if lower.ends_with(".zip") {
        "application/zip"
    } else if lower.ends_with(".mp4") {
        "video/mp4"
    } else if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else {
        "application/octet-stream"
    }
}

fn viewer_options_for_name(data_dir: &Path, name: &str) -> Vec<LibraryViewerOption> {
    viewer_ids_for_name(name)
        .into_iter()
        .filter_map(|id| installed_viewer_option(data_dir, id))
        .collect()
}

fn viewer_ids_for_name(name: &str) -> Vec<&'static str> {
    let lower = name.to_lowercase();
    if archive_family_for_name(&lower).is_some() {
        vec!["archive-manager"]
    } else if lower.ends_with(".md") || lower.ends_with(".txt") {
        vec!["documents"]
    } else if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".gif")
    {
        vec!["image-viewer"]
    } else if lower.ends_with(".mp4") {
        vec!["video-viewer"]
    } else if lower.ends_with(".pdf") {
        vec!["documents"]
    } else if lower.ends_with(".gba") {
        vec!["gba-emulator"]
    } else {
        Vec::new()
    }
}

fn installed_viewer_option(data_dir: &Path, id: &str) -> Option<LibraryViewerOption> {
    crate::api::browser_capsules::list_launchable_browser_capsules(data_dir)
        .into_iter()
        .find(|capsule| capsule.name == id && capsule.role == elastos_common::CapsuleRole::Viewer)
        .map(|capsule| LibraryViewerOption {
            id: capsule.name.clone(),
            label: viewer_label(&capsule.name).to_string(),
            description: capsule.description,
            default: true,
        })
}

fn viewer_label(id: &str) -> &str {
    match id {
        "documents" => "Documents",
        "image-viewer" => "Image Viewer",
        "video-viewer" => "Video Viewer",
        "gba-emulator" => "GBA Emulator",
        "archive-manager" => "Archive",
        _ => id,
    }
}

fn provider_ok(data: Value) -> Value {
    json!({
        "status": "ok",
        "data": data,
    })
}

fn provider_error(code: &str, message: &str) -> Value {
    json!({
        "status": "error",
        "code": code,
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_provider_exposes_object_scheme_only() {
        let registry = Arc::new(ProviderRegistry::new());
        let provider = ObjectProvider::new(PathBuf::new(), Arc::downgrade(&registry));
        let schemes = provider.schemes();

        assert!(schemes.contains(&"object"));
        assert!(!schemes.contains(&"library"));
        assert_eq!(provider.name(), "object-provider");
    }

    #[test]
    fn protected_content_provider_response_rejects_authority_fields() {
        let forbidden = [
            "raw_cek",
            "wallet_rpc",
            "chain_rpc",
            "kubo_api",
            "ipfs_api",
            "elacity_sdk",
            "elacity_sdk_token",
            "contract_sdk",
            "key_backend_sdk",
        ];
        for key in forbidden {
            let value = json!({
                "schema": "elastos.test/v1",
                "nested": [{ key: "must-not-cross-boundary" }]
            });
            assert!(
                reject_forbidden_protected_content_fields(&value).is_err(),
                "{key} must be rejected before app/viewer handoff"
            );
        }

        assert!(reject_forbidden_protected_content_fields(&json!({
            "schema": "elastos.decrypt.session/v1",
            "output": "viewer_capsule_session:fixture"
        }))
        .is_ok());
    }

    #[tokio::test]
    #[ignore = "requires ELASTOS_LIVE_IPFS_PROVIDER_BIN and ELASTOS_LIVE_IPFS_DATA_DIR"]
    async fn library_live_ipfs_publish_provider_route_smoke() {
        let Ok(ipfs_provider_bin) = std::env::var("ELASTOS_LIVE_IPFS_PROVIDER_BIN") else {
            eprintln!("skipping live Library publish smoke: ELASTOS_LIVE_IPFS_PROVIDER_BIN unset");
            return;
        };
        let Ok(ipfs_data_dir) = std::env::var("ELASTOS_LIVE_IPFS_DATA_DIR") else {
            eprintln!("skipping live Library publish smoke: ELASTOS_LIVE_IPFS_DATA_DIR unset");
            return;
        };
        let ipfs_provider_bin = PathBuf::from(ipfs_provider_bin);
        let ipfs_data_dir = PathBuf::from(ipfs_data_dir);
        assert!(
            ipfs_provider_bin.is_file(),
            "ipfs-provider missing: {}",
            ipfs_provider_bin.display()
        );
        assert!(
            ipfs_data_dir.is_dir(),
            "IPFS data dir missing: {}",
            ipfs_data_dir.display()
        );

        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let ipfs_config = elastos_runtime::provider::BridgeProviderConfig {
            base_path: ipfs_data_dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let bridge = Arc::new(
            elastos_runtime::provider::ProviderBridge::spawn(&ipfs_provider_bin, ipfs_config)
                .await
                .expect("spawn live ipfs-provider"),
        );
        let ipfs_provider: Arc<dyn Provider> = Arc::new(
            elastos_runtime::provider::CapsuleProvider::with_scheme(Arc::clone(&bridge), "ipfs"),
        );
        registry
            .register_sub_provider("ipfs", ipfs_provider)
            .await
            .unwrap();
        let content_provider = Arc::new(crate::content::ContentProvider::new(
            dir.path().to_path_buf(),
            Arc::downgrade(&registry),
        ));
        registry.register(content_provider.clone()).await;
        registry
            .register_sub_provider("content", content_provider)
            .await
            .unwrap();

        let principal_id = "did:key:z6MklibraryLiveIpfsSmoke";
        let roots = handle_object_provider_runtime_request(
            dir.path(),
            Arc::clone(&registry),
            &json!({
                "op": "roots",
                "principal_id": principal_id,
            }),
        )
        .await;
        assert_eq!(roots["status"], "ok", "{roots}");
        let public_uri = roots["data"]["roots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|root| root["id"] == "public")
            .and_then(|root| root["uri"].as_str())
            .expect("public Library root")
            .to_string();

        let body = format!("Library live IPFS provider route smoke {}\n", now_ts());
        let file_uri = format!("{public_uri}/library-live-ipfs-provider-smoke.txt");
        let write = handle_object_provider_runtime_request(
            dir.path(),
            Arc::clone(&registry),
            &json!({
                "op": "write",
                "principal_id": principal_id,
                "uri": file_uri,
                "mime": "text/plain",
                "data": base64::engine::general_purpose::STANDARD.encode(body.as_bytes()),
            }),
        )
        .await;
        assert_eq!(write["status"], "ok", "{write}");
        let revision = write["data"]["object"]["revision"]
            .as_str()
            .expect("write revision")
            .to_string();

        let publish = handle_object_provider_runtime_request(
            dir.path(),
            Arc::clone(&registry),
            &json!({
                "op": "publish",
                "principal_id": principal_id,
                "uri": file_uri,
                "if_revision": revision,
            }),
        )
        .await;
        assert_eq!(publish["status"], "ok", "{publish}");
        let cid = publish["data"]["cid"].as_str().expect("publish cid");
        assert!(!cid.trim().is_empty());
        assert_eq!(publish["data"]["uri"], format!("elastos://{cid}"));
        assert_eq!(publish["data"]["availability"]["status"], "local_pinned");
        assert_eq!(publish["data"]["object"]["published_cid"], cid);

        let status = handle_object_provider_runtime_request(
            dir.path(),
            Arc::clone(&registry),
            &json!({
                "op": "status",
                "principal_id": principal_id,
                "uri": file_uri,
            }),
        )
        .await;
        assert_eq!(status["status"], "ok", "{status}");
        assert_eq!(status["data"]["published"]["cid"], cid);

        bridge
            .shutdown()
            .await
            .expect("shutdown live ipfs-provider");
        println!("library live IPFS provider route cid={cid}");
    }
}
