//! Browser request and provider receipt validation helpers.

use super::*;

pub(in crate::api::gateway) fn browser_request_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| *value == "http" || *value == "https")
        .unwrap_or("https");
    Some(format!("{proto}://{host}"))
}

pub(in crate::api::gateway) fn browser_url_to_stream_target(
    value: &str,
) -> anyhow::Result<(String, String)> {
    let trimmed = value.trim();
    let candidate = if has_url_scheme(trimmed) {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate)?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        anyhow::bail!("Only http and https addresses can be opened by Browser");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Browser URL must include a host"))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("Browser URL must use a known port"))?;
    let stream_scheme = if parsed.scheme() == "https" {
        "tls"
    } else {
        "tcp"
    };
    Ok((
        parsed.to_string(),
        format!("{stream_scheme}://{host}:{port}"),
    ))
}

fn has_url_scheme(value: &str) -> bool {
    let Some(index) = value.find(':') else {
        return false;
    };
    let scheme = &value[..index];
    let Some(first) = scheme.bytes().next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
}

pub(in crate::api::gateway) fn browser_viewport_value(
    viewport: BrowserViewportRequest,
) -> anyhow::Result<serde_json::Value> {
    if viewport.width < 320
        || viewport.width > 3840
        || viewport.height < 240
        || viewport.height > 2160
    {
        anyhow::bail!("Browser viewport must be within 320x240 and 3840x2160");
    }
    Ok(serde_json::json!({
        "width": viewport.width,
        "height": viewport.height,
    }))
}

pub(in crate::api::gateway) fn browser_webrtc_signal_value(
    input: BrowserWebrtcSignalRequest,
) -> anyhow::Result<serde_json::Value> {
    match input.signal_type.as_str() {
        "offer" => {
            if input.candidate.is_some() {
                anyhow::bail!("Browser WebRTC offer must not include a candidate");
            }
            let sdp = input
                .sdp
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Browser WebRTC offer missing sdp"))?
                .trim();
            validate_browser_webrtc_sdp("offer", sdp)?;
            Ok(serde_json::json!({
                "schema": "elastos.browser.webrtc-offer/v1",
                "type": "offer",
                "sdp": sdp,
            }))
        }
        "answer" => {
            if input.candidate.is_some() {
                anyhow::bail!("Browser WebRTC answer must not include a candidate");
            }
            let sdp = input
                .sdp
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Browser WebRTC answer missing sdp"))?
                .trim();
            validate_browser_webrtc_sdp("answer", sdp)?;
            Ok(serde_json::json!({
                "schema": "elastos.browser.webrtc-answer/v1",
                "type": "answer",
                "sdp": sdp,
            }))
        }
        "candidate" => {
            if input.sdp.is_some() {
                anyhow::bail!("Browser WebRTC candidate must not include sdp");
            }
            let candidate = input
                .candidate
                .ok_or_else(|| anyhow::anyhow!("Browser WebRTC candidate missing candidate"))?;
            validate_browser_webrtc_candidate(&candidate)?;
            Ok(serde_json::json!({
                "schema": "elastos.browser.webrtc-candidate/v1",
                "type": "candidate",
                "candidate": candidate,
            }))
        }
        "end_of_candidates" => {
            if input.sdp.is_some() || input.candidate.is_some() {
                anyhow::bail!("Browser WebRTC end_of_candidates must not include sdp or candidate");
            }
            Ok(serde_json::json!({
                "schema": "elastos.browser.webrtc-end-of-candidates/v1",
                "type": "end_of_candidates",
            }))
        }
        _ => anyhow::bail!("Browser WebRTC signal type is unsupported"),
    }
}

fn validate_browser_webrtc_sdp(kind: &str, sdp: &str) -> anyhow::Result<()> {
    if sdp.is_empty() {
        anyhow::bail!("Browser WebRTC {kind} is empty");
    }
    if sdp.len() > 256 * 1024 {
        anyhow::bail!("Browser WebRTC {kind} is too large");
    }
    for line in sdp.lines() {
        if line.starts_with("a=candidate:") || line == "a=end-of-candidates" {
            anyhow::bail!(
                "Browser WebRTC {kind} must send ICE candidates through candidate messages"
            );
        }
    }
    Ok(())
}

fn validate_browser_webrtc_candidate(candidate: &serde_json::Value) -> anyhow::Result<()> {
    if !candidate.is_object() {
        anyhow::bail!("Browser WebRTC candidate must be an object");
    }
    let candidate_line = candidate
        .get("candidate")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Browser WebRTC candidate missing candidate line"))?;
    if candidate_line.trim().is_empty() || candidate_line.len() > 32 * 1024 {
        anyhow::bail!("Browser WebRTC candidate line is invalid");
    }
    if let Some(sdp_mid) = candidate.get("sdpMid").and_then(|value| value.as_str()) {
        if sdp_mid.is_empty() || sdp_mid.len() > 64 || sdp_mid.contains(char::is_whitespace) {
            anyhow::bail!("Browser WebRTC candidate sdpMid is invalid");
        }
    }
    if let Some(index) = candidate
        .get("sdpMLineIndex")
        .and_then(|value| value.as_u64())
    {
        if index > 32 {
            anyhow::bail!("Browser WebRTC candidate sdpMLineIndex is invalid");
        }
    }
    let encoded = serde_json::to_vec(candidate)?;
    if encoded.len() > 64 * 1024 {
        anyhow::bail!("Browser WebRTC candidate is too large");
    }
    Ok(())
}

