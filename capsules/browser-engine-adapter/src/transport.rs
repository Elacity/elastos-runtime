use super::*;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const VZ_TRANSPORT_AUTHORITY_SCHEMA: &str = "elastos.browser.vz-transport-authority/v1";
pub(super) const VZ_TRANSPORT_SECRET_SCHEMA: &str = "elastos.browser.vz-transport-secret/v1";

pub(super) fn validate_vz_transport_launch(
    authority: &Value,
    secret: &Value,
    generation: &str,
    stream_id: &str,
    page_id: &str,
    vm_id: &str,
    principal_id: Option<&str>,
) -> Result<(), String> {
    validate_vz_transport_authority(authority)?;
    if authority.get("generation").and_then(Value::as_str) != Some(generation)
        || authority.get("page_id").and_then(Value::as_str) != Some(page_id)
        || authority.get("vm_id").and_then(Value::as_str) != Some(vm_id)
        || authority
            .pointer("/egress/stream_id")
            .and_then(Value::as_str)
            != Some(stream_id)
        || authority.get("principal_id").and_then(Value::as_str) != principal_id
    {
        return Err("Browser VZ transport launch binding changed".to_string());
    }
    validate_vz_transport_secret(authority, secret)
}

pub(super) fn validate_vz_transport_authority(authority: &Value) -> Result<(), String> {
    let object = authority
        .as_object()
        .ok_or_else(|| "Browser VZ transport authority must be an object".to_string())?;
    let keys = [
        "schema",
        "binding_hash",
        "generation",
        "page_id",
        "vm_id",
        "principal_id",
        "egress",
        "media",
        "turn",
        "bootstrap_vsock_port",
        "expires_at_unix_ms",
    ];
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || authority.get("schema").and_then(Value::as_str) != Some(VZ_TRANSPORT_AUTHORITY_SCHEMA)
    {
        return Err("Browser VZ transport authority shape is invalid".to_string());
    }
    require_sha256(authority.get("binding_hash"), "binding_hash")?;
    require_sha256(authority.get("generation"), "generation")?;
    for field in ["page_id", "vm_id", "principal_id"] {
        require_safe_id(authority.get(field), field, 512)?;
    }
    let egress = validate_stream(authority.get("egress"), false)?;
    let media = validate_stream(authority.get("media"), true)?;
    if egress.0 == media.0 || egress.1 == media.1 || egress.2 == media.2 {
        return Err("Browser VZ transport streams must be distinct".to_string());
    }
    let bootstrap_port = require_u32(
        authority.get("bootstrap_vsock_port"),
        "bootstrap_vsock_port",
    )?;
    if bootstrap_port == egress.2 || bootstrap_port == media.2 || egress.2 == media.2 {
        return Err("Browser VZ transport vsock ports must be distinct".to_string());
    }
    let expires_at = authority
        .get("expires_at_unix_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "Browser VZ transport expiry is invalid".to_string())?;
    if expires_at > current_unix_ms().saturating_add(24 * 60 * 60 * 1_000) {
        return Err("Browser VZ transport expiry exceeds its bounded horizon".to_string());
    }
    validate_turn(authority.get("turn"), expires_at)?;
    let mut unsigned = authority.clone();
    unsigned
        .as_object_mut()
        .expect("validated authority object")
        .remove("binding_hash");
    let expected = sha256_label(&canonical_json_bytes(&unsigned)?);
    if authority.get("binding_hash").and_then(Value::as_str) != Some(expected.as_str()) {
        return Err("Browser VZ transport binding hash mismatch".to_string());
    }
    if serde_json::to_vec(authority).map_or(true, |bytes| bytes.len() > 32 * 1024) {
        return Err("Browser VZ transport authority is too large".to_string());
    }
    Ok(())
}

fn validate_live_vz_transport_authority(authority: &Value) -> Result<(), String> {
    validate_vz_transport_authority(authority)?;
    if authority
        .get("expires_at_unix_ms")
        .and_then(Value::as_u64)
        .is_none_or(|value| value <= current_unix_ms())
    {
        return Err("Browser VZ transport authority is expired".to_string());
    }
    Ok(())
}

pub(super) fn validate_vz_transport_secret(
    authority: &Value,
    secret: &Value,
) -> Result<(), String> {
    validate_live_vz_transport_authority(authority)?;
    let object = secret
        .as_object()
        .ok_or_else(|| "Browser VZ transport secret must be an object".to_string())?;
    if object.len() != 4
        || !["schema", "binding_hash", "credential", "auth_secret"]
            .iter()
            .all(|key| object.contains_key(*key))
        || secret.get("schema").and_then(Value::as_str) != Some(VZ_TRANSPORT_SECRET_SCHEMA)
        || secret.get("binding_hash") != authority.get("binding_hash")
    {
        return Err("Browser VZ transport secret binding is invalid".to_string());
    }
    let credential = require_secret(secret.get("credential"), "credential")?;
    let auth_secret = require_secret(secret.get("auth_secret"), "auth_secret")?;
    if authority
        .pointer("/turn/credential_hash")
        .and_then(Value::as_str)
        != Some(sha256_label(credential.as_bytes()).as_str())
        || authority
            .pointer("/turn/auth_secret_hash")
            .and_then(Value::as_str)
            != Some(sha256_label(auth_secret.as_bytes()).as_str())
    {
        return Err("Browser VZ transport secret hash mismatch".to_string());
    }
    let username = authority
        .pointer("/turn/username")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser VZ TURN username is missing".to_string())?;
    let mut mac = Hmac::<Sha1>::new_from_slice(auth_secret.as_bytes())
        .map_err(|_| "Browser VZ TURN secret is invalid".to_string())?;
    mac.update(username.as_bytes());
    let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    if expected != credential {
        return Err("Browser VZ TURN credential is invalid".to_string());
    }
    Ok(())
}

pub(super) fn validate_vz_transport_effect_receipt(
    receipt: &Value,
    authority: &Value,
) -> Result<(), String> {
    validate_vz_transport_authority(authority)?;
    let object = receipt
        .as_object()
        .ok_or_else(|| "Browser VZ transport effect receipt must be an object".to_string())?;
    let keys = [
        "schema",
        "binding_hash",
        "generation",
        "page_id",
        "vm_id",
        "expires_at_unix_ms",
        "terminal",
        "effects",
    ];
    let effects = receipt
        .get("effects")
        .and_then(Value::as_object)
        .ok_or_else(|| "Browser VZ transport effect receipt is missing effects".to_string())?;
    let effect_keys = [
        "vz_network_devices_zero",
        "guest_bootstrap_validated",
        "guest_loopback_only",
        "guest_interfaces",
        "guest_default_route_absent",
        "guest_direct_network_absent",
        "ordinary_stream_fixed_target",
        "media_stream_fixed_target",
        "turn_launch_owned",
        "turn_listener_loopback",
        "hibernation_disabled",
    ];
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || effects.len() != effect_keys.len()
        || effect_keys.iter().any(|key| !effects.contains_key(*key))
        || receipt.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vz-transport-effect-receipt/v1")
        || receipt.get("binding_hash") != authority.get("binding_hash")
        || receipt.get("generation") != authority.get("generation")
        || receipt.get("page_id") != authority.get("page_id")
        || receipt.get("vm_id") != authority.get("vm_id")
        || receipt.get("expires_at_unix_ms") != authority.get("expires_at_unix_ms")
        || receipt.get("terminal").and_then(Value::as_bool) != Some(true)
        || effects.get("guest_interfaces") != Some(&json!(["lo"]))
        || effect_keys
            .iter()
            .filter(|key| **key != "guest_interfaces")
            .any(|key| effects.get(*key).and_then(Value::as_bool) != Some(true))
        || value_contains_transport_secret(receipt)
    {
        return Err("Browser VZ supervisor transport effect receipt is not exact".to_string());
    }
    Ok(())
}

pub(super) fn vz_public_transport_proof(
    authority: &Value,
    receipt: &Value,
) -> Result<Value, String> {
    validate_vz_transport_effect_receipt(receipt, authority)?;
    Ok(json!({
        "schema": "elastos.browser.vz-transport-public-proof/v1",
        "binding_hash": authority["binding_hash"],
        "generation": authority["generation"],
        "page_id": authority["page_id"],
        "vm_id": authority["vm_id"],
        "expires_at_unix_ms": authority["expires_at_unix_ms"],
        "credential_hash": authority["turn"]["credential_hash"],
        "egress": {
            "stream_id": authority["egress"]["stream_id"],
            "target": authority["egress"]["target"],
            "runtime_socket_hash": sha256_label(
                authority["egress"]["runtime_socket_path"]
                    .as_str()
                    .unwrap_or_default()
                    .as_bytes()
            ),
            "vsock_port": authority["egress"]["vsock_port"],
        },
        "media": {
            "stream_id": authority["media"]["stream_id"],
            "target": authority["media"]["target"],
            "runtime_socket_hash": sha256_label(
                authority["media"]["runtime_socket_path"]
                    .as_str()
                    .unwrap_or_default()
                    .as_bytes()
            ),
            "vsock_port": authority["media"]["vsock_port"],
        },
        "turn": {
            "guest_url": authority["turn"]["guest_url"],
            "listen_host": authority["turn"]["listen_host"],
            "listen_port": authority["turn"]["listen_port"],
            "advertised_host": authority["turn"]["advertised_host"],
            "relay_host": authority["turn"]["relay_host"],
            "relay_port_min": authority["turn"]["relay_port_min"],
            "relay_port_max": authority["turn"]["relay_port_max"],
            "protocols": authority["turn"]["protocols"],
        },
        "effects": receipt["effects"],
    }))
}

pub(super) fn value_contains_transport_secret(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(value_contains_transport_secret),
        Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "credential" | "auth_secret" | "transport_secret"
            ) || value_contains_transport_secret(value)
        }),
        _ => false,
    }
}

