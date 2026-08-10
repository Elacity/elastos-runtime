//! Principal-scoped grants for model-provider runs (P4 / deny-by-default).
//!
//! `gateway_provider_proxy` fails closed on `model/runs_create` unless the
//! calling principal holds a grant for `elastos://model/<offer>/runs_create`.
//! This module is the *seam*; the backing store here is a minimal per-principal
//! JSON file. When the runtime's grant model lands (CapabilityManager-backed),
//! only this module's internals change — the proxy call-site and the typed
//! denial stay identical.

use std::path::Path as FsPath;

const GRANTS_SCHEMA: &str = "elastos.model.grants/v1";

fn grants_path(data_dir: &FsPath, principal_id: &str) -> std::path::PathBuf {
    // Principal ids are hex/opaque ids; keep them as the filename verbatim so a
    // grant file is unambiguous and never traverses the grants dir.
    data_dir
        .join("grants")
        .join(format!("{principal_id}.json"))
}

/// True iff `principal_id` holds a grant covering `resource` exactly.
/// Fail-closed: any read/parse problem, or a missing entry, is `false`.
pub(in crate::api::gateway) fn has_grant(
    data_dir: &FsPath,
    principal_id: &str,
    resource: &str,
) -> bool {
    let path = grants_path(data_dir, principal_id);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value
        .get("grants")
        .and_then(|g| g.as_array())
        .map(|grants| {
            grants
                .iter()
                .any(|g| g.as_str().map(|s| s == resource).unwrap_or(false))
        })
        .unwrap_or(false)
}

/// Record a grant for `principal_id` on `resource`. Dev/dogfood seeding path —
/// the operator-facing consent UX is an open question pending the runtime grant
/// model. Atomic write so a partial file never reads as a valid store.
pub(in crate::api::gateway) fn record_grant(
    data_dir: &FsPath,
    principal_id: &str,
    resource: &str,
) -> anyhow::Result<()> {
    let dir = data_dir.join("grants");
    std::fs::create_dir_all(&dir)?;
    let path = grants_path(data_dir, principal_id);

    let mut grants: Vec<String> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v.get("grants").and_then(|g| g.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !grants.iter().any(|g| g == resource) {
        grants.push(resource.to_string());
    }

    let doc = serde_json::json!({
        "schema": GRANTS_SCHEMA,
        "principal_id": principal_id,
        "grants": grants,
    });

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp = path.with_file_name(format!(
        ".{principal_id}.{}.{}.tmp",
        std::process::id(),
        unique
    ));
    let result = (|| -> anyhow::Result<()> {
        std::fs::write(&temp, serde_json::to_vec_pretty(&doc)?)?;
        std::fs::rename(&temp, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}
