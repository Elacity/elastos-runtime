//! Canonical Assistant workspace adoption. Runtime retains all legacy sources;
//! the capsule owns the new document shape. Adoption never dispatches a run.
use super::*;
use serde_json::Value;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

pub(super) const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 48;
const SCHEMA: &str = "elastos.assistant.workspace/v2";
const RELATIVE_PATH: &str = ".AppData/ElastOS/Assistant/workspace-v2.json";
const MIGRATED: &str = "assistant_workspace_migrated";
const CONFLICT: &str = "assistant_workspace_revision_conflict";
const SOURCE_CONFLICT: &str = "assistant_workspace_migration_conflict";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkspace {
    schema: String,
    revision: u64,
    document: Value,
    // Immutable server-owned originals, retained independently of client saves.
    legacy: Value,
    migration_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PutRequest {
    schema: String,
    if_revision: u64,
    document: Value,
    #[serde(default)]
    migration_revision: Option<String>,
}

pub(super) fn mutation_guard() -> anyhow::Result<MutexGuard<'static, ()>> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Assistant workspace mutation lock poisoned"))
}

pub(super) fn migration_error_response(error: &anyhow::Error) -> Option<Response> {
    let code = [MIGRATED, CONFLICT, SOURCE_CONFLICT]
        .into_iter()
        .find(|code| error.chain().any(|cause| cause.to_string() == *code))?;
    Some(
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "schema": "elastos.assistant.workspace.error/v1",
                "code": code,
                "message": if code == MIGRATED {
                    "This workspace is now owned by Assistant. Reopen Assistant to continue."
                } else {
                    "The saved workspace changed. Keep local work and reload before saving."
                }
            })),
        )
            .into_response(),
    )
}

pub(super) async fn get(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context = match require_home_launch_token_context(&state.data_dir, &headers, "assistant") {
        Ok(context) => context,
        Err(error) => return home_error_response(error),
    };
    match read(&state.data_dir, &context) {
        Ok(value) => Json(value).into_response(),
        Err(error) => home_error_response(error),
    }
}

pub(super) async fn put(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<PutRequest>,
) -> Response {
    let context = match require_home_launch_token_context(&state.data_dir, &headers, "assistant") {
        Ok(context) => context,
        Err(error) => return home_error_response(error),
    };
    if let Err(error) = validate_document(&request.schema, &request.document) {
        return (StatusCode::BAD_REQUEST, error.to_string()).into_response();
    }
    match save(&state.data_dir, &context, request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            migration_error_response(&error).unwrap_or_else(|| home_error_response(error))
        }
    }
}

fn object_location(
    data_dir: &Path,
    principal_id: &str,
    relative: &str,
) -> anyhow::Result<(String, PathBuf)> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let uri = format!("{root}/{relative}");
    let path = rooted_localhost_fs_path(data_dir, &uri)
        .ok_or_else(|| anyhow::anyhow!("invalid Assistant workspace object root"))?;
    Ok((uri, path))
}

fn optional_object(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
    relative: &str,
    max_bytes: usize,
) -> anyhow::Result<Option<Vec<u8>>> {
    let (uri, path) = object_location(data_dir, &context.principal_id, relative)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let root = crate::auth::principal_localhost_root(&context.principal_id);
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        &context.principal_id,
        &root,
        &uri,
        &path,
    )?;
    anyhow::ensure!(
        bytes.len() <= max_bytes,
        "Assistant workspace source exceeds its byte limit"
    );
    Ok(Some(bytes))
}

fn load(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<StoredWorkspace>> {
    let Some(bytes) = optional_object(data_dir, context, RELATIVE_PATH, MAX_BYTES)? else {
        return Ok(None);
    };
    let stored: StoredWorkspace = serde_json::from_slice(&bytes)?;
    validate_document(&stored.schema, &stored.document)?;
    anyhow::ensure!(
        stored.revision > 0,
        "invalid adopted Assistant workspace revision"
    );
    validate_legacy(&stored.legacy)?;
    anyhow::ensure!(
        migration_revision(&stored.legacy)? == stored.migration_revision,
        "Assistant workspace migration identity mismatch"
    );
    Ok(Some(stored))
}

// Called while holding mutation_guard, also held by all legacy write seams.
pub(super) fn ensure_legacy_writable(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    if load(data_dir, context)?.is_some() {
        anyhow::bail!(MIGRATED);
    }
    Ok(())
}

pub(super) fn legacy_home_agent(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<Option<Value>> {
    let Some(bytes) = optional_object(
        data_dir,
        context,
        ".AppData/ElastOS/Home/browser-state.json",
        HOME_BROWSER_STATE_MAX_BYTES,
    )?
    else {
        return Ok(None);
    };
    // Read the source before Home's window sanitizer. Empty/retired windows
    // do not make a legacy conversation disappear.
    let raw: Value = serde_json::from_slice(&bytes)?;
    let state: HomeBrowserStateSummary = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        state.schema == HOME_BROWSER_STATE_SCHEMA
            && state.principal_id == context.principal_id
            && state.localhost_root == crate::auth::principal_localhost_root(&context.principal_id),
        "invalid legacy Home workspace identity"
    );
    if let Some(session) = raw.get("session").filter(|value| !value.is_null()) {
        anyhow::ensure!(session.is_object(), "invalid legacy Home session");
        if let Some(agent) = session.get("agent") {
            anyhow::ensure!(
                agent.is_object() || agent.is_null(),
                "invalid legacy Home Agent document"
            );
            return Ok(Some(agent.clone()));
        }
    }
    Ok(None)
}

fn capture_legacy(data_dir: &Path, context: &HomeLaunchTokenContext) -> anyhow::Result<Value> {
    let assistant = optional_object(
        data_dir,
        context,
        ".AppData/ElastOS/Assistant/workspace.json",
        gateway_assistant::ASSISTANT_WORKSPACE_MAX_BYTES,
    )?
    .map(|bytes| gateway_assistant::validate_legacy_workspace_bytes(&bytes))
    .transpose()?;
    let home_agent = optional_object(
        data_dir,
        context,
        ".AppData/ElastOS/HomeAgent/workspace.json",
        gateway_home_agent::HOME_AGENT_WORKSPACE_MAX_BYTES,
    )?
    .map(|bytes| gateway_home_agent::validate_legacy_workspace_bytes(&bytes))
    .transpose()?;
    Ok(
        serde_json::json!({ "assistant": assistant, "homeAgent": home_agent, "homeSessionAgent": legacy_home_agent(data_dir, context)? }),
    )
}

fn validate_legacy(legacy: &Value) -> anyhow::Result<()> {
    let object = legacy
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid retained Assistant sources"))?;
    anyhow::ensure!(
        object.len() == 3
            && ["assistant", "homeAgent", "homeSessionAgent"]
                .iter()
                .all(|key| object.contains_key(*key)),
        "invalid retained Assistant source keys"
    );
    anyhow::ensure!(
        json_depth(legacy) <= MAX_DEPTH,
        "retained Assistant sources exceed depth limit"
    );
    Ok(())
}

fn migration_revision(legacy: &Value) -> anyhow::Result<String> {
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(legacy)?))
    ))
}

