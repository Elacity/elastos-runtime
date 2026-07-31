//! Runtime-owned Browser VZ transport authority.
//!
//! Only the public, hash-bound authority returned by this module is durable.
//! The TURN REST secret and its derived launch credential remain transient and
//! are carried only in the private launch request.

use super::*;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use rand::RngCore as _;
use serde::Deserialize;
use sha1::Sha1;
use std::fs::File;
use std::io::Read as _;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub(in crate::api::gateway) const BROWSER_VZ_TRANSPORT_AUTHORITY_SCHEMA: &str =
    "elastos.browser.vz-transport-authority/v1";
pub(in crate::api::gateway) const BROWSER_VZ_TRANSPORT_SECRET_SCHEMA: &str =
    "elastos.browser.vz-transport-secret/v1";

const BROWSER_VZ_TRANSPORT_CONFIG_SCHEMA: &str = "elastos.browser.vz-transport-config/v1";
const BROWSER_VZ_TRANSPORT_STREAM_SCHEMA: &str = "elastos.browser.vz-transport-stream/v1";
const BROWSER_VZ_TURN_AUTHORITY_SCHEMA: &str = "elastos.browser.vz-turn-authority/v1";
const MAX_BROWSER_VZ_TRANSPORT_CONFIG_BYTES: u64 = 32 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserVzTransportConfig {
    schema: String,
    enabled: bool,
    turn_listen_host: String,
    turn_advertised_host: String,
    turn_relay_host: String,
    turn_port_start: u16,
    turn_port_end: u16,
    turn_relay_port_start: u16,
    turn_relay_port_end: u16,
    turn_relay_block_size: u16,
    guest_turn_host: String,
    guest_turn_port: u16,
    bootstrap_vsock_port: u32,
    egress_vsock_port: u32,
    media_vsock_port: u32,
    ttl_secs: u64,
}

pub(in crate::api::gateway) struct BrowserVzTransportLaunch {
    pub(in crate::api::gateway) authority: serde_json::Value,
    pub(in crate::api::gateway) secret: serde_json::Value,
}

pub(in crate::api::gateway) struct BrowserVzTransportLaunchBinding<'a> {
    pub(in crate::api::gateway) generation: &'a str,
    pub(in crate::api::gateway) page_id: &'a str,
    pub(in crate::api::gateway) vm_id: &'a str,
    pub(in crate::api::gateway) principal_id: &'a str,
    pub(in crate::api::gateway) egress_stream_id: &'a str,
    pub(in crate::api::gateway) egress_target: &'a str,
    pub(in crate::api::gateway) egress_runtime_socket_path: &'a str,
}

