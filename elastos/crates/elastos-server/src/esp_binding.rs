use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const ESP_REQUEST_BINDING_SCHEMA: &str = "elastos.esp.request-binding/v1";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EspRequestBinding {
    pub schema: String,
    pub request_id: String,
    pub principal: String,
    pub capsule: String,
    pub interface: Option<String>,
    pub method: String,
    pub resources: Vec<String>,
    pub sha256: String,
    pub bytes: usize,
    pub truncated: bool,
    pub preview: serde_json::Value,
}

pub fn esp_request_binding(
    request_id: &str,
    principal: &str,
    capsule: &str,
    interface: Option<&str>,
    method: &str,
    resources: impl IntoIterator<Item = String>,
    body: &serde_json::Value,
) -> EspRequestBinding {
    let canonical = canonical_json(body);
    let encoded = serde_json::to_vec(&canonical).expect("JSON values always serialize");
    let mut resources = resources
        .into_iter()
        .map(|resource| resource.trim().to_string())
        .filter(|resource| !resource.is_empty())
        .collect::<Vec<_>>();
    resources.sort();
    resources.dedup();
    let truncated = encoded.len() > 1024;

    EspRequestBinding {
        schema: ESP_REQUEST_BINDING_SCHEMA.to_string(),
        request_id: request_id.trim().to_string(),
        principal: principal.trim().to_string(),
        capsule: capsule.trim().to_string(),
        interface: interface
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        method: method.trim().to_string(),
        resources,
        sha256: hex::encode(Sha256::digest(&encoded)),
        bytes: encoded.len(),
        truncated,
        preview: if truncated {
            serde_json::Value::Null
        } else {
            canonical
        },
    }
}

fn canonical_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => {
            let ordered = object
                .iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_binding_is_canonical_and_exact() {
        let first = esp_request_binding(
            "req-1",
            "person:1",
            "marketplace",
            Some("elastos.marketplace.catalog"),
            "capsule.open",
            ["elastos://capsules/*".to_string()],
            &serde_json::json!({ "target": "browser", "options": { "b": 2, "a": 1 } }),
        );
        let second = esp_request_binding(
            "req-1",
            "person:1",
            "marketplace",
            Some("elastos.marketplace.catalog"),
            "capsule.open",
            ["elastos://capsules/*".to_string()],
            &serde_json::json!({ "options": { "a": 1, "b": 2 }, "target": "browser" }),
        );

        assert_eq!(first, second);
        assert_eq!(first.schema, ESP_REQUEST_BINDING_SCHEMA);
        assert_eq!(first.sha256.len(), 64);
    }
}