pub(super) fn value_contains_exact_transport_secret(value: &Value, secret: &Value) -> bool {
    let credential = secret.get("credential").and_then(Value::as_str);
    let auth_secret = secret.get("auth_secret").and_then(Value::as_str);
    match value {
        Value::String(text) => {
            credential == Some(text.as_str()) || auth_secret == Some(text.as_str())
        }
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_exact_transport_secret(value, secret)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_exact_transport_secret(value, secret)),
        _ => false,
    }
}

fn validate_stream(
    value: Option<&Value>,
    loopback_target: bool,
) -> Result<(String, String, u32), String> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| "Browser VZ transport stream must be an object".to_string())?;
    let keys = [
        "schema",
        "stream_id",
        "target",
        "runtime_socket_path",
        "vsock_port",
    ];
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || object.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vz-transport-stream/v1")
    {
        return Err("Browser VZ transport stream shape is invalid".to_string());
    }
    let stream_id = require_safe_id(object.get("stream_id"), "stream_id", 512)?;
    let target = object
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser VZ transport target is missing".to_string())?;
    let parsed = url::Url::parse(target)
        .map_err(|err| format!("Browser VZ transport target is invalid: {err}"))?;
    if !matches!(parsed.scheme(), "tcp" | "tls")
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "Browser VZ transport target requires tcp/tls and an explicit port".to_string(),
        );
    }
    if loopback_target {
        let host = parsed
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .filter(std::net::IpAddr::is_loopback)
            .ok_or_else(|| "Browser VZ media target must be loopback".to_string())?;
        let _ = host;
    }
    let runtime_path = object
        .get("runtime_socket_path")
        .and_then(Value::as_str)
        .filter(|path| {
            path.starts_with('/') && path.len() <= 103 && !path.contains(['\0', '\r', '\n'])
        })
        .ok_or_else(|| "Browser VZ transport Runtime socket is invalid".to_string())?;
    let port = require_u32(object.get("vsock_port"), "vsock_port")?;
    Ok((
        stream_id.to_string(),
        format!("{}\n{runtime_path}", parsed.as_str()),
        port,
    ))
}