pub(in crate::api::gateway) fn prepare_browser_vz_transport_launch(
    data_dir: &Path,
    binding: BrowserVzTransportLaunchBinding<'_>,
) -> Result<Option<BrowserVzTransportLaunch>, String> {
    let Some(config) = read_browser_vz_transport_config(data_dir)? else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }
    validate_browser_vz_transport_config(&config)?;
    for (label, value) in [
        ("generation", binding.generation),
        ("page_id", binding.page_id),
        ("vm_id", binding.vm_id),
        ("principal_id", binding.principal_id),
        ("egress_stream_id", binding.egress_stream_id),
    ] {
        if value.is_empty() || value.len() > 512 || !is_safe_runtime_id(value) {
            return Err(format!(
                "Browser VZ transport {label} must be a bounded safe identifier"
            ));
        }
    }
    validate_absolute_transport_socket_path(
        "egress_runtime_socket_path",
        binding.egress_runtime_socket_path,
    )?;
    let egress_target = validate_transport_target(binding.egress_target, false)?;

    let media_stream_id = format!(
        "stream:vz-media:{}",
        hex::encode(Sha256::digest(
            format!(
                "{}\n{}\n{}\nmedia",
                binding.generation, binding.page_id, binding.vm_id
            )
            .as_bytes()
        ))
    );
    let media_runtime_socket_path = browser_runtime_stream_socket_path(data_dir, &media_stream_id)
        .map_err(|err| format!("Browser VZ media stream path is invalid: {err}"))?
        .to_string_lossy()
        .to_string();
    validate_absolute_transport_socket_path(
        "media_runtime_socket_path",
        &media_runtime_socket_path,
    )?;

    let turn_port = select_port(
        binding.generation,
        "turn-listener",
        config.turn_port_start,
        config.turn_port_end,
    )?;
    let (relay_port_min, relay_port_max) = select_port_block(
        binding.generation,
        "turn-relay",
        config.turn_relay_port_start,
        config.turn_relay_port_end,
        config.turn_relay_block_size,
    )?;
    let expires_at_unix_secs = current_unix_secs()?
        .checked_add(config.ttl_secs)
        .ok_or_else(|| "Browser VZ transport expiry overflowed".to_string())?;
    let expires_at_unix_ms = expires_at_unix_secs
        .checked_mul(1_000)
        .ok_or_else(|| "Browser VZ transport expiry overflowed".to_string())?;

    let mut auth_secret_bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut auth_secret_bytes);
    let auth_secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(auth_secret_bytes);
    let generation_suffix = binding
        .generation
        .strip_prefix("sha256:")
        .unwrap_or(binding.generation)
        .chars()
        .take(24)
        .collect::<String>();
    let username = format!("{expires_at_unix_secs}:{generation_suffix}");
    let credential = turn_rest_credential(&auth_secret, &username)?;
    let credential_hash = sha256_label(credential.as_bytes());
    let auth_secret_hash = sha256_label(auth_secret.as_bytes());

    let guest_url = format!(
        "turn:{}:{}?transport=tcp",
        config.guest_turn_host, config.guest_turn_port
    );
    let media_target = format!("tcp://{}:{turn_port}", config.turn_listen_host);
    let authority_without_hash = serde_json::json!({
        "schema": BROWSER_VZ_TRANSPORT_AUTHORITY_SCHEMA,
        "generation": binding.generation,
        "page_id": binding.page_id,
        "vm_id": binding.vm_id,
        "principal_id": binding.principal_id,
        "egress": {
            "schema": BROWSER_VZ_TRANSPORT_STREAM_SCHEMA,
            "stream_id": binding.egress_stream_id,
            "target": egress_target,
            "runtime_socket_path": binding.egress_runtime_socket_path,
            "vsock_port": config.egress_vsock_port,
        },
        "media": {
            "schema": BROWSER_VZ_TRANSPORT_STREAM_SCHEMA,
            "stream_id": media_stream_id,
            "target": media_target,
            "runtime_socket_path": media_runtime_socket_path,
            "vsock_port": config.media_vsock_port,
        },
        "turn": {
            "schema": BROWSER_VZ_TURN_AUTHORITY_SCHEMA,
            "guest_url": guest_url,
            "guest_host": config.guest_turn_host,
            "guest_port": config.guest_turn_port,
            "listen_host": config.turn_listen_host,
            "listen_port": turn_port,
            "advertised_host": config.turn_advertised_host,
            "relay_host": config.turn_relay_host,
            "relay_port_min": relay_port_min,
            "relay_port_max": relay_port_max,
            "protocols": ["turn", "tcp"],
            "username": username,
            "credential_hash": credential_hash,
            "auth_secret_hash": auth_secret_hash,
        },
        "bootstrap_vsock_port": config.bootstrap_vsock_port,
        "expires_at_unix_ms": expires_at_unix_ms,
    });
    let binding_hash = sha256_label(canonical_json_bytes(&authority_without_hash)?.as_slice());
    let mut authority = authority_without_hash;
    authority
        .as_object_mut()
        .expect("Browser VZ authority is an object")
        .insert(
            "binding_hash".to_string(),
            serde_json::Value::String(binding_hash.clone()),
        );
    validate_browser_vz_transport_authority(&authority)?;
    let secret = serde_json::json!({
        "schema": BROWSER_VZ_TRANSPORT_SECRET_SCHEMA,
        "binding_hash": binding_hash,
        "credential": credential,
        "auth_secret": auth_secret,
    });
    validate_browser_vz_transport_secret(&authority, &secret)?;
    Ok(Some(BrowserVzTransportLaunch { authority, secret }))
}

