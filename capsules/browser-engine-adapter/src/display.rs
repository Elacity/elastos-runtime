use super::*;

pub(super) fn display_session_receipt(
    display_mode: BrowserDisplayMode,
    stream_id: &str,
    page_id: &str,
) -> Value {
    json!({
        "schema": "elastos.browser.display-session/v1",
        "session_id": format!("display:{stream_id}"),
        "mode": display_mode,
        "network_mode": "runtime_net_only",
        "direct_network": false,
        "input": "runtime_route",
        "signaling_url": format!("/api/apps/browser/pages/{}/webrtc", page_id.replace(':', "%3A")),
        "display_backend": "proof_surface",
        "backend_class": "proof_surface",
        "media_transport": "runtime_relay",
        "audio": false,
        "video": true,
    })
}

pub(super) fn validate_display_session(
    display_session: &Value,
    expected_display_mode: BrowserDisplayMode,
) -> Result<(), String> {
    if display_session
        .get("schema")
        .and_then(|value| value.as_str())
        != Some("elastos.browser.display-session/v1")
    {
        return Err(
            "browser engine supervisor returned invalid display session schema".to_string(),
        );
    }
    if display_session.get("mode").and_then(|value| value.as_str())
        != Some(expected_display_mode.as_str())
    {
        return Err("browser engine supervisor display mode mismatch".to_string());
    }
    if display_session
        .get("network_mode")
        .and_then(|value| value.as_str())
        != Some("runtime_net_only")
    {
        return Err(
            "browser engine supervisor display session must report runtime_net_only".to_string(),
        );
    }
    if display_session
        .get("direct_network")
        .and_then(|value| value.as_bool())
        != Some(false)
    {
        return Err(
            "browser engine supervisor display session reported direct network authority"
                .to_string(),
        );
    }
    match expected_display_mode {
        BrowserDisplayMode::WebrtcRemoteDisplay => {
            let input_mode = display_session
                .get("input")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if input_mode != "datachannel" && input_mode != "runtime_route" {
                return Err(
                    "webrtc_remote_display display session must use datachannel or runtime_route input"
                        .to_string(),
                );
            }
            if input_mode == "datachannel" {
                validate_display_size(display_session)?;
            }
            if display_session
                .get("signaling_url")
                .and_then(|value| value.as_str())
                .filter(|value| {
                    value.starts_with("/api/apps/browser/pages/")
                        && !value.contains(char::is_whitespace)
                })
                .is_none()
            {
                return Err(
                    "webrtc_remote_display display session missing runtime signaling_url"
                        .to_string(),
                );
            }
            let offerer = display_session
                .get("offerer")
                .and_then(|value| value.as_str())
                .unwrap_or("browser");
            if offerer != "browser" && offerer != "engine" {
                return Err("webrtc_remote_display offerer must be browser or engine".to_string());
            }
            if offerer == "engine" {
                let Some(initial_offer) = display_session.get("initial_offer") else {
                    return Err(
                        "engine-offer WebRTC display sessions require initial_offer".to_string()
                    );
                };
                validate_initial_webrtc_offer(initial_offer)?;
            }
            let audio = display_session
                .get("audio")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if audio {
                let backend_class = display_session
                    .get("backend_class")
                    .and_then(|value| value.as_str());
                let display_backend = display_session
                    .get("display_backend")
                    .and_then(|value| value.as_str());
                if backend_class == Some("proof_surface")
                    || display_backend == Some("cdp_screencast_i420")
                {
                    return Err(
                        "webrtc_remote_display audio requires a product compositor backend"
                            .to_string(),
                    );
                }
                if display_session
                    .get("video")
                    .and_then(|value| value.as_bool())
                    != Some(true)
                {
                    return Err(
                        "webrtc_remote_display audio sessions must also advertise video"
                            .to_string(),
                    );
                }
            }
        }
        BrowserDisplayMode::NativeSurface => {
            if display_session
                .get("surface_id")
                .and_then(|value| value.as_str())
                .filter(|value| is_safe_id(value))
                .is_none()
            {
                return Err("native_surface display session missing safe surface_id".to_string());
            }
        }
    }
    Ok(())
}

pub(super) fn validate_display_size(display_session: &Value) -> Result<(), String> {
    let width = display_session
        .get("width")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            "datachannel WebRTC display sessions must report display width".to_string()
        })?;
    let height = display_session
        .get("height")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            "datachannel WebRTC display sessions must report display height".to_string()
        })?;
    if !(320..=3840).contains(&width) || !(240..=2160).contains(&height) {
        return Err(
            "datachannel WebRTC display size must be within 320x240 and 3840x2160".to_string(),
        );
    }
    Ok(())
}

