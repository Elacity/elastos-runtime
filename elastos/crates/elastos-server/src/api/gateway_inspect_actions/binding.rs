use sha2::Sha256;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

pub(super) fn inspect_action_request_binding(request: &serde_json::Value) -> serde_json::Value {
    let canonical = canonical_json(request);
    let encoded = serde_json::to_vec(&canonical).unwrap_or_default();
    let sha256 = hex::encode(<Sha256 as sha2::Digest>::digest(&encoded));
    let preview = if encoded.len() <= 1024 {
        canonical
    } else {
        serde_json::Value::Null
    };
    serde_json::json!({
        "schema": "elastos.inspect.request-binding/v1",
        "sha256": sha256,
        "bytes": encoded.len(),
        "truncated": encoded.len() > 1024,
        "preview": preview,
    })
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        other => other.clone(),
    }
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