pub(in crate::api::gateway) fn validate_browser_vz_transport_authority(
    authority: &serde_json::Value,
) -> Result<(), String> {
    let object = authority
        .as_object()
        .ok_or_else(|| "Browser VZ transport authority must be an object".to_string())?;
    let exact_keys = [
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
    if object.len() != exact_keys.len()
        || exact_keys.iter().any(|key| !object.contains_key(*key))
        || authority.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_VZ_TRANSPORT_AUTHORITY_SCHEMA)
    {
        return Err("Browser VZ transport authority shape is invalid".to_string());
    }
    for field in ["binding_hash", "generation"] {
        require_sha256_label(authority, field)?;
    }
    for field in ["page_id", "vm_id", "principal_id"] {
        require_safe_id(authority, field, 512)?;
    }
    let egress = validate_transport_stream(authority.get("egress"), false)?;
    let media = validate_transport_stream(authority.get("media"), true)?;
    if egress.0 == media.0 || egress.1 == media.1 || egress.2 == media.2 {
        return Err("Browser VZ egress and media bindings must be distinct".to_string());
    }
    let bootstrap_vsock_port = authority
        .get("bootstrap_vsock_port")
        .and_then(serde_json::Value::as_u64)
        .filter(|port| *port > 0 && *port <= u32::MAX as u64)
        .ok_or_else(|| "Browser VZ bootstrap vsock port is invalid".to_string())?;
    if [egress.2, media.2].contains(&bootstrap_vsock_port) {
        return Err("Browser VZ vsock ports must be distinct".to_string());
    }
    let expires_at = authority
        .get("expires_at_unix_ms")
        .and_then(serde_json::Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "Browser VZ transport expiry is invalid".to_string())?;
    let now = current_unix_secs()?;
    if expires_at / 1_000
        > now
            .checked_add(24 * 60 * 60)
            .ok_or_else(|| "Browser VZ transport expiry overflowed".to_string())?
    {
        return Err("Browser VZ transport expiry exceeds the bounded horizon".to_string());
    }
    validate_turn_authority(authority.get("turn"), expires_at)?;

    let mut unsigned = authority.clone();
    unsigned
        .as_object_mut()
        .expect("validated Browser VZ authority object")
        .remove("binding_hash");
    let expected_hash = sha256_label(canonical_json_bytes(&unsigned)?.as_slice());
    if authority
        .get("binding_hash")
        .and_then(serde_json::Value::as_str)
        != Some(expected_hash.as_str())
    {
        return Err("Browser VZ transport authority binding hash mismatch".to_string());
    }
    if serde_json::to_vec(authority).map_or(true, |bytes| bytes.len() > 32 * 1024) {
        return Err("Browser VZ transport authority is too large".to_string());
    }
    Ok(())
}

pub(in crate::api::gateway) fn validate_live_browser_vz_transport_authority(
    authority: &serde_json::Value,
) -> Result<(), String> {
    validate_browser_vz_transport_authority(authority)?;
    let expires_at = authority
        .get("expires_at_unix_ms")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "Browser VZ transport expiry is invalid".to_string())?;
    if expires_at / 1_000 <= current_unix_secs()? {
        return Err("Browser VZ transport authority is expired".to_string());
    }
    Ok(())
}

pub(in crate::api::gateway) fn validate_browser_vz_transport_secret(
    authority: &serde_json::Value,
    secret: &serde_json::Value,
) -> Result<(), String> {
    validate_live_browser_vz_transport_authority(authority)?;
    let object = secret
        .as_object()
        .ok_or_else(|| "Browser VZ transport secret must be an object".to_string())?;
    if object.len() != 4
        || !["schema", "binding_hash", "credential", "auth_secret"]
            .iter()
            .all(|key| object.contains_key(*key))
        || secret.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_VZ_TRANSPORT_SECRET_SCHEMA)
        || secret.get("binding_hash") != authority.get("binding_hash")
    {
        return Err("Browser VZ transport secret binding is invalid".to_string());
    }
    let credential = require_bounded_secret(secret, "credential")?;
    let auth_secret = require_bounded_secret(secret, "auth_secret")?;
    if authority
        .pointer("/turn/credential_hash")
        .and_then(serde_json::Value::as_str)
        != Some(sha256_label(credential.as_bytes()).as_str())
        || authority
            .pointer("/turn/auth_secret_hash")
            .and_then(serde_json::Value::as_str)
            != Some(sha256_label(auth_secret.as_bytes()).as_str())
    {
        return Err("Browser VZ transport secret hash mismatch".to_string());
    }
    let username = authority
        .pointer("/turn/username")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Browser VZ TURN username is missing".to_string())?;
    if turn_rest_credential(auth_secret, username)? != credential {
        return Err("Browser VZ TURN REST credential is invalid".to_string());
    }
    Ok(())
}

pub(in crate::api::gateway) fn validate_browser_vz_transport_effect_receipt(
    authority: &serde_json::Value,
    receipt: &serde_json::Value,
) -> Result<(), String> {
    validate_browser_vz_transport_authority(authority)?;
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
        .and_then(serde_json::Value::as_object)
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
        || receipt.get("schema").and_then(serde_json::Value::as_str)
            != Some("elastos.browser.vz-transport-effect-receipt/v1")
        || receipt.get("binding_hash") != authority.get("binding_hash")
        || receipt.get("generation") != authority.get("generation")
        || receipt.get("page_id") != authority.get("page_id")
        || receipt.get("vm_id") != authority.get("vm_id")
        || receipt.get("expires_at_unix_ms") != authority.get("expires_at_unix_ms")
        || receipt.get("terminal").and_then(serde_json::Value::as_bool) != Some(true)
        || effects.get("guest_interfaces") != Some(&serde_json::json!(["lo"]))
        || effect_keys
            .iter()
            .filter(|key| **key != "guest_interfaces")
            .any(|key| effects.get(*key).and_then(serde_json::Value::as_bool) != Some(true))
        || browser_vz_value_contains_secret(receipt)
    {
        return Err("Browser VZ provider transport effect receipt is not exact".to_string());
    }
    Ok(())
}

