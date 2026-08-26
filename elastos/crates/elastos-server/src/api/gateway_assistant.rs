use super::*;
use elastos_model_contract::validate_run_id;
use std::sync::{Mutex, OnceLock};

pub(super) const ASSISTANT_WORKSPACE_MAX_BYTES: usize = 256 * 1024;

const ASSISTANT_CAPSULE_ID: &str = "assistant";
const ASSISTANT_WORKSPACE_SCHEMA: &str = "elastos.assistant.workspace/v1";
const ASSISTANT_WORKSPACE_RELATIVE_ROOT: &str = ".AppData/ElastOS/Assistant";
const ASSISTANT_WORKSPACE_FILE: &str = "workspace.json";
const ASSISTANT_WORKSPACE_MAX_SESSIONS: usize = 24;
const ASSISTANT_WORKSPACE_MAX_MESSAGES_PER_SESSION: usize = 64;
const ASSISTANT_WORKSPACE_MAX_SESSION_ID_BYTES: usize = 128;
const ASSISTANT_WORKSPACE_MAX_SESSION_TITLE_BYTES: usize = 160;
const ASSISTANT_WORKSPACE_MAX_MESSAGE_CONTENT_BYTES: usize = 8 * 1024;
const ASSISTANT_WORKSPACE_MAX_DRAFT_BYTES: usize = 16 * 1024;
const ASSISTANT_WORKSPACE_MAX_MODE_BYTES: usize = 32;
const ASSISTANT_WORKSPACE_MAX_ROLE_BYTES: usize = 32;
const ASSISTANT_WORKSPACE_MAX_OFFER_ID_BYTES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct AssistantWorkspace {
    schema: String,
    revision: u64,
    #[serde(default)]
    sessions: Vec<AssistantWorkspaceSession>,
    draft: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected_offer_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AssistantWorkspaceSession {
    id: String,
    title: String,
    mode: AssistantWorkspaceSessionMode,
    #[serde(default)]
    messages: Vec<AssistantWorkspaceMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AssistantWorkspaceSessionMode {
    Build,
    Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AssistantWorkspaceMessage {
    role: AssistantWorkspaceMessageRole,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AssistantWorkspaceMessageRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssistantWorkspacePutRequest {
    schema: String,
    if_revision: u64,
    #[serde(default)]
    sessions: Vec<AssistantWorkspaceSession>,
    draft: String,
    #[serde(default)]
    selected_offer_id: Option<String>,
}

pub(super) async fn assistant_workspace_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, ASSISTANT_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return super::home_error_response(err),
        };
    match load_assistant_workspace(&state.data_dir, &context) {
        Ok(workspace) => Json(workspace).into_response(),
        Err(err) => super::home_error_response(err),
    }
}

pub(super) async fn assistant_workspace_put(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<AssistantWorkspacePutRequest>,
) -> Response {
    let context =
        match require_home_launch_token_context(&state.data_dir, &headers, ASSISTANT_CAPSULE_ID) {
            Ok(context) => context,
            Err(err) => return super::home_error_response(err),
        };
    if let Err(err) = validate_workspace_put_request(&request) {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    match save_assistant_workspace(&state.data_dir, &context, request) {
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
            "{localhost_root}/{ASSISTANT_WORKSPACE_RELATIVE_ROOT}"
        )),
    ]
}

fn assistant_workspace_uri(context: &HomeLaunchTokenContext) -> String {
    format!(
        "{}/{ASSISTANT_WORKSPACE_RELATIVE_ROOT}/{ASSISTANT_WORKSPACE_FILE}",
        crate::auth::principal_localhost_root(&context.principal_id)
    )
}

fn assistant_workspace_path(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, &assistant_workspace_uri(context))
        .ok_or_else(|| anyhow::anyhow!("invalid Assistant workspace root"))
}

fn default_assistant_workspace() -> AssistantWorkspace {
    AssistantWorkspace {
        schema: ASSISTANT_WORKSPACE_SCHEMA.to_string(),
        revision: 0,
        sessions: Vec::new(),
        draft: String::new(),
        selected_offer_id: None,
    }
}

fn load_assistant_workspace(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<AssistantWorkspace> {
    let path = assistant_workspace_path(data_dir, context)?;
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_assistant_workspace());
        }
        Err(err) => return Err(err.into()),
    }
    let principal_id = &context.principal_id;
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    let bytes = crate::auth::read_principal_root_object(
        data_dir,
        principal_id,
        &localhost_root,
        &assistant_workspace_uri(context),
        &path,
    )?;
    validate_workspace_bytes(&bytes)?;
    let workspace: AssistantWorkspace = serde_json::from_slice(&bytes)?;
    validate_workspace(&workspace)?;
    Ok(workspace)
}

