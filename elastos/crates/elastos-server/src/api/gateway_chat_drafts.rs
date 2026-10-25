//! Chat draft persistence. Runtime retains unsent drafts per conversation
//! (Community and direct conversations) in a principal-protected object.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

pub(super) const MAX_BYTES: usize = 512 * 1024;
const MAX_DEPTH: usize = 16;
const SCHEMA: &str = "elastos.chat.drafts/v1";
const RELATIVE_PATH: &str = ".AppData/ElastOS/Chat/drafts.json";
const CONFLICT: &str = "chat_drafts_revision_conflict";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDrafts {
    schema: String,
    revision: u64,
    /// Map from conversation ID (or "community" for the shared room) to draft text.
    /// An absent key means no draft for that conversation.
    drafts: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PutRequest {
    schema: String,
    if_revision: u64,
    drafts: BTreeMap<String, String>,
}

pub(super) fn mutation_guard() -> anyhow::Result<MutexGuard<'static, ()>> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Chat drafts mutation lock poisoned"))
}

pub(super) fn principal_root_protected_object_inventory(
    localhost_root: &str,
) -> Vec<crate::auth::PrincipalRootProtectedObjectDeclarationV1> {
    vec![
        crate::auth::PrincipalRootProtectedObjectDeclarationV1::root(format!(
            "{localhost_root}/.AppData/ElastOS/Chat"
        )),
    ]
}

pub(super) async fn get(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let context = match require_home_launch_token_context(&state.data_dir, &headers, "chat-room") {
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
    let context = match require_home_launch_token_context(&state.data_dir, &headers, "chat-room") {
        Ok(context) => context,
        Err(error) => return home_error_response(error),
    };
    if let Err(error) = validate_drafts(&request.schema, &request.drafts) {
        return (StatusCode::BAD_REQUEST, error.to_string()).into_response();
    }
    match save(&state.data_dir, &context, request) {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            if error.chain().any(|cause| cause.to_string() == CONFLICT) {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "schema": "elastos.chat.drafts.error/v1",
                        "code": CONFLICT,
                        "message": "The saved drafts changed. Reload before saving."
                    })),
                )
                    .into_response();
            }
            home_error_response(error)
        }
    }
}

fn object_location(data_dir: &Path, principal_id: &str) -> anyhow::Result<(String, PathBuf)> {
    let root = crate::auth::principal_localhost_root(principal_id);
    let uri = format!("{root}/{RELATIVE_PATH}");
    let path = rooted_localhost_fs_path(data_dir, &uri)
        .ok_or_else(|| anyhow::anyhow!("invalid Chat drafts object root"))?;
    Ok((uri, path))
}

fn load(data_dir: &Path, context: &HomeLaunchTokenContext) -> anyhow::Result<Option<StoredDrafts>> {
    let (uri, path) = object_location(data_dir, &context.principal_id)?;
    let bytes = match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {
            let root = crate::auth::principal_localhost_root(&context.principal_id);
            crate::auth::read_principal_root_object(
                data_dir,
                &context.principal_id,
                &root,
                &uri,
                &path,
            )?
        }
    };
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Chat drafts object exceeds its byte limit"
    );
    let stored: StoredDrafts = serde_json::from_slice(&bytes)?;
    validate_drafts(&stored.schema, &stored.drafts)?;
    anyhow::ensure!(stored.revision > 0, "invalid Chat drafts revision");
    Ok(Some(stored))
}

fn response(stored: &StoredDrafts) -> Value {
    serde_json::json!({
        "schema": stored.schema,
        "revision": stored.revision,
        "drafts": stored.drafts
    })
}