pub(in crate::api::gateway) fn validate_browser_webrtc_response(
    signal_type: &str,
    data: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if signal_type == "offer" {
        return validate_browser_webrtc_answer(data);
    }
    if data.get("schema").and_then(|value| value.as_str())
        != Some("elastos.browser.webrtc-signal-ack/v1")
    {
        anyhow::bail!("browser-engine provider returned an invalid WebRTC signal ack schema");
    }
    if data.get("type").and_then(|value| value.as_str()) != Some(signal_type) {
        anyhow::bail!("browser-engine provider returned mismatched WebRTC signal ack");
    }
    Ok(data)
}

fn validate_browser_webrtc_answer(data: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    if data.get("schema").and_then(|value| value.as_str())
        != Some("elastos.browser.webrtc-answer/v1")
    {
        anyhow::bail!("browser-engine provider returned an invalid WebRTC answer schema");
    }
    if data.get("type").and_then(|value| value.as_str()) != Some("answer") {
        anyhow::bail!("browser-engine provider returned a non-answer WebRTC response");
    }
    let sdp = data
        .get("sdp")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("browser-engine WebRTC answer missing sdp"))?;
    if sdp.trim().is_empty() || sdp.len() > 256 * 1024 {
        anyhow::bail!("browser-engine WebRTC answer has invalid sdp");
    }
    Ok(data)
}

pub(in crate::api::gateway) fn browser_screenshot_bytes(
    data: serde_json::Value,
) -> anyhow::Result<(&'static str, Bytes)> {
    if data.get("schema").and_then(|value| value.as_str()) != Some("elastos.browser.screenshot/v1")
    {
        anyhow::bail!("browser-engine provider returned an invalid screenshot response");
    }
    let content_type = match data.get("content_type").and_then(|value| value.as_str()) {
        Some("image/png") => "image/png",
        _ => anyhow::bail!("browser-engine screenshot content type is unsupported"),
    };
    let encoded = data
        .get("base64")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("browser-engine screenshot response missing image"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| anyhow::anyhow!("browser-engine screenshot is invalid base64: {err}"))?;
    Ok((content_type, Bytes::from(bytes)))
}

pub(in crate::api::gateway) fn is_safe_runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

pub(in crate::api::gateway) fn validate_browser_engine_page(
    page: serde_json::Value,
    expected_display_mode: BrowserDisplayMode,
) -> anyhow::Result<serde_json::Value> {
    if page.get("schema").and_then(|value| value.as_str()) != Some("elastos.browser.engine.page/v1")
    {
        anyhow::bail!(
            "browser-engine provider did not return an elastos.browser.engine.page/v1 receipt"
        );
    }
    if page.get("direct_network").and_then(|value| value.as_bool()) != Some(false) {
        anyhow::bail!("browser-engine provider attempted to report direct network authority");
    }
    if page
        .get("wallet_injection")
        .and_then(|value| value.as_bool())
        != Some(false)
    {
        anyhow::bail!("browser-engine provider attempted to report wallet injection authority");
    }
    let display_session = page
        .get("display_session")
        .ok_or_else(|| anyhow::anyhow!("browser-engine provider omitted display_session"))?;
    if display_session
        .get("schema")
        .and_then(|value| value.as_str())
        != Some("elastos.browser.display-session/v1")
    {
        anyhow::bail!("browser-engine provider returned an invalid display session");
    }
    let mode = display_session
        .get("mode")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("browser-engine display session omitted mode"))?;
    if mode != expected_display_mode.as_str() {
        anyhow::bail!(
            "browser-engine provider returned display mode {mode}, expected {}",
            expected_display_mode.as_str()
        );
    }
    if display_session
        .get("direct_network")
        .and_then(|value| value.as_bool())
        != Some(false)
    {
        anyhow::bail!("browser-engine display session attempted to report direct network");
    }
    match expected_display_mode {
        BrowserDisplayMode::DiagnosticFrame => {
            if display_session
                .get("input")
                .and_then(|value| value.as_str())
                != Some("runtime_route")
            {
                anyhow::bail!("diagnostic Browser display must use runtime_route input");
            }
            if display_session
                .get("audio")
                .and_then(|value| value.as_bool())
                == Some(true)
                || display_session
                    .get("video")
                    .and_then(|value| value.as_bool())
                    == Some(true)
            {
                anyhow::bail!("diagnostic Browser display cannot claim audio/video media");
            }
        }
        BrowserDisplayMode::WebrtcRemoteDisplay => {
            let backend_class = display_session
                .get("backend_class")
                .and_then(|value| value.as_str());
            let display_backend = display_session
                .get("display_backend")
                .and_then(|value| value.as_str());
            if display_session
                .get("audio")
                .and_then(|value| value.as_bool())
                == Some(true)
                && (backend_class == Some("proof_surface")
                    || display_backend == Some("cdp_screencast_i420"))
            {
                anyhow::bail!("webrtc_remote_display audio requires a product compositor backend");
            }
        }
        BrowserDisplayMode::NativeSurface => {
            if display_session.get("surface_id").is_none() {
                anyhow::bail!("native Browser display requires a surface_id");
            }
        }
    }
    Ok(page)
}
