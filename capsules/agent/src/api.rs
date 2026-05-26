//! Carrier-kernel client + capability acquisition for the agent capsule.

use anyhow::{anyhow, Result};
use elastos_guest::runtime::RuntimeClient;
use std::cell::RefCell;

thread_local! {
    static CLIENT: RefCell<RuntimeClient> = RefCell::new(RuntimeClient::new());
}

fn with_client<F, R>(f: F) -> R
where
    F: FnOnce(&mut RuntimeClient) -> R,
{
    CLIENT.with(|client| f(&mut client.borrow_mut()))
}

/// Request a capability token through the capsule kernel.
pub fn acquire_capability(resource: &str, action: &str) -> Result<String> {
    with_client(|client| {
        client
            .request_capability(resource, action)
            .map_err(|err| anyhow!("Capability request failed: {}", err))
    })
}

/// Invoke an ElastOS resource through the capsule kernel.
pub fn carrier_invoke(
    cap_token: &str,
    uri: &str,
    operation: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    with_client(|client| {
        let result = client
            .carrier_invoke(uri, operation, body, cap_token)
            .map_err(|err| anyhow!("Carrier invoke {} {} failed: {}", operation, uri, err))?;
        if result.get("status").and_then(|s| s.as_str()) == Some("error") {
            let code = result["code"].as_str().unwrap_or("unknown");
            let message = result["message"].as_str().unwrap_or("Unknown error");
            return Err(anyhow!("[{}] {}", code, message));
        }
        Ok(result)
    })
}

/// Fail fast when the capsule was not booted with a runtime bridge.
pub fn ensure_runtime_bridge() -> Result<()> {
    if RuntimeClient::is_bridge_configured() {
        Ok(())
    } else {
        Err(anyhow!("runtime Carrier bridge is not configured"))
    }
}