pub(super) fn validate_webrtc_signal(signal: &Value) -> Result<&'static str, String> {
    match signal.get("schema").and_then(|value| value.as_str()) {
        Some("elastos.browser.webrtc-offer/v1") => {
            if signal.get("type").and_then(|value| value.as_str()) != Some("offer") {
                return Err("WebRTC signal must be an offer".to_string());
            }
            let Some(sdp) = signal.get("sdp").and_then(|value| value.as_str()) else {
                return Err("WebRTC offer missing sdp".to_string());
            };
            validate_webrtc_sdp("offer", sdp)?;
            if signal.get("candidate").is_some() {
                return Err("WebRTC offer must not include candidate fields".to_string());
            }
            Ok("offer")
        }
        Some("elastos.browser.webrtc-answer/v1") => {
            if signal.get("type").and_then(|value| value.as_str()) != Some("answer") {
                return Err("WebRTC signal must be an answer".to_string());
            }
            let Some(sdp) = signal.get("sdp").and_then(|value| value.as_str()) else {
                return Err("WebRTC answer missing sdp".to_string());
            };
            validate_webrtc_sdp("answer", sdp)?;
            if signal.get("candidate").is_some() {
                return Err("WebRTC answer must not include candidate fields".to_string());
            }
            Ok("answer")
        }
        Some("elastos.browser.webrtc-candidate/v1") => {
            if signal.get("type").and_then(|value| value.as_str()) != Some("candidate") {
                return Err("WebRTC signal must be a candidate".to_string());
            }
            if signal.get("sdp").is_some() {
                return Err("WebRTC candidate must not include sdp".to_string());
            }
            let Some(candidate) = signal.get("candidate").and_then(|value| value.as_object())
            else {
                return Err("WebRTC candidate missing candidate object".to_string());
            };
            let Some(candidate_line) = candidate.get("candidate").and_then(|value| value.as_str())
            else {
                return Err("WebRTC candidate missing candidate line".to_string());
            };
            if candidate_line.trim().is_empty() || candidate_line.len() > 32 * 1024 {
                return Err("WebRTC candidate line is invalid".to_string());
            }
            Ok("candidate")
        }
        Some("elastos.browser.webrtc-end-of-candidates/v1") => {
            if signal.get("type").and_then(|value| value.as_str()) != Some("end_of_candidates") {
                return Err("WebRTC signal must be end_of_candidates".to_string());
            }
            if signal.get("sdp").is_some() || signal.get("candidate").is_some() {
                return Err(
                    "WebRTC end_of_candidates must not include sdp or candidate".to_string()
                );
            }
            Ok("end_of_candidates")
        }
        _ => Err("WebRTC signal uses an unsupported schema".to_string()),
    }
}

pub(super) fn validate_webrtc_sdp(kind: &str, sdp: &str) -> Result<(), String> {
    if sdp.trim().is_empty() || sdp.len() > 256 * 1024 {
        return Err(format!("WebRTC {kind} sdp is invalid"));
    }
    if sdp
        .lines()
        .any(|line| line.starts_with("a=candidate:") || line == "a=end-of-candidates")
    {
        return Err(format!(
            "WebRTC {kind} must send ICE candidates through candidate messages"
        ));
    }
    Ok(())
}

pub(super) fn validate_initial_webrtc_offer(signal: &Value) -> Result<(), String> {
    if signal.get("schema").and_then(|value| value.as_str())
        != Some("elastos.browser.webrtc-offer/v1")
    {
        return Err(
            "engine-offer WebRTC initial_offer must use elastos.browser.webrtc-offer/v1"
                .to_string(),
        );
    }
    if signal.get("type").and_then(|value| value.as_str()) != Some("offer") {
        return Err("engine-offer WebRTC initial_offer must be an offer".to_string());
    }
    let Some(sdp) = signal.get("sdp").and_then(|value| value.as_str()) else {
        return Err("engine-offer WebRTC initial_offer missing sdp".to_string());
    };
    if sdp.trim().is_empty() || sdp.len() > 256 * 1024 {
        return Err("engine-offer WebRTC initial_offer has invalid sdp".to_string());
    }
    Ok(())
}

pub(super) fn validate_webrtc_response(signal_type: &str, signal: &Value) -> Result<(), String> {
    if signal_type == "offer" {
        return validate_webrtc_answer(signal);
    }
    if signal.get("schema").and_then(|value| value.as_str())
        != Some("elastos.browser.webrtc-signal-ack/v1")
    {
        return Err("WebRTC signal ack must use elastos.browser.webrtc-signal-ack/v1".to_string());
    }
    if signal.get("type").and_then(|value| value.as_str()) != Some(signal_type) {
        return Err("WebRTC signal ack type mismatch".to_string());
    }
    Ok(())
}

pub(super) fn validate_webrtc_answer(signal: &Value) -> Result<(), String> {
    if signal.get("schema").and_then(|value| value.as_str())
        != Some("elastos.browser.webrtc-answer/v1")
    {
        return Err("WebRTC answer must use elastos.browser.webrtc-answer/v1".to_string());
    }
    if signal.get("type").and_then(|value| value.as_str()) != Some("answer") {
        return Err("WebRTC signal must be an answer".to_string());
    }
    let Some(sdp) = signal.get("sdp").and_then(|value| value.as_str()) else {
        return Err("WebRTC answer missing sdp".to_string());
    };
    if sdp.trim().is_empty() || sdp.len() > 256 * 1024 {
        return Err("WebRTC answer sdp is invalid".to_string());
    }
    Ok(())
}
