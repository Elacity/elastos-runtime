use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::esp_binding::{esp_request_binding, EspRequestBinding};
use sha2::Sha256;

static INSPECT_ACTION_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(super) fn inspect_action_request_nonce() -> String {
    let sequence = INSPECT_ACTION_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{nanos}-{sequence}")
}

pub(super) fn inspect_action_request_id(
    id: &str,
    operation: &str,
    now: u64,
    nonce: &str,
) -> String {
    let digest =
        <Sha256 as sha2::Digest>::digest(format!("{id}:{operation}:{now}:{nonce}").as_bytes());
    format!("inspect-act-{now}-{}", &hex::encode(digest)[..16])
}

pub(super) fn inspect_action_request_binding(
    request_id: &str,
    principal: &str,
    capsule: &str,
    operation: &str,
    plan: &serde_json::Value,
    request: &serde_json::Value,
) -> EspRequestBinding {
    esp_request_binding(
        request_id,
        principal,
        capsule,
        None,
        operation,
        inspect_action_resources(plan),
        request,
    )
}

fn inspect_action_resources(plan: &serde_json::Value) -> Vec<String> {
    plan.get("capabilities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|capability| capability.get("resource"))
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_action_request_id_uses_nonce() {
        let first = inspect_action_request_id("capsule:exit-provider", "status", 42, "0");
        let second = inspect_action_request_id("capsule:exit-provider", "status", 42, "1");

        assert_ne!(first, second);
        assert!(first.starts_with("inspect-act-42-"));
        assert!(second.starts_with("inspect-act-42-"));
    }
}