pub(in crate::api::gateway) fn browser_vz_public_transport_proof(
    authority: &serde_json::Value,
    receipt: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    validate_browser_vz_transport_effect_receipt(authority, receipt)?;
    Ok(serde_json::json!({
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

fn browser_vz_value_contains_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values.iter().any(browser_vz_value_contains_secret),
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "credential" | "auth_secret" | "transport_secret"
            ) || browser_vz_value_contains_secret(value)
        }),
        _ => false,
    }
}

fn read_browser_vz_transport_config(
    data_dir: &Path,
) -> Result<Option<BrowserVzTransportConfig>, String> {
    let path = data_dir
        .join("config")
        .join("browser-vz-vsock-transport.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "Browser VZ transport config metadata failed at {}: {err}",
                path.display()
            ))
        }
    };
    if !metadata.is_file() || metadata.len() > MAX_BROWSER_VZ_TRANSPORT_CONFIG_BYTES {
        return Err("Browser VZ transport config must be a bounded regular file".to_string());
    }
    #[cfg(unix)]
    {
        if metadata.mode() & 0o077 != 0 {
            return Err("Browser VZ transport config must be owner-only".to_string());
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Browser VZ transport config must be owned by Runtime".to_string());
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(&path)
        .and_then(|file| {
            file.take(MAX_BROWSER_VZ_TRANSPORT_CONFIG_BYTES + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|err| format!("Browser VZ transport config read failed: {err}"))?;
    if bytes.len() as u64 > MAX_BROWSER_VZ_TRANSPORT_CONFIG_BYTES {
        return Err("Browser VZ transport config is too large".to_string());
    }
    let config = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Browser VZ transport config is invalid JSON: {err}"))?;
    Ok(Some(config))
}

fn validate_browser_vz_transport_config(config: &BrowserVzTransportConfig) -> Result<(), String> {
    if config.schema != BROWSER_VZ_TRANSPORT_CONFIG_SCHEMA {
        return Err("Browser VZ transport config schema is unsupported".to_string());
    }
    for (label, host) in [
        ("turn_listen_host", config.turn_listen_host.as_str()),
        ("guest_turn_host", config.guest_turn_host.as_str()),
    ] {
        let ip: std::net::IpAddr = host
            .parse()
            .map_err(|_| format!("Browser VZ {label} must be a literal loopback IP"))?;
        if !ip.is_loopback() {
            return Err(format!("Browser VZ {label} must be loopback"));
        }
    }
    validate_endpoint_host("turn_advertised_host", &config.turn_advertised_host)?;
    let relay_ip: std::net::IpAddr = config
        .turn_relay_host
        .parse()
        .map_err(|_| "Browser VZ turn_relay_host must be a literal IP".to_string())?;
    if relay_ip.is_unspecified() || relay_ip.is_multicast() {
        return Err("Browser VZ turn_relay_host is invalid".to_string());
    }
    validate_port_range(
        "turn listener",
        config.turn_port_start,
        config.turn_port_end,
    )?;
    validate_port_range(
        "TURN relay",
        config.turn_relay_port_start,
        config.turn_relay_port_end,
    )?;
    if !(2..=64).contains(&config.turn_relay_block_size)
        || u32::from(config.turn_relay_block_size)
            > u32::from(config.turn_relay_port_end) - u32::from(config.turn_relay_port_start) + 1
    {
        return Err("Browser VZ TURN relay block size is invalid".to_string());
    }
    if config.guest_turn_port == 0 {
        return Err("Browser VZ guest TURN port must be non-zero".to_string());
    }
    let vsock_ports = [
        config.bootstrap_vsock_port,
        config.egress_vsock_port,
        config.media_vsock_port,
    ];
    if vsock_ports.contains(&0)
        || vsock_ports[0] == vsock_ports[1]
        || vsock_ports[0] == vsock_ports[2]
        || vsock_ports[1] == vsock_ports[2]
    {
        return Err("Browser VZ vsock ports must be non-zero and distinct".to_string());
    }
    if !(120..=3_600).contains(&config.ttl_secs) {
        return Err("Browser VZ transport ttl_secs must be 120..3600".to_string());
    }
    Ok(())
}

fn validate_turn_authority(
    value: Option<&serde_json::Value>,
    expires_at_unix_ms: u64,
) -> Result<(), String> {
    let turn = value
        .and_then(serde_json::Value::as_object)
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
    if turn.len() != keys.len()
        || keys.iter().any(|key| !turn.contains_key(*key))
        || turn.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_VZ_TURN_AUTHORITY_SCHEMA)
    {
        return Err("Browser VZ TURN authority shape is invalid".to_string());
    }
    let guest_host = require_loopback_ip(turn.get("guest_host"), "guest_host")?;
    let listen_host = require_loopback_ip(turn.get("listen_host"), "listen_host")?;
    let guest_port = require_json_port(turn.get("guest_port"), "guest_port")?;
    let listen_port = require_json_port(turn.get("listen_port"), "listen_port")?;
    let advertised_host = turn
        .get("advertised_host")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Browser VZ TURN advertised_host is invalid".to_string())?;
    validate_endpoint_host("TURN advertised_host", advertised_host)?;
    let relay_host = turn
        .get("relay_host")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.parse::<std::net::IpAddr>().ok())
        .filter(|value| !value.is_unspecified() && !value.is_multicast())
        .ok_or_else(|| "Browser VZ TURN relay_host must be a literal usable IP".to_string())?;
    let relay_min = require_json_port(turn.get("relay_port_min"), "relay_port_min")?;
    let relay_max = require_json_port(turn.get("relay_port_max"), "relay_port_max")?;
    if relay_min > relay_max {
        return Err("Browser VZ TURN relay range is invalid".to_string());
    }
    if turn.get("guest_url").and_then(serde_json::Value::as_str)
        != Some(format!("turn:{guest_host}:{guest_port}?transport=tcp").as_str())
        || turn.get("protocols") != Some(&serde_json::json!(["turn", "tcp"]))
    {
        return Err("Browser VZ TURN protocol binding is invalid".to_string());
    }
    let _ = (listen_host, listen_port, relay_host);
    let username = turn
        .get("username")
        .and_then(serde_json::Value::as_str)
        .filter(|value| value.len() <= 256 && !value.contains(['\0', '\r', '\n']))
        .ok_or_else(|| "Browser VZ TURN username is invalid".to_string())?;
    let username_expiry = username
        .split_once(':')
        .and_then(|(value, suffix)| {
            (!suffix.is_empty())
                .then(|| value.parse::<u64>().ok())
                .flatten()
        })
        .ok_or_else(|| "Browser VZ TURN username does not bind an expiry".to_string())?;
    if username_expiry.checked_mul(1_000) != Some(expires_at_unix_ms) {
        return Err("Browser VZ TURN credential expiry mismatch".to_string());
    }
    for field in ["credential_hash", "auth_secret_hash"] {
        let value = turn
            .get(field)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("Browser VZ TURN {field} is missing"))?;
        if !is_sha256_label(value) {
            return Err(format!("Browser VZ TURN {field} is invalid"));
        }
    }
    Ok(())
}