fn save_assistant_workspace(
    data_dir: &std::path::Path,
    context: &HomeLaunchTokenContext,
    request: AssistantWorkspacePutRequest,
) -> anyhow::Result<AssistantWorkspace> {
    let _guard = assistant_workspace_mutation_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("assistant workspace mutation lock poisoned"))?;
    let current = load_assistant_workspace(data_dir, context)?;
    if request.if_revision != current.revision {
        anyhow::bail!("assistant workspace revision conflict");
    }
    let next = AssistantWorkspace {
        schema: request.schema,
        revision: current
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("assistant workspace revision overflow"))?,
        sessions: request.sessions,
        draft: request.draft,
        selected_offer_id: request.selected_offer_id,
    };
    validate_workspace(&next)?;
    let bytes = serde_json::to_vec_pretty(&next)?;
    validate_workspace_bytes(&bytes)?;
    let path = assistant_workspace_path(data_dir, context)?;
    let principal_id = &context.principal_id;
    let localhost_root = crate::auth::principal_localhost_root(principal_id);
    crate::auth::write_protected_principal_root_object(
        data_dir,
        principal_id,
        &localhost_root,
        &assistant_workspace_uri(context),
        &path,
        &bytes,
    )?;
    Ok(next)
}

fn validate_workspace_put_request(request: &AssistantWorkspacePutRequest) -> anyhow::Result<()> {
    validate_workspace(&AssistantWorkspace {
        schema: request.schema.clone(),
        revision: request.if_revision,
        sessions: request.sessions.clone(),
        draft: request.draft.clone(),
        selected_offer_id: request.selected_offer_id.clone(),
    })
}

fn validate_workspace(workspace: &AssistantWorkspace) -> anyhow::Result<()> {
    if workspace.schema != ASSISTANT_WORKSPACE_SCHEMA {
        anyhow::bail!("assistant workspace schema must be {ASSISTANT_WORKSPACE_SCHEMA}");
    }
    if workspace.sessions.len() > ASSISTANT_WORKSPACE_MAX_SESSIONS {
        anyhow::bail!(
            "assistant workspace exceeds {} sessions",
            ASSISTANT_WORKSPACE_MAX_SESSIONS
        );
    }
    validate_optional_offer_id(workspace.selected_offer_id.as_deref())?;
    validate_bounded_text(
        &workspace.draft,
        "assistant workspace draft",
        ASSISTANT_WORKSPACE_MAX_DRAFT_BYTES,
    )?;
    let mut session_ids = std::collections::BTreeSet::new();
    for session in &workspace.sessions {
        validate_trimmed_bounded_text(
            &session.id,
            "assistant workspace session id",
            ASSISTANT_WORKSPACE_MAX_SESSION_ID_BYTES,
        )?;
        if !session_ids.insert(session.id.clone()) {
            anyhow::bail!("assistant workspace session ids must be unique");
        }
        validate_bounded_text(
            &session.title,
            "assistant workspace session title",
            ASSISTANT_WORKSPACE_MAX_SESSION_TITLE_BYTES,
        )?;
        validate_enum_name(
            &session.mode,
            "assistant workspace session mode",
            ASSISTANT_WORKSPACE_MAX_MODE_BYTES,
        )?;
        if session.messages.len() > ASSISTANT_WORKSPACE_MAX_MESSAGES_PER_SESSION {
            anyhow::bail!(
                "assistant workspace session exceeds {} messages",
                ASSISTANT_WORKSPACE_MAX_MESSAGES_PER_SESSION
            );
        }
        for message in &session.messages {
            validate_enum_name(
                &message.role,
                "assistant workspace message role",
                ASSISTANT_WORKSPACE_MAX_ROLE_BYTES,
            )?;
            validate_bounded_text(
                &message.content,
                "assistant workspace message content",
                ASSISTANT_WORKSPACE_MAX_MESSAGE_CONTENT_BYTES,
            )?;
            if let Some(run_id) = &message.run_id {
                validate_run_id(run_id).map_err(|err| anyhow::anyhow!(err.to_string()))?;
            }
        }
    }
    Ok(())
}

fn validate_workspace_bytes(bytes: &[u8]) -> anyhow::Result<()> {
    if bytes.len() > ASSISTANT_WORKSPACE_MAX_BYTES {
        anyhow::bail!(
            "assistant workspace exceeds {} bytes",
            ASSISTANT_WORKSPACE_MAX_BYTES
        );
    }
    Ok(())
}

fn validate_optional_offer_id(offer_id: Option<&str>) -> anyhow::Result<()> {
    let Some(offer_id) = offer_id else {
        return Ok(());
    };
    validate_trimmed_bounded_text(
        offer_id,
        "assistant workspace selected offer id",
        ASSISTANT_WORKSPACE_MAX_OFFER_ID_BYTES,
    )
}

fn validate_trimmed_bounded_text(value: &str, label: &str, max_bytes: usize) -> anyhow::Result<()> {
    if value.trim().is_empty() || value.trim() != value {
        anyhow::bail!("{label} must be a trimmed non-empty string");
    }
    validate_bounded_text(value, label, max_bytes)
}

fn validate_bounded_text(value: &str, label: &str, max_bytes: usize) -> anyhow::Result<()> {
    if value.len() > max_bytes {
        anyhow::bail!("{label} exceeds {max_bytes} bytes");
    }
    Ok(())
}

fn validate_enum_name<T>(value: &T, label: &str, max_bytes: usize) -> anyhow::Result<()>
where
    T: Serialize,
{
    let encoded = serde_json::to_string(value)?;
    let Some(name) = encoded
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        anyhow::bail!("{label} must serialize as a string");
    };
    validate_trimmed_bounded_text(name, label, max_bytes)
}

fn is_revision_conflict(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string() == "assistant workspace revision conflict")
}

fn assistant_workspace_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