fn validate_turn(value: Option<&Value>, expires_at: u64) -> Result<(), String> {
    let object = value
        .and_then(Value::as_object)
        .ok_or_else(|| "Browser VZ TURN authority must be an object".to_string())?;
    let keys = [
        "schema",
        "guest_url",
        "guest_host",
        "guest_port",
        "listen_host",
        "listen_port",
        "advertised_host",
        "relay_host",
        "relay_port_min",
        "relay_port_max",
        "protocols",
        "username",
        "credential_hash",
        "auth_secret_hash",
    ];
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || object.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vz-turn-authority/v1")
    {
        return Err("Browser VZ TURN authority shape is invalid".to_string());
    }
    let guest_host = require_loopback(object.get("guest_host"), "guest_host")?;
    require_loopback(object.get("listen_host"), "listen_host")?;
    let guest_port = require_u16(object.get("guest_port"), "guest_port")?;
    require_u16(object.get("listen_port"), "listen_port")?;
    let relay_min = require_u16(object.get("relay_port_min"), "relay_port_min")?;
    let relay_max = require_u16(object.get("relay_port_max"), "relay_port_max")?;
    if relay_min > relay_max || relay_max - relay_min + 1 > 64 {
        return Err("Browser VZ TURN relay range is invalid".to_string());
    }
    let advertised = object
        .get("advertised_host")
        .and_then(Value::as_str)
        .filter(|host| {
            !host.is_empty()
                && host.len() <= 253
                && !host.contains(['\0', '\r', '\n', '/', '\\', ' ', '\t'])
        })
        .ok_or_else(|| "Browser VZ TURN advertised endpoint is invalid".to_string())?;
    let _ = advertised;
    let relay_host = object
        .get("relay_host")
        .and_then(Value::as_str)
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .filter(|host| !host.is_unspecified() && !host.is_multicast())
        .ok_or_else(|| "Browser VZ TURN relay host is invalid".to_string())?;
    let _ = relay_host;
    if object.get("guest_url").and_then(Value::as_str)
        != Some(format!("turn:{guest_host}:{guest_port}?transport=tcp").as_str())
        || object.get("protocols") != Some(&json!(["turn", "tcp"]))
    {
        return Err("Browser VZ TURN protocol binding is invalid".to_string());
    }
    let username = object
        .get("username")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 256 && !value.contains(['\0', '\r', '\n']))
        .ok_or_else(|| "Browser VZ TURN username is invalid".to_string())?;
    let expiry = username
        .split_once(':')
        .and_then(|(expiry, suffix)| (!suffix.is_empty()).then_some(expiry))
        .and_then(|expiry| expiry.parse::<u64>().ok())
        .and_then(|expiry| expiry.checked_mul(1_000))
        .ok_or_else(|| "Browser VZ TURN username expiry is invalid".to_string())?;
    if expiry != expires_at {
        return Err("Browser VZ TURN username expiry changed".to_string());
    }
    require_sha256(object.get("credential_hash"), "credential_hash")?;
    require_sha256(object.get("auth_secret_hash"), "auth_secret_hash")?;
    Ok(())
}