fn read(data_dir: &Path, context: &HomeLaunchTokenContext) -> anyhow::Result<Value> {
    let _guard = mutation_guard()?;
    if let Some(stored) = load(data_dir, context)? {
        return Ok(response(&stored));
    }
    Ok(serde_json::json!({
        "schema": SCHEMA,
        "revision": 0,
        "drafts": {}
    }))
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
    let stored = StoredDrafts {
        schema: request.schema,
        revision: revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Chat drafts revision overflow"))?,
        drafts: request.drafts,
    };
    let bytes = serde_json::to_vec_pretty(&stored)?;
    anyhow::ensure!(
        bytes.len() <= MAX_BYTES,
        "Chat drafts object exceeds {} bytes",
        MAX_BYTES
    );
    let (uri, path) = object_location(data_dir, &context.principal_id)?;
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

fn validate_drafts(schema: &str, drafts: &BTreeMap<String, String>) -> anyhow::Result<()> {
    anyhow::ensure!(schema == SCHEMA, "Chat drafts schema must be {SCHEMA}");
    // Validate conversation keys: either "community" or non-empty direct IDs
    for key in drafts.keys() {
        anyhow::ensure!(
            key == "community" || !key.is_empty(),
            "Chat draft key must be 'community' or a non-empty conversation ID"
        );
    }
    // Validate depth (simple check for nested structures if stored as JSON)
    let value = serde_json::to_value(drafts)?;
    anyhow::ensure!(
        json_depth(&value) <= MAX_DEPTH,
        "Chat drafts object exceeds depth limit"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn mock_context() -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: "test-principal".to_string(),
            session_id: "test-session".to_string(),
            proof_binding_id: None,
            grant_id: "test-grant".to_string(),
        }
    }

    #[test]
    fn test_validate_drafts_accepts_valid() {
        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "Hello community".to_string());
        drafts.insert("user123".to_string(), "Hi there".to_string());

        assert!(validate_drafts(SCHEMA, &drafts).is_ok());
    }

    #[test]
    fn test_validate_drafts_rejects_wrong_schema() {
        let drafts = BTreeMap::new();
        assert!(validate_drafts("wrong.schema/v1", &drafts).is_err());
    }

    #[test]
    fn test_validate_drafts_rejects_empty_key() {
        let mut drafts = BTreeMap::new();
        drafts.insert("".to_string(), "text".to_string());

        assert!(validate_drafts(SCHEMA, &drafts).is_err());
    }

    #[test]
    fn test_json_depth() {
        let shallow = serde_json::json!({"a": "b"});
        assert_eq!(json_depth(&shallow), 1);

        let nested = serde_json::json!({"a": {"b": {"c": "d"}}});
        assert_eq!(json_depth(&nested), 3);
    }

    #[test]
    fn test_read_creates_empty_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let context = mock_context();

        let result = read(temp_dir.path(), &context).unwrap();
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("schema").unwrap().as_str().unwrap(), SCHEMA);
        assert_eq!(obj.get("revision").unwrap().as_u64().unwrap(), 0);
        assert!(obj.get("drafts").unwrap().as_object().unwrap().is_empty());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let context = mock_context();

        // Set up principal root protection
        crate::auth::store_test_principal_root_protection(temp_dir.path(), &context.principal_id);

        // First save
        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "Test draft".to_string());

        let request = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0,
            drafts: drafts.clone(),
        };

        let result = save(temp_dir.path(), &context, request).unwrap();
        assert_eq!(result.get("revision").unwrap().as_u64().unwrap(), 1);

        // Load and verify
        let loaded = read(temp_dir.path(), &context).unwrap();
        assert_eq!(loaded.get("revision").unwrap().as_u64().unwrap(), 1);
        assert_eq!(
            loaded
                .get("drafts")
                .unwrap()
                .get("community")
                .unwrap()
                .as_str()
                .unwrap(),
            "Test draft"
        );
    }

    #[test]
    fn test_save_detects_revision_conflict() {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let context = mock_context();

        // Set up principal root protection
        crate::auth::store_test_principal_root_protection(temp_dir.path(), &context.principal_id);

        // First save
        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "First".to_string());

        let request1 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0,
            drafts: drafts.clone(),
        };
        save(temp_dir.path(), &context, request1).unwrap();

        // Try to save with stale revision
        drafts.insert("community".to_string(), "Second".to_string());
        let request2 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0, // Stale
            drafts,
        };

        let result = save(temp_dir.path(), &context, request2);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(CONFLICT));
    }

    #[test]
    fn test_save_removes_draft_when_empty_map() {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let context = mock_context();

        // Set up principal root protection
        crate::auth::store_test_principal_root_protection(temp_dir.path(), &context.principal_id);

        // Save with drafts
        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "Text".to_string());

        let request1 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0,
            drafts,
        };
        save(temp_dir.path(), &context, request1).unwrap();

        // Clear drafts
        let request2 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 1,
            drafts: BTreeMap::new(),
        };

        let result = save(temp_dir.path(), &context, request2).unwrap();
        assert!(result
            .get("drafts")
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn test_separate_drafts_per_conversation() {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let context = mock_context();

        // Set up principal root protection
        crate::auth::store_test_principal_root_protection(temp_dir.path(), &context.principal_id);

        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "Community draft".to_string());
        drafts.insert("user1".to_string(), "Direct draft 1".to_string());
        drafts.insert("user2".to_string(), "Direct draft 2".to_string());

        let request = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0,
            drafts,
        };

        save(temp_dir.path(), &context, request).unwrap();

        let loaded = read(temp_dir.path(), &context).unwrap();
        let loaded_drafts = loaded.get("drafts").unwrap().as_object().unwrap();

        assert_eq!(loaded_drafts.len(), 3);
        assert_eq!(
            loaded_drafts.get("community").unwrap().as_str().unwrap(),
            "Community draft"
        );
        assert_eq!(
            loaded_drafts.get("user1").unwrap().as_str().unwrap(),
            "Direct draft 1"
        );
        assert_eq!(
            loaded_drafts.get("user2").unwrap().as_str().unwrap(),
            "Direct draft 2"
        );
    }

    #[test]
    fn test_send_removes_draft() {
        let temp_dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let context = mock_context();

        // Set up principal root protection
        crate::auth::store_test_principal_root_protection(temp_dir.path(), &context.principal_id);

        // Save multiple drafts
        let mut drafts = BTreeMap::new();
        drafts.insert("community".to_string(), "Community".to_string());
        drafts.insert("user1".to_string(), "User 1".to_string());

        let request1 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 0,
            drafts,
        };
        save(temp_dir.path(), &context, request1).unwrap();

        // Simulate send: remove one draft
        let mut drafts = BTreeMap::new();
        drafts.insert("user1".to_string(), "User 1".to_string());

        let request2 = PutRequest {
            schema: SCHEMA.to_string(),
            if_revision: 1,
            drafts,
        };

        let result = save(temp_dir.path(), &context, request2).unwrap();
        let result_drafts = result.get("drafts").unwrap().as_object().unwrap();

        assert_eq!(result_drafts.len(), 1);
        assert!(!result_drafts.contains_key("community"));
        assert!(result_drafts.contains_key("user1"));
    }
}
