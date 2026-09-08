//! Bounded operator requests. Authentication and writer admission stay in Runtime.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserRefAction {
    Click,
    Type,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserOperatorRequest {
    pub schema: String,
    pub document_generation: String,
    pub actions: Vec<BrowserRefAction>,
    pub duration_ms: u32,
    pub max_actions: u8,
    pub reason: String,
}

pub fn browser_operator_id_valid(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BrowserOperatorRequest {
    pub fn valid(&self) -> bool {
        self.schema == "elastos.browser.operator-request/v1"
            && browser_operator_id_valid(&self.document_generation)
            && (2000..=30000).contains(&self.duration_ms)
            && (1..=16).contains(&self.max_actions)
            && (1..=2).contains(&self.actions.len())
            && (self.actions.len() != 2 || self.actions[0] != self.actions[1])
            && !self.reason.trim().is_empty()
            && self.reason.len() <= 240
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserRefInput {
    pub schema: String,
    pub request_id: String,
    pub admission_id: String,
    pub document_generation: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub action: BrowserRefAction,
    #[serde(default)]
    pub text: Option<String>,
}

impl BrowserRefInput {
    pub fn valid(&self) -> bool {
        self.schema == "elastos.browser.ref-input/v1"
            && browser_operator_id_valid(&self.request_id)
            && !self.admission_id.is_empty()
            && self.admission_id.len() <= 64
            && browser_operator_id_valid(&self.document_generation)
            && self
                .reference
                .split_once(':')
                .is_some_and(|(snapshot, index)| {
                    browser_operator_id_valid(snapshot)
                        && !index.is_empty()
                        && index.len() <= 3
                        && index.bytes().all(|b| b.is_ascii_digit())
                        && index
                            .parse::<usize>()
                            .is_ok_and(|n| n < 512 && index == n.to_string())
                })
            && match self.action {
                BrowserRefAction::Click => self.text.is_none(),
                BrowserRefAction::Type => self.text.as_ref().is_some_and(|s| {
                    !s.is_empty() && s.len() <= 1024 && !s.chars().any(char::is_control)
                }),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn operator_requests_bound_actions_duration_quota_and_unknown_authority() {
        let value = json!({"schema":"elastos.browser.operator-request/v1", "document_generation":"a".repeat(32),
            "actions":["click","type"],"duration_ms":30000,"max_actions":16,"reason":"Complete this form"});
        assert!(
            serde_json::from_value::<BrowserOperatorRequest>(value.clone())
                .unwrap()
                .valid()
        );
        for (field, invalid) in [
            ("actions", json!(["click", "click"])),
            ("duration_ms", json!(30001)),
            ("max_actions", json!(0)),
            ("document_generation", json!("old")),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(!serde_json::from_value::<BrowserOperatorRequest>(changed)
                .unwrap()
                .valid());
        }
        let mut changed = value;
        changed["principal_id"] = json!("claimed-owner");
        assert!(serde_json::from_value::<BrowserOperatorRequest>(changed).is_err());
    }

    #[test]
    fn operator_ref_input_is_typed_bounded_and_canonical() {
        let value = json!({"schema":"elastos.browser.ref-input/v1", "request_id":"c".repeat(32),
            "admission_id":"admission", "document_generation":"a".repeat(32),
            "ref":format!("{}:0","b".repeat(32)),"action":"type","text":"Hello"});
        assert!(serde_json::from_value::<BrowserRefInput>(value.clone())
            .unwrap()
            .valid());
        for (field, invalid) in [
            ("text", json!("x".repeat(1025))),
            ("text", json!("\n")),
            ("ref", json!(format!("{}:00", "b".repeat(32)))),
            ("action", json!("click")),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(!serde_json::from_value::<BrowserRefInput>(changed)
                .unwrap()
                .valid());
        }
    }
}

/// The provider-facing projection shares the public ref payload. Runtime alone
/// supplies lease commands after the owner has approved the authenticated session.
pub fn browser_operator_event_valid(value: &serde_json::Value) -> bool {
    let Some(mut object) = value.as_object().cloned() else {
        return false;
    };
    match object
        .remove("type")
        .and_then(|v| v.as_str().map(str::to_owned))
        .as_deref()
    {
        Some("operator_ref") => {
            serde_json::from_value::<BrowserRefInput>(serde_json::Value::Object(object))
                .is_ok_and(|input| input.valid())
        }
        Some("operator_lease") => {
            let id = object
                .get("admission_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if !(32..=36).contains(&id.len())
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
            {
                return false;
            }
            match object.get("command").and_then(|v| v.as_str()) {
                Some("release") => object.len() == 2,
                Some("acquire") => {
                    object.len() == 5
                        && object
                            .get("document_generation")
                            .and_then(|v| v.as_str())
                            .is_some_and(browser_operator_id_valid)
                        && object
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .is_some_and(|n| (2000..=30000).contains(&n))
                        && object
                            .get("actions")
                            .and_then(|v| v.as_array())
                            .is_some_and(|actions| {
                                (1..=2).contains(&actions.len())
                                    && (actions.len() != 2 || actions[0] != actions[1])
                                    && actions.iter().all(|action| {
                                        matches!(action.as_str(), Some("click" | "type"))
                                    })
                            })
                }
                _ => false,
            }
        }
        _ => false,
    }
}
