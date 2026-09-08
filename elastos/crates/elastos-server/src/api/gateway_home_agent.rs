use super::*;
use serde_json::Value;
use std::sync::{Mutex, OnceLock};

pub(super) const HOME_AGENT_WORKSPACE_MAX_BYTES: usize = 1024 * 1024;

const HOME_AGENT_CAPSULE_ID: &str = "home-agent";
const HOME_AGENT_WORKSPACE_SCHEMA: &str = "elastos.home-agent.workspace/v1";
const HOME_AGENT_WORKSPACE_RELATIVE_ROOT: &str = ".AppData/ElastOS/HomeAgent";
const HOME_AGENT_WORKSPACE_FILE: &str = "workspace.json";
const HOME_AGENT_WORKSPACE_MAX_DEPTH: usize = 24;

/// The Home Agent's workspace: the capsule owns the document's shape, the
/// Runtime owns where it lives, who may read it, its size and its revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeAgentWorkspace {
    schema: String,
    revision: u64,
    document: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeAgentWorkspacePutRequest {
    schema: String,
    if_revision: u64,
    document: Value,
}

pub(super) async fn home_agent_workspace_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, HOME_AGENT_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return super::home_error_response(err),
        };
    match load_home_agent_workspace(&state.data_dir, &context) {
        Ok(workspace) => Json(workspace).into_response(),
        Err(err) => super::home_error_response(err),
    }
}

pub(super) async fn home_agent_workspace_put(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<HomeAgentWorkspacePutRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, HOME_AGENT_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return super::home_error_response(err),
        };
    if let Err(err) = validate_workspace_put_request(&request) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    match save_home_agent_workspace(&state.data_dir, &context, request) {
        Ok(workspace) => Json(workspace).into_response(),
        Err(err) if is_revision_conflict(&err) => {
            (StatusCode::CONFLICT, err.to_string()).into_response()
        }
        Err(err) => super::home_error_response(err),
    }
}

pub(super) fn principal_root_protected_object_inventory(
    localhost_root: &str,
) -> Vec<crate::auth::PrincipalRootProtectedObjectDeclarationV1> {
    vec![
        crate::auth::PrincipalRootProtectedObjectDeclarationV1::root(format!(
            "{localhost_root}/{HOME_AGENT_WORKSPACE_RELATIVE_ROOT}"
        )),
    ]
}

fn home_agent_workspace_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/{HOME_AGENT_WORKSPACE_RELATIVE_ROOT}/{HOME_AGENT_WORKSPACE_FILE}",
        crate::auth::principal_localhost_root(&context.principal_id)
    )
}

fn home_agent_workspace_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &home_agent_workspace_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Home Agent workspace root"))
}

fn default_home_agent_workspace() -> HomeAgentWorkspace {
    HomeAgentWorkspace {
        schema: HOME_AGENT_WORKSPACE_SCHEMA.to_string(),
        revision: 0,
        document: Value::Object(serde_json::Map::new()),
    }
}

fn load_home_agent_workspace(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<HomeAgentWorkspace> {
    let path = home_agent_workspace_path(data_dir, context)?;
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_home_agent_workspace());
        }
        Err(err) => return Err(err.into()),
    }
    let principal_id = &context.principal_id;
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &localhost_root,
        &home_agent_workspace_uri(context),
        &path,
    )?;
    validate_workspace_bytes(&bytes)?;
    let workspace: HomeAgentWorkspace = serde_json::from_slice(&bytes)?;
    validate_workspace(&workspace)?;
    Ok(workspace)
}

fn save_home_agent_workspace(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request: HomeAgentWorkspacePutRequest,
) -> anyhow::Result<HomeAgentWorkspace> {
    let _guard = home_agent_workspace_mutation_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("home agent workspace mutation lock poisoned"))?;
    let current = load_home_agent_workspace(data_dir, context)?;
    if request.if_revision != current.revision {
        anyhow::bail!("home agent workspace revision conflict");
    }
    let next = HomeAgentWorkspace {
        schema: request.schema,
        revision: current
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("home agent workspace revision overflow"))?,
        document: request.document,
    };
    validate_workspace(&next)?;
    let bytes = serde_json::to_vec_pretty(&next)?;
    validate_workspace_bytes(&bytes)?;
    let path = home_agent_workspace_path(data_dir, context)?;
    let principal_id = &context.principal_id;
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        &localhost_root,
        &home_agent_workspace_uri(context),
        &path,
        &bytes,
    )?;
    Ok(next)
}

fn validate_workspace_put_request(request: &HomeAgentWorkspacePutRequest) -> anyhow::Result<()> {
    validate_workspace(&HomeAgentWorkspace {
        schema: request.schema.clone(),
        revision: request.if_revision,
        document: request.document.clone(),
    })
}

fn validate_workspace(workspace: &HomeAgentWorkspace) -> anyhow::Result<()> {
    if workspace.schema != HOME_AGENT_WORKSPACE_SCHEMA {
        anyhow::bail!("home agent workspace schema must be {HOME_AGENT_WORKSPACE_SCHEMA}");
    }
    if !workspace.document.is_object() {
        anyhow::bail!("home agent workspace document must be a JSON object");
    }
    if json_depth(&workspace.document) > HOME_AGENT_WORKSPACE_MAX_DEPTH {
        anyhow::bail!(
            "home agent workspace document exceeds depth {}",
            HOME_AGENT_WORKSPACE_MAX_DEPTH
        );
    }
    Ok(())
}

fn validate_workspace_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.len() > HOME_AGENT_WORKSPACE_MAX_BYTES {
        anyhow::bail!(
            "home agent workspace exceeds {} bytes",
            HOME_AGENT_WORKSPACE_MAX_BYTES
        );
    }
    Ok(())
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn is_revision_conflict(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string() == "home agent workspace revision conflict")
}

fn home_agent_workspace_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
