use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use super::binding::inspect_action_request_binding;
use crate::esp_binding::EspRequestBinding;

pub(super) const INSPECT_ACTION_SCHEMA: &str = "elastos.inspect.action-request/v1";
const INSPECT_ACTIONS_DIR: &str = "inspect-actions";
static INSPECT_ACTION_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct InspectActionRequestRecord {
    pub(super) schema: String,
    pub(super) request_id: String,
    pub(super) principal_id: String,
    pub(super) session_id: String,
    pub(super) id: String,
    pub(super) operation: String,
    pub(super) request: serde_json::Value,
    pub(super) plan: serde_json::Value,
    #[serde(default)]
    pub(super) request_binding: Option<EspRequestBinding>,
    pub(super) status: String,
    pub(super) created_at: u64,
    pub(super) updated_at: u64,
    #[serde(default)]
    pub(super) result: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) error: Option<String>,
}

pub(super) fn pending_inspect_action_requests(
    data_dir: &Path,
    principal_id: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut records = load_inspect_action_records(data_dir)?;
    records.sort_by_key(|record| record.created_at);
    Ok(records
        .into_iter()
        .filter(|record| record.status == "pending" && record.principal_id == principal_id)
        .map(|record| {
            serde_json::json!({
                "schema": record.schema,
                "request_id": record.request_id,
                "id": record.id,
                "operation": record.operation,
                "plan": record.plan,
                "request_binding": record.request_binding.unwrap_or_else(|| inspect_action_request_binding(
                    &record.request_id,
                    &record.principal_id,
                    &record.id,
                    &record.operation,
                    &record.plan,
                    &record.request,
                )),
                "status": record.status,
                "created_at": record.created_at,
            })
        })
        .collect())
}

pub(super) fn read_inspect_action_record(
    data_dir: &Path,
    request_id: &str,
) -> anyhow::Result<InspectActionRequestRecord> {
    let _guard = inspect_action_store_lock()?;
    read_inspect_action_record_unlocked(data_dir, request_id)
}

pub(super) fn claim_pending_inspect_action_record(
    data_dir: &Path,
    request_id: &str,
    principal_id: &str,
    claim_status: &str,
    updated_at: u64,
) -> anyhow::Result<InspectActionRequestRecord> {
    let _guard = inspect_action_store_lock()?;
    let mut record = read_inspect_action_record_unlocked(data_dir, request_id)?;
    if record.status != "pending" {
        anyhow::bail!("Inspector action request is not pending");
    }
    if record.principal_id != principal_id {
        anyhow::bail!("Inspector action request belongs to a different principal");
    }
    record.status = claim_status.to_string();
    record.updated_at = updated_at;
    write_inspect_action_record_unlocked(data_dir, &record)?;
    Ok(record)
}

fn read_inspect_action_record_unlocked(
    data_dir: &Path,
    request_id: &str,
) -> anyhow::Result<InspectActionRequestRecord> {
    let path = inspect_action_path(data_dir, request_id)?;
    let data = std::fs::read_to_string(path)?;
    let record: InspectActionRequestRecord = serde_json::from_str(&data)?;
    Ok(record)
}

pub(super) fn write_inspect_action_record(
    data_dir: &Path,
    record: &InspectActionRequestRecord,
) -> anyhow::Result<()> {
    let _guard = inspect_action_store_lock()?;
    write_inspect_action_record_unlocked(data_dir, record)
}

fn write_inspect_action_record_unlocked(
    data_dir: &Path,
    record: &InspectActionRequestRecord,
) -> anyhow::Result<()> {
    let root = inspect_actions_root(data_dir);
    std::fs::create_dir_all(&root)?;
    let path = inspect_action_path(data_dir, &record.request_id)?;
    let data = serde_json::to_vec_pretty(record)?;
    let mut pending = tempfile::NamedTempFile::new_in(&root)?;
    std::io::Write::write_all(&mut pending, &data)?;
    pending.as_file().sync_all()?;
    pending.persist(path)?;
    Ok(())
}

fn inspect_action_store_lock() -> anyhow::Result<std::sync::MutexGuard<'static, ()>> {
    INSPECT_ACTION_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Inspector action store lock is poisoned"))
}

fn inspect_actions_root(data_dir: &Path) -> PathBuf {
    data_dir.join(INSPECT_ACTIONS_DIR)
}

fn inspect_action_path(data_dir: &Path, request_id: &str) -> anyhow::Result<PathBuf> {
    if !request_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        anyhow::bail!("invalid Inspector action request id");
    }
    Ok(inspect_actions_root(data_dir).join(format!("{request_id}.json")))
}

fn load_inspect_action_records(data_dir: &Path) -> anyhow::Result<Vec<InspectActionRequestRecord>> {
    let root = inspect_actions_root(data_dir);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(Vec::new());
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let data = std::fs::read_to_string(path)?;
        let record: InspectActionRequestRecord = serde_json::from_str(&data)?;
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    fn pending_record(request_id: &str) -> InspectActionRequestRecord {
        InspectActionRequestRecord {
            schema: INSPECT_ACTION_SCHEMA.to_string(),
            request_id: request_id.to_string(),
            principal_id: "person:test".to_string(),
            session_id: "session:test".to_string(),
            id: "capsule:test".to_string(),
            operation: "write".to_string(),
            request: serde_json::json!({}),
            plan: serde_json::json!({}),
            request_binding: None,
            status: "pending".to_string(),
            created_at: 1,
            updated_at: 1,
            result: None,
            error: None,
        }
    }

    #[test]
    fn only_one_caller_can_claim_a_pending_action() {
        let dir = tempfile::tempdir().unwrap();
        let request_id = "inspect-action-test";
        write_inspect_action_record(dir.path(), &pending_record(request_id)).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let handles = (0..2)
            .map(|_| {
                let data_dir = dir.path().to_path_buf();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    claim_pending_inspect_action_record(
                        &data_dir,
                        request_id,
                        "person:test",
                        "approving",
                        2,
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let outcomes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_err()).count(),
            1
        );
        assert_eq!(
            read_inspect_action_record(dir.path(), request_id)
                .unwrap()
                .status,
            "approving"
        );
    }
}
