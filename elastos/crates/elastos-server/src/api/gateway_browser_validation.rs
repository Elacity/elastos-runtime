//! Browser request and provider receipt validation helpers.

use super::*;

pub(in crate::api::gateway) fn browser_request_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let default_proto = if browser_origin_host_is_loopback(host) {
        "http"
    } else {
        "https"
    };
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| *value == "http" || *value == "https")
        .unwrap_or(default_proto);
    Some(format!("{proto}://{host}"))
}

fn browser_origin_host_is_loopback(host: &str) -> bool {
    let trimmed = host.trim();
    let host = if let Some(rest) = trimmed.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        trimmed.split(':').next().unwrap_or_default()
    }
    .to_ascii_lowercase();
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1")
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

pub(in crate::api::gateway) fn validate_browser_launch_contract(
    display_mode: BrowserDisplayMode,
    guarantee_level: BrowserGuaranteeLevel,
) -> anyhow::Result<()> {
    match guarantee_level {
        BrowserGuaranteeLevel::MechanismMicrovm => {
            if display_mode == BrowserDisplayMode::WebrtcRemoteDisplay {
                Ok(())
            } else {
                anyhow::bail!("mechanism_microvm Browser launches require webrtc_remote_display")
            }
        }
        BrowserGuaranteeLevel::OperatorRbi => {
            if display_mode == BrowserDisplayMode::WebrtcRemoteDisplay {
                Ok(())
            } else {
                anyhow::bail!("operator_rbi Browser launches require webrtc_remote_display")
            }
        }
        BrowserGuaranteeLevel::PolicyWebview => {
            if display_mode == BrowserDisplayMode::NativeSurface {
                Ok(())
            } else {
                anyhow::bail!("policy_webview Browser launches require native_surface")
            }
        }
        BrowserGuaranteeLevel::Diagnostic => {
            anyhow::bail!("diagnostic Browser frame launches are not supported")
        }
    }
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

pub(in crate::api::gateway) fn is_safe_runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

pub(in crate::api::gateway) fn browser_instance_id(
    value: Option<String>,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    let Some(identifier) = value.strip_prefix("browser:") else {
        return Err("invalid Browser instance binding".to_string());
    };
    let compact = identifier.replace('-', "");
    let hyphens_are_canonical = identifier.len() == 32
        || (identifier.len() == 36
            && identifier.as_bytes().get(8) == Some(&b'-')
            && identifier.as_bytes().get(13) == Some(&b'-')
            && identifier.as_bytes().get(18) == Some(&b'-')
            && identifier.as_bytes().get(23) == Some(&b'-'));
    if compact.len() != 32
        || !compact
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || !hyphens_are_canonical
    {
        return Err("invalid Browser instance binding".to_string());
    }
    Ok(Some(value.to_string()))
}

pub(in crate::api::gateway) fn validate_browser_engine_page(
    page: serde_json::Value,
    expected_display_mode: BrowserDisplayMode,
    expected_guarantee_level: BrowserGuaranteeLevel,
) -> anyhow::Result<serde_json::Value> {
    if page.get("schema").and_then(|value| value.as_str()) != Some("elastos.browser.engine.page/v1")
    {
        anyhow::bail!(
            "browser-engine provider did not return an elastos.browser.engine.page/v1 receipt"
        );
    }
    if page.get("provider").and_then(|value| value.as_str()) != Some(BROWSER_ENGINE_PROVIDER_ID)
        || page
            .get("protocol_version")
            .and_then(|value| value.as_str())
            != Some(BROWSER_ENGINE_PROTOCOL_VERSION)
    {
        anyhow::bail!(
            "browser-engine provider returned an unsupported provider identity or protocol version"
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
    let view = page
        .get("view")
        .ok_or_else(|| anyhow::anyhow!("browser-engine provider omitted view geometry"))?;
    if view.get("schema").and_then(|value| value.as_str()) != Some("elastos.browser.view/v1") {
        anyhow::bail!("browser-engine provider returned an invalid view geometry");
    }
    let view_width = browser_display_dimension(view, "width", "view")?;
    let view_height = browser_display_dimension(view, "height", "view")?;
    let session_width = browser_display_dimension(display_session, "width", "display_session")?;
    let session_height = browser_display_dimension(display_session, "height", "display_session")?;
    match expected_display_mode {
        BrowserDisplayMode::WebrtcRemoteDisplay => {
            let backend_class = display_session
                .get("backend_class")
                .and_then(|value| value.as_str());
            let display_backend = display_session
                .get("display_backend")
                .and_then(|value| value.as_str());
            if !same_display_ratio(session_width, session_height, view_width, view_height) {
                anyhow::bail!(
                    "webrtc_remote_display stream dimensions must preserve the Runtime view aspect ratio"
                );
            }
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
    validate_browser_engine_guarantee(&page, expected_guarantee_level)?;
    Ok(browser_visible_engine_page(&page, expected_guarantee_level))
}

fn browser_visible_engine_page(
    page: &serde_json::Value,
    guarantee_level: BrowserGuaranteeLevel,
) -> serde_json::Value {
    let mut visible = serde_json::Map::new();
    for key in [
        "schema",
        "provider",
        "protocol_version",
        "page_id",
        "adapter",
        "engine",
        "url",
        "actual_url",
        "title",
        "stream_id",
        "network_mode",
        "direct_network",
        "wallet_injection",
        "display_mode",
    ] {
        copy_browser_visible_field(&mut visible, page, key);
    }
    if let Some(display_session) = page.get("display_session") {
        let mut visible_display_session = browser_visible_object_fields(
            display_session,
            &[
                "schema",
                "session_id",
                "mode",
                "width",
                "height",
                "network_mode",
                "direct_network",
                "input",
                "input_protocol",
                "display_backend",
                "backend_class",
                "media_transport",
                "audio",
                "video",
                "ice_servers",
                "offerer",
                "initial_offer",
                "audio_offer",
                "signaling_url",
                "surface_id",
            ],
        );
        if guarantee_level == BrowserGuaranteeLevel::MechanismMicrovm {
            copy_browser_visible_field(
                visible_display_session
                    .as_object_mut()
                    .expect("Browser-visible display session is an object"),
                display_session,
                "ice_connection_policy",
            );
        }
        visible.insert("display_session".to_string(), visible_display_session);
    }
    if let Some(view) = page.get("view") {
        visible.insert(
            "view".to_string(),
            browser_visible_object_fields(view, &["schema", "mode", "width", "height"]),
        );
    }
    if let Some(wallet_bridge) = page.get("wallet_bridge") {
        visible.insert(
            "wallet_bridge".to_string(),
            browser_visible_object_fields(
                wallet_bridge,
                &[
                    "schema",
                    "mode",
                    "accounts",
                    "default_chain_namespace",
                    "signing",
                ],
            ),
        );
    }
    serde_json::Value::Object(visible)
}

fn browser_visible_object_fields(value: &serde_json::Value, fields: &[&str]) -> serde_json::Value {
    let mut visible = serde_json::Map::new();
    for key in fields {
        copy_browser_visible_field(&mut visible, value, key);
    }
    serde_json::Value::Object(visible)
}

fn copy_browser_visible_field(
    target: &mut serde_json::Map<String, serde_json::Value>,
    source: &serde_json::Value,
    key: &str,
) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

fn browser_display_dimension(
    value: &serde_json::Value,
    field: &str,
    label: &str,
) -> anyhow::Result<u64> {
    let dimension = value
        .get(field)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow::anyhow!("browser-engine {label}.{field} is required"))?;
    if (field == "width" && !(320..=3840).contains(&dimension))
        || (field == "height" && !(240..=2160).contains(&dimension))
    {
        anyhow::bail!("browser-engine {label}.{field} is outside the supported viewport range");
    }
    Ok(dimension)
}

fn same_display_ratio(
    left_width: u64,
    left_height: u64,
    right_width: u64,
    right_height: u64,
) -> bool {
    let left = (left_width as u128) * (right_height as u128);
    let right = (right_width as u128) * (left_height as u128);
    left.abs_diff(right) <= 2
}

fn validate_browser_engine_guarantee(
    page: &serde_json::Value,
    expected_guarantee_level: BrowserGuaranteeLevel,
) -> anyhow::Result<()> {
    let engine = page
        .get("engine")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let isolation_kind = page
        .pointer("/isolation/kind")
        .and_then(|value| value.as_str());
    let display_backend_class = page
        .pointer("/display_session/backend_class")
        .and_then(|value| value.as_str());
    match expected_guarantee_level {
        BrowserGuaranteeLevel::MechanismMicrovm => {
            if engine != "chromium_microvm" || isolation_kind != Some("per_launch_vm_target") {
                anyhow::bail!("Browser launch requested mechanism_microvm but provider did not return a per-launch Chromium microVM");
            }
            if page
                .pointer("/display_session/mode")
                .and_then(|value| value.as_str())
                == Some("webrtc_remote_display")
            {
                let audio = page
                    .pointer("/display_session/audio")
                    .and_then(|value| value.as_bool());
                let video = page
                    .pointer("/display_session/video")
                    .and_then(|value| value.as_bool());
                if display_backend_class != Some("product_compositor")
                    || audio != Some(true)
                    || video != Some(true)
                {
                    anyhow::bail!("Browser launch requested mechanism_microvm but provider did not return an audio/video product compositor");
                }
                if page
                    .pointer("/display_session/media_transport")
                    .and_then(|value| value.as_str())
                    != Some("runtime_relay")
                {
                    anyhow::bail!(
                        "Browser VM WebRTC display must use runtime_relay media transport"
                    );
                }
                if page
                    .pointer("/display_session/ice_connection_policy")
                    .and_then(|value| value.as_str())
                    != Some("engine_relay_only")
                    || page.pointer("/display_session/ice_servers").is_some()
                    || page
                        .pointer("/display_session/offerer")
                        .and_then(|value| value.as_str())
                        != Some("engine")
                {
                    anyhow::bail!(
                        "Browser VM WebRTC display must use engine_relay_only without Browser ICE credentials"
                    );
                }
            }
        }
        BrowserGuaranteeLevel::OperatorRbi => {
            if !matches!(engine, "selkies_gstreamer" | "hosted_remote_browser")
                || display_backend_class != Some("product_compositor")
            {
                anyhow::bail!("Browser launch requested operator_rbi but provider did not return an operator remote-browser product compositor");
            }
        }
        BrowserGuaranteeLevel::PolicyWebview => {
            if !matches!(engine, "cef" | "webview2" | "geckoview" | "wkwebview") {
                anyhow::bail!("Browser launch requested policy_webview but provider did not return a policy WebView engine");
            }
        }
        BrowserGuaranteeLevel::Diagnostic => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn browser_instance_binding_accepts_only_canonical_random_ids() {
        for valid in [
            "browser:00112233445566778899aabbccddeeff",
            "browser:00112233-4455-6677-8899-aabbccddeeff",
        ] {
            assert_eq!(
                browser_instance_id(Some(valid.to_string())).unwrap(),
                Some(valid.to_string())
            );
        }
        for invalid in [
            "",
            "browser:",
            "browser:00112233445566778899AABBCCDDEEFF",
            "browser:00112233-4455-6677-8899a-abbccddeeff",
            "other:00112233445566778899aabbccddeeff",
        ] {
            assert!(browser_instance_id(Some(invalid.to_string())).is_err());
        }
    }

    fn page(
        display_mode: &str,
        view: serde_json::Value,
        display_session: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "schema": "elastos.browser.engine.page/v1",
            "provider": "browser-engine-adapter",
            "protocol_version": BROWSER_ENGINE_PROTOCOL_VERSION,
            "page_id": "page:test",
            "adapter": "test-adapter",
            "engine": "contract_proof",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "display_session": display_session,
            "view": view,
            "display_mode": display_mode
        })
    }

    fn browser_view(width: u64, height: u64) -> serde_json::Value {
        json!({
            "schema": "elastos.browser.view/v1",
            "mode": "webrtc_remote_display",
            "width": width,
            "height": height
        })
    }

    fn webrtc_display(width: u64, height: u64) -> serde_json::Value {
        json!({
            "schema": "elastos.browser.display-session/v1",
            "session_id": "display:test",
            "mode": "webrtc_remote_display",
            "width": width,
            "height": height,
            "input": "datachannel",
            "input_protocol": "selkies_v1",
            "audio": false,
            "video": true,
            "display_backend": "selkies_gstreamer_webrtc",
            "backend_class": "product_compositor",
            "network_mode": "runtime_net_only",
            "direct_network": false
        })
    }

    fn mechanism_microvm_page() -> serde_json::Value {
        let mut display = webrtc_display(1920, 1080);
        display["display_backend"] =
            serde_json::Value::String("vm_selkies_gstreamer_webrtc".to_string());
        display["media_transport"] = serde_json::Value::String("runtime_relay".to_string());
        display["audio"] = serde_json::Value::Bool(true);
        display["video"] = serde_json::Value::Bool(true);
        display["offerer"] = serde_json::Value::String("engine".to_string());
        display["ice_connection_policy"] =
            serde_json::Value::String("engine_relay_only".to_string());
        let mut page = page("webrtc_remote_display", browser_view(1280, 720), display);
        page["engine"] = serde_json::Value::String("chromium_microvm".to_string());
        page["isolation"] = json!({
            "schema": "elastos.browser.engine.isolation/v1",
            "kind": "per_launch_vm_target",
            "session_dir": "/tmp/elastos-browser-vm-sessions/test"
        });
        page
    }

    #[test]
    fn diagnostic_launch_contract_is_not_supported() {
        let err = validate_browser_launch_contract(
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::Diagnostic,
        )
        .expect_err("diagnostic frame launches must fail closed");
        assert!(err
            .to_string()
            .contains("diagnostic Browser frame launches are not supported"));
    }

    #[test]
    fn display_session_dimensions_are_required() {
        let mut display = webrtc_display(900, 520);
        display.as_object_mut().unwrap().remove("width");
        let bad = page("webrtc_remote_display", browser_view(900, 520), display);
        let err = validate_browser_engine_page(
            bad,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::OperatorRbi,
        )
        .expect_err("display sessions without dimensions must fail closed");
        assert!(err
            .to_string()
            .contains("browser-engine display_session.width is required"));
    }

    #[test]
    fn webrtc_stream_must_preserve_runtime_view_ratio() {
        let bad = page(
            "webrtc_remote_display",
            browser_view(1000, 700),
            webrtc_display(1920, 1080),
        );
        let err = validate_browser_engine_page(
            bad,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::OperatorRbi,
        )
        .expect_err("stretched WebRTC display geometry must fail closed");
        assert!(err
            .to_string()
            .contains("stream dimensions must preserve the Runtime view aspect ratio"));
    }

    #[test]
    fn fixed_stream_webrtc_accepts_matching_view_ratio() {
        let mut ok = page(
            "webrtc_remote_display",
            browser_view(1280, 720),
            webrtc_display(1920, 1080),
        );
        ok["engine"] = serde_json::Value::String("selkies_gstreamer".to_string());
        validate_browser_engine_page(
            ok,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::OperatorRbi,
        )
        .expect("fixed WebRTC stream may differ in size when the aspect ratio matches");
    }

    #[test]
    fn engine_page_validation_returns_browser_visible_receipt() {
        let mut display = webrtc_display(1280, 720);
        display["signaling_url"] =
            serde_json::Value::String("/api/apps/browser/pages/page%3Atest/webrtc".to_string());
        display["ice_servers"] = json!([{
            "urls": ["turn:relay.invalid:3478"],
            "username": "session-user",
            "credential": "session-credential"
        }]);
        display["control_socket_path"] = serde_json::Value::String("/tmp/private.sock".to_string());
        display["profile"] = json!({ "disk_path": "/private/profile" });

        let mut ok = page("webrtc_remote_display", browser_view(1280, 720), display);
        ok["engine"] = serde_json::Value::String("selkies_gstreamer".to_string());
        ok["actual_url"] = serde_json::Value::String("https://example.org/".to_string());
        ok["control_socket_path"] = serde_json::Value::String("/tmp/private.sock".to_string());
        ok["adapter_ipc"] = json!({ "path": "/tmp/adapter.sock" });
        ok["relay_ipc"] = json!({ "path": "/tmp/relay.sock" });
        ok["principal_id"] = serde_json::Value::String("person:local:secret".to_string());
        ok["profile"] = json!({ "disk_path": "/private/profile" });
        ok["isolation"] = json!({ "session_dir": "/private/session" });
        ok["view"]["control_socket_path"] =
            serde_json::Value::String("/tmp/private-view.sock".to_string());
        ok["wallet_bridge"] = json!({
            "schema": "elastos.browser.wallet-bridge/v1",
            "mode": "runtime_mediated_eip1193",
            "accounts": 1,
            "default_chain_namespace": "eip155:20",
            "signing": "approval_required",
            "home_token": "secret-token",
            "principal_id": "person:local:secret"
        });

        let visible = validate_browser_engine_page(
            ok,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::OperatorRbi,
        )
        .expect("valid engine page should return a sanitized visible receipt");

        assert_eq!(visible["schema"], "elastos.browser.engine.page/v1");
        assert_eq!(visible["actual_url"], "https://example.org/");
        assert_eq!(
            visible["display_session"]["signaling_url"],
            "/api/apps/browser/pages/page%3Atest/webrtc"
        );
        assert_eq!(
            visible["display_session"]["ice_servers"][0]["credential"],
            "session-credential"
        );
        assert_eq!(visible["wallet_bridge"]["mode"], "runtime_mediated_eip1193");
        assert!(visible.get("control_socket_path").is_none());
        assert!(visible.get("adapter_ipc").is_none());
        assert!(visible.get("relay_ipc").is_none());
        assert!(visible.get("principal_id").is_none());
        assert!(visible.get("profile").is_none());
        assert!(visible.get("isolation").is_none());
        assert!(visible
            .pointer("/display_session/control_socket_path")
            .is_none());
        assert!(visible.pointer("/display_session/profile").is_none());
        assert!(visible.pointer("/view/control_socket_path").is_none());
        assert!(visible.pointer("/wallet_bridge/home_token").is_none());
        assert!(visible.pointer("/wallet_bridge/principal_id").is_none());
    }

    #[test]
    fn mechanism_microvm_preserves_exact_engine_relay_policy_without_ice_credentials() {
        let visible = validate_browser_engine_page(
            mechanism_microvm_page(),
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .expect("valid VZ engine relay policy should remain Browser-visible");

        assert_eq!(
            visible["display_session"]["ice_connection_policy"],
            "engine_relay_only"
        );
        assert!(visible["display_session"].get("ice_servers").is_none());
    }

    #[test]
    fn mechanism_microvm_rejects_missing_or_credentialed_engine_relay_policy() {
        let mut missing = mechanism_microvm_page();
        missing["display_session"]
            .as_object_mut()
            .unwrap()
            .remove("ice_connection_policy");
        let missing_err = validate_browser_engine_page(
            missing,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .expect_err("missing VZ engine relay policy must fail closed");
        assert!(missing_err
            .to_string()
            .contains("must use engine_relay_only without Browser ICE credentials"));

        let mut credentialed = mechanism_microvm_page();
        credentialed["display_session"]["ice_servers"] = json!([{
            "urls": ["turn:127.0.0.1:3478"],
            "username": "guest-user",
            "credential": "guest-secret"
        }]);
        let credentialed_err = validate_browser_engine_page(
            credentialed,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .expect_err("VZ guest TURN credentials must not become Browser-visible");
        assert!(credentialed_err
            .to_string()
            .contains("must use engine_relay_only without Browser ICE credentials"));
    }

    #[test]
    fn mechanism_microvm_webrtc_requires_audio_video_product_display() {
        let mut display = webrtc_display(1920, 1080);
        display["display_backend"] =
            serde_json::Value::String("vm_selkies_gstreamer_webrtc".to_string());
        display["media_transport"] = serde_json::Value::String("runtime_relay".to_string());
        display["audio"] = serde_json::Value::Bool(false);
        let mut bad = page("webrtc_remote_display", browser_view(1280, 720), display);
        bad["engine"] = serde_json::Value::String("chromium_microvm".to_string());
        bad["isolation"] = json!({
            "schema": "elastos.browser.engine.isolation/v1",
            "kind": "per_launch_vm_target",
            "session_dir": "/tmp/elastos-browser-vm-sessions/test"
        });

        let err = validate_browser_engine_page(
            bad,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .expect_err("video-only VM product display must fail closed");
        assert!(err.to_string().contains("audio/video product compositor"));
    }

    #[test]
    fn mechanism_microvm_webrtc_requires_video_product_display() {
        let mut display = webrtc_display(1920, 1080);
        display["display_backend"] =
            serde_json::Value::String("vm_selkies_gstreamer_webrtc".to_string());
        display["media_transport"] = serde_json::Value::String("runtime_relay".to_string());
        display["video"] = serde_json::Value::Bool(false);
        let mut bad = page("webrtc_remote_display", browser_view(1280, 720), display);
        bad["engine"] = serde_json::Value::String("chromium_microvm".to_string());
        bad["isolation"] = json!({
            "schema": "elastos.browser.engine.isolation/v1",
            "kind": "per_launch_vm_target",
            "session_dir": "/tmp/elastos-browser-vm-sessions/test"
        });

        let err = validate_browser_engine_page(
            bad,
            BrowserDisplayMode::WebrtcRemoteDisplay,
            BrowserGuaranteeLevel::MechanismMicrovm,
        )
        .expect_err("video-less VM product display must fail closed");
        assert!(err.to_string().contains("audio/video product compositor"));
    }
}