fn validate_transport_stream(
    value: Option<&serde_json::Value>,
    require_loopback_target: bool,
) -> Result<(String, String, u64), String> {
    let stream = value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "Browser VZ transport stream must be an object".to_string())?;
    let keys = [
        "schema",
        "stream_id",
        "target",
        "runtime_socket_path",
        "vsock_port",
    ];
    if stream.len() != keys.len()
        || keys.iter().any(|key| !stream.contains_key(*key))
        || stream.get("schema").and_then(serde_json::Value::as_str)
            != Some(BROWSER_VZ_TRANSPORT_STREAM_SCHEMA)
    {
        return Err("Browser VZ transport stream shape is invalid".to_string());
    }
    let stream_id = stream
        .get("stream_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 512 && is_safe_runtime_id(value))
        .ok_or_else(|| "Browser VZ transport stream_id is invalid".to_string())?;
    let target = stream
        .get("target")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Browser VZ transport target is missing".to_string())?;
    let target = validate_transport_target(target, require_loopback_target)?;
    let runtime_path = stream
        .get("runtime_socket_path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Browser VZ transport runtime socket is missing".to_string())?;
    validate_absolute_transport_socket_path("runtime_socket_path", runtime_path)?;
    let vsock_port = stream
        .get("vsock_port")
        .and_then(serde_json::Value::as_u64)
        .filter(|port| *port > 0 && *port <= u32::MAX as u64)
        .ok_or_else(|| "Browser VZ transport vsock port is invalid".to_string())?;
    Ok((
        stream_id.to_string(),
        format!("{target}\n{runtime_path}"),
        vsock_port,
    ))
}

fn validate_transport_target(value: &str, require_loopback: bool) -> Result<String, String> {
    let parsed = url::Url::parse(value)
        .map_err(|err| format!("Browser VZ transport target is invalid: {err}"))?;
    if !matches!(parsed.scheme(), "tcp" | "tls")
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Browser VZ transport target must use an explicit tcp/tls port".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "Browser VZ transport target requires a host".to_string())?;
    if require_loopback {
        let ip: std::net::IpAddr = host
            .parse()
            .map_err(|_| "Browser VZ media target must use a literal loopback IP".to_string())?;
        if !ip.is_loopback() {
            return Err("Browser VZ media target must be loopback".to_string());
        }
    }
    Ok(parsed.to_string())
}

