//! Carrier-based runtime communication for the chat capsule.
//!
//! This replaces the old HTTP-era wrapper with SDK calls via the elastos-guest
//! runtime client. The client detects the substrate (WASM/microVM/host) and
//! uses the appropriate transport automatically.

use anyhow::{anyhow, Result};
use elastos_guest::runtime::RuntimeClient;
use serde::{de::DeserializeOwned, Serialize};
use std::cell::RefCell;

use crate::app::Message;

const CHAT_STATE_ROOT: &str = "Users/self/.AppData/LocalHost/Chat";

thread_local! {
    static CLIENT: RefCell<RuntimeClient> = RefCell::new(RuntimeClient::new());
}

fn with_client<F, R>(f: F) -> R
where
    F: FnOnce(&mut RuntimeClient) -> R,
{
    CLIENT.with(|c| f(&mut c.borrow_mut()))
}

/// Request a capability token, blocks until the shell grants it.
pub fn acquire_capability(resource: &str, action: &str) -> Result<String> {
    with_client(|client| {
        client
            .request_capability(resource, action)
            .map_err(|e| anyhow!("Capability request failed: {}", e))
    })
}

/// Call a provider operation via the runtime.
pub fn provider_call(
    cap_token: &str,
    scheme: &str,
    op: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value> {
    with_client(|client| {
        client
            .provider_call(scheme, op, body, cap_token)
            .map_err(|e| anyhow!("Provider call {}/{} failed: {}", scheme, op, e))
    })
}

fn rooted_chat_path(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    format!("{}/{}", CHAT_STATE_ROOT, trimmed)
}

/// Save a JSON-serializable value to storage.
pub fn save_json<T: Serialize>(storage_token: &str, path: &str, value: &T) -> Result<()> {
    let json = serde_json::to_string(value)?;
    let rooted_path = rooted_chat_path(path);
    let body = serde_json::json!({
        "path": rooted_path,
        "token": storage_token,
        "content": json.as_bytes(),
        "append": false,
    });
    provider_call(storage_token, "localhost", "write", &body)?;
    Ok(())
}

/// Load a JSON value from storage.
pub fn load_json<T: DeserializeOwned>(storage_token: &str, path: &str) -> Result<Option<T>> {
    let body = serde_json::json!({ "path": rooted_chat_path(path), "token": storage_token });
    let result = provider_call(storage_token, "localhost", "read", &body)?;

    if let Some(data) = result.get("data").and_then(|d| d.get("data")) {
        if let Some(bytes) = data.as_array() {
            let byte_vec: Vec<u8> = bytes
                .iter()
                .filter_map(|v| v.as_u64().map(|b| b as u8))
                .collect();
            let text = String::from_utf8(byte_vec)?;
            let value: T = serde_json::from_str(&text)?;
            return Ok(Some(value));
        }
        if let Some(s) = data.as_str() {
            let value: T = serde_json::from_str(s)?;
            return Ok(Some(value));
        }
    }
    Ok(None)
}

/// Load chat history from storage.
pub fn load_history(storage_token: &str, channel: &str) -> Result<Vec<Message>> {
    let path = format!("chat/history/{}.json", channel.trim_start_matches('#'));
    load_json::<Vec<Message>>(storage_token, &path).map(|opt| opt.unwrap_or_default())
}

/// Append a message to chat history.
pub fn append_message(storage_token: &str, channel: &str, message: &Message) -> Result<()> {
    let path = format!("chat/history/{}.json", channel.trim_start_matches('#'));
    let mut existing = load_json::<Vec<Message>>(storage_token, &path)?.unwrap_or_default();
    existing.push(message.clone());
    if existing.len() > 1000 {
        existing = existing.split_off(existing.len() - 1000);
    }
    save_json(storage_token, &path, &existing)
}
