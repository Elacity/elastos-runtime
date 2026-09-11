//! Optional, read-only inspection of the acquired Engine document. Versioned
//! discovery and requests do not grant script execution or a writer lease.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const BROWSER_INSPECTION_REQUEST_SCHEMA: &str = "elastos.browser.inspect-request/v1";
pub const BROWSER_INSPECTION_MAX_RESPONSE_BYTES: usize = 32768;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserInspectionRequest {
    pub schema: String,
    #[serde(default = "default_limit")]
    pub limit: u16,
    #[serde(default)]
    pub cursor: Option<String>,
}

fn default_limit() -> u16 {
    64
}
fn id_valid(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn cursor_valid(cursor: &str) -> bool {
    cursor.split_once(':').is_some_and(|(id, index)| {
        id_valid(id)
            && !index.is_empty()
            && index.len() <= 3
            && index.bytes().all(|b| b.is_ascii_digit())
            && index.parse::<usize>().is_ok_and(|n| n < 512)
    })
}

impl BrowserInspectionRequest {
    pub fn validate(&self) -> Result<(), BrowserInspectionError> {
        if self.schema != BROWSER_INSPECTION_REQUEST_SCHEMA {
            return Err(BrowserInspectionError::Unsupported);
        }
        if !(1..=64).contains(&self.limit)
            || self.cursor.as_deref().is_some_and(|s| !cursor_valid(s))
        {
            return Err(BrowserInspectionError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InspectionCapabilities {
    schema: String,
    page_id: String,
    formats: Vec<String>,
    max_nodes: usize,
    max_page_nodes: usize,
    max_snapshot_bytes: usize,
    max_response_bytes: usize,
    snapshot_ttl_ms: usize,
    timeout_ms: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InspectionNode {
    #[serde(rename = "ref")]
    reference: String,
    role: String,
    name: String,
    description: String,
    value: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InspectionResult {
    schema: String,
    page_id: String,
    document_generation: String,
    snapshot_id: String,
    nodes: Vec<InspectionNode>,
    next_cursor: Option<String>,
    truncated: bool,
}

/// Re-encode only the declared projection. Private provider metadata is never
/// passed through as document content, including on capability discovery.
pub fn validate_browser_inspection_result(
    page_id: &str,
    request: Option<&BrowserInspectionRequest>,
    value: Value,
) -> Result<Value, BrowserInspectionError> {
    let invalid = BrowserInspectionError::Failed;
    if serde_json::to_vec(&value).map_err(|_| invalid)?.len()
        > BROWSER_INSPECTION_MAX_RESPONSE_BYTES
    {
        return Err(invalid);
    }
    if let Some(request) = request {
        request.validate()?;
        let result: InspectionResult = serde_json::from_value(value).map_err(|_| invalid)?;
        let offset = request
            .cursor
            .as_deref()
            .and_then(|c| c.split_once(':'))
            .map(|(_, n)| n.parse::<usize>().unwrap_or(512))
            .unwrap_or(0);
        if result.schema != "elastos.browser.inspect-result/v1"
            || result.page_id != page_id
            || !id_valid(&result.document_generation)
            || !id_valid(&result.snapshot_id)
            || result.nodes.len() > usize::from(request.limit)
            || offset + result.nodes.len() > 512
            || request
                .cursor
                .as_deref()
                .is_some_and(|c| c.split_once(':').unwrap().0 != result.snapshot_id)
            || result.next_cursor.as_deref().is_some_and(|c| {
                !cursor_valid(c)
                    || c != format!("{}:{}", result.snapshot_id, offset + result.nodes.len())
                    || result.nodes.is_empty()
            })
        {
            return Err(invalid);
        }
        for (index, node) in result.nodes.iter().enumerate() {
            if node.reference != format!("{}:{}", result.snapshot_id, offset + index)
                || [&node.role, &node.name, &node.description]
                    .into_iter()
                    .any(|s| s.len() > 1024)
                || node.value.as_ref().is_some_and(|s| s.len() > 1024)
            {
                return Err(invalid);
            }
        }
        serde_json::to_value(result).map_err(|_| invalid)
    } else {
        let result: InspectionCapabilities = serde_json::from_value(value).map_err(|_| invalid)?;
        if result.schema != "elastos.browser.inspect-capabilities/v1"
            || result.page_id != page_id
            || result.formats != ["accessibility_tree"]
            || result.max_nodes != 512
            || result.max_page_nodes != 64
            || result.max_snapshot_bytes != 131072
            || result.max_response_bytes != 32768
            || result.snapshot_ttl_ms != 30000
            || result.timeout_ms != 1500
        {
            return Err(BrowserInspectionError::Unsupported);
        }
        serde_json::to_value(result).map_err(|_| invalid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserInspectionError {
    Invalid,
    Unsupported,
    Stale,
    Busy,
    OwnerChanged,
    Failed,
}
impl BrowserInspectionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_inspection",
            Self::Unsupported => "inspection_unsupported",
            Self::Stale => "stale_inspection",
            Self::Busy => "inspection_busy",
            Self::OwnerChanged => "inspection_owner_changed",
            Self::Failed => "inspection_failed",
        }
    }
    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Invalid,
            Self::Unsupported,
            Self::Stale,
            Self::Busy,
            Self::OwnerChanged,
            Self::Failed,
        ]
        .into_iter()
        .find(|value| value.code() == code)
    }
    pub fn http_status(self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::Unsupported => 501,
            Self::Stale | Self::Busy | Self::OwnerChanged => 409,
            Self::Failed => 503,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> BrowserInspectionRequest {
        BrowserInspectionRequest {
            schema: BROWSER_INSPECTION_REQUEST_SCHEMA.into(),
            limit: 1,
            cursor: None,
        }
    }
    fn result() -> Value {
        json!({"schema":"elastos.browser.inspect-result/v1", "page_id":"page:test",
            "document_generation":"a".repeat(32), "snapshot_id":"b".repeat(32),
            "nodes":[{"ref":format!("{}:0", "b".repeat(32)),"role":"button","name":"Continue","description":"","value":null}],
            "next_cursor":format!("{}:1", "b".repeat(32)),"truncated":false})
    }
    #[test]
    fn inspection_rejects_undeclared_authority_versions_and_bounds() {
        let mut value = serde_json::to_value(request()).unwrap();
        value["principal_id"] = json!("foreign");
        assert!(serde_json::from_value::<BrowserInspectionRequest>(value).is_err());
        let mut req = request();
        req.schema = "future".into();
        assert!(req.validate().is_err());
        req = request();
        req.limit = 65;
        assert!(req.validate().is_err());
        req = request();
        req.cursor = Some("unscoped".into());
        assert!(req.validate().is_err());
    }
    #[test]
    fn inspection_validates_exact_page_cursor_nodes_and_public_shape() {
        assert!(
            validate_browser_inspection_result("page:test", Some(&request()), result()).is_ok()
        );
        for field in [
            "page_id",
            "document_generation",
            "snapshot_id",
            "next_cursor",
        ] {
            let mut value = result();
            value[field] = json!("foreign");
            assert!(
                validate_browser_inspection_result("page:test", Some(&request()), value).is_err(),
                "{field}"
            );
        }
        let mut value = result();
        value["private_socket"] = json!("private");
        assert!(validate_browser_inspection_result("page:test", Some(&request()), value).is_err());
        let mut value = result();
        value["nodes"][0]["name"] = json!("x".repeat(32768));
        assert!(validate_browser_inspection_result("page:test", Some(&request()), value).is_err());
    }
}
