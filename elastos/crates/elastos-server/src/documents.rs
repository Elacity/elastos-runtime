use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail};
use base64::Engine as _;
use elastos_common::localhost::rooted_localhost_fs_path;
use elastos_runtime::provider::{
    Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DOCUMENTS_SCHEMA: &str = "elastos.document/v1";
const DOCUMENTS_DEFAULT_TITLE: &str = "Untitled";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsListItem {
    pub doc_did: String,
    pub document_uri: String,
    pub title: String,
    pub file_name: String,
    pub working_copy_uri: String,
    pub updated_at: u64,
    #[serde(default)]
    pub latest_published_cid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsDocumentView {
    pub doc_did: String,
    pub document_uri: String,
    pub title: String,
    pub file_name: String,
    pub working_copy_uri: String,
    pub body: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub latest_published_cid: Option<String>,
    #[serde(default)]
    pub publish_history: Vec<DocumentsPublishRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentsCreateRequest {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentsSaveRequest {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocumentsSaveAsRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsPublishRecord {
    pub cid: String,
    pub published_at: u64,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsPublishExport {
    pub doc_did: String,
    pub title: String,
    pub file_name: String,
    pub owner_did: String,
    pub body: String,
    pub next_version: u64,
    #[serde(default)]
    pub latest_published_cid: Option<String>,
    #[serde(default)]
    pub latest_published_content_digest: Option<String>,
    #[serde(default)]
    pub latest_published_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsPublishResponse {
    pub uri: String,
    pub route: String,
    pub cid: String,
    pub published_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentsUnpublishResponse {
    pub uri: String,
    pub cid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DocumentsMetadata {
    pub schema: String,
    pub doc_did: String,
    pub owner_did: String,
    pub title: String,
    pub file_name: String,
    pub working_copy_uri: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub latest_published_cid: Option<String>,
    #[serde(default)]
    pub publish_history: Vec<DocumentsPublishRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum DocumentsProviderRequest {
    Summary,
    Create {
        #[serde(default)]
        title: Option<String>,
    },
    Get {
        doc_did: String,
    },
    Save {
        doc_did: String,
        title: String,
        body: String,
    },
    SaveAs {
        doc_did: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        file_name: Option<String>,
        body: String,
    },
    Delete {
        doc_did: String,
    },
    Publish {
        doc_did: String,
    },
    Unpublish {
        doc_did: String,
    },
}

pub struct DocumentsProvider {
    data_dir: PathBuf,
    registry: Weak<ProviderRegistry>,
}

impl DocumentsProvider {
    pub fn new(data_dir: PathBuf, registry: Weak<ProviderRegistry>) -> Self {
        Self { data_dir, registry }
    }
}

#[async_trait::async_trait]
impl Provider for DocumentsProvider {
    async fn handle(&self, _request: ResourceRequest) -> Result<ResourceResponse, ProviderError> {
        Err(ProviderError::Provider(
            "documents provider does not support URI resource routing; use raw operations".into(),
        ))
    }

    fn schemes(&self) -> Vec<&'static str> {
        vec!["documents"]
    }

    fn name(&self) -> &'static str {
        "documents-provider"
    }

    async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
        let data_dir = self.data_dir.clone();
        let request = match serde_json::from_value::<DocumentsProviderRequest>(request.clone()) {
            Ok(request) => request,
            Err(err) => return Ok(provider_error("invalid_request", &err.to_string())),
        };

        let result = match request {
            DocumentsProviderRequest::Publish { doc_did } => {
                let Some(registry) = self.registry.upgrade() else {
                    return Ok(provider_error(
                        "documents_error",
                        "documents provider registry unavailable",
                    ));
                };
                documents_publish_via_provider_plane(&data_dir, &registry, &doc_did)
                    .await
                    .map(|published| json!(published))
            }
            DocumentsProviderRequest::Unpublish { doc_did } => {
                let Some(registry) = self.registry.upgrade() else {
                    return Ok(provider_error(
                        "documents_error",
                        "documents provider registry unavailable",
                    ));
                };
                documents_unpublish_via_provider_plane(&data_dir, &registry, &doc_did)
                    .await
                    .map(|unpublished| json!(unpublished))
            }
            request => tokio::task::spawn_blocking(move || {
                handle_provider_request_inner(&data_dir, request)
            })
            .await
            .map_err(|err| anyhow!("documents provider task failed: {err}"))
            .and_then(|result| result),
        };

        Ok(match result {
            Ok(data) => provider_ok(data),
            Err(err) => provider_error("documents_error", &documents_operation_error_message(err)),
        })
    }
}

pub struct DocumentsClient {
    registry: Arc<ProviderRegistry>,
}

impl DocumentsClient {
    pub fn new(registry: Arc<ProviderRegistry>) -> Self {
        Self { registry }
    }

    pub async fn summary(&self) -> anyhow::Result<Vec<DocumentsListItem>> {
        #[derive(Deserialize)]
        struct SummaryData {
            documents: Vec<DocumentsListItem>,
        }

        let data = self.request(json!({ "op": "summary" })).await?;
        Ok(serde_json::from_value::<SummaryData>(data)?.documents)
    }

    pub async fn create(&self, title: Option<&str>) -> anyhow::Result<DocumentsDocumentView> {
        #[derive(Deserialize)]
        struct DocumentData {
            document: DocumentsDocumentView,
        }

        let data = self
            .request(json!({
                "op": "create",
                "title": title,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentData>(data)?.document)
    }

    pub async fn get(&self, doc_did: &str) -> anyhow::Result<DocumentsDocumentView> {
        #[derive(Deserialize)]
        struct DocumentData {
            document: DocumentsDocumentView,
        }

        let data = self
            .request(json!({
                "op": "get",
                "doc_did": doc_did,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentData>(data)?.document)
    }

    pub async fn save(
        &self,
        doc_did: &str,
        title: &str,
        body: &str,
    ) -> anyhow::Result<DocumentsDocumentView> {
        #[derive(Deserialize)]
        struct DocumentData {
            document: DocumentsDocumentView,
        }

        let data = self
            .request(json!({
                "op": "save",
                "doc_did": doc_did,
                "title": title,
                "body": body,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentData>(data)?.document)
    }

    pub async fn save_as(
        &self,
        doc_did: &str,
        title: Option<&str>,
        file_name: Option<&str>,
        body: &str,
    ) -> anyhow::Result<DocumentsDocumentView> {
        #[derive(Deserialize)]
        struct DocumentData {
            document: DocumentsDocumentView,
        }

        let data = self
            .request(json!({
                "op": "save_as",
                "doc_did": doc_did,
                "title": title,
                "file_name": file_name,
                "body": body,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentData>(data)?.document)
    }

    pub async fn publish(&self, doc_did: &str) -> anyhow::Result<DocumentsPublishResponse> {
        let data = self
            .request(json!({
                "op": "publish",
                "doc_did": doc_did,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentsPublishResponse>(data)?)
    }

    pub async fn unpublish(&self, doc_did: &str) -> anyhow::Result<DocumentsUnpublishResponse> {
        let data = self
            .request(json!({
                "op": "unpublish",
                "doc_did": doc_did,
            }))
            .await?;
        Ok(serde_json::from_value::<DocumentsUnpublishResponse>(data)?)
    }

    pub async fn delete(&self, doc_did: &str) -> anyhow::Result<()> {
        let _ = self
            .request(json!({
                "op": "delete",
                "doc_did": doc_did,
            }))
            .await?;
        Ok(())
    }

    async fn request(&self, request: Value) -> anyhow::Result<Value> {
        let response = self
            .registry
            .send_raw("documents", &request)
            .await
            .map_err(|err| anyhow!("documents provider unavailable: {}", err))?;
        parse_provider_response(response)
    }
}

fn parse_provider_response(response: Value) -> anyhow::Result<Value> {
    match response.get("status").and_then(|value| value.as_str()) {
        Some("ok") => Ok(response.get("data").cloned().unwrap_or(Value::Null)),
        Some("error") => bail!(
            "{}",
            response
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("documents provider error")
        ),
        _ => bail!("invalid documents provider response"),
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

fn handle_provider_request_inner(
    data_dir: &Path,
    request: DocumentsProviderRequest,
) -> anyhow::Result<Value> {
    match request {
        DocumentsProviderRequest::Summary => Ok(json!({
            "documents": documents_load_summary(data_dir)?,
        })),
        DocumentsProviderRequest::Create { title } => Ok(json!({
            "document": documents_create_local(data_dir, title.as_deref())?,
        })),
        DocumentsProviderRequest::Get { doc_did } => Ok(json!({
            "document": documents_load_document(data_dir, &doc_did)?,
        })),
        DocumentsProviderRequest::Save {
            doc_did,
            title,
            body,
        } => Ok(json!({
            "document": documents_save_local(data_dir, &doc_did, &title, &body)?,
        })),
        DocumentsProviderRequest::SaveAs {
            doc_did,
            title,
            file_name,
            body,
        } => Ok(json!({
            "document": documents_save_as_local(
                data_dir,
                &doc_did,
                title.as_deref(),
                file_name.as_deref(),
                &body,
            )?,
        })),
        DocumentsProviderRequest::Delete { doc_did } => {
            documents_delete_local(data_dir, &doc_did)?;
            Ok(json!({}))
        }
        DocumentsProviderRequest::Publish { .. } | DocumentsProviderRequest::Unpublish { .. } => {
            bail!("publish operations require the async provider plane")
        }
    }
}

async fn documents_publish_via_provider_plane(
    data_dir: &Path,
    registry: &ProviderRegistry,
    doc_did: &str,
) -> anyhow::Result<DocumentsPublishResponse> {
    let export = documents_export_publish(data_dir, doc_did)?;

    let publish_input = tempfile::Builder::new()
        .prefix("elastos-documents-publish-")
        .tempdir()?;
    let input_path = publish_input.path().join(&export.file_name);
    std::fs::write(&input_path, export.body.as_bytes())?;

    let viewer_dir = documents_viewer_dir(data_dir)?;
    let (bundle, share_meta) = crate::shares::build_share_bundle_with_viewer_dir(
        &input_path,
        &export.doc_did,
        export.next_version,
        export.latest_published_cid.as_deref(),
        Some(&export.owner_did),
        &viewer_dir,
    )?;
    let content_digest = share_meta.content_digest.clone();
    if let Some(existing) = documents_existing_publish_response(&export, &content_digest) {
        return Ok(existing);
    }

    let cid = ipfs_add_directory_via_provider(registry, bundle.path()).await?;
    let published_at = share_meta.created_at;
    documents_finish_publish(
        data_dir,
        &export.doc_did,
        &cid,
        published_at,
        &content_digest,
    )?;

    Ok(DocumentsPublishResponse {
        uri: format!("elastos://{cid}"),
        route: format!("/s/{cid}/"),
        cid,
        published_at,
    })
}

async fn documents_unpublish_via_provider_plane(
    data_dir: &Path,
    registry: &ProviderRegistry,
    doc_did: &str,
) -> anyhow::Result<DocumentsUnpublishResponse> {
    let document = documents_load_document(data_dir, doc_did)?;
    let cid = document
        .latest_published_cid
        .ok_or_else(|| anyhow!("document is not published"))?;

    ipfs_unpin_via_provider(registry, &cid).await?;
    documents_unpublish_local(data_dir, doc_did)?;

    Ok(DocumentsUnpublishResponse {
        uri: format!("elastos://{cid}"),
        cid,
    })
}

fn documents_existing_publish_response(
    export: &DocumentsPublishExport,
    content_digest: &str,
) -> Option<DocumentsPublishResponse> {
    if export.latest_published_content_digest.as_deref()? != content_digest {
        return None;
    }
    let cid = export.latest_published_cid.clone()?;
    Some(DocumentsPublishResponse {
        uri: format!("elastos://{cid}"),
        route: format!("/s/{cid}/"),
        cid,
        published_at: export.latest_published_at.unwrap_or_default(),
    })
}

async fn ipfs_add_directory_via_provider(
    registry: &ProviderRegistry,
    dir: &Path,
) -> anyhow::Result<String> {
    let mut files = Vec::new();
    crate::ipfs::collect_files_for_ipfs(dir, dir, &mut files)?;
    if files.is_empty() {
        bail!("No files found in {}", dir.display());
    }

    let mut entries = Vec::new();
    for rel_path in &files {
        let abs_path = dir.join(rel_path);
        let bytes = std::fs::read(&abs_path)?;
        entries.push(json!({
            "path": rel_path.to_string_lossy().replace('\\', "/"),
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }));
    }

    let response = registry
        .send_raw(
            "ipfs",
            &json!({
                "op": "add_directory",
                "files": entries,
                "pin": true,
            }),
        )
        .await
        .map_err(|err| anyhow!("ipfs provider unavailable: {err}"))?;
    ipfs_response_cid(&response)
}

async fn ipfs_unpin_via_provider(registry: &ProviderRegistry, cid: &str) -> anyhow::Result<()> {
    let response = registry
        .send_raw(
            "ipfs",
            &json!({
                "op": "unpin",
                "cid": cid,
            }),
        )
        .await
        .map_err(|err| anyhow!("ipfs provider unavailable: {err}"))?;
    ipfs_response_ok(&response, "ipfs unpin")
}

fn ipfs_response_cid(response: &Value) -> anyhow::Result<String> {
    ipfs_response_ok(response, "ipfs add_directory")?;
    response
        .get("data")
        .and_then(|data| data.get("cid"))
        .and_then(|cid| cid.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("No CID in ipfs-provider response"))
}

fn ipfs_response_ok(response: &Value, operation: &str) -> anyhow::Result<()> {
    if response.get("status").and_then(|status| status.as_str()) == Some("error") {
        let message = response
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        bail!("{operation} failed: {message}");
    }
    Ok(())
}

fn documents_operation_error_message(err: anyhow::Error) -> String {
    let text = err.to_string();
    if text.contains("ipfs provider unavailable")
        || text.contains("No provider for scheme: ipfs")
        || text.contains("no provider for scheme: ipfs")
        || text.contains("ipfs-provider")
    {
        return "Publishing is unavailable on this device.".to_string();
    }
    text
}

fn documents_viewer_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let viewer_dir = data_dir.join("capsules").join("documents");
    if viewer_dir.join("index.html").is_file() {
        return Ok(viewer_dir);
    }

    bail!(
        "Viewer 'documents' not installed.\n\n\
         Run first:\n\n\
         \x20 elastos setup --with documents\n\n\
         Then try again."
    )
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn documents_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, "Users/self/Documents")
        .ok_or_else(|| anyhow!("invalid documents root"))
}

fn documents_metadata_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, "ElastOS/Documents")
        .ok_or_else(|| anyhow!("invalid document metadata root"))
}

fn documents_metadata_path(data_dir: &Path, doc_did: &str) -> anyhow::Result<PathBuf> {
    Ok(documents_metadata_root(data_dir)?
        .join(doc_did)
        .join("document.json"))
}

fn documents_working_copy_uri(file_name: &str) -> String {
    format!("localhost://Users/self/Documents/{file_name}")
}

fn documents_object_uri(doc_did: &str) -> String {
    format!("localhost://ElastOS/Documents/{doc_did}")
}

fn documents_normalize_title(title: Option<&str>, default_title: &str) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_title)
        .to_string()
}

fn documents_slugify_file_name(title: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() {
        "untitled".to_string()
    } else {
        slug.to_string()
    };
    format!("{slug}.md")
}

fn documents_validate_file_name(file_name: &str) -> anyhow::Result<String> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        bail!("file name must not be empty");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed == "." || trimmed == ".." {
        bail!("invalid document file name");
    }
    let normalized = if trimmed.to_ascii_lowercase().ends_with(".md") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.md")
    };
    Ok(normalized)
}

fn documents_reserve_file_name(
    docs_root: &Path,
    requested: &str,
    exclude_name: Option<&str>,
) -> anyhow::Result<String> {
    let requested = documents_validate_file_name(requested)?;
    let stem = requested
        .strip_suffix(".md")
        .unwrap_or(requested.as_str())
        .to_string();
    let mut counter = 1usize;
    let mut candidate = requested;
    loop {
        let is_same = exclude_name
            .map(|value| value.eq_ignore_ascii_case(&candidate))
            .unwrap_or(false);
        if is_same || !docs_root.join(&candidate).exists() {
            return Ok(candidate);
        }
        counter += 1;
        candidate = format!("{stem}-{counter}.md");
    }
}

fn documents_generate_did() -> String {
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let (_signing_key, did) = elastos_identity::derive_did(&secret);
    did
}

fn documents_load_metadata(data_dir: &Path, doc_did: &str) -> anyhow::Result<DocumentsMetadata> {
    let path = documents_metadata_path(data_dir, doc_did)?;
    let bytes =
        std::fs::read(&path).map_err(|err| anyhow!("document metadata not found: {err}"))?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn documents_save_metadata(data_dir: &Path, metadata: &DocumentsMetadata) -> anyhow::Result<()> {
    let path = documents_metadata_path(data_dir, &metadata.doc_did)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(metadata)?)?;
    Ok(())
}

fn documents_load_body(data_dir: &Path, metadata: &DocumentsMetadata) -> anyhow::Result<String> {
    let body = std::fs::read_to_string(documents_root(data_dir)?.join(&metadata.file_name))
        .map_err(|err| anyhow!("document body not found: {err}"))?;
    Ok(body)
}

fn documents_view(metadata: &DocumentsMetadata, body: String) -> DocumentsDocumentView {
    DocumentsDocumentView {
        doc_did: metadata.doc_did.clone(),
        document_uri: documents_object_uri(&metadata.doc_did),
        title: metadata.title.clone(),
        file_name: metadata.file_name.clone(),
        working_copy_uri: metadata.working_copy_uri.clone(),
        body,
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        latest_published_cid: metadata.latest_published_cid.clone(),
        publish_history: metadata.publish_history.clone(),
    }
}

fn documents_list_item(metadata: &DocumentsMetadata) -> DocumentsListItem {
    DocumentsListItem {
        doc_did: metadata.doc_did.clone(),
        document_uri: documents_object_uri(&metadata.doc_did),
        title: metadata.title.clone(),
        file_name: metadata.file_name.clone(),
        working_copy_uri: metadata.working_copy_uri.clone(),
        updated_at: metadata.updated_at,
        latest_published_cid: metadata.latest_published_cid.clone(),
    }
}

fn documents_load_metadata_index(data_dir: &Path) -> anyhow::Result<Vec<DocumentsMetadata>> {
    let docs_root = documents_root(data_dir)?;
    let metadata_root = documents_metadata_root(data_dir)?;

    if !docs_root.is_dir() || !metadata_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut documents = Vec::new();
    for entry in std::fs::read_dir(&metadata_root)? {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let path = entry.path().join("document.json");
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        let metadata = serde_json::from_slice::<DocumentsMetadata>(&bytes)?;
        if docs_root.join(&metadata.file_name).is_file() {
            documents.push(metadata);
        }
    }

    documents.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.title.cmp(&right.title))
    });
    Ok(documents)
}

fn documents_load_summary(data_dir: &Path) -> anyhow::Result<Vec<DocumentsListItem>> {
    Ok(documents_load_metadata_index(data_dir)?
        .into_iter()
        .map(|metadata| documents_list_item(&metadata))
        .collect())
}

pub fn documents_load_document(
    data_dir: &Path,
    doc_did: &str,
) -> anyhow::Result<DocumentsDocumentView> {
    let metadata = documents_load_metadata(data_dir, doc_did)?;
    let body = documents_load_body(data_dir, &metadata)?;
    Ok(documents_view(&metadata, body))
}

fn documents_create_local(
    data_dir: &Path,
    requested_title: Option<&str>,
) -> anyhow::Result<DocumentsDocumentView> {
    let docs_root = documents_root(data_dir)?;
    std::fs::create_dir_all(&docs_root)?;
    let owner_did = elastos_identity::load_or_create_did(data_dir)?.1;
    let title = documents_normalize_title(requested_title, DOCUMENTS_DEFAULT_TITLE);
    let file_name =
        documents_reserve_file_name(&docs_root, &documents_slugify_file_name(&title), None)?;
    let body = String::new();
    std::fs::write(docs_root.join(&file_name), &body)?;
    let ts = now_ts();
    let metadata = DocumentsMetadata {
        schema: DOCUMENTS_SCHEMA.to_string(),
        doc_did: documents_generate_did(),
        owner_did,
        title,
        file_name: file_name.clone(),
        working_copy_uri: documents_working_copy_uri(&file_name),
        created_at: ts,
        updated_at: ts,
        latest_published_cid: None,
        publish_history: Vec::new(),
    };
    documents_save_metadata(data_dir, &metadata)?;
    Ok(documents_view(&metadata, body))
}

pub fn documents_save_local(
    data_dir: &Path,
    doc_did: &str,
    requested_title: &str,
    body: &str,
) -> anyhow::Result<DocumentsDocumentView> {
    let mut metadata = documents_load_metadata(data_dir, doc_did)?;
    let title = documents_normalize_title(Some(requested_title), DOCUMENTS_DEFAULT_TITLE);
    std::fs::write(documents_root(data_dir)?.join(&metadata.file_name), body)?;
    metadata.title = title;
    metadata.updated_at = now_ts();
    documents_save_metadata(data_dir, &metadata)?;
    Ok(documents_view(&metadata, body.to_string()))
}

pub fn documents_save_as_local(
    data_dir: &Path,
    doc_did: &str,
    requested_title: Option<&str>,
    requested_file_name: Option<&str>,
    body: &str,
) -> anyhow::Result<DocumentsDocumentView> {
    let source = documents_load_metadata(data_dir, doc_did)?;
    let docs_root = documents_root(data_dir)?;
    let owner_did = elastos_identity::load_or_create_did(data_dir)?.1;
    let title = documents_normalize_title(requested_title, &source.title);
    let desired_name = match requested_file_name {
        Some(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => documents_slugify_file_name(&title),
    };
    let file_name = documents_reserve_file_name(&docs_root, &desired_name, None)?;
    std::fs::write(docs_root.join(&file_name), body)?;
    let ts = now_ts();
    let metadata = DocumentsMetadata {
        schema: DOCUMENTS_SCHEMA.to_string(),
        doc_did: documents_generate_did(),
        owner_did,
        title,
        file_name: file_name.clone(),
        working_copy_uri: documents_working_copy_uri(&file_name),
        created_at: ts,
        updated_at: ts,
        latest_published_cid: None,
        publish_history: Vec::new(),
    };
    documents_save_metadata(data_dir, &metadata)?;
    Ok(documents_view(&metadata, body.to_string()))
}

pub fn documents_delete_local(data_dir: &Path, doc_did: &str) -> anyhow::Result<()> {
    let metadata = documents_load_metadata(data_dir, doc_did)?;
    remove_file_if_exists(documents_root(data_dir)?.join(&metadata.file_name))?;
    let metadata_dir = documents_metadata_root(data_dir)?.join(&metadata.doc_did);
    match std::fs::remove_dir_all(&metadata_dir) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn documents_export_publish(
    data_dir: &Path,
    doc_did: &str,
) -> anyhow::Result<DocumentsPublishExport> {
    let metadata = documents_load_metadata(data_dir, doc_did)?;
    let body = documents_load_body(data_dir, &metadata)?;
    let latest_record = documents_latest_publish_record(&metadata);
    let latest_published_content_digest = latest_record.map(|record| record.content_digest.clone());
    let latest_published_at = latest_record.map(|record| record.published_at);
    Ok(DocumentsPublishExport {
        doc_did: metadata.doc_did,
        title: metadata.title,
        file_name: metadata.file_name,
        owner_did: metadata.owner_did,
        body,
        next_version: metadata.publish_history.len() as u64 + 1,
        latest_published_cid: metadata.latest_published_cid,
        latest_published_content_digest,
        latest_published_at,
    })
}

fn documents_latest_publish_record(
    metadata: &DocumentsMetadata,
) -> Option<&DocumentsPublishRecord> {
    let latest_cid = metadata.latest_published_cid.as_deref()?;
    metadata
        .publish_history
        .iter()
        .rev()
        .find(|record| record.cid == latest_cid)
}

fn documents_finish_publish(
    data_dir: &Path,
    doc_did: &str,
    cid: &str,
    published_at: u64,
    content_digest: &str,
) -> anyhow::Result<()> {
    let mut metadata = documents_load_metadata(data_dir, doc_did)?;
    metadata.latest_published_cid = Some(cid.to_string());
    metadata.publish_history.push(DocumentsPublishRecord {
        cid: cid.to_string(),
        published_at,
        content_digest: content_digest.to_string(),
    });
    metadata.updated_at = now_ts();
    documents_save_metadata(data_dir, &metadata)
}

fn documents_unpublish_local(
    data_dir: &Path,
    doc_did: &str,
) -> anyhow::Result<DocumentsDocumentView> {
    let mut metadata = documents_load_metadata(data_dir, doc_did)?;
    if metadata.latest_published_cid.is_none() {
        bail!("document is not published");
    }
    metadata.latest_published_cid = None;
    metadata.updated_at = now_ts();
    documents_save_metadata(data_dir, &metadata)?;
    let body = documents_load_body(data_dir, &metadata)?;
    Ok(documents_view(&metadata, body))
}

fn remove_file_if_exists(path: PathBuf) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CID: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";

    struct MockIpfsProvider {
        cid: String,
        add_count: tokio::sync::Mutex<usize>,
        unpinned: tokio::sync::Mutex<Vec<String>>,
    }

    impl MockIpfsProvider {
        fn new(cid: &str) -> Self {
            Self {
                cid: cid.to_string(),
                add_count: tokio::sync::Mutex::new(0),
                unpinned: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for MockIpfsProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "mock ipfs provider only supports raw operations".into(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "mock-ipfs-provider"
        }

        async fn send_raw(&self, request: &Value) -> Result<Value, ProviderError> {
            match request.get("op").and_then(|op| op.as_str()) {
                Some("add_directory") => {
                    *self.add_count.lock().await += 1;
                    Ok(json!({
                        "status": "ok",
                        "data": {
                            "cid": self.cid,
                        },
                    }))
                }
                Some("unpin") => {
                    let cid = request
                        .get("cid")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    self.unpinned.lock().await.push(cid);
                    Ok(json!({
                        "status": "ok",
                        "data": {},
                    }))
                }
                _ => Ok(provider_error(
                    "unsupported",
                    "unsupported mock ipfs operation",
                )),
            }
        }
    }

    fn install_test_documents_viewer(data_dir: &Path) {
        let viewer_dir = data_dir.join("capsules").join("documents");
        std::fs::create_dir_all(&viewer_dir).unwrap();
        std::fs::write(
            viewer_dir.join("index.html"),
            "<!doctype html><title>Documents</title>",
        )
        .unwrap();
    }

    #[test]
    fn documents_summary_is_read_only_on_empty_store() {
        let dir = tempfile::tempdir().unwrap();

        let summary = documents_load_summary(dir.path()).unwrap();

        assert!(summary.is_empty());
        assert!(!documents_root(dir.path()).unwrap().exists());
        assert!(!documents_metadata_root(dir.path()).unwrap().exists());
    }

    #[test]
    fn documents_summary_does_not_import_orphan_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let docs_root = documents_root(dir.path()).unwrap();
        std::fs::create_dir_all(&docs_root).unwrap();
        std::fs::write(docs_root.join("Legacy.md"), "# Legacy\n").unwrap();

        let summary = documents_load_summary(dir.path()).unwrap();

        assert!(summary.is_empty());
        assert!(!documents_metadata_root(dir.path()).unwrap().exists());
        assert!(docs_root.join("Legacy.md").is_file());
    }

    #[test]
    fn documents_create_explicitly_adds_summary_object() {
        let dir = tempfile::tempdir().unwrap();

        let document = documents_create_local(dir.path(), Some("Project Plan")).unwrap();
        let summary = documents_load_summary(dir.path()).unwrap();

        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].doc_did, document.doc_did);
        assert_eq!(
            summary[0].document_uri,
            documents_object_uri(&document.doc_did)
        );
        assert_eq!(summary[0].title, "Project Plan");
    }

    #[test]
    fn documents_publish_export_and_finish_records_revision() {
        let dir = tempfile::tempdir().unwrap();
        let document = documents_create_local(dir.path(), Some("Publish Me")).unwrap();
        let document = documents_save_local(
            dir.path(),
            &document.doc_did,
            "Publish Me",
            "# Publish Me\n",
        )
        .unwrap();

        let export = documents_export_publish(dir.path(), &document.doc_did).unwrap();

        assert_eq!(export.doc_did, document.doc_did);
        assert_eq!(export.body, "# Publish Me\n");
        assert_eq!(export.next_version, 1);
        assert_eq!(export.latest_published_cid, None);

        documents_finish_publish(
            dir.path(),
            &document.doc_did,
            "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
            42,
            "sha256:abc123",
        )
        .unwrap();

        let republish = documents_export_publish(dir.path(), &document.doc_did).unwrap();
        let loaded = documents_load_document(dir.path(), &document.doc_did).unwrap();
        let summary = documents_load_summary(dir.path()).unwrap();

        assert_eq!(republish.next_version, 2);
        assert_eq!(
            republish.latest_published_cid.as_deref(),
            Some("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
        );
        assert_eq!(
            republish.latest_published_content_digest.as_deref(),
            Some("sha256:abc123")
        );
        assert_eq!(republish.latest_published_at, Some(42));
        assert_eq!(
            loaded.latest_published_cid.as_deref(),
            Some("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
        );
        assert_eq!(loaded.publish_history.len(), 1);
        assert_eq!(loaded.publish_history[0].published_at, 42);
        assert_eq!(loaded.publish_history[0].content_digest, "sha256:abc123");
        assert_eq!(
            summary[0].latest_published_cid.as_deref(),
            Some("bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi")
        );

        let unpublished = documents_unpublish_local(dir.path(), &document.doc_did).unwrap();
        let summary = documents_load_summary(dir.path()).unwrap();
        let republish = documents_export_publish(dir.path(), &document.doc_did).unwrap();

        assert_eq!(unpublished.latest_published_cid, None);
        assert_eq!(unpublished.publish_history.len(), 1);
        assert_eq!(summary[0].latest_published_cid, None);
        assert_eq!(republish.latest_published_cid, None);
        assert_eq!(republish.latest_published_content_digest, None);
        assert_eq!(republish.latest_published_at, None);
        assert_eq!(republish.next_version, 2);
        assert!(documents_unpublish_local(dir.path(), &document.doc_did).is_err());
    }

    #[test]
    fn documents_publish_reuses_current_cid_for_same_content_digest() {
        let export = DocumentsPublishExport {
            doc_did: "did:key:z6Mkdoc".to_string(),
            title: "Notes".to_string(),
            file_name: "notes.md".to_string(),
            owner_did: "did:key:z6Mkowner".to_string(),
            body: "# Notes\n".to_string(),
            next_version: 2,
            latest_published_cid: Some(TEST_CID.to_string()),
            latest_published_content_digest: Some("sha256:same".to_string()),
            latest_published_at: Some(123),
        };

        let reused = documents_existing_publish_response(&export, "sha256:same").unwrap();

        assert_eq!(reused.cid, TEST_CID);
        assert_eq!(reused.uri, format!("elastos://{TEST_CID}"));
        assert_eq!(reused.route, format!("/s/{TEST_CID}/"));
        assert_eq!(reused.published_at, 123);
        assert!(documents_existing_publish_response(&export, "sha256:changed").is_none());
    }

    #[tokio::test]
    async fn documents_publish_and_unpublish_use_provider_plane() {
        let dir = tempfile::tempdir().unwrap();
        install_test_documents_viewer(dir.path());
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register(Arc::new(DocumentsProvider::new(
                dir.path().to_path_buf(),
                Arc::downgrade(&registry),
            )))
            .await;
        let ipfs_provider = Arc::new(MockIpfsProvider::new(TEST_CID));
        registry
            .register_sub_provider("ipfs", ipfs_provider.clone())
            .await
            .unwrap();

        let client = DocumentsClient::new(registry);
        let document = client.create(Some("Provider Plane")).await.unwrap();
        client
            .save(
                &document.doc_did,
                "Provider Plane",
                "# Provider Plane\n\nRuntime-owned publish.\n",
            )
            .await
            .unwrap();

        let published = client.publish(&document.doc_did).await.unwrap();
        assert_eq!(published.cid, TEST_CID);
        assert_eq!(published.uri, format!("elastos://{TEST_CID}"));
        assert_eq!(published.route, format!("/s/{TEST_CID}/"));
        assert_eq!(*ipfs_provider.add_count.lock().await, 1);

        let republished = client.publish(&document.doc_did).await.unwrap();
        assert_eq!(republished.cid, TEST_CID);
        assert_eq!(
            *ipfs_provider.add_count.lock().await,
            1,
            "unchanged content must reuse the current CID"
        );

        let unpublished = client.unpublish(&document.doc_did).await.unwrap();
        assert_eq!(unpublished.cid, TEST_CID);
        assert_eq!(unpublished.uri, format!("elastos://{TEST_CID}"));
        assert_eq!(
            ipfs_provider.unpinned.lock().await.as_slice(),
            [TEST_CID.to_string()]
        );

        let loaded = client.get(&document.doc_did).await.unwrap();
        assert_eq!(loaded.latest_published_cid, None);
    }

    #[tokio::test]
    async fn documents_publish_requires_ipfs_provider() {
        let dir = tempfile::tempdir().unwrap();
        install_test_documents_viewer(dir.path());
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register(Arc::new(DocumentsProvider::new(
                dir.path().to_path_buf(),
                Arc::downgrade(&registry),
            )))
            .await;
        let client = DocumentsClient::new(registry);
        let document = client.create(Some("Draft")).await.unwrap();

        let err = client.publish(&document.doc_did).await.unwrap_err();
        assert_eq!(err.to_string(), "Publishing is unavailable on this device.");
    }

    #[test]
    fn documents_delete_removes_object_and_working_copy() {
        let dir = tempfile::tempdir().unwrap();
        let document = documents_create_local(dir.path(), Some("Delete Me")).unwrap();
        let body_path = documents_root(dir.path())
            .unwrap()
            .join(&document.file_name);
        let metadata_dir = documents_metadata_root(dir.path())
            .unwrap()
            .join(&document.doc_did);

        assert!(body_path.is_file());
        assert!(metadata_dir.join("document.json").is_file());

        documents_delete_local(dir.path(), &document.doc_did).unwrap();

        assert!(!body_path.exists());
        assert!(!metadata_dir.exists());
        assert!(documents_load_summary(dir.path()).unwrap().is_empty());
        assert!(documents_load_document(dir.path(), &document.doc_did).is_err());
    }
}