fn require_safe_id<'a>(
    value: Option<&'a Value>,
    field: &str,
    max: usize,
) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= max && is_safe_id(text))
        .ok_or_else(|| format!("Browser VZ transport {field} is invalid"))
}

fn require_sha256(value: Option<&Value>, field: &str) -> Result<(), String> {
    if value.and_then(Value::as_str).is_some_and(|text| {
        text.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) {
        Ok(())
    } else {
        Err(format!("Browser VZ transport {field} is invalid"))
    }
}

fn require_secret<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 512 && !text.contains(['\0', '\r', '\n']))
        .ok_or_else(|| format!("Browser VZ transport secret {field} is invalid"))
}

fn require_loopback(value: Option<&Value>, field: &str) -> Result<String, String> {
    value
        .and_then(Value::as_str)
        .and_then(|text| {
            text.parse::<std::net::IpAddr>()
                .ok()
                .filter(std::net::IpAddr::is_loopback)
                .map(|_| text.to_string())
        })
        .ok_or_else(|| format!("Browser VZ TURN {field} must be loopback"))
}

fn require_u32(value: Option<&Value>, field: &str) -> Result<u32, String> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Browser VZ transport {field} is invalid"))
}

fn require_u16(value: Option<&Value>, field: &str) -> Result<u16, String> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Browser VZ TURN {field} is invalid"))
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            Value::Object(values) => {
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                let mut sorted = serde_json::Map::new();
                for key in keys {
                    sorted.insert(key.clone(), canonical(&values[key]));
                }
                Value::Object(sorted)
            }
            value => value.clone(),
        }
    }
    serde_json::to_vec(&canonical(value)).map_err(|err| err.to_string())
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Value, Value) {
        let expires_at = (current_unix_ms() / 1_000 + 300) * 1_000;
        let username = format!("{}:adapterproof", expires_at / 1_000);
        let auth_secret = "adapter-transport-secret";
        let mut mac = Hmac::<Sha1>::new_from_slice(auth_secret.as_bytes()).unwrap();
        mac.update(username.as_bytes());
        let credential =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let mut authority = json!({
            "schema": VZ_TRANSPORT_AUTHORITY_SCHEMA,
            "generation": format!("sha256:{}", "a".repeat(64)),
            "page_id": "page:vz-adapter",
            "vm_id": "vm:vz-adapter",
            "principal_id": "person:local:adapter",
            "egress": {
                "schema": "elastos.browser.vz-transport-stream/v1",
                "stream_id": "stream:vz-adapter",
                "target": "tls://example.invalid:443",
                "runtime_socket_path": "/tmp/vz-adapter-egress.sock",
                "vsock_port": 19091,
            },
            "media": {
                "schema": "elastos.browser.vz-transport-stream/v1",
                "stream_id": "stream:vz-adapter-media",
                "target": "tcp://127.0.0.1:49160",
                "runtime_socket_path": "/tmp/vz-adapter-media.sock",
                "vsock_port": 19094,
            },
            "turn": {
                "schema": "elastos.browser.vz-turn-authority/v1",
                "guest_url": "turn:127.0.0.1:3478?transport=tcp",
                "guest_host": "127.0.0.1",
                "guest_port": 3478,
                "listen_host": "127.0.0.1",
                "listen_port": 49160,
                "advertised_host": "192.0.2.10",
                "relay_host": "192.0.2.10",
                "relay_port_min": 55000,
                "relay_port_max": 55019,
                "protocols": ["turn", "tcp"],
                "username": username,
                "credential_hash": sha256_label(credential.as_bytes()),
                "auth_secret_hash": sha256_label(auth_secret.as_bytes()),
            },
            "bootstrap_vsock_port": 19093,
            "expires_at_unix_ms": expires_at,
        });
        authority["binding_hash"] = json!(sha256_label(&canonical_json_bytes(&authority).unwrap()));
        let secret = json!({
            "schema": VZ_TRANSPORT_SECRET_SCHEMA,
            "binding_hash": authority["binding_hash"],
            "credential": credential,
            "auth_secret": auth_secret,
        });
        (authority, secret)
    }

    fn receipt(authority: &Value) -> Value {
        json!({
            "schema": "elastos.browser.vz-transport-effect-receipt/v1",
            "binding_hash": authority["binding_hash"],
            "generation": authority["generation"],
            "page_id": authority["page_id"],
            "vm_id": authority["vm_id"],
            "expires_at_unix_ms": authority["expires_at_unix_ms"],
            "terminal": true,
            "effects": {
                "vz_network_devices_zero": true,
                "guest_bootstrap_validated": true,
                "guest_loopback_only": true,
                "guest_interfaces": ["lo"],
                "guest_default_route_absent": true,
                "guest_direct_network_absent": true,
                "ordinary_stream_fixed_target": true,
                "media_stream_fixed_target": true,
                "turn_launch_owned": true,
                "turn_listener_loopback": true,
                "hibernation_disabled": true,
            },
        })
    }

    #[test]
    fn adapter_rejects_substitution_replay_and_malformed_effect_receipts() {
        let (authority, secret) = fixture();
        validate_vz_transport_launch(
            &authority,
            &secret,
            authority["generation"].as_str().unwrap(),
            "stream:vz-adapter",
            "page:vz-adapter",
            "vm:vz-adapter",
            Some("person:local:adapter"),
        )
        .unwrap();

        let mut substituted = authority.clone();
        substituted["page_id"] = json!("page:vz-other");
        assert!(validate_vz_transport_authority(&substituted).is_err());

        let mut replay = secret.clone();
        replay["binding_hash"] = json!(format!("sha256:{}", "b".repeat(64)));
        assert!(validate_vz_transport_secret(&authority, &replay).is_err());

        let mut malformed = receipt(&authority);
        malformed["effects"]["turn_launch_owned"] = json!(false);
        assert!(validate_vz_transport_effect_receipt(&malformed, &authority).is_err());
        assert!(value_contains_exact_transport_secret(
            &json!({"renamed_private_value": secret["credential"]}),
            &secret,
        ));
    }
}