fn select_port(generation: &str, label: &str, start: u16, end: u16) -> Result<u16, String> {
    validate_port_range(label, start, end)?;
    let digest = Sha256::digest(format!("{generation}\n{label}").as_bytes());
    let value = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    let size = u64::from(end) - u64::from(start) + 1;
    Ok(u16::try_from(u64::from(start) + value % size).expect("selected port fits"))
}

fn select_port_block(
    generation: &str,
    label: &str,
    start: u16,
    end: u16,
    block_size: u16,
) -> Result<(u16, u16), String> {
    validate_port_range(label, start, end)?;
    let available = u32::from(end) - u32::from(start) + 1;
    if block_size == 0 || u32::from(block_size) > available {
        return Err(format!("Browser VZ {label} block size is invalid"));
    }
    let blocks = available / u32::from(block_size);
    let digest = Sha256::digest(format!("{generation}\n{label}").as_bytes());
    let value = u64::from_be_bytes(digest[..8].try_into().expect("SHA-256 prefix"));
    let offset = u32::try_from(value % u64::from(blocks)).expect("block offset fits")
        * u32::from(block_size);
    let min = u32::from(start) + offset;
    let max = min + u32::from(block_size) - 1;
    Ok((
        u16::try_from(min).expect("relay min fits"),
        u16::try_from(max).expect("relay max fits"),
    ))
}

fn validate_port_range(label: &str, start: u16, end: u16) -> Result<(), String> {
    if start == 0 || end == 0 || start > end {
        return Err(format!("Browser VZ {label} port range is invalid"));
    }
    Ok(())
}

fn validate_endpoint_host(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 253
        || value.contains(['\0', '\r', '\n', '/', '\\', ' ', '\t'])
    {
        return Err(format!("Browser VZ {label} is invalid"));
    }
    Ok(())
}

fn validate_absolute_transport_socket_path(label: &str, value: &str) -> Result<(), String> {
    if !value.starts_with('/') || value.len() > 103 || value.contains(['\0', '\r', '\n']) {
        return Err(format!("Browser VZ {label} is invalid"));
    }
    Ok(())
}

fn require_safe_id(value: &serde_json::Value, field: &str, max_len: usize) -> Result<(), String> {
    if value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| !text.is_empty() && text.len() <= max_len && is_safe_runtime_id(text))
    {
        Ok(())
    } else {
        Err(format!("Browser VZ transport {field} is invalid"))
    }
}

fn require_sha256_label(value: &serde_json::Value, field: &str) -> Result<(), String> {
    if value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_sha256_label)
    {
        Ok(())
    } else {
        Err(format!("Browser VZ transport {field} is invalid"))
    }
}

fn require_loopback_ip(value: Option<&serde_json::Value>, field: &str) -> Result<String, String> {
    let text = value
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Browser VZ TURN {field} is invalid"))?;
    let ip: std::net::IpAddr = text
        .parse()
        .map_err(|_| format!("Browser VZ TURN {field} must be a literal loopback IP"))?;
    if !ip.is_loopback() {
        return Err(format!("Browser VZ TURN {field} must be loopback"));
    }
    Ok(text.to_string())
}

fn require_json_port(value: Option<&serde_json::Value>, field: &str) -> Result<u16, String> {
    value
        .and_then(serde_json::Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| format!("Browser VZ TURN {field} is invalid"))
}

fn require_bounded_secret<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= 512 && !text.contains(['\0', '\r', '\n']))
        .ok_or_else(|| format!("Browser VZ transport secret {field} is invalid"))
}

fn is_sha256_label(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn sha256_label(value: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value)))
}

fn turn_rest_credential(auth_secret: &str, username: &str) -> Result<String, String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(auth_secret.as_bytes())
        .map_err(|_| "Browser VZ TURN auth secret is invalid".to_string())?;
    mac.update(username.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

fn canonical_json_bytes(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            serde_json::Value::Object(values) => {
                let mut sorted = serde_json::Map::new();
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for key in keys {
                    sorted.insert(key.clone(), canonical(&values[key]));
                }
                serde_json::Value::Object(sorted)
            }
            value => value.clone(),
        }
    }
    serde_json::to_vec(&canonical(value)).map_err(|err| err.to_string())
}