fn response(stored: &StoredWorkspace) -> Value {
    serde_json::json!({ "schema": stored.schema, "revision": stored.revision, "document": stored.document })
}

fn read(data_dir: &Path, context: &HomeLaunchTokenContext) -> anyhow::Result<Value> {
    let _guard = mutation_guard()?;
    if let Some(stored) = load(data_dir, context)? {
        return Ok(response(&stored));
    }
    let legacy = capture_legacy(data_dir, context)?;
    validate_legacy(&legacy)?;
    Ok(
        serde_json::json!({ "schema": SCHEMA, "revision": 0, "document": {}, "migration_revision": migration_revision(&legacy)?, "legacy": legacy }),
    )
}

fn save(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
    request: PutRequest,
) -> anyhow::Result<Value> {
    let _guard = mutation_guard()?;
    let current = load(data_dir, context)?;
    let revision = current.as_ref().map(|stored| stored.revision).unwrap_or(0);
    if revision != request.if_revision {
        anyhow::bail!(CONFLICT);
    }
    let (legacy, source_revision) = if let Some(current) = current {
        (current.legacy, current.migration_revision)
    } else {
        let legacy = capture_legacy(data_dir, context)?;
        let source_revision = migration_revision(&legacy)?;
        if request.migration_revision.as_deref() != Some(source_revision.as_str()) {
            anyhow::bail!(SOURCE_CONFLICT);
        }
        (legacy, source_revision)
    };
    validate_legacy(&legacy)?;
    let stored = StoredWorkspace {
        schema: request.schema,
        revision: revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Assistant workspace revision overflow"))?,
        document: request.document,
        legacy,
        migration_revision: source_revision,
    };
    let bytes = serde_json::to_vec_pretty(&stored)?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Assistant workspace including retained sources exceeds {} bytes",
        MAX_BYTES
    );
    let (uri, path) = object_location(data_dir, &context.principal_id, RELATIVE_PATH)?;
    let root = crate::auth::principal_localhost_root(&context.principal_id);
    crate::auth::write_protected_principal_root_object(
        data_dir,
        &context.principal_id,
        &root,
        &uri,
        &path,
        &bytes,
    )?;
    Ok(response(&stored))
}

fn validate_document(schema: &str, document: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        schema == SCHEMA,
        "Assistant workspace schema must be {SCHEMA}"
    );
    anyhow::ensure!(
        document.is_object(),
        "Assistant workspace document must be an object"
    );
    anyhow::ensure!(
        json_depth(document) <= MAX_DEPTH,
        "Assistant workspace document exceeds depth limit"
    );
    Ok(())
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(fields) => 1 + fields.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Window/session saves remain available. They preserve the legacy Agent field
/// verbatim; after adoption, an old Agent writer gets an explicit conflict.
pub(super) fn preserve_legacy_home_agent(
    data_dir: &Path,
    context: &HomeLaunchTokenContext,
    input: &mut HomeBrowserStateUpdate,
) -> anyhow::Result<()> {
    let existing = legacy_home_agent(data_dir, context)?;
    let adopted = load(data_dir, context)?.is_some();
    let Some(session) = input.session.as_mut() else {
        return Ok(());
    };
    anyhow::ensure!(
        session.as_ref().is_none_or(Value::is_object),
        "Home session must be an object or null while retaining Agent history"
    );
    if let Some(proposed) = session.as_ref().and_then(|value| value.get("agent")) {
        if adopted && Some(proposed) != existing.as_ref() {
            anyhow::bail!(MIGRATED);
        }
    }
    if let Some(agent) = existing {
        if session.is_none() {
            *session = Some(serde_json::json!({ "windows": [] }));
        }
        if let Some(object) = session.as_mut().and_then(Value::as_object_mut) {
            object.entry("agent").or_insert(agent);
        }
    }
    Ok(())
}
