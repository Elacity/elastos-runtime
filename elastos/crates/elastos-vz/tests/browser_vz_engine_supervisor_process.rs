use std::io::Write as _;
use std::process::{Command, Stdio};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use base64::Engine as _;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use hmac::{Hmac, Mac};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use serde_json::{json, Value};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use sha1::Sha1;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use sha2::{Digest, Sha256};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::os::unix::net::UnixListener;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn missing_transport_exits_before_any_vz_or_path_effect() {
    let root = tempfile::tempdir().unwrap();
    let session_root = root.path().join("sessions");
    let socket_root = root.path().join("sockets");
    let runtime_socket = root.path().join("must-not-be-opened.sock");
    let request = serde_json::json!({
        "schema": "elastos.browser.vm-engine.open/v1",
        "launch_request": {
            "schema": "elastos.browser.engine.launch-request/v1",
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "stream_id": "stream:missing-transport-process-proof",
            "lifecycle_generation": format!("sha256:{}", "a".repeat(64)),
            "page_id": "page:missing-transport-process-proof",
            "vm_id": "vm:missing-transport-process-proof",
            "principal_id": "person:local:missing-transport-process-proof",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "adapter_ipc": {
                "kind": "unix_socket",
                "runtime_stream_path": runtime_socket,
            },
        },
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_browser-vz-engine-supervisor"))
        .env("ELASTOS_BROWSER_VM_TRACE", "1")
        .env("ELASTOS_BROWSER_VM_ROOT", &session_root)
        .env("ELASTOS_BROWSER_VM_SOCKET_ROOT", &socket_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let write_result = child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes());
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        panic!("failed to write missing-transport request: {error}");
    }
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("browser-vz-engine-supervisor stage=read_request"));
    assert!(stderr.contains("binding is incomplete"), "{stderr}");
    for forbidden_stage in [
        "validated_request",
        "resolved_paths",
        "provider_init_start",
        "load_vm_start",
        "start_vm_start",
        "spawn_egress_bridge",
    ] {
        assert!(
            !stderr.contains(forbidden_stage),
            "missing transport reached {forbidden_stage}: {stderr}",
        );
    }
    assert!(!stderr.contains("elastos.browser.vz-launch-settlement/v1"));
    assert!(!session_root.exists());
    assert!(!socket_root.exists());
    assert!(!runtime_socket.exists());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn anonymous_private_stdin_pipe_reaches_preflight_without_any_effect() {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
            Value::Object(values) => {
                let mut sorted = serde_json::Map::new();
                let mut keys = values.keys().collect::<Vec<_>>();
                keys.sort_unstable();
                for key in keys {
                    sorted.insert(key.clone(), canonical(&values[key]));
                }
                Value::Object(sorted)
            }
            value => value.clone(),
        }
    }

    fn sha256_label(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let encoded = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        format!("sha256:{encoded}")
    }

    let root = tempfile::tempdir().unwrap();
    let session_root = root.path().join("sessions");
    let socket_root = root.path().join("sockets");
    let runtime_socket = root.path().join("runtime.sock");
    let _runtime_listener = UnixListener::bind(&runtime_socket).unwrap();
    let kernel = root.path().join("vmlinux");
    let rootfs = root.path().join("rootfs.ext4");
    std::fs::write(&kernel, b"preflight-only kernel fixture").unwrap();
    std::fs::write(&rootfs, b"preflight-only rootfs fixture").unwrap();

    let expires_at_unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 300;
    let expires_at_unix_ms = expires_at_unix_secs * 1_000;
    let generation = format!("sha256:{}", "b".repeat(64));
    let username = format!("{expires_at_unix_secs}:stdin-pipe-proof");
    let auth_secret = "stdin-pipe-proof-auth-secret";
    let mut mac = Hmac::<Sha1>::new_from_slice(auth_secret.as_bytes()).unwrap();
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let mut authority = json!({
        "schema": "elastos.browser.vz-transport-authority/v1",
        "generation": generation,
        "page_id": "page:vz-stdin-pipe-proof",
        "vm_id": "vm:vz-stdin-pipe-proof",
        "principal_id": "person:local:stdin-pipe-proof",
        "egress": {
            "schema": "elastos.browser.vz-transport-stream/v1",
            "stream_id": "stream:egress-stdin-pipe-proof",
            "target": "tls://example.invalid:443",
            "runtime_socket_path": runtime_socket,
            "vsock_port": 19091,
        },
        "media": {
            "schema": "elastos.browser.vz-transport-stream/v1",
            "stream_id": "stream:media-stdin-pipe-proof",
            "target": "tcp://127.0.0.1:49160",
            "runtime_socket_path": root.path().join("media.sock"),
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
        "expires_at_unix_ms": expires_at_unix_ms,
    });
    let binding_hash = sha256_label(&serde_json::to_vec(&canonical(&authority)).unwrap());
    authority["binding_hash"] = json!(binding_hash);
    let secret = json!({
        "schema": "elastos.browser.vz-transport-secret/v1",
        "binding_hash": binding_hash,
        "credential": credential,
        "auth_secret": auth_secret,
    });
    let request = json!({
        "schema": "elastos.browser.vm-engine.open/v1",
        "launch_request": {
            "schema": "elastos.browser.engine.launch-request/v1",
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "stream_id": authority["egress"]["stream_id"],
            "lifecycle_generation": generation,
            "page_id": authority["page_id"],
            "vm_id": authority["vm_id"],
            "principal_id": authority["principal_id"],
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "adapter_ipc": {
                "kind": "unix_socket",
                "runtime_stream_path": authority["egress"]["runtime_socket_path"],
            },
            "transport_authority": authority,
            "transport_secret": secret,
        },
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_browser-vz-engine-supervisor"))
        .env("ELASTOS_BROWSER_VM_TRACE", "1")
        .env("ELASTOS_BROWSER_VM_ROOT", &session_root)
        .env("ELASTOS_BROWSER_VM_SOCKET_ROOT", &socket_root)
        .env("ELASTOS_BROWSER_VM_KERNEL", &kernel)
        .env("ELASTOS_BROWSER_VM_ROOTFS", &rootfs)
        .env("ELASTOS_BROWSER_VM_DATA_DIR", root.path())
        .env("ELASTOS_BROWSER_VM_TURN_PROGRAM", "/usr/bin/true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        stderr.contains("elastos.browser.vz-launch-settlement/v1"),
        "{stderr}"
    );
    assert!(stderr.contains("\"state\":\"did_not_act\""), "{stderr}");
    assert!(
        !stderr.contains("private stdin must be an owner-only file or owned pipe"),
        "{stderr}"
    );
    assert!(!session_root.exists());
    assert!(!socket_root.exists());
}
