//! Shared outbound-network and path-safety validators.
//!
//! These enforce security invariants (SSRF egress policy, HTTP-header
//! CRLF-injection guard, and content path-traversal guard) that were previously
//! copy-pasted byte-for-byte across `content.rs` and `carrier.rs`. Keeping a
//! single implementation prevents silent security drift, where tightening one
//! copy leaves the other on the weaker rule. Call sites pass a `label` so the
//! human-readable error prefix stays specific to each surface.

/// Reject an outbound endpoint URL that violates the SSRF egress policy:
/// inline credentials are forbidden, and only `https` or loopback `http`
/// (127.0.0.1 / localhost / ::1) is permitted. `label` names the surface for
/// the error message (e.g. `"carrier external endpoint"`).
pub(crate) fn validate_outbound_endpoint_url(raw: &str, label: &str) -> Result<(), String> {
    let url = url::Url::parse(raw).map_err(|err| format!("invalid {label} URL: {err}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} URL must not contain inline credentials"));
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => Ok(()),
        _ => Err(format!("{label} URL must use https or local loopback http")),
    }
}

/// Reject an HTTP header value carrying a CR or LF (header-injection guard).
/// `label` names the header for the error message (e.g.
/// `"carrier authorization header"`).
pub(crate) fn validate_outbound_header_value(value: &str, label: &str) -> Result<(), String> {
    if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(format!("{label} contains invalid newline"));
    }
    Ok(())
}

/// Reject a content fetch path that could escape its intended root: it must be
/// relative, contain no backslash or NUL, and have no empty / `.` / `..`
/// segments. An empty path is allowed (means "the root itself").
pub(crate) fn validate_content_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("content fetch path must be relative".to_string());
    }
    if path.contains('\\') || path.contains('\0') {
        return Err("content fetch path contains invalid characters".to_string());
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err("content fetch path contains an invalid segment".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_url_rejects_credentials_and_non_loopback_http() {
        assert!(validate_outbound_endpoint_url(
            "https://example.com/x",
            "carrier external endpoint"
        )
        .is_ok());
        assert!(validate_outbound_endpoint_url(
            "http://127.0.0.1:8080/x",
            "carrier external endpoint"
        )
        .is_ok());
        assert!(
            validate_outbound_endpoint_url("http://localhost/x", "carrier external endpoint")
                .is_ok()
        );
        // Inline credentials forbidden.
        assert_eq!(
            validate_outbound_endpoint_url("https://user:pass@example.com/x", "operator alert"),
            Err("operator alert URL must not contain inline credentials".to_string())
        );
        // Non-loopback http forbidden.
        assert_eq!(
            validate_outbound_endpoint_url("http://example.com/x", "operator alert"),
            Err("operator alert URL must use https or local loopback http".to_string())
        );
    }

    #[test]
    fn outbound_header_rejects_crlf() {
        assert!(
            validate_outbound_header_value("Bearer abc", "carrier authorization header").is_ok()
        );
        assert_eq!(
            validate_outbound_header_value("a\r\nInjected: 1", "carrier authorization header"),
            Err("carrier authorization header contains invalid newline".to_string())
        );
        assert!(
            validate_outbound_header_value("x\ny", "operator alert authorization header").is_err()
        );
    }

    #[test]
    fn content_path_rejects_traversal() {
        assert!(validate_content_path("").is_ok());
        assert!(validate_content_path("a/b/c.txt").is_ok());
        assert!(validate_content_path("/abs").is_err());
        assert!(validate_content_path("a/../b").is_err());
        assert!(validate_content_path("a/./b").is_err());
        assert!(validate_content_path("a\\b").is_err());
        assert!(validate_content_path("a\0b").is_err());
        assert!(validate_content_path("a//b").is_err());
    }
}
