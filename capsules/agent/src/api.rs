//! HTTP client + capability acquisition for the ElastOS runtime API.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct RequestResponse {
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct StatusResponse {
    status: String,
    #[serde(default)]
    token: Option<String>,
}

/// Request a capability token, poll until shell grants it.
pub fn acquire_capability(api: &str, token: &str, resource: &str, action: &str) -> Result<String> {
    let body = serde_json::json!({
        "resource": resource,
        "action": action,
    });

    let resp = ureq::post(&format!("{}/api/capability/request", api))
        .set("Authorization", &format!("Bearer {}", token))
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| anyhow!("Capability request failed: {}", e))?;

    let req_resp: RequestResponse = resp.into_json()?;

    // If auto-granted, return token directly
    if req_resp.status.as_deref() == Some("granted") {
        if let Some(cap_token) = req_resp.token {
            return Ok(cap_token);
        }
    }

    let request_id = req_resp
        .request_id
        .ok_or_else(|| anyhow!("No request_id in response"))?;

    // Poll until granted (max 10 seconds)
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() > deadline {
            return Err(anyhow!("Capability grant timed out after 10s"));
        }

        std::thread::sleep(Duration::from_millis(50));

        let resp = ureq::get(&format!("{}/api/capability/request/{}", api, request_id))
            .set("Authorization", &format!("Bearer {}", token))
            .call();

        if let Ok(resp) = resp {
            if let Ok(status) = resp.into_json::<StatusResponse>() {
                match status.status.as_str() {
                    "granted" => {
                        return status.token.ok_or_else(|| anyhow!("Granted but no token"));
                    }
                    "denied" => return Err(anyhow!("Capability denied")),
                    "expired" => return Err(anyhow!("Capability request expired")),
                    _ => {} // still pending
                }
            }
        }
    }
}

/// Call a provider operation via the generic proxy.
pub fn provider_call(
    api: &str,
    session_token: &str,
    cap_token: &str,
    scheme: &str,
    op: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    let resp = ureq::post(&format!("{}/api/provider/{}/{}", api, scheme, op))
        .set("Authorization", &format!("Bearer {}", session_token))
        .set("X-Capability-Token", cap_token)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
        .map_err(|e| anyhow!("Provider call {}/{} failed: {}", scheme, op, e))?;

    let result: serde_json::Value = resp.into_json()?;

    if result.get("status").and_then(|s| s.as_str()) == Some("error") {
        let code = result["code"].as_str().unwrap_or("unknown");
        let message = result["message"].as_str().unwrap_or("Unknown error");
        return Err(anyhow!("[{}] {}", code, message));
    }

    Ok(result)
}