fn current_unix_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "Browser VZ transport requires a valid system clock".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    fn write_config(root: &Path) {
        let config_dir = root.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let path = config_dir.join("browser-vz-vsock-transport.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema": BROWSER_VZ_TRANSPORT_CONFIG_SCHEMA,
                "enabled": true,
                "turn_listen_host": "127.0.0.1",
                "turn_advertised_host": "127.0.0.1",
                "turn_relay_host": "127.0.0.1",
                "turn_port_start": 43000,
                "turn_port_end": 43031,
                "turn_relay_port_start": 43100,
                "turn_relay_port_end": 43227,
                "turn_relay_block_size": 8,
                "guest_turn_host": "127.0.0.1",
                "guest_turn_port": 3478,
                "bootstrap_vsock_port": 19090,
                "egress_vsock_port": 19091,
                "media_vsock_port": 19093,
                "ttl_secs": 300
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn effect_receipt(authority: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({
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

    #[tokio::test]
    async fn reserved_generation_prepares_vz_transport_authority() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path());
        let principal_id = "person:local:vz-reservation-proof";
        let reservation = reserve_browser_launch(
            root.path(),
            principal_id,
            BrowserLaunchLifecycle {
                owner_launch_id: "launch:vz-reservation-proof".to_string(),
                url: "https://example.com/".to_string(),
                exit_id: "local-runtime".to_string(),
                engine_route_provider: "browser-engine".to_string(),
                selected_engine_adapter: Some("browser-vm-product".to_string()),
                profile_key_hash: None,
                vm_key_hash: None,
            },
        )
        .await
        .unwrap();

        let generation = reservation.generation().to_string();
        assert!(is_sha256_label(&generation));
        assert_eq!(generation.len(), "sha256:".len() + 64);
        assert_eq!(
            browser_lifecycle_hash("short-hash-proof").unwrap().len(),
            "sha256:".len() + 16
        );

        let launch = prepare_browser_vz_transport_launch(
            root.path(),
            BrowserVzTransportLaunchBinding {
                generation: &generation,
                page_id: reservation.page_id(),
                vm_id: reservation.vm_id(),
                principal_id,
                egress_stream_id: "stream:vz-reservation-proof",
                egress_target: "tls://example.com:443",
                egress_runtime_socket_path: "/tmp/elastos-vz-reservation-proof.sock",
            },
        )
        .unwrap()
        .unwrap();
        validate_browser_vz_transport_authority(&launch.authority).unwrap();
        assert_eq!(
            launch.authority["generation"].as_str(),
            Some(generation.as_str())
        );

        release_browser_launch(&reservation).await;
    }

    #[test]
    fn authority_binds_exact_launch_without_persisting_secrets() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path());
        let launch = prepare_browser_vz_transport_launch(
            root.path(),
            BrowserVzTransportLaunchBinding {
                generation:
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                page_id: "page:vz-proof",
                vm_id: "browser-vm-proof",
                principal_id: "person:local:proof",
                egress_stream_id: "stream:proof",
                egress_target: "tls://example.com:443",
                egress_runtime_socket_path: "/tmp/elastos-egress-proof.sock",
            },
        )
        .unwrap()
        .unwrap();

        validate_browser_vz_transport_authority(&launch.authority).unwrap();
        validate_browser_vz_transport_secret(&launch.authority, &launch.secret).unwrap();
        let durable = serde_json::to_string(&launch.authority).unwrap();
        let secret = serde_json::to_string(&launch.secret).unwrap();
        assert!(!durable.contains(launch.secret["credential"].as_str().unwrap()));
        assert!(!durable.contains(launch.secret["auth_secret"].as_str().unwrap()));
        assert!(secret.contains(BROWSER_VZ_TRANSPORT_SECRET_SCHEMA));
        assert_eq!(
            launch.authority.pointer("/turn/protocols"),
            Some(&serde_json::json!(["turn", "tcp"]))
        );
        assert_eq!(
            launch.authority.pointer("/turn/guest_url"),
            Some(&serde_json::json!("turn:127.0.0.1:3478?transport=tcp"))
        );
        assert_ne!(
            launch.authority.pointer("/egress/stream_id"),
            launch.authority.pointer("/media/stream_id")
        );
        let receipt = effect_receipt(&launch.authority);
        validate_browser_vz_transport_effect_receipt(&launch.authority, &receipt).unwrap();
        let public = browser_vz_public_transport_proof(&launch.authority, &receipt).unwrap();
        assert!(public.get("credential").is_none());
        assert!(public.get("auth_secret").is_none());
    }

    #[test]
    fn authority_rejects_binding_substitution_replay_and_expiry() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path());
        let launch = prepare_browser_vz_transport_launch(
            root.path(),
            BrowserVzTransportLaunchBinding {
                generation:
                    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                page_id: "page:vz-proof",
                vm_id: "browser-vm-proof",
                principal_id: "person:local:proof",
                egress_stream_id: "stream:proof",
                egress_target: "tls://example.com:443",
                egress_runtime_socket_path: "/tmp/elastos-egress-proof.sock",
            },
        )
        .unwrap()
        .unwrap();

        let mut substituted = launch.authority.clone();
        substituted["vm_id"] = serde_json::json!("browser-vm-substituted");
        assert!(validate_browser_vz_transport_authority(&substituted)
            .unwrap_err()
            .contains("binding hash"));

        let mut replayed_secret = launch.secret.clone();
        replayed_secret["binding_hash"] = serde_json::json!(
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
        assert!(
            validate_browser_vz_transport_secret(&launch.authority, &replayed_secret)
                .unwrap_err()
                .contains("binding")
        );

        let mut expired = launch.authority.clone();
        expired["expires_at_unix_ms"] = serde_json::json!(1_000);
        let suffix = expired["turn"]["username"]
            .as_str()
            .unwrap()
            .split_once(':')
            .unwrap()
            .1
            .to_string();
        expired["turn"]["username"] = serde_json::json!(format!("1:{suffix}"));
        let mut unsigned = expired.clone();
        unsigned.as_object_mut().unwrap().remove("binding_hash");
        expired["binding_hash"] =
            serde_json::json!(sha256_label(&canonical_json_bytes(&unsigned).unwrap()));
        assert!(validate_live_browser_vz_transport_authority(&expired).is_err());
        validate_browser_vz_transport_authority(&expired).unwrap();

        let mut malformed = effect_receipt(&launch.authority);
        malformed["effects"]["turn_process_owned"] = serde_json::json!(true);
        assert!(
            validate_browser_vz_transport_effect_receipt(&launch.authority, &malformed).is_err()
        );
    }

    #[test]
    fn runtime_accepts_only_exact_typed_transport_terminal_cleanup() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path());
        let launch = prepare_browser_vz_transport_launch(
            root.path(),
            BrowserVzTransportLaunchBinding {
                generation:
                    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                page_id: "page:vz-terminal",
                vm_id: "browser-vm-terminal",
                principal_id: "person:local:terminal",
                egress_stream_id: "stream:terminal",
                egress_target: "tls://example.com:443",
                egress_runtime_socket_path: "/tmp/elastos-egress-terminal.sock",
            },
        )
        .unwrap()
        .unwrap();
        let transport_receipt = effect_receipt(&launch.authority);
        let binding = serde_json::json!({
            "schema": BROWSER_ENGINE_CLEANUP_BINDING_SCHEMA,
            "page_id": launch.authority["page_id"],
            "generation": launch.authority["generation"],
            "stream_id": launch.authority["egress"]["stream_id"],
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm",
            "principal_id": launch.authority["principal_id"],
            "isolated_session": true,
            "transport_authority": launch.authority,
            "transport_receipt": transport_receipt,
        });
        let receipt = serde_json::json!({
            "schema": BROWSER_ENGINE_CLEANUP_RESULT_SCHEMA,
            "page_id": launch.authority["page_id"],
            "generation": launch.authority["generation"],
            "binding": binding,
            "terminal": true,
            "effects": {
                "page_absent": true,
                "child_absent": true,
                "vm_absent": true,
                "route_absent": true,
                "socket_absent": true,
                "transport_session_absent": true,
                "turn_process_absent": true,
                "turn_listener_absent": true,
                "turn_relay_ports_absent": true,
                "ordinary_vsock_bridge_absent": true,
                "media_vsock_bridge_absent": true,
                "bootstrap_vsock_bridge_absent": true,
                "hibernation_state_absent": true,
            },
        });
        super::super::validate_browser_dispatched_transport_terminal_receipt(
            &receipt,
            &launch.authority,
            launch.authority["generation"].as_str().unwrap(),
            launch.authority["egress"]["stream_id"].as_str().unwrap(),
            Some("browser-vm-product"),
        )
        .unwrap();

        let mut malformed = receipt.clone();
        malformed["effects"]["turn_relay_ports_absent"] = serde_json::json!(false);
        assert!(
            super::super::validate_browser_dispatched_transport_terminal_receipt(
                &malformed,
                &launch.authority,
                launch.authority["generation"].as_str().unwrap(),
                launch.authority["egress"]["stream_id"].as_str().unwrap(),
                Some("browser-vm-product"),
            )
            .is_err()
        );

        let mut secret_bearing = receipt;
        secret_bearing["credential"] = serde_json::json!("must-not-escape");
        assert!(
            super::super::validate_browser_dispatched_transport_terminal_receipt(
                &secret_bearing,
                &launch.authority,
                launch.authority["generation"].as_str().unwrap(),
                launch.authority["egress"]["stream_id"].as_str().unwrap(),
                Some("browser-vm-product"),
            )
            .is_err()
        );
    }

    #[test]
    fn config_must_be_owner_only() {
        let root = tempfile::tempdir().unwrap();
        write_config(root.path());
        let path = root
            .path()
            .join("config")
            .join("browser-vz-vsock-transport.json");
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        #[cfg(unix)]
        assert!(read_browser_vz_transport_config(root.path())
            .unwrap_err()
            .contains("owner-only"));
    }
}
