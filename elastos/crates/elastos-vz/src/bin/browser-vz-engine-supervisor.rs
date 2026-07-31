use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use elastos_common::{
    CapsuleManifest, CapsuleRole, CapsuleType, MicroVmConfig, Permissions, ResourceLimits,
    SCHEMA_V1,
};
use elastos_compute::{CapsuleHandle, ComputeProvider};
use elastos_vz::{VmConfig, VzConfig, VzProvider};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing_subscriber::{fmt, EnvFilter};

const OPEN_REQUEST_ENV: &str = "ELASTOS_BROWSER_VM_OPEN_REQUEST";
const VZ_TRANSPORT_AUTHORITY_SCHEMA: &str = "elastos.browser.vz-transport-authority/v1";
const VZ_TRANSPORT_SECRET_SCHEMA: &str = "elastos.browser.vz-transport-secret/v1";
const DEFAULT_CONTROL_PORT: u32 = 19092;
const DEFAULT_RELAY_PORT: u32 = 19091;
const DEFAULT_PROFILE_DISK_MIB: u64 = 2048;
const UNIX_SOCKET_PATH_BUDGET: usize = 100;
const EGRESS_COPY_BUFFER_BYTES: usize = 256 * 1024;
const MAX_CONTROL_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_CONTROL_HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTROL_HTTP_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_CONTROL_PROXY_REQUEST_TIMEOUT_MS: u32 = 120_000;
const DEFAULT_CONTROL_STATUS_PROBE_TIMEOUT_MS: u32 = 3_000;
const BROWSER_VM_TARGET_VERSION: &str = match option_env!("ELASTOS_RELEASE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
const ICE_CONFIG_BOOT_ARG: &str = "elastos.browser_ice_config_hex";
const DISPLAY_MODE_BOOT_ARG: &str = "elastos.browser_display_mode";
const DISPLAY_WIDTH_BOOT_ARG: &str = "elastos.browser_width";
const DISPLAY_HEIGHT_BOOT_ARG: &str = "elastos.browser_height";
const DEFAULT_HIBERNATION_MAX_ENTRIES: u32 = 4;
const DEFAULT_HIBERNATION_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;
const HIBERNATION_LIFETIME_LOCK: &str = ".lifetime.lock";
const DISK_LIFETIME_LOCK_SUFFIX: &str = ".lifetime.lock";
const VM_ICE_ENV_KEYS: [&str; 8] = [
    "ELASTOS_BROWSER_VM_ICE_SERVER",
    "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON",
    "ELASTOS_BROWSER_VM_ICE_USERNAME",
    "ELASTOS_BROWSER_VM_ICE_CREDENTIAL",
    "ELASTOS_BROWSER_VM_ICE_TRANSPORT_POLICY",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_HOST_IPV4",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_GUEST_IPV4",
    "ELASTOS_BROWSER_VM_MEDIA_RELAY_PREFIX",
];

#[tokio::main]
async fn main() {
    init_tracing();
    if let Err(error) = run().await {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,vm_console=info"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn run() -> Result<(), String> {
    if !elastos_vz::is_supported() {
        return Err(
            "browser-vz-engine-supervisor requires macOS arm64 with Apple Virtualization.framework"
                .to_string(),
        );
    }

    trace_stage("read_request", "");
    let (request, request_from_stdin) = read_open_request()?;
    let launch = request
        .get("launch_request")
        .ok_or_else(|| "Browser VM open request missing launch_request".to_string())?;
    let transport = validate_launch_request(launch, request_from_stdin)?;
    trace_stage(
        "validated_request",
        format!(
            "stream_id={}",
            launch
                .get("stream_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
    );

    let paths = LaunchPaths::from_env(launch)?;
    trace_stage(
        "resolved_paths",
        format!(
            "kernel={} rootfs={} initramfs={} runtime_stream={}",
            paths.kernel_path.display(),
            paths.rootfs_path.display(),
            paths
                .initramfs_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none".to_string()),
            paths.runtime_stream_path.display()
        ),
    );
    let manifest = browser_vm_manifest(paths.memory_mib, paths.vcpu_count);
    let mut boot_args = "console=hvc0 reboot=k panic=1 root=/dev/vda rootfstype=ext4 rw init=/opt/elastos/bin/browser-vm-init random.trust_cpu=on".to_string();
    if let Ok(override_args) = std::env::var("ELASTOS_BROWSER_VM_BOOT_ARGS") {
        if !override_args.trim().is_empty() {
            boot_args = override_args;
        }
    }
    if !boot_args
        .split_whitespace()
        .any(|arg| arg.starts_with("elastos.browser_epoch="))
    {
        let _ = write!(
            &mut boot_args,
            " elastos.browser_epoch={}",
            current_unix_seconds()
        );
    }
    append_browser_display_boot_args(&mut boot_args, launch, transport.as_ref())?;
    let profile_disk = prepare_browser_profile_disk(&request)?;
    append_browser_profile_boot_arg(&mut boot_args, &profile_disk.profile_key);

    fs::create_dir_all(&paths.session_dir).map_err(|err| err.to_string())?;
    fs::create_dir_all(&paths.state_dir).map_err(|err| err.to_string())?;
    prepare_socket_path(&paths.control_socket_path)?;
    let turn = transport
        .as_ref()
        .map(|transport| LaunchTurn::start(&paths, transport))
        .transpose()?;
    let hibernation = if transport.is_some() {
        None
    } else {
        BrowserVmHibernation::from_env(
            &paths,
            launch,
            &profile_disk.profile_key,
            &profile_disk.path,
            &boot_args,
        )?
    };
    let launch_rootfs = prepare_launch_rootfs(&paths, hibernation.as_ref())?;
    trace_stage(
        "prepared_rootfs",
        format!(
            "base={} launch={}",
            paths.rootfs_path.display(),
            launch_rootfs.path.display()
        ),
    );

    trace_stage("provider_init_start", "");
    let provider = Arc::new(
        VzProvider::new(
            VzConfig::new()
                .with_kernel_path(&paths.kernel_path)
                .with_state_dir(&paths.state_dir)
                .with_rootfs_cache_dir(&paths.rootfs_cache_dir),
        )
        .map_err(|err| err.to_string())?,
    );
    provider.init().await.map_err(|err| err.to_string())?;
    trace_stage("provider_init_done", "");

    let mut vm_config = VmConfig {
        vm_id: browser_vm_id(transport.as_ref())?,
        kernel_path: paths.kernel_path.clone(),
        boot_args,
        rootfs_path: launch_rootfs.path.clone(),
        rootfs_readonly: false,
        mem_size_mib: paths.memory_mib,
        vcpu_count: paths.vcpu_count,
        http_port: None,
        data_disk_path: None,
        vsock_cid: 3,
        network: None,
        network_disabled: transport.is_some(),
        interactive_stdio: false,
        carrier_socket_path: None,
        initramfs_path: paths.initramfs_path.clone(),
    };
    vm_config.data_disk_path = Some(profile_disk.path.clone());

    trace_stage(
        "load_vm_start",
        format!("boot_args={}", vm_config.boot_args),
    );
    let handle = provider
        .load_with_vm_config(vm_config, manifest)
        .await
        .map_err(|err| err.to_string())?;
    trace_stage("load_vm_done", "");
    if !restore_browser_vm_hibernation(Arc::clone(&provider), &handle, hibernation.as_ref()).await?
    {
        trace_stage("start_vm_start", "");
        if let Err(error) = provider.start(&handle).await {
            let _ = fs::remove_dir_all(&paths.session_dir);
            return Err(error.to_string());
        }
        trace_stage("start_vm_done", "");
    }

    let guest_transport_receipt = match transport.as_ref() {
        Some(transport) => {
            match bootstrap_vz_transport(Arc::clone(&provider), &handle, transport).await {
                Ok(receipt) => Some(receipt),
                Err(error) => {
                    let _ = provider.stop(&handle).await;
                    let _ = fs::remove_dir_all(&paths.session_dir);
                    return Err(error);
                }
            }
        }
        None => None,
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    trace_stage("spawn_egress_bridge", "");
    let egress_max_sessions = env_u32("ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS", 16)?;
    if egress_max_sessions == 0 || egress_max_sessions > 256 {
        return Err("ELASTOS_BROWSER_VM_EGRESS_MAX_SESSIONS must be from 1 to 256".to_string());
    }
    let egress_port = transport
        .as_ref()
        .and_then(|transport| {
            transport
                .authority
                .pointer("/egress/vsock_port")
                .and_then(Value::as_u64)
        })
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(paths.relay_port);
    let egress = if env_bool("ELASTOS_BROWSER_VM_DISABLE_EGRESS_BRIDGE", false) {
        tokio::spawn(async {})
    } else {
        spawn_egress_bridge(
            Arc::clone(&provider),
            handle.clone(),
            egress_port,
            paths.runtime_stream_path.clone(),
            Arc::clone(&shutdown),
            egress_max_sessions as usize,
        )
    };
    let media = transport.as_ref().map(|transport| {
        spawn_egress_bridge(
            Arc::clone(&provider),
            handle.clone(),
            transport
                .authority
                .pointer("/media/vsock_port")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .expect("validated Browser VZ media vsock port"),
            PathBuf::from(
                transport
                    .authority
                    .pointer("/media/runtime_socket_path")
                    .and_then(Value::as_str)
                    .expect("validated Browser VZ media Runtime socket"),
            ),
            Arc::clone(&shutdown),
            egress_max_sessions as usize,
        )
    });
    trace_stage("spawn_control_proxy", "");
    let control_proxy = spawn_control_proxy(
        Arc::clone(&provider),
        handle.clone(),
        paths.control_port,
        paths.control_socket_path.clone(),
        Arc::clone(&shutdown),
    )?;

    trace_stage("open_guest_page_start", "");
    let mut result = match open_guest_page(
        Arc::clone(&provider),
        &handle,
        paths.control_port,
        &request,
        &paths,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            if let Ok(delay_ms) = env_u32("ELASTOS_BROWSER_VM_DEBUG_HOLD_ON_OPEN_ERROR_MS", 0) {
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms as u64)).await;
                }
            }
            shutdown.store(true, Ordering::Relaxed);
            let _ = provider.stop(&handle).await;
            return Err(error);
        }
    };
    if let Some(transport) = transport.as_ref() {
        if result.get("page_id") != transport.authority.get("page_id") {
            shutdown.store(true, Ordering::Relaxed);
            let _ = provider.stop(&handle).await;
            return Err("Browser VZ guest page identity changed".to_string());
        }
        result["vm_id"] = transport
            .authority
            .get("vm_id")
            .cloned()
            .expect("validated Browser VZ vm_id");
        result["transport_authority"] = transport.authority.clone();
        result["transport_receipt"] = vz_transport_effect_receipt(
            transport,
            guest_transport_receipt
                .as_ref()
                .expect("Browser VZ guest transport bootstrap receipt"),
        )?;
        if let Some(display) = result
            .get_mut("display_session")
            .and_then(Value::as_object_mut)
        {
            display.remove("ice_servers");
            display.insert(
                "ice_connection_policy".to_string(),
                json!("engine_relay_only"),
            );
        }
    }
    trace_stage("open_guest_page_done", "");

    println!(
        "{}",
        serde_json::to_string(&result).map_err(|err| err.to_string())?
    );
    wait_for_shutdown_or_transport_expiry(transport.as_ref()).await;
    shutdown.store(true, Ordering::Relaxed);
    let _ = control_proxy.join();
    let _ = egress.await;
    if let Some(media) = media {
        let _ = media.await;
    }
    save_browser_vm_hibernation(Arc::clone(&provider), &handle, hibernation.as_ref()).await;
    let _ = provider.stop(&handle).await;
    drop(turn);
    let _ = fs::remove_file(&paths.control_socket_path);
    let _ = fs::remove_dir_all(&paths.session_dir);
    Ok(())
}

fn browser_vm_id(transport: Option<&VzTransportLaunch>) -> Result<String, String> {
    match transport {
        Some(transport) => transport
            .authority
            .get("vm_id")
            .and_then(Value::as_str)
            .filter(|vm_id| safe_id(vm_id))
            .map(str::to_string)
            .ok_or_else(|| "Browser VZ transport VM identity is invalid".to_string()),
        None => Ok(format!("browser-vm-{}", uuid::Uuid::new_v4())),
    }
}

fn trace_stage(stage: &str, detail: impl AsRef<str>) {
    let enabled = std::env::var("ELASTOS_BROWSER_VM_TRACE")
        .map(|value| value != "0")
        .unwrap_or(false);
    if enabled {
        let detail = detail.as_ref();
        if detail.is_empty() {
            eprintln!("browser-vz-engine-supervisor stage={stage}");
        } else {
            eprintln!("browser-vz-engine-supervisor stage={stage} {detail}");
        }
    }
}

fn read_open_request() -> Result<(Value, bool), String> {
    let mut stdin = String::new();
    std::io::stdin()
        .read_to_string(&mut stdin)
        .map_err(|err| err.to_string())?;
    let (raw, from_stdin) = if stdin.trim().is_empty() {
        (
            std::env::var(OPEN_REQUEST_ENV)
                .map_err(|_| format!("{OPEN_REQUEST_ENV} is required"))?,
            false,
        )
    } else {
        (stdin, true)
    };
    serde_json::from_str(&raw)
        .map(|request| (request, from_stdin))
        .map_err(|err| format!("Browser VM open request is invalid JSON: {err}"))
}

#[derive(Debug, Clone)]
struct VzTransportLaunch {
    authority: Value,
    secret: Value,
}

fn validate_launch_request(
    launch: &Value,
    request_from_stdin: bool,
) -> Result<Option<VzTransportLaunch>, String> {
    if launch.get("schema").and_then(Value::as_str)
        != Some("elastos.browser.engine.launch-request/v1")
    {
        return Err("unsupported Browser VM launch request schema".to_string());
    }
    if launch.get("engine").and_then(Value::as_str) != Some("chromium_microvm") {
        return Err("Browser VZ launcher accepts only engine=chromium_microvm".to_string());
    }
    let display_mode = launch
        .get("display_mode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if display_mode != "webrtc_remote_display" {
        return Err("Browser VZ launcher requires display_mode=webrtc_remote_display".to_string());
    }
    if launch.get("guarantee_level").and_then(Value::as_str) != Some("mechanism_microvm") {
        return Err("Browser VZ launcher requires guarantee_level=mechanism_microvm".to_string());
    }
    if launch.get("network_mode").and_then(Value::as_str) != Some("runtime_net_only")
        || launch.get("direct_network").and_then(Value::as_bool) != Some(false)
        || launch.get("wallet_injection").and_then(Value::as_bool) != Some(false)
    {
        return Err(
            "Browser VZ launcher requires runtime_net_only, direct_network=false, wallet_injection=false"
              .to_string(),
        );
    }
    for field in ["adapter", "stream_id"] {
        if !safe_id(
            launch
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) {
            return Err(format!(
                "Browser VZ launch field {field} must be a safe identifier"
            ));
        }
    }
    let _ = launch_viewport(launch)?;
    let runtime_stream_path = launch
      .get("adapter_ipc")
      .and_then(|value| value.get("runtime_stream_path"))
      .and_then(Value::as_str)
      .ok_or_else(|| {
            "Browser VZ launcher requires adapter_ipc.runtime_stream_path for Runtime-mediated egress".to_string()
        })?;
    validate_absolute_path("adapter_ipc.runtime_stream_path", runtime_stream_path)?;
    validate_vz_transport_launch(launch, request_from_stdin)
}

fn validate_vz_transport_launch(
    launch: &Value,
    request_from_stdin: bool,
) -> Result<Option<VzTransportLaunch>, String> {
    let fields = [
        launch.get("page_id"),
        launch.get("vm_id"),
        launch.get("transport_authority"),
        launch.get("transport_secret"),
    ];
    if fields.iter().all(Option::is_none) {
        return Ok(None);
    }
    if fields.iter().any(Option::is_none) {
        return Err("Browser VZ transport launch binding is incomplete".to_string());
    }
    if !request_from_stdin {
        return Err(
            "Browser VZ transport secrets are accepted only through the private stdin launch pipe"
                .to_string(),
        );
    }
    let authority = launch
        .get("transport_authority")
        .cloned()
        .expect("checked Browser VZ authority");
    let secret = launch
        .get("transport_secret")
        .cloned()
        .expect("checked Browser VZ secret");
    validate_vz_transport_authority(&authority, true)?;
    validate_vz_transport_secret(&authority, &secret)?;
    if launch.get("lifecycle_generation") != authority.get("generation")
        || launch.get("page_id") != authority.get("page_id")
        || launch.get("vm_id") != authority.get("vm_id")
        || launch.get("stream_id") != authority.pointer("/egress/stream_id")
        || launch.get("principal_id") != authority.get("principal_id")
        || launch.pointer("/adapter_ipc/runtime_stream_path")
            != authority.pointer("/egress/runtime_socket_path")
    {
        return Err("Browser VZ transport launch identity changed".to_string());
    }
    Ok(Some(VzTransportLaunch { authority, secret }))
}

fn validate_vz_transport_authority(authority: &Value, require_live: bool) -> Result<(), String> {
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
        || !sha256_label_is_safe(authority.get("binding_hash"))
        || !sha256_label_is_safe(authority.get("generation"))
    {
        return Err("Browser VZ transport authority shape is invalid".to_string());
    }
    for field in ["page_id", "vm_id", "principal_id"] {
        let value = authority
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 512 && safe_id(value))
            .ok_or_else(|| format!("Browser VZ transport {field} is invalid"))?;
        let _ = value;
    }
    let egress = validate_vz_transport_stream(authority.get("egress"), false)?;
    let media = validate_vz_transport_stream(authority.get("media"), true)?;
    let bootstrap_port = authority
        .get("bootstrap_vsock_port")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "Browser VZ bootstrap vsock port is invalid".to_string())?;
    if egress.0 == media.0
        || egress.1 == media.1
        || egress.2 == media.2
        || bootstrap_port == egress.2
        || bootstrap_port == media.2
    {
        return Err("Browser VZ transport bindings must be distinct".to_string());
    }
    let expires_at = authority
        .get("expires_at_unix_ms")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| "Browser VZ transport expiry is invalid".to_string())?;
    let now = current_unix_millis()?;
    if expires_at > now.saturating_add(24 * 60 * 60 * 1_000) || (require_live && expires_at <= now)
    {
        return Err("Browser VZ transport authority expiry is invalid".to_string());
    }
    validate_vz_turn_authority(authority.get("turn"), expires_at)?;
    let mut unsigned = authority.clone();
    unsigned
        .as_object_mut()
        .expect("validated Browser VZ authority object")
        .remove("binding_hash");
    let expected = sha256_label(&canonical_json_bytes(&unsigned)?);
    if authority.get("binding_hash").and_then(Value::as_str) != Some(expected.as_str())
        || serde_json::to_vec(authority).map_or(true, |bytes| bytes.len() > 32 * 1024)
    {
        return Err("Browser VZ transport authority binding hash mismatch".to_string());
    }
    Ok(())
}

fn validate_vz_transport_secret(authority: &Value, secret: &Value) -> Result<(), String> {
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
    let credential = bounded_transport_secret(secret.get("credential"), "credential")?;
    let auth_secret = bounded_transport_secret(secret.get("auth_secret"), "auth_secret")?;
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
        .map_err(|_| "Browser VZ TURN auth secret is invalid".to_string())?;
    mac.update(username.as_bytes());
    let expected = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    if expected != credential {
        return Err("Browser VZ TURN credential mismatch".to_string());
    }
    Ok(())
}

fn validate_vz_transport_stream(
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
    let stream_id = object
        .get("stream_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 512 && safe_id(value))
        .ok_or_else(|| "Browser VZ transport stream_id is invalid".to_string())?;
    let target = object
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser VZ transport target is missing".to_string())?;
    let parsed = url::Url::parse(target)
        .map_err(|err| format!("Browser VZ transport target is invalid: {err}"))?;
    if !matches!(parsed.scheme(), "tcp" | "tls")
        || parsed.port().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Browser VZ transport target is invalid".to_string());
    }
    if loopback_target
        && !parsed
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback())
    {
        return Err("Browser VZ media target must be loopback".to_string());
    }
    let runtime_path = object
        .get("runtime_socket_path")
        .and_then(Value::as_str)
        .filter(|path| {
            path.starts_with('/') && path.len() <= 103 && !path.contains(['\0', '\r', '\n'])
        })
        .ok_or_else(|| "Browser VZ transport Runtime socket is invalid".to_string())?;
    let port = object
        .get("vsock_port")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "Browser VZ transport vsock port is invalid".to_string())?;
    Ok((
        stream_id.to_string(),
        format!("{}\n{runtime_path}", parsed.as_str()),
        port,
    ))
}

fn validate_vz_turn_authority(
    value: Option<&Value>,
    expires_at_unix_ms: u64,
) -> Result<(), String> {
    let turn = value
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
    if turn.len() != keys.len()
        || keys.iter().any(|key| !turn.contains_key(*key))
        || turn.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vz-turn-authority/v1")
    {
        return Err("Browser VZ TURN authority shape is invalid".to_string());
    }
    let guest_host = require_loopback_ip(turn.get("guest_host"), "guest_host")?;
    require_loopback_ip(turn.get("listen_host"), "listen_host")?;
    let guest_port = require_json_port(turn.get("guest_port"), "guest_port")?;
    require_json_port(turn.get("listen_port"), "listen_port")?;
    let advertised_host = turn
        .get("advertised_host")
        .and_then(Value::as_str)
        .filter(|host| {
            !host.is_empty()
                && host.len() <= 253
                && !host.contains(['\0', '\r', '\n', '/', '\\', ' ', '\t'])
        })
        .ok_or_else(|| "Browser VZ TURN advertised host is invalid".to_string())?;
    let _ = advertised_host;
    let relay_host = turn
        .get("relay_host")
        .and_then(Value::as_str)
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .filter(|host| !host.is_unspecified() && !host.is_multicast())
        .ok_or_else(|| "Browser VZ TURN relay host is invalid".to_string())?;
    let _ = relay_host;
    let relay_min = require_json_port(turn.get("relay_port_min"), "relay_port_min")?;
    let relay_max = require_json_port(turn.get("relay_port_max"), "relay_port_max")?;
    if relay_min > relay_max
        || u32::from(relay_max) - u32::from(relay_min) + 1 > 64
        || turn.get("guest_url").and_then(Value::as_str)
            != Some(format!("turn:{guest_host}:{guest_port}?transport=tcp").as_str())
        || turn.get("protocols") != Some(&json!(["turn", "tcp"]))
    {
        return Err("Browser VZ TURN endpoint binding is invalid".to_string());
    }
    let username = turn
        .get("username")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 256 && !value.contains(['\0', '\r', '\n'])
        })
        .ok_or_else(|| "Browser VZ TURN username is invalid".to_string())?;
    let username_expiry = username
        .split_once(':')
        .and_then(|(expiry, suffix)| (!suffix.is_empty()).then_some(expiry))
        .and_then(|expiry| expiry.parse::<u64>().ok())
        .and_then(|expiry| expiry.checked_mul(1_000))
        .ok_or_else(|| "Browser VZ TURN username expiry is invalid".to_string())?;
    if username_expiry != expires_at_unix_ms
        || !sha256_label_is_safe(turn.get("credential_hash"))
        || !sha256_label_is_safe(turn.get("auth_secret_hash"))
    {
        return Err("Browser VZ TURN authority hash or expiry changed".to_string());
    }
    Ok(())
}

fn require_loopback_ip(value: Option<&Value>, field: &str) -> Result<String, String> {
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

fn require_json_port(value: Option<&Value>, field: &str) -> Result<u16, String> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("Browser VZ TURN {field} is invalid"))
}

fn bounded_transport_secret<'a>(value: Option<&'a Value>, field: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 512 && !value.contains(['\0', '\r', '\n'])
        })
        .ok_or_else(|| format!("Browser VZ transport secret {field} is invalid"))
}

fn sha256_label_is_safe(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|value| {
        value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    })
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_encode(&Sha256::digest(bytes)))
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

fn current_unix_millis() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())
        .and_then(|duration| {
            u64::try_from(duration.as_millis())
                .map_err(|_| "current Unix time is too large".to_string())
        })
}

fn launch_viewport(launch: &Value) -> Result<Option<(u64, u64)>, String> {
    let Some(viewport) = launch.get("viewport") else {
        return Ok(None);
    };
    let width = viewport
        .get("width")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Browser VZ launch viewport.width must be an integer".to_string())?;
    let height = viewport
        .get("height")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Browser VZ launch viewport.height must be an integer".to_string())?;
    if !(320..=3840).contains(&width) || !(240..=2160).contains(&height) {
        return Err("Browser VZ launch viewport must be within 320x240 and 3840x2160".to_string());
    }
    Ok(Some((width, height)))
}

fn append_browser_display_boot_args(
    boot_args: &mut String,
    launch: &Value,
    transport: Option<&VzTransportLaunch>,
) -> Result<(), String> {
    if let Some(transport) = transport {
        append_browser_display_boot_args_with_ice_config(boot_args, launch, true, None)?;
        let bootstrap_port = transport
            .authority
            .get("bootstrap_vsock_port")
            .and_then(Value::as_u64)
            .ok_or_else(|| "Browser VZ bootstrap port is missing".to_string())?;
        write!(
            boot_args,
            " elastos.browser_vz_transport=vsock_v1 elastos.browser_bootstrap_port={bootstrap_port}"
        )
        .map_err(|err| err.to_string())?;
        return Ok(());
    }
    let display_mode = launch
        .get("display_mode")
        .and_then(Value::as_str)
        .unwrap_or("webrtc_remote_display");
    let ice_has_turn_server = if display_mode == "webrtc_remote_display" {
        ice_env_has_turn_server()?
    } else {
        true
    };
    let ice_config_hex = ice_boot_config_hex()?;
    append_browser_display_boot_args_with_ice_config(
        boot_args,
        launch,
        ice_has_turn_server,
        ice_config_hex.as_deref(),
    )
}

fn append_browser_display_boot_args_with_ice_config(
    boot_args: &mut String,
    launch: &Value,
    ice_has_turn_server: bool,
    ice_config_hex: Option<&str>,
) -> Result<(), String> {
    let display_mode = launch
        .get("display_mode")
        .and_then(Value::as_str)
        .unwrap_or("webrtc_remote_display");
    if display_mode == "webrtc_remote_display" && !ice_has_turn_server {
        return Err(
            "Browser VZ webrtc_remote_display requires ELASTOS_BROWSER_VM_ICE_SERVER or ELASTOS_BROWSER_VM_ICE_SERVERS_JSON with at least one turn:/turns: URL for media_transport=runtime_relay"
              .to_string(),
        );
    }
    write!(boot_args, " {DISPLAY_MODE_BOOT_ARG}={display_mode}").map_err(|err| err.to_string())?;
    if let Some((width, height)) = launch_viewport(launch)? {
        write!(
            boot_args,
            " {DISPLAY_WIDTH_BOOT_ARG}={width} {DISPLAY_HEIGHT_BOOT_ARG}={height}"
        )
        .map_err(|err| err.to_string())?;
    }
    if let Some(config_hex) = ice_config_hex {
        write!(boot_args, " {ICE_CONFIG_BOOT_ARG}={config_hex}").map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn ice_boot_config_hex() -> Result<Option<String>, String> {
    let mut config = serde_json::Map::new();
    for key in VM_ICE_ENV_KEYS {
        if let Some(value) = env_non_empty(key) {
            config.insert(key.to_string(), json!(value));
        }
    }
    if config.is_empty() {
        return Ok(None);
    }
    let raw = serde_json::to_vec(&Value::Object(config)).map_err(|err| err.to_string())?;
    if raw.len() > 4096 {
        return Err("Browser VM ICE boot config is too large for kernel boot args".to_string());
    }
    Ok(Some(hex_encode(&raw)))
}

fn ice_env_has_turn_server() -> Result<bool, String> {
    let mut urls = Vec::new();
    if let Some(url) = env_non_empty("ELASTOS_BROWSER_VM_ICE_SERVER") {
        urls.push(url);
    }
    if let Some(raw) = env_non_empty("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON") {
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|err| format!("ELASTOS_BROWSER_VM_ICE_SERVERS_JSON is invalid JSON: {err}"))?;
        let entries = parsed
            .as_array()
            .ok_or_else(|| "ELASTOS_BROWSER_VM_ICE_SERVERS_JSON must be an array".to_string())?;
        for entry in entries {
            collect_ice_urls(entry, &mut urls)?;
        }
    }
    Ok(urls.iter().any(|url| {
        let lower = url.trim().to_ascii_lowercase();
        lower.starts_with("turn:") || lower.starts_with("turns:")
    }))
}

fn collect_ice_urls(entry: &Value, urls: &mut Vec<String>) -> Result<(), String> {
    if let Some(url) = entry.as_str() {
        urls.push(validate_ice_url(url)?);
        return Ok(());
    }
    let object = entry.as_object().ok_or_else(|| {
        "ICE server entries must be URL strings or RTCIceServer objects".to_string()
    })?;
    let value = object
        .get("urls")
        .ok_or_else(|| "ICE server entries must include urls".to_string())?;
    if let Some(url) = value.as_str() {
        urls.push(validate_ice_url(url)?);
        return Ok(());
    }
    let values = value
        .as_array()
        .ok_or_else(|| "ICE server urls must be a string or array".to_string())?;
    if values.is_empty() || values.len() > 8 {
        return Err("ICE server urls must contain 1..8 entries".to_string());
    }
    for url in values {
        let url = url
            .as_str()
            .ok_or_else(|| "ICE server urls entries must be strings".to_string())?;
        urls.push(validate_ice_url(url)?);
    }
    Ok(())
}

fn validate_ice_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.is_empty()
        || trimmed.len() > 512
        || trimmed
            .bytes()
            .any(|byte| byte == b'\0' || byte == b'\r' || byte == b'\n')
        || !(lower.starts_with("stun:")
            || lower.starts_with("turn:")
            || lower.starts_with("turns:"))
    {
        return Err(
            "ICE server URLs must use stun:, turn:, or turns: without control characters"
                .to_string(),
        );
    }
    Ok(trimmed.to_string())
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
}

fn validate_absolute_path(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || !value.starts_with('/') {
        return Err(format!("{label} must be an absolute path"));
    }
    if value
        .bytes()
        .any(|byte| byte.is_ascii_whitespace() || byte == b'\0')
    {
        return Err(format!("{label} must not contain whitespace or NUL"));
    }
    Ok(())
}

#[derive(Debug)]
struct LaunchPaths {
    data_dir: PathBuf,
    kernel_path: PathBuf,
    rootfs_path: PathBuf,
    initramfs_path: Option<PathBuf>,
    state_dir: PathBuf,
    rootfs_cache_dir: PathBuf,
    session_dir: PathBuf,
    control_socket_path: PathBuf,
    runtime_stream_path: PathBuf,
    control_port: u32,
    relay_port: u32,
    memory_mib: u32,
    vcpu_count: u8,
}

struct LaunchTurn {
    child: std::process::Child,
    config_path: PathBuf,
}

impl LaunchTurn {
    fn start(paths: &LaunchPaths, transport: &VzTransportLaunch) -> Result<Self, String> {
        let program = env_path("ELASTOS_BROWSER_VM_TURN_PROGRAM")
            .ok_or_else(|| "ELASTOS_BROWSER_VM_TURN_PROGRAM is required".to_string())?;
        let metadata = fs::metadata(&program)
            .map_err(|err| format!("Browser VZ TURN program is unavailable: {err}"))?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("Browser VZ TURN program must be an executable regular file".to_string());
        }
        let turn = transport
            .authority
            .get("turn")
            .and_then(Value::as_object)
            .ok_or_else(|| "Browser VZ TURN authority is missing".to_string())?;
        let secret = transport
            .secret
            .get("auth_secret")
            .and_then(Value::as_str)
            .ok_or_else(|| "Browser VZ TURN private launch secret is missing".to_string())?;
        let listen_host = turn
            .get("listen_host")
            .and_then(Value::as_str)
            .ok_or_else(|| "Browser VZ TURN listen host is missing".to_string())?;
        let listen_port = require_json_port(turn.get("listen_port"), "listen_port")?;
        let advertised_host = turn
            .get("advertised_host")
            .and_then(Value::as_str)
            .ok_or_else(|| "Browser VZ TURN advertised host is missing".to_string())?;
        let relay_host = turn
            .get("relay_host")
            .and_then(Value::as_str)
            .ok_or_else(|| "Browser VZ TURN relay host is missing".to_string())?;
        let relay_min = require_json_port(turn.get("relay_port_min"), "relay_port_min")?;
        let relay_max = require_json_port(turn.get("relay_port_max"), "relay_port_max")?;
        let expires_at = transport
            .authority
            .get("expires_at_unix_ms")
            .and_then(Value::as_u64)
            .ok_or_else(|| "Browser VZ TURN expiry is missing".to_string())?;
        let remaining_secs = expires_at
            .checked_sub(current_unix_millis()?)
            .map(|millis| millis.div_ceil(1_000))
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| "Browser VZ TURN authority is expired".to_string())?;
        let config_path = paths.session_dir.join("turnserver.conf");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config_path)
            .map_err(|err| format!("Browser VZ TURN config creation failed: {err}"))?;
        let realm_suffix = transport
            .authority
            .get("binding_hash")
            .and_then(Value::as_str)
            .and_then(|value| value.strip_prefix("sha256:"))
            .unwrap_or("invalid")
            .chars()
            .take(16)
            .collect::<String>();
        let config = format!(
            "listening-ip={listen_host}\n\
             relay-ip={relay_host}\n\
             listening-port={listen_port}\n\
             external-ip={advertised_host}/{relay_host}\n\
             min-port={relay_min}\n\
             max-port={relay_max}\n\
             realm=elastos-browser-{realm_suffix}\n\
             fingerprint\n\
             use-auth-secret\n\
             static-auth-secret={secret}\n\
             no-udp\n\
             no-tls\n\
             no-dtls\n\
             no-cli\n\
             no-daemon\n\
             stale-nonce=120\n\
             max-allocate-lifetime={remaining_secs}\n\
             channel-lifetime={remaining_secs}\n\
             permission-lifetime={remaining_secs}\n"
        );
        file.write_all(config.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|err| format!("Browser VZ TURN config write failed: {err}"))?;
        drop(file);
        let child = std::process::Command::new(&program)
            .arg("-c")
            .arg(&config_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|err| format!("Browser VZ TURN process failed to start: {err}"))?;
        let mut turn = Self { child, config_path };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = turn
                .child
                .try_wait()
                .map_err(|err| format!("Browser VZ TURN process status failed: {err}"))?
            {
                let _ = fs::remove_file(&turn.config_path);
                return Err(format!(
                    "Browser VZ TURN process exited before readiness: {status}"
                ));
            }
            if std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::new(
                    listen_host
                        .parse()
                        .map_err(|_| "Browser VZ TURN listen host is invalid".to_string())?,
                    listen_port,
                ),
                Duration::from_millis(100),
            )
            .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Browser VZ TURN listener did not become ready".to_string());
            }
            thread::sleep(Duration::from_millis(25));
        }
        fs::remove_file(&turn.config_path)
            .map_err(|err| format!("Browser VZ TURN config retirement failed: {err}"))?;
        Ok(turn)
    }
}

impl Drop for LaunchTurn {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.config_path);
    }
}

impl LaunchPaths {
    fn from_env(launch: &Value) -> Result<Self, String> {
        let data_dir = env_path("ELASTOS_BROWSER_VM_DATA_DIR").unwrap_or_else(default_data_dir);
        let kernel_path =
            env_path("ELASTOS_BROWSER_VM_KERNEL").unwrap_or_else(|| data_dir.join("bin/vmlinux"));
        let rootfs_path = env_path("ELASTOS_BROWSER_VM_ROOTFS")
            .unwrap_or_else(|| data_dir.join("browser-vm/rootfs.ext4"));
        let initramfs_path = env_path("ELASTOS_BROWSER_VM_INITRAMFS").or_else(|| {
            let default_initrd = data_dir.join("bin/initrd");
            default_initrd.is_file().then_some(default_initrd)
        });
        let state_dir =
            env_path("ELASTOS_BROWSER_VM_STATE_DIR").unwrap_or_else(|| data_dir.join("vz-browser"));
        let rootfs_cache_dir = data_dir.join("rootfs-cache");
        let session_root =
            env_path("ELASTOS_BROWSER_VM_ROOT").unwrap_or_else(|| PathBuf::from("/tmp/evzs"));
        let session_dir = session_root.join(format!(
            "{}-{}",
            path_segment(
                launch
                    .get("stream_id")
                    .and_then(Value::as_str)
                    .unwrap_or("stream")
            ),
            uuid::Uuid::new_v4().simple()
        ));
        let control_socket_path = session_dir.join("c.sock");
        let runtime_stream_path = PathBuf::from(
            launch
                .get("adapter_ipc")
                .and_then(|value| value.get("runtime_stream_path"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "Browser VZ launcher requires adapter_ipc.runtime_stream_path".to_string()
                })?,
        );
        validate_unix_socket_path_budget("control_socket_path", &control_socket_path)?;
        validate_unix_socket_path_budget("adapter_ipc.runtime_stream_path", &runtime_stream_path)?;
        for (label, path) in [
            ("kernel", &kernel_path),
            ("rootfs", &rootfs_path),
            ("adapter_ipc.runtime_stream_path", &runtime_stream_path),
        ] {
            if !path.exists() {
                return Err(format!("{label} does not exist: {}", path.display()));
            }
        }
        validate_runtime_stream_socket_path(&runtime_stream_path)?;
        if let Some(initramfs_path) = initramfs_path.as_ref() {
            if !initramfs_path.exists() {
                return Err(format!(
                    "initramfs does not exist: {}",
                    initramfs_path.display()
                ));
            }
        }
        Ok(Self {
            data_dir,
            kernel_path,
            rootfs_path,
            initramfs_path,
            state_dir,
            rootfs_cache_dir,
            session_dir,
            control_socket_path,
            runtime_stream_path,
            control_port: env_u32(
                "ELASTOS_BROWSER_VM_CONTROL_BRIDGE_PORT",
                DEFAULT_CONTROL_PORT,
            )?,
            relay_port: env_u32("ELASTOS_BROWSER_VM_RELAY_PORT", DEFAULT_RELAY_PORT)?,
            memory_mib: env_u32("ELASTOS_BROWSER_VM_MEMORY_MIB", 2048)?,
            vcpu_count: env_u32("ELASTOS_BROWSER_VM_VCPUS", 2)? as u8,
        })
    }
}

#[derive(Debug)]
struct LifetimeFileLock {
    _path: PathBuf,
    file: File,
}

impl Drop for LifetimeFileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

impl LifetimeFileLock {
    fn acquire_lock_file(path: &Path, resource: &str) -> Result<Self, String> {
        Self::acquire(path, path, resource)
    }

    fn acquire_disk_sidecar(disk_path: &Path, resource: &str) -> Result<Self, String> {
        Self::acquire(&disk_lifetime_lock_path(disk_path), disk_path, resource)
    }

    fn acquire(lock_path: &Path, resource_path: &Path, resource: &str) -> Result<Self, String> {
        let file = open_lifetime_lock(lock_path, true)?;
        match try_lock_exclusive(&file)? {
            true => Ok(Self {
                _path: lock_path.to_path_buf(),
                file,
            }),
            false => Err(resources_in_use_error(resource, resource_path)),
        }
    }
}

fn disk_lifetime_lock_path(disk_path: &Path) -> PathBuf {
    let mut lock_path = disk_path.as_os_str().to_os_string();
    lock_path.push(DISK_LIFETIME_LOCK_SUFFIX);
    PathBuf::from(lock_path)
}

fn open_lifetime_lock(path: &Path, create: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        options.create(true).mode(0o600);
    }
    let file = options.open(path).map_err(|err| err.to_string())?;
    if create {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|err| err.to_string())?;
    }
    Ok(file)
}

fn try_lock_exclusive(file: &File) -> Result<bool, String> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(error.to_string())
}

fn resources_in_use_error(resource: &str, path: &Path) -> String {
    json!({
        "schema": "elastos.browser.engine.launch-error/v1",
        "code": "resources_in_use",
        "message": format!("{resource} is already attached to an active Browser VM"),
        "resource": resource,
        "path": path,
    })
    .to_string()
}

#[derive(Debug)]
struct BrowserVmHibernation {
    key: String,
    state_dir: PathBuf,
    state_path: PathBuf,
    state_tmp_path: PathBuf,
    metadata_path: PathBuf,
    launch_rootfs_path: PathBuf,
    _lease: HibernationLease,
}

#[derive(Debug)]
struct HibernationLease {
    _lock: LifetimeFileLock,
}

impl HibernationLease {
    fn acquire(state_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(state_dir).map_err(|err| err.to_string())?;
        Ok(Self {
            _lock: LifetimeFileLock::acquire_lock_file(
                &state_dir.join(HIBERNATION_LIFETIME_LOCK),
                "shared Browser VZ hibernation state",
            )?,
        })
    }
}

impl BrowserVmHibernation {
    fn from_env(
        paths: &LaunchPaths,
        launch: &Value,
        profile_key: &str,
        profile_disk_path: &Path,
        boot_args: &str,
    ) -> Result<Option<Self>, String> {
        if !env_bool("ELASTOS_BROWSER_VM_HIBERNATION", false) {
            return Ok(None);
        }
        let hibernation_root = env_path("ELASTOS_BROWSER_VM_HIBERNATION_DIR")
            .unwrap_or_else(|| paths.data_dir.join("browser-vm/hibernation"));
        validate_clean_absolute_path("ELASTOS_BROWSER_VM_HIBERNATION_DIR", &hibernation_root)?;
        let max_entries = env_u32(
            "ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES",
            DEFAULT_HIBERNATION_MAX_ENTRIES,
        )?;
        if !(1..=32).contains(&max_entries) {
            return Err(
                "ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES must be from 1 to 32".to_string(),
            );
        }
        let max_age_secs = env_u64(
            "ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS",
            DEFAULT_HIBERNATION_MAX_AGE_SECS,
        )?;
        if !(60 * 60..=30 * 24 * 60 * 60).contains(&max_age_secs) {
            return Err(
                "ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS must be from 3600 to 2592000"
                    .to_string(),
            );
        }
        let key_material = json!({
            "schema": "elastos.browser.vm-hibernation-key/v1",
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
            },
            "engine": {
                "kind": "chromium_microvm",
                "display_mode": launch.get("display_mode").and_then(Value::as_str),
                "guarantee_level": launch.get("guarantee_level").and_then(Value::as_str),
                "network_mode": "runtime_net_only",
                "direct_network": false,
                "wallet_injection": false,
            },
            "resources": {
                "memory_mib": paths.memory_mib,
                "vcpu_count": paths.vcpu_count,
            },
            "artifacts": {
                "kernel": file_fingerprint(&paths.kernel_path)?,
                "rootfs": file_fingerprint(&paths.rootfs_path)?,
                "initramfs": paths
                  .initramfs_path
                  .as_ref()
                  .map(|path| file_fingerprint(path))
                  .transpose()?,
            },
            "profile": {
                "profile_key": profile_key,
                "disk": profile_disk_identity(profile_disk_path)?,
            },
            "boot_args": boot_args,
        });
        let key = sha256_json(&key_material)?;
        let state_dir = hibernation_root.join(&key[..2]).join(&key);
        let state_path = state_dir.join("machine.state");
        let state_tmp_path = state_dir.join("machine.state.tmp");
        let metadata_path = state_dir.join("metadata.json");
        let launch_rootfs_path = state_dir.join("rootfs.ext4");
        let lease = HibernationLease::acquire(&state_dir)?;
        match prune_hibernation_cache(
            &hibernation_root,
            &state_dir,
            max_entries as usize,
            Duration::from_secs(max_age_secs),
        ) {
            Ok(removed) if removed > 0 => trace_stage(
                "hibernate_cache_pruned",
                format!("removed={removed} max_entries={max_entries}"),
            ),
            Ok(_) => {}
            Err(error) => {
                eprintln!("browser-vz-engine-supervisor hibernate_cache_prune_failed error={error}")
            }
        }
        Ok(Some(Self {
            key,
            state_dir,
            state_path,
            state_tmp_path,
            metadata_path,
            launch_rootfs_path,
            _lease: lease,
        }))
    }
}

#[derive(Debug)]
struct HibernationCacheEntry {
    path: PathBuf,
    last_modified: SystemTime,
    active: bool,
}

fn prune_hibernation_cache(
    root: &Path,
    protected: &Path,
    max_entries: usize,
    max_age: Duration,
) -> Result<usize, String> {
    prune_hibernation_cache_at(root, protected, max_entries, max_age, SystemTime::now())
}

fn prune_hibernation_cache_at(
    root: &Path,
    protected: &Path,
    max_entries: usize,
    max_age: Duration,
    now: SystemTime,
) -> Result<usize, String> {
    let mut entries = hibernation_cache_entries(root)?;
    for entry in &mut entries {
        entry.active = hibernation_entry_has_live_lease(&entry.path)?;
    }

    let mut removed = 0;
    for entry in &entries {
        if entry.path != protected
            && !entry.active
            && now.duration_since(entry.last_modified).unwrap_or_default() > max_age
        {
            remove_hibernation_cache_entry(&entry.path)?;
            removed += 1;
        }
    }

    entries.retain(|entry| entry.path.exists());
    entries.sort_by(|left, right| {
        left.last_modified
            .cmp(&right.last_modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut remaining = entries.len();
    for entry in entries {
        if remaining <= max_entries {
            break;
        }
        if entry.path == protected || entry.active {
            continue;
        }
        remove_hibernation_cache_entry(&entry.path)?;
        removed += 1;
        remaining -= 1;
    }
    Ok(removed)
}

fn hibernation_cache_entries(root: &Path) -> Result<Vec<HibernationCacheEntry>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for prefix in fs::read_dir(root).map_err(|err| err.to_string())? {
        let prefix = prefix.map_err(|err| err.to_string())?;
        if !prefix.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let prefix_name = prefix.file_name();
        let Some(prefix_name) = prefix_name.to_str() else {
            continue;
        };
        if !is_lower_hex(prefix_name, 2) {
            continue;
        }
        for candidate in fs::read_dir(prefix.path()).map_err(|err| err.to_string())? {
            let candidate = candidate.map_err(|err| err.to_string())?;
            if !candidate
                .file_type()
                .map_err(|err| err.to_string())?
                .is_dir()
            {
                continue;
            }
            let key = candidate.file_name();
            let Some(key) = key.to_str() else {
                continue;
            };
            if !is_lower_hex(key, 64) || !key.starts_with(prefix_name) {
                continue;
            }
            let path = candidate.path();
            entries.push(HibernationCacheEntry {
                last_modified: hibernation_entry_last_modified(&path)?,
                path,
                active: false,
            });
        }
    }
    Ok(entries)
}

fn hibernation_entry_last_modified(path: &Path) -> Result<SystemTime, String> {
    let mut modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|err| err.to_string())?;
    for name in [
        "machine.state",
        "machine.state.tmp",
        "metadata.json",
        "rootfs.ext4",
    ] {
        match fs::metadata(path.join(name)).and_then(|metadata| metadata.modified()) {
            Ok(candidate) if candidate > modified => modified = candidate,
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(modified)
}

fn hibernation_entry_has_live_lease(path: &Path) -> Result<bool, String> {
    let lock_path = path.join(HIBERNATION_LIFETIME_LOCK);
    let file = match open_lifetime_lock(&lock_path, false) {
        Ok(file) => file,
        Err(_) if !lock_path.exists() => return Ok(false),
        Err(error) => return Err(error),
    };
    let acquired = try_lock_exclusive(&file)?;
    if acquired {
        let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    Ok(!acquired)
}

fn remove_hibernation_cache_entry(path: &Path) -> Result<(), String> {
    let prefix = path.parent().map(Path::to_path_buf);
    fs::remove_dir_all(path).map_err(|err| err.to_string())?;
    if let Some(prefix) = prefix {
        let _ = fs::remove_dir(prefix);
    }
    Ok(())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn env_u32(name: &str, default: u32) -> Result<u32, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u32>()
            .map_err(|_| format!("{name} must be a positive integer")),
        Err(_) => Ok(default),
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<u64>()
            .map_err(|_| format!("{name} must be a positive integer")),
        Err(_) => Ok(default),
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"),
        Err(_) => default,
    }
}

fn default_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Application Support/elastos");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("elastos");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/elastos");
    }
    PathBuf::from("/var/lib/elastos")
}

fn validate_clean_absolute_path(label: &str, path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component.as_os_str() == std::ffi::OsStr::new(".."))
        || path.to_string_lossy().contains(['\0', '\r', '\n'])
    {
        return Err(format!(
            "{label} must be an absolute path without traversal or control characters"
        ));
    }
    Ok(())
}

fn file_fingerprint(path: &Path) -> Result<Value, String> {
    let metadata =
        fs::metadata(path).map_err(|err| format!("failed to stat {}: {err}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(json!({
        "path": path.to_string_lossy(),
        "len": metadata.len(),
        "modified_unix_nanos": metadata_modified_unix_nanos(&metadata),
    }))
}

fn profile_disk_identity(path: &Path) -> Result<Value, String> {
    let metadata = fs::metadata(path).map_err(|err| {
        format!(
            "failed to stat Browser profile disk {}: {err}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "Browser profile disk is not a file: {}",
            path.display()
        ));
    }
    Ok(json!({
        "path": path.to_string_lossy(),
        "len": metadata.len(),
        "dev": metadata.dev(),
        "ino": metadata.ino(),
    }))
}

fn metadata_modified_unix_nanos(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn sha256_json(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|err| err.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex_encode(&hasher.finalize()))
}

fn path_segment(value: &str) -> String {
    let mut segment = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(24)
        .collect::<String>();
    if segment.is_empty() {
        segment.push_str("session");
    }
    segment
}

fn validate_unix_socket_path_budget(label: &str, path: &Path) -> Result<(), String> {
    let text = path.to_string_lossy();
    if text.len() >= UNIX_SOCKET_PATH_BUDGET {
        return Err(format!(
            "{label} is too long for macOS Unix sockets ({} bytes): {}",
            text.len(),
            path.display()
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct PreparedLaunchRootfs {
    path: PathBuf,
    _lock: Option<LifetimeFileLock>,
}

fn prepare_launch_rootfs(
    paths: &LaunchPaths,
    hibernation: Option<&BrowserVmHibernation>,
) -> Result<PreparedLaunchRootfs, String> {
    if let Some(hibernation) = hibernation {
        if !hibernation.launch_rootfs_path.exists() {
            if hibernation.state_path.exists() {
                let _ = fs::remove_file(&hibernation.state_path);
            }
            clone_or_copy_file(&paths.rootfs_path, &hibernation.launch_rootfs_path).map_err(
                |err| {
                    format!(
                        "failed to prepare Browser VM hibernation rootfs {} from {}: {}",
                        hibernation.launch_rootfs_path.display(),
                        paths.rootfs_path.display(),
                        err
                    )
                },
            )?;
        }
        return Ok(PreparedLaunchRootfs {
            path: hibernation.launch_rootfs_path.clone(),
            _lock: None,
        });
    }
    if std::env::var("ELASTOS_BROWSER_VM_ROOTFS_PER_LAUNCH")
        .map(|value| value == "0" || value.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return Ok(PreparedLaunchRootfs {
            path: paths.rootfs_path.clone(),
            _lock: Some(LifetimeFileLock::acquire_disk_sidecar(
                &paths.rootfs_path,
                "shared writable Browser VZ rootfs",
            )?),
        });
    }

    let launch_rootfs = paths.session_dir.join("rootfs.ext4");
    clone_or_copy_file(&paths.rootfs_path, &launch_rootfs).map_err(|err| {
        format!(
            "failed to prepare per-launch Browser VM rootfs {} from {}: {}",
            launch_rootfs.display(),
            paths.rootfs_path.display(),
            err
        )
    })?;
    Ok(PreparedLaunchRootfs {
        path: launch_rootfs,
        _lock: None,
    })
}

fn clone_or_copy_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::remove_file(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    #[cfg(target_os = "macos")]
    {
        let clone_status = std::process::Command::new("/bin/cp")
            .arg("-c")
            .arg(source)
            .arg(destination)
            .status();
        if matches!(clone_status, Ok(status) if status.success()) {
            return Ok(());
        }
    }

    fs::copy(source, destination)?;
    let permissions = fs::metadata(source)?.permissions();
    fs::set_permissions(destination, permissions)?;
    Ok(())
}

fn profile_disk_from_request(request: &Value) -> Result<(String, PathBuf), String> {
    let profile = request
        .get("profile")
        .and_then(Value::as_object)
        .ok_or_else(|| "Browser profile descriptor is required".to_string())?;
    if profile.get("schema").and_then(Value::as_str) != Some("elastos.browser.profile/v1")
        || profile.get("scope").and_then(Value::as_str) != Some("active_principal")
        || profile.get("storage").and_then(Value::as_str) != Some("principal_owned_profile_disk")
        || profile.get("storage_posture").and_then(Value::as_str)
            != Some("principal_owned_reset_scoped_unprotected")
        || profile.get("protected_storage").and_then(Value::as_bool) != Some(false)
        || profile.get("encrypted").and_then(Value::as_bool) != Some(false)
        || profile.get("recoverable").and_then(Value::as_bool) != Some(false)
        || profile.get("recovery").and_then(Value::as_str) != Some("not_recovery_kit_packaged")
        || profile.get("reset").and_then(Value::as_str) != Some("whole_profile")
    {
        return Err("unsupported Browser profile descriptor".to_string());
    }
    if profile.get("public_uri").and_then(Value::as_str)
        != Some("localhost://Users/self/BrowserProfiles/default/profile.ext4")
    {
        return Err("Browser profile public_uri must use the Users/self alias".to_string());
    }
    let uri = profile
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser profile uri is required".to_string())?;
    if !uri.starts_with("localhost://Users/")
        || !uri.ends_with("/BrowserProfiles/default/profile.ext4")
        || uri.contains(['\0', '\r', '\n'])
        || uri.contains("/../")
        || uri.ends_with("/..")
    {
        return Err(
            "Browser profile uri must be under the active principal BrowserProfiles root"
                .to_string(),
        );
    }
    let profile_key = profile
        .get("profile_key")
        .and_then(Value::as_str)
        .ok_or_else(|| "Browser profile_key is required".to_string())?;
    if !elastos_common::is_safe_browser_profile_key(profile_key)
        || !profile_key.starts_with("profile-")
        || profile_key.len() != 72
    {
        return Err("Browser profile_key is unsafe".to_string());
    }
    let disk_path = PathBuf::from(
        profile
            .get("disk_path")
            .and_then(Value::as_str)
            .ok_or_else(|| "Browser profile disk_path is required".to_string())?,
    );
    validate_profile_disk_path(&disk_path)?;
    Ok((profile_key.to_string(), disk_path))
}

#[derive(Debug)]
struct PreparedBrowserProfileDisk {
    profile_key: String,
    path: PathBuf,
    _lock: LifetimeFileLock,
}

fn prepare_browser_profile_disk(request: &Value) -> Result<PreparedBrowserProfileDisk, String> {
    let (profile_key, disk_path) = profile_disk_from_request(request)?;
    if let Some(parent) = disk_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create Browser profile disk root failed: {err}"))?;
    }
    ensure_sparse_profile_disk(&disk_path)?;
    Ok(PreparedBrowserProfileDisk {
        profile_key,
        _lock: LifetimeFileLock::acquire_disk_sidecar(
            &disk_path,
            "principal Browser profile disk",
        )?,
        path: disk_path,
    })
}

fn append_browser_profile_boot_arg(boot_args: &mut String, profile_key: &str) {
    *boot_args = format!(
        "{boot_args} elastos.browser_profile={profile_key} elastos.browser_profile_disk=required"
    );
}

#[cfg(test)]
fn attach_browser_profile_disk(vm_config: &mut VmConfig, request: &Value) -> Result<(), String> {
    let profile_disk = prepare_browser_profile_disk(request)?;
    vm_config.data_disk_path = Some(profile_disk.path);
    append_browser_profile_boot_arg(&mut vm_config.boot_args, &profile_disk.profile_key);
    Ok(())
}

fn validate_profile_disk_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| component.as_os_str() == std::ffi::OsStr::new(".."))
        || path.to_string_lossy().contains(['\0', '\r', '\n'])
        || !path.ends_with("BrowserProfiles/default/profile.ext4")
    {
        return Err(
            "Browser profile disk_path must be an absolute active-principal profile disk path"
                .to_string(),
        );
    }
    Ok(())
}

fn ensure_sparse_profile_disk(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let size_mib = env_u64(
        "ELASTOS_BROWSER_VM_PROFILE_DISK_MIB",
        DEFAULT_PROFILE_DISK_MIB,
    )?;
    if !(128..=65536).contains(&size_mib) {
        return Err("ELASTOS_BROWSER_VM_PROFILE_DISK_MIB must be 128..65536".to_string());
    }
    let file = File::create(path).map_err(|err| {
        format!(
            "create Browser profile disk {} failed: {err}",
            path.display()
        )
    })?;
    file.set_len(size_mib * 1024 * 1024).map_err(|err| {
        format!(
            "resize Browser profile disk {} failed: {err}",
            path.display()
        )
    })?;
    Ok(())
}

fn browser_vm_manifest(memory_mib: u32, vcpu_count: u8) -> CapsuleManifest {
    CapsuleManifest {
        schema: SCHEMA_V1.to_string(),
        version: BROWSER_VM_TARGET_VERSION.to_string(),
        name: "browser-vm-target".to_string(),
        description: Some("ElastOS Browser VM target".to_string()),
	        author: Some("elastos".to_string()),
	        role: CapsuleRole::App,
	        capsule_type: CapsuleType::MicroVM,
	        runtime_abi: None,
	        bus_contract: None,
	        wit_world_sha256: None,
	        execution: None,
	        projections: Vec::new(),
	        entrypoint: "rootfs.ext4".to_string(),
        requires: Vec::new(),
        provides: None,
        authority: None,
        capabilities: Vec::new(),
        interfaces: Vec::new(),
        resources: ResourceLimits {
            memory_mb: memory_mib,
            cpu_shares: 100,
            gpu: false,
        },
        permissions: Permissions::default(),
        microvm: Some(MicroVmConfig {
            kernel: None,
            boot_args:
                "console=hvc0 reboot=k panic=1 root=/dev/vda rootfstype=ext4 rw init=/opt/elastos/bin/browser-vm-init random.trust_cpu=on"
                  .to_string(),
            http_port: None,
            vcpu_count: Some(vcpu_count),
            rootfs_cid: None,
            kernel_cid: None,
            rootfs_size: None,
            persistent_storage_mb: None,
        }),
        providers: None,
        viewer: None,
        signature: None,
    }
}

fn prepare_socket_path(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(path).map_err(|err| err.to_string())
        }
        Ok(_) => Err(format!(
            "Browser VZ control socket path exists and is not a socket: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

async fn restore_browser_vm_hibernation(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    hibernation: Option<&BrowserVmHibernation>,
) -> Result<bool, String> {
    let Some(hibernation) = hibernation else {
        return Ok(false);
    };
    if !hibernation.state_path.exists() {
        trace_stage(
            "hibernate_restore_skip",
            format!("key={} reason=no_state", hibernation.key),
        );
        return Ok(false);
    }
    match provider.supports_hibernation(handle).await {
        Ok(true) => {}
        Ok(false) => {
            trace_stage(
                "hibernate_restore_skip",
                format!("key={} reason=unsupported_config", hibernation.key),
            );
            return Ok(false);
        }
        Err(error) => return Err(error.to_string()),
    }
    trace_stage(
        "hibernate_restore_start",
        format!(
            "key={} state={}",
            hibernation.key,
            hibernation.state_path.display()
        ),
    );
    match provider
        .restore_from_hibernation(handle, &hibernation.state_path)
        .await
    {
        Ok(()) => {
            trace_stage("hibernate_restore_done", format!("key={}", hibernation.key));
            Ok(true)
        }
        Err(error) => {
            eprintln!(
                "browser-vz-engine-supervisor hibernate_restore_failed key={} error={}",
                hibernation.key, error
            );
            discard_bad_hibernation_state(hibernation);
            Ok(false)
        }
    }
}

async fn save_browser_vm_hibernation(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    hibernation: Option<&BrowserVmHibernation>,
) {
    let Some(hibernation) = hibernation else {
        return;
    };
    match provider.supports_hibernation(handle).await {
        Ok(true) => {}
        Ok(false) => {
            trace_stage(
                "hibernate_save_skip",
                format!("key={} reason=unsupported_config", hibernation.key),
            );
            return;
        }
        Err(error) => {
            eprintln!(
                "browser-vz-engine-supervisor hibernate_support_check_failed key={} error={}",
                hibernation.key, error
            );
            return;
        }
    }
    if let Err(error) = fs::create_dir_all(&hibernation.state_dir) {
        eprintln!(
            "browser-vz-engine-supervisor hibernate_prepare_failed key={} error={}",
            hibernation.key, error
        );
        return;
    }
    let _ = fs::remove_file(&hibernation.state_tmp_path);
    trace_stage(
        "hibernate_save_start",
        format!(
            "key={} state={}",
            hibernation.key,
            hibernation.state_path.display()
        ),
    );
    match provider
        .hibernate_to(handle, &hibernation.state_tmp_path)
        .await
    {
        Ok(()) => {
            if let Err(error) = fs::rename(&hibernation.state_tmp_path, &hibernation.state_path) {
                eprintln!(
                    "browser-vz-engine-supervisor hibernate_publish_failed key={} error={}",
                    hibernation.key, error
                );
                discard_hibernation_tmp_state(hibernation);
                return;
            }
            if let Err(error) = write_hibernation_metadata(hibernation) {
                eprintln!(
                    "browser-vz-engine-supervisor hibernate_metadata_failed key={} error={}",
                    hibernation.key, error
                );
            }
            trace_stage("hibernate_save_done", format!("key={}", hibernation.key));
        }
        Err(error) => {
            eprintln!(
                "browser-vz-engine-supervisor hibernate_save_failed key={} error={}",
                hibernation.key, error
            );
            discard_hibernation_tmp_state(hibernation);
        }
    }
}

fn discard_bad_hibernation_state(hibernation: &BrowserVmHibernation) {
    let _ = fs::remove_file(&hibernation.state_path);
}

fn discard_hibernation_tmp_state(hibernation: &BrowserVmHibernation) {
    let _ = fs::remove_file(&hibernation.state_tmp_path);
}

fn write_hibernation_metadata(hibernation: &BrowserVmHibernation) -> Result<(), String> {
    let metadata = json!({
        "schema": "elastos.browser.vm-hibernation-state/v1",
        "key": hibernation.key,
        "created_at_unix_secs": current_unix_seconds(),
        "state_path": hibernation.state_path.to_string_lossy(),
        "rootfs_path": hibernation.launch_rootfs_path.to_string_lossy(),
    });
    fs::write(
        &hibernation.metadata_path,
        serde_json::to_vec_pretty(&metadata).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn validate_runtime_stream_socket_path(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|err| err.to_string())?;
    if !metadata.file_type().is_socket() {
        return Err(format!(
            "adapter_ipc.runtime_stream_path is not a Unix socket: {}",
            path.display()
        ));
    }
    Ok(())
}

async fn open_guest_page(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    control_port: u32,
    request: &Value,
    paths: &LaunchPaths,
) -> Result<Value, String> {
    let guest_request = guest_control_open_request(request);
    let ready_timeout = Duration::from_millis(env_u32(
        "ELASTOS_BROWSER_VM_CONTROL_READY_TIMEOUT_MS",
        90_000,
    )? as u64);
    let deadline = Instant::now() + ready_timeout;
    let mut last_error = String::new();
    let mut result = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break Err(format!(
                "timed out opening Browser VM guest page via control port {control_port}: {last_error}"
            ));
        }
        let attempt_timeout = remaining.min(Duration::from_secs(1));
        match connect_vsock_with_retry(provider.clone(), handle, control_port, attempt_timeout)
            .await
        {
            Ok(fd) => {
                match http_json_over_fd(fd, "POST", "/pages", Some(&guest_request), Some(remaining))
                {
                    Ok(result) => break Ok(result),
                    Err(error) if is_retryable_guest_control_open_error(&error) => {
                        last_error = error;
                    }
                    Err(error) if is_guest_control_response_timeout(&error) => {
                        let probe =
                            probe_guest_control_status(provider.clone(), handle, control_port)
                                .await;
                        break Err(format!("{error}; guest control status probe: {probe}"));
                    }
                    Err(error) => break Err(error),
                }
            }
            Err(error) => {
                last_error = error;
            }
        }
        tokio::time::sleep(
            Duration::from_millis(500).min(deadline.saturating_duration_since(Instant::now())),
        )
        .await;
    }?;
    result["engine"] = json!("chromium_microvm");
    result["control_socket_path"] = json!(paths.control_socket_path.to_string_lossy().to_string());
    result["isolated_session"] = json!(true);
    result["isolation"] = json!({
        "schema": "elastos.browser.engine.isolation/v1",
        "kind": "per_launch_vm_target",
        "session_dir": paths.session_dir.to_string_lossy().to_string(),
    });
    if let Some(display) = result
        .get_mut("display_session")
        .and_then(Value::as_object_mut)
    {
        display.insert(
            "display_backend".to_string(),
            json!("vm_selkies_gstreamer_webrtc"),
        );
        display.insert("media_transport".to_string(), json!("runtime_relay"));
        normalize_display_media_from_offer(display);
        display.insert("network_mode".to_string(), json!("runtime_net_only"));
        display.insert("direct_network".to_string(), json!(false));
    }
    Ok(result)
}

async fn probe_guest_control_status(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    control_port: u32,
) -> String {
    let timeout_ms = env_u32(
        "ELASTOS_BROWSER_VM_CONTROL_STATUS_PROBE_TIMEOUT_MS",
        DEFAULT_CONTROL_STATUS_PROBE_TIMEOUT_MS,
    )
    .unwrap_or(DEFAULT_CONTROL_STATUS_PROBE_TIMEOUT_MS)
    .clamp(1, 30_000);
    let timeout = Duration::from_millis(timeout_ms as u64);
    trace_stage("guest_control_status_probe_start", "");
    let result = match connect_vsock_with_retry(provider, handle, control_port, timeout).await {
        Ok(fd) => match http_json_over_fd(fd, "GET", "/status", None, Some(timeout)) {
            Ok(value) => format!("ok {}", bounded_json(&value, 512)),
            Err(error) => format!("http_error {error}"),
        },
        Err(error) => format!("connect_error {error}"),
    };
    trace_stage("guest_control_status_probe_done", &result);
    result
}

fn bounded_json(value: &Value, limit: usize) -> String {
    let mut text = serde_json::to_string(value).unwrap_or_else(|_| "<invalid-json>".to_string());
    if text.len() > limit {
        text.truncate(limit);
        text.push_str("...");
    }
    text
}

fn guest_control_open_request(request: &Value) -> Value {
    let mut guest_request = request.clone();
    guest_request["schema"] = json!("elastos.browser.vm-guest.open/v1");
    guest_request["launch_request"]["engine"] = json!("selkies_gstreamer");
    if let Some(launch) = guest_request
        .get_mut("launch_request")
        .and_then(Value::as_object_mut)
    {
        launch.remove("transport_authority");
        launch.remove("transport_secret");
        launch.remove("vm_id");
    }
    guest_request
}

fn normalize_display_media_from_offer(display: &mut serde_json::Map<String, Value>) {
    let video_sdp = display
        .get("initial_offer")
        .and_then(|offer| offer.get("sdp"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let audio_sdp = display
        .get("audio_offer")
        .and_then(|offer| offer.get("sdp"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| video_sdp.clone());
    if let Some(sdp) = video_sdp {
        display.insert(
            "video".to_string(),
            json!(sdp_has_media_kind(&sdp, "video")),
        );
    }
    if let Some(sdp) = audio_sdp {
        display.insert(
            "audio".to_string(),
            json!(sdp_has_media_kind(&sdp, "audio")),
        );
    }
}

fn sdp_has_media_kind(sdp: &str, kind: &str) -> bool {
    let prefix = format!("m={kind} ");
    sdp.lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .any(|line| line.starts_with(&prefix))
}

fn is_retryable_guest_control_open_error(error: &str) -> bool {
    error.contains("Browser VM guest control HTTP 503")
        || error.contains("Connection reset")
        || error.contains("Broken pipe")
}

fn is_guest_control_response_timeout(error: &str) -> bool {
    error.contains("Browser VM control HTTP response timed out")
}

async fn connect_vsock_with_retry(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    port: u32,
    timeout: Duration,
) -> Result<OwnedFd, String> {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        let attempt_timeout = Duration::from_millis(env_u32(
            "ELASTOS_BROWSER_VM_VSOCK_ATTEMPT_TIMEOUT_MS",
            1_000,
        )? as u64)
        .min(deadline.saturating_duration_since(Instant::now()));
        match tokio::time::timeout(attempt_timeout, provider.vsock_connect(handle, port)).await {
            Ok(Ok(fd)) => return Ok(fd),
            Ok(Err(error)) => {
                last_error = error.to_string();
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(_) => {
                last_error = format!(
                    "connect attempt timed out after {} ms",
                    attempt_timeout.as_millis()
                );
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(format!(
        "timed out connecting to Browser VM guest vsock port {port}: {last_error}"
    ))
}

async fn bootstrap_vz_transport(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    transport: &VzTransportLaunch,
) -> Result<Value, String> {
    let port = transport
        .authority
        .get("bootstrap_vsock_port")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| "Browser VZ bootstrap vsock port is missing".to_string())?;
    let timeout = Duration::from_millis(env_u32(
        "ELASTOS_BROWSER_VM_TRANSPORT_BOOTSTRAP_TIMEOUT_MS",
        90_000,
    )? as u64);
    let fd = connect_vsock_with_retry(provider, handle, port, timeout).await?;
    let mut stream = File::from(fd);
    set_file_read_timeout(&stream, timeout)?;
    set_file_write_timeout(&stream, timeout)?;
    let request = json!({
        "schema": "elastos.browser.vz-transport-bootstrap/v1",
        "authority": transport.authority,
        "secret": transport.secret,
    });
    let mut bytes = serde_json::to_vec(&request).map_err(|err| err.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Browser VZ transport bootstrap request is too large".to_string());
    }
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .and_then(|_| stream.flush())
        .map_err(|err| format!("Browser VZ transport bootstrap write failed: {err}"))?;
    let mut response = Vec::new();
    stream
        .take(64 * 1024 + 1)
        .read_to_end(&mut response)
        .map_err(|err| format!("Browser VZ transport bootstrap read failed: {err}"))?;
    if response.len() > 64 * 1024 {
        return Err("Browser VZ transport bootstrap receipt is too large".to_string());
    }
    let receipt: Value = serde_json::from_slice(
        response
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .ok_or_else(|| "Browser VZ transport bootstrap receipt is empty".to_string())?,
    )
    .map_err(|err| format!("Browser VZ transport bootstrap receipt is invalid JSON: {err}"))?;
    validate_vz_transport_bootstrap_receipt(&receipt, &transport.authority)?;
    Ok(receipt)
}

fn validate_vz_transport_bootstrap_receipt(
    receipt: &Value,
    authority: &Value,
) -> Result<(), String> {
    let object = receipt
        .as_object()
        .ok_or_else(|| "Browser VZ transport bootstrap receipt must be an object".to_string())?;
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
        .ok_or_else(|| "Browser VZ transport bootstrap effects are missing".to_string())?;
    let effect_keys = [
        "descriptor_validated",
        "authority_owner_only",
        "ice_config_owner_only",
        "loopback_only",
        "interfaces",
        "default_route_absent",
        "direct_network_probe_failed",
    ];
    if object.len() != keys.len()
        || keys.iter().any(|key| !object.contains_key(*key))
        || effects.len() != effect_keys.len()
        || effect_keys.iter().any(|key| !effects.contains_key(*key))
        || receipt.get("schema").and_then(Value::as_str)
            != Some("elastos.browser.vz-transport-bootstrap-receipt/v1")
        || receipt.get("binding_hash") != authority.get("binding_hash")
        || receipt.get("generation") != authority.get("generation")
        || receipt.get("page_id") != authority.get("page_id")
        || receipt.get("vm_id") != authority.get("vm_id")
        || receipt.get("expires_at_unix_ms") != authority.get("expires_at_unix_ms")
        || receipt.get("terminal").and_then(Value::as_bool) != Some(true)
        || effects.get("interfaces") != Some(&json!(["lo"]))
        || effect_keys
            .iter()
            .filter(|key| **key != "interfaces")
            .any(|key| effects.get(*key).and_then(Value::as_bool) != Some(true))
        || value_contains_transport_secret(receipt)
    {
        return Err(
            "Browser VZ guest did not return an exact transport bootstrap receipt".to_string(),
        );
    }
    Ok(())
}

fn vz_transport_effect_receipt(
    transport: &VzTransportLaunch,
    guest_receipt: &Value,
) -> Result<Value, String> {
    validate_vz_transport_bootstrap_receipt(guest_receipt, &transport.authority)?;
    let receipt = json!({
        "schema": "elastos.browser.vz-transport-effect-receipt/v1",
        "binding_hash": transport.authority["binding_hash"],
        "generation": transport.authority["generation"],
        "page_id": transport.authority["page_id"],
        "vm_id": transport.authority["vm_id"],
        "expires_at_unix_ms": transport.authority["expires_at_unix_ms"],
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
    });
    validate_vz_transport_effect_receipt(&receipt, &transport.authority)?;
    Ok(receipt)
}

fn validate_vz_transport_effect_receipt(receipt: &Value, authority: &Value) -> Result<(), String> {
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
        return Err(
            "Browser VZ supervisor did not produce an exact transport effect receipt".to_string(),
        );
    }
    Ok(())
}

fn value_contains_transport_secret(value: &Value) -> bool {
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

async fn connect_vsock_once(
    provider: &VzProvider,
    handle: &CapsuleHandle,
    port: u32,
) -> Result<OwnedFd, String> {
    let attempt_timeout = Duration::from_millis(env_u32(
        "ELASTOS_BROWSER_VM_VSOCK_ATTEMPT_TIMEOUT_MS",
        1_000,
    )? as u64);
    match tokio::time::timeout(attempt_timeout, provider.vsock_connect(handle, port)).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "connect attempt timed out after {} ms",
            attempt_timeout.as_millis()
        )),
    }
}

async fn connect_vsock_until_shutdown(
    provider: Arc<VzProvider>,
    handle: &CapsuleHandle,
    port: u32,
    shutdown: &AtomicBool,
) -> Result<OwnedFd, String> {
    let mut last_error = String::new();
    while !shutdown.load(Ordering::Relaxed) {
        match connect_vsock_once(&provider, handle, port).await {
            Ok(fd) => return Ok(fd),
            Err(error) => {
                last_error = error;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(format!(
        "shutdown before Browser VM guest vsock port {port} was ready: {last_error}"
    ))
}

fn http_json_over_fd(
    fd: OwnedFd,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Option<Duration>,
) -> Result<Value, String> {
    let mut stream = File::from(fd);
    if let Some(timeout) = timeout {
        set_file_read_timeout(&stream, timeout)?;
        set_file_write_timeout(&stream, timeout)?;
    }
    let body_bytes = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|err| err.to_string())?
        .unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: browser-vm-guest\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body_bytes.len()
    )
  .map_err(|err| err.to_string())?;
    if !body_bytes.is_empty() {
        stream
            .write_all(&body_bytes)
            .map_err(|err| err.to_string())?;
    }
    stream.flush().map_err(|err| err.to_string())?;
    let response = read_one_http_response(&mut stream, timeout)?;
    parse_http_json_response(&response)
}

fn parse_http_json_response(response: &[u8]) -> Result<Value, String> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            format!(
                "Browser VM guest control returned an invalid HTTP response: len={} preview={}",
                response.len(),
                response_preview(response)
            )
        })?;
    let (head, body) = response.split_at(split + 4);
    let head_text =
        std::str::from_utf8(head).map_err(|_| "Browser VM guest HTTP head is not UTF-8")?;
    let status_line = head_text.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "Browser VM guest HTTP status is invalid".to_string())?;
    let parsed: Value = serde_json::from_slice(body)
        .map_err(|err| format!("Browser VM guest control response is not JSON: {err}"))?;
    if !(200..300).contains(&status) {
        let error = parsed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Browser VM guest control returned an error")
            .to_string();
        let mut message = format!("Browser VM guest control HTTP {status}: {error}");
        if let Some(logs) = parsed.get("logs") {
            let logs_text = serde_json::to_string(logs)
                .unwrap_or_else(|_| "<failed to encode guest logs>".to_string());
            let mut bounded = logs_text.chars().take(20_000).collect::<String>();
            if logs_text.len() > bounded.len() {
                bounded.push_str("...");
            }
            message.push_str(" logs=");
            message.push_str(&bounded);
        }
        return Err(message);
    }
    Ok(parsed)
}

fn response_preview(response: &[u8]) -> String {
    let mut preview = String::new();
    for byte in response.iter().take(160) {
        let _ = write!(preview, "{byte:02x}");
    }
    if response.len() > 160 {
        preview.push_str("...");
    }
    preview
}

fn spawn_control_proxy(
    provider: Arc<VzProvider>,
    handle: CapsuleHandle,
    control_port: u32,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<thread::JoinHandle<()>, String> {
    let listener = UnixListener::bind(&socket_path).map_err(|err| err.to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|err| err.to_string())?;
    Ok(thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((host_stream, _)) => {
                    if let Err(error) = host_stream.set_nonblocking(false) {
                        eprintln!("Browser VM host control proxy client setup failed: {error}");
                        continue;
                    }
                    let provider = Arc::clone(&provider);
                    let handle = handle.clone();
                    thread::spawn(move || {
                        let runtime = match tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                        {
                            Ok(runtime) => runtime,
                            Err(error) => {
                                let _ = write_error_response(host_stream, &error.to_string());
                                return;
                            }
                        };
                        let request_timeout = Duration::from_millis(
                            env_u32(
                                "ELASTOS_BROWSER_VM_CONTROL_PROXY_REQUEST_TIMEOUT_MS",
                                DEFAULT_CONTROL_PROXY_REQUEST_TIMEOUT_MS,
                            )
                            .unwrap_or(DEFAULT_CONTROL_PROXY_REQUEST_TIMEOUT_MS)
                                as u64,
                        );
                        match runtime.block_on(connect_vsock_with_retry(
                            Arc::clone(&provider),
                            &handle,
                            control_port,
                            request_timeout,
                        )) {
                            Ok(fd) => {
                                if let Err(error) = proxy_http_control_request(
                                    host_stream,
                                    File::from(fd),
                                    request_timeout,
                                ) {
                                    eprintln!("Browser VM host control proxy failed: {error}");
                                }
                            }
                            Err(error) => {
                                let _ = write_error_response(host_stream, &error.to_string());
                            }
                        }
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    }))
}

fn proxy_http_control_request(
    mut host_stream: UnixStream,
    mut guest_stream: File,
    request_timeout: Duration,
) -> Result<(), String> {
    set_file_read_timeout(&guest_stream, request_timeout)?;
    set_file_write_timeout(&guest_stream, request_timeout)?;
    let request = read_one_http_request(&mut host_stream)?;
    guest_stream
        .write_all(&request)
        .map_err(|err| err.to_string())?;
    guest_stream.flush().map_err(|err| err.to_string())?;

    let response = read_one_http_response(&mut guest_stream, Some(request_timeout))?;
    host_stream
        .write_all(&response)
        .map_err(|err| err.to_string())?;
    host_stream.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn read_one_http_request(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 8192];
    let header_end = loop {
        if let Some(position) = find_header_end(&request) {
            break position;
        }
        if request.len() > MAX_CONTROL_HTTP_HEADER_BYTES {
            return Err("Browser VM control HTTP request headers are too large".to_string());
        }
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP request closed before headers".to_string());
        }
        request.extend_from_slice(&buffer[..read]);
    };
    let content_length = parse_content_length(&request[..header_end])?;
    if content_length > MAX_CONTROL_HTTP_BODY_BYTES {
        return Err("Browser VM control HTTP request body is too large".to_string());
    }
    let total = header_end + 4 + content_length;
    while request.len() < total {
        let read = stream.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("Browser VM control HTTP request closed before body".to_string());
        }
        request.extend_from_slice(&buffer[..read]);
    }
    request.truncate(total);
    Ok(request)
}

fn read_one_http_response(stream: &mut File, timeout: Option<Duration>) -> Result<Vec<u8>, String> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    let header_end = loop {
        if let Some(position) = find_header_end(&response) {
            break position;
        }
        if response.len() > MAX_CONTROL_HTTP_HEADER_BYTES {
            return Err("Browser VM control HTTP response headers are too large".to_string());
        }
        let read = read_control_http_response_chunk(stream, &mut buffer, deadline)?;
        if read == 0 {
            return Err("Browser VM control HTTP response closed before headers".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
    };
    let content_length = parse_content_length(&response[..header_end])?;
    if content_length > MAX_CONTROL_HTTP_RESPONSE_BYTES {
        return Err("Browser VM control HTTP response body is too large".to_string());
    }
    let total = header_end + 4 + content_length;
    while response.len() < total {
        let read = read_control_http_response_chunk(stream, &mut buffer, deadline)?;
        if read == 0 {
            return Err("Browser VM control HTTP response closed before body".to_string());
        }
        response.extend_from_slice(&buffer[..read]);
    }
    response.truncate(total);
    Ok(response)
}

fn read_control_http_response_chunk(
    stream: &mut File,
    buffer: &mut [u8],
    deadline: Option<Instant>,
) -> Result<usize, String> {
    wait_for_control_http_response_readable(stream, deadline)?;
    stream.read(buffer).map_err(|err| {
        if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
            "Browser VM control HTTP response timed out".to_string()
        } else {
            err.to_string()
        }
    })
}

fn wait_for_control_http_response_readable(
    stream: &File,
    deadline: Option<Instant>,
) -> Result<(), String> {
    let Some(deadline) = deadline else {
        return Ok(());
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Browser VM control HTTP response timed out".to_string());
        }
        let timeout_ms = remaining.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut poll_fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            return Err("Browser VM control HTTP response timed out".to_string());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == ErrorKind::Interrupted {
            continue;
        }
        return Err(error.to_string());
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header: &[u8]) -> Result<usize, String> {
    let text = std::str::from_utf8(header)
        .map_err(|_| "Browser VM control HTTP request headers are not UTF-8".to_string())?;
    let mut content_length = 0_usize;
    for line in text.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Browser VM control HTTP request Content-Length is invalid")?;
        }
    }
    Ok(content_length)
}

fn spawn_egress_bridge(
    provider: Arc<VzProvider>,
    handle: CapsuleHandle,
    relay_port: u32,
    runtime_stream_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    max_sessions: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut session_id = 0_u64;
        let session_slots = Arc::new(Semaphore::new(max_sessions));
        while !shutdown.load(Ordering::Relaxed) {
            let Ok(session_slot) = Arc::clone(&session_slots).acquire_owned().await else {
                break;
            };
            match connect_vsock_until_shutdown(
                Arc::clone(&provider),
                &handle,
                relay_port,
                &shutdown,
            )
            .await
            {
                Ok(fd) => {
                    session_id += 1;
                    let session_id = session_id;
                    let trace_egress = env_bool("ELASTOS_BROWSER_VM_TRACE_EGRESS", false);
                    if trace_egress {
                        eprintln!("Browser VM host egress bridge accepted session {session_id}");
                    }
                    let host_path = runtime_stream_path.clone();
                    let _worker = tokio::task::spawn_blocking(move || {
                        let _session_slot = session_slot;
                        let guest_stream = File::from(fd);
                        let result = (|| -> Result<(), String> {
                            let runtime_stream = UnixStream::connect(host_path).map_err(|err| {
                                format!("Browser VM Runtime Exit relay unavailable: {err}")
                            })?;
                            let (guest_to_runtime, runtime_to_guest) = forward_pair(
                                DuplexStream::File(guest_stream),
                                DuplexStream::Unix(runtime_stream),
                            )?;
                            if trace_egress {
                                eprintln!(
                                    "Browser VM host egress bridge session {session_id} guest_to_runtime={guest_to_runtime} runtime_to_guest={runtime_to_guest}"
                                );
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            eprintln!(
                                "Browser VM host egress bridge session {session_id} failed: {error}"
                            );
                        }
                    });
                }
                Err(_) => {
                    drop(session_slot);
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    })
}

enum DuplexStream {
    Unix(UnixStream),
    File(File),
}

impl DuplexStream {
    fn try_clone(&self) -> Result<Self, String> {
        match self {
            Self::Unix(stream) => stream
                .try_clone()
                .map(Self::Unix)
                .map_err(|err| err.to_string()),
            Self::File(file) => file
                .try_clone()
                .map(Self::File)
                .map_err(|err| err.to_string()),
        }
    }

    fn shutdown_write(&self) {
        match self {
            Self::Unix(stream) => {
                let _ = stream.shutdown(Shutdown::Write);
            }
            Self::File(file) => {
                let _ = shutdown_file_write(file);
            }
        }
    }

    fn shutdown_both(&self) {
        match self {
            Self::Unix(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
            }
            Self::File(file) => {
                let _ = shutdown_file(file, libc::SHUT_RDWR);
            }
        }
    }

    fn set_read_timeout(&self, timeout: Duration) -> Result<(), String> {
        match self {
            Self::Unix(stream) => stream
                .set_read_timeout(Some(timeout))
                .map_err(|err| err.to_string()),
            Self::File(file) => set_file_read_timeout(file, timeout),
        }
    }
}

fn shutdown_file_write(file: &File) -> Result<(), String> {
    shutdown_file(file, libc::SHUT_WR)
}

fn shutdown_file(file: &File, how: libc::c_int) -> Result<(), String> {
    let result = unsafe { libc::shutdown(file.as_raw_fd(), how) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn set_file_read_timeout(file: &File, timeout: Duration) -> Result<(), String> {
    set_file_socket_timeout(file, libc::SO_RCVTIMEO, timeout)
}

fn set_file_write_timeout(file: &File, timeout: Duration) -> Result<(), String> {
    set_file_socket_timeout(file, libc::SO_SNDTIMEO, timeout)
}

fn set_file_socket_timeout(
    file: &File,
    option_name: libc::c_int,
    timeout: Duration,
) -> Result<(), String> {
    let timeval = libc::timeval {
        tv_sec: timeout.as_secs().min(i64::MAX as u64) as libc::time_t,
        tv_usec: timeout.subsec_micros() as libc::suseconds_t,
    };
    let result = unsafe {
        libc::setsockopt(
            file.as_raw_fd(),
            libc::SOL_SOCKET,
            option_name,
            &timeval as *const libc::timeval as *const libc::c_void,
            std::mem::size_of_val(&timeval) as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

impl Read for DuplexStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.read(buffer),
            Self::File(file) => file.read(buffer),
        }
    }
}

impl Write for DuplexStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Unix(stream) => stream.write(buffer),
            Self::File(file) => file.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Unix(stream) => stream.flush(),
            Self::File(file) => file.flush(),
        }
    }
}

fn forward_pair(left: DuplexStream, right: DuplexStream) -> Result<(u64, u64), String> {
    let mut left_to_right_in = left.try_clone()?;
    let mut right_to_left_out = left;
    let mut right_to_left_in = right.try_clone()?;
    let mut left_to_right_out = right;
    left_to_right_in.set_read_timeout(Duration::from_millis(250))?;
    right_to_left_in.set_read_timeout(Duration::from_millis(250))?;
    let done = Arc::new(AtomicBool::new(false));
    let done_to_right = Arc::clone(&done);
    let to_right = thread::spawn(move || {
        let result = copy_stream_until_done(
            &mut left_to_right_in,
            &mut left_to_right_out,
            &done_to_right,
        );
        done_to_right.store(true, Ordering::Relaxed);
        left_to_right_out.shutdown_write();
        result
    });
    let to_left = copy_stream_until_done(&mut right_to_left_in, &mut right_to_left_out, &done);
    done.store(true, Ordering::Relaxed);
    right_to_left_out.shutdown_both();
    let to_right = to_right
        .join()
        .map_err(|_| "Browser VM bridge worker panicked".to_string())??;
    Ok((to_right, to_left?))
}

fn copy_stream_until_done<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    done: &AtomicBool,
) -> Result<u64, String> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; EGRESS_COPY_BUFFER_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(copied),
            Ok(read) => {
                writer
                    .write_all(&buffer[..read])
                    .map_err(|err| err.to_string())?;
                copied = copied.saturating_add(read as u64);
            }
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if done.load(Ordering::Relaxed) {
                    return Ok(copied);
                }
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

fn write_error_response(mut stream: UnixStream, message: &str) -> Result<(), String> {
    let body = serde_json::to_vec(&json!({ "error": message })).map_err(|err| err.to_string())?;
    write!(
        stream,
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
  .map_err(|err| err.to_string())?;
    stream.write_all(&body).map_err(|err| err.to_string())
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

async fn wait_for_shutdown_or_transport_expiry(transport: Option<&VzTransportLaunch>) {
    let Some(transport) = transport else {
        wait_for_shutdown_signal().await;
        return;
    };
    let expires_at = transport
        .authority
        .get("expires_at_unix_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let remaining = current_unix_millis()
        .ok()
        .and_then(|now| expires_at.checked_sub(now))
        .unwrap_or(0);
    tokio::select! {
        _ = wait_for_shutdown_signal() => {}
        _ = tokio::time::sleep(Duration::from_millis(remaining)) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead as _;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::sync::{mpsc, Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn transport_fixture(suffix: char) -> VzTransportLaunch {
        let expires_at = (current_unix_millis().unwrap() / 1_000 + 300) * 1_000;
        let generation = format!("sha256:{}", suffix.to_string().repeat(64));
        let username = format!("{}:{}", expires_at / 1_000, suffix.to_string().repeat(16));
        let auth_secret = format!("transport-secret-{}", suffix.to_string().repeat(32));
        let mut mac = Hmac::<Sha1>::new_from_slice(auth_secret.as_bytes()).unwrap();
        mac.update(username.as_bytes());
        let credential =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let mut authority = json!({
            "schema": "elastos.browser.vz-transport-authority/v1",
            "generation": generation,
            "page_id": format!("page:vz-{suffix}"),
            "vm_id": format!("vm:vz-{suffix}"),
            "principal_id": format!("person:local:{suffix}"),
            "egress": {
                "schema": "elastos.browser.vz-transport-stream/v1",
                "stream_id": format!("stream:egress-{suffix}"),
                "target": "tls://example.invalid:443",
                "runtime_socket_path": format!("/tmp/vz-egress-{suffix}.sock"),
                "vsock_port": 19091,
            },
            "media": {
                "schema": "elastos.browser.vz-transport-stream/v1",
                "stream_id": format!("stream:media-{suffix}"),
                "target": "tcp://127.0.0.1:49160",
                "runtime_socket_path": format!("/tmp/vz-media-{suffix}.sock"),
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
        let binding_hash = sha256_label(&canonical_json_bytes(&authority).unwrap());
        authority["binding_hash"] = json!(binding_hash);
        let secret = json!({
            "schema": "elastos.browser.vz-transport-secret/v1",
            "binding_hash": binding_hash,
            "credential": credential,
            "auth_secret": auth_secret,
        });
        VzTransportLaunch { authority, secret }
    }

    fn guest_transport_receipt(authority: &Value) -> Value {
        json!({
            "schema": "elastos.browser.vz-transport-bootstrap-receipt/v1",
            "binding_hash": authority["binding_hash"],
            "generation": authority["generation"],
            "page_id": authority["page_id"],
            "vm_id": authority["vm_id"],
            "expires_at_unix_ms": authority["expires_at_unix_ms"],
            "terminal": true,
            "effects": {
                "descriptor_validated": true,
                "authority_owner_only": true,
                "ice_config_owner_only": true,
                "loopback_only": true,
                "interfaces": ["lo"],
                "default_route_absent": true,
                "direct_network_probe_failed": true,
            },
        })
    }

    #[test]
    fn vz_transport_rejects_substitution_replay_expiry_and_malformed_receipts() {
        let first = transport_fixture('a');
        let second = transport_fixture('b');
        validate_vz_transport_authority(&first.authority, true).unwrap();
        validate_vz_transport_secret(&first.authority, &first.secret).unwrap();
        assert_eq!(
            browser_vm_id(Some(&first)).unwrap(),
            first.authority["vm_id"].as_str().unwrap()
        );

        let mut substituted = first.authority.clone();
        substituted["vm_id"] = json!("vm:vz-substituted");
        assert!(validate_vz_transport_authority(&substituted, true).is_err());
        assert!(validate_vz_transport_secret(&first.authority, &second.secret).is_err());

        let mut expired = first.authority.clone();
        expired["expires_at_unix_ms"] = json!(1);
        assert!(validate_vz_transport_authority(&expired, true).is_err());

        let guest = guest_transport_receipt(&first.authority);
        let effect = vz_transport_effect_receipt(&first, &guest).unwrap();
        validate_vz_transport_effect_receipt(&effect, &first.authority).unwrap();
        let mut malformed = effect;
        malformed["effects"]["media_stream_fixed_target"] = json!(false);
        assert!(validate_vz_transport_effect_receipt(&malformed, &first.authority).is_err());
    }

    #[test]
    fn launch_owned_turn_failure_is_typed_and_retires_private_config() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _turn_program = EnvVarRestore::capture("ELASTOS_BROWSER_VM_TURN_PROGRAM");
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("turn-failure.sh");
        fs::write(&program, "#!/bin/sh\nexit 17\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("ELASTOS_BROWSER_VM_TURN_PROGRAM", &program);
        let session_dir = root.path().join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let paths = LaunchPaths {
            data_dir: root.path().to_path_buf(),
            kernel_path: root.path().join("kernel"),
            rootfs_path: root.path().join("rootfs"),
            initramfs_path: None,
            state_dir: root.path().join("state"),
            rootfs_cache_dir: root.path().join("cache"),
            session_dir: session_dir.clone(),
            control_socket_path: session_dir.join("control.sock"),
            runtime_stream_path: session_dir.join("runtime.sock"),
            control_port: 19092,
            relay_port: 19091,
            memory_mib: 2048,
            vcpu_count: 2,
        };
        let error = match LaunchTurn::start(&paths, &transport_fixture('c')) {
            Ok(_) => panic!("failing TURN fixture unexpectedly started"),
            Err(error) => error,
        };
        assert!(error.contains("exited before readiness"));
        assert!(!session_dir.join("turnserver.conf").exists());
    }

    struct EnvVarRestore {
        name: &'static str,
        value: Option<std::ffi::OsString>,
    }

    impl EnvVarRestore {
        fn capture(name: &'static str) -> Self {
            Self {
                name,
                value: std::env::var_os(name),
            }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            match self.value.as_ref() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    fn with_hibernation_env<T>(root: &Path, test: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let _enabled = EnvVarRestore::capture("ELASTOS_BROWSER_VM_HIBERNATION");
        let _dir = EnvVarRestore::capture("ELASTOS_BROWSER_VM_HIBERNATION_DIR");
        let _max_entries = EnvVarRestore::capture("ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES");
        let _max_age = EnvVarRestore::capture("ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS");
        std::env::set_var("ELASTOS_BROWSER_VM_HIBERNATION", "1");
        std::env::set_var("ELASTOS_BROWSER_VM_HIBERNATION_DIR", root);
        std::env::remove_var("ELASTOS_BROWSER_VM_HIBERNATION_MAX_ENTRIES");
        std::env::remove_var("ELASTOS_BROWSER_VM_HIBERNATION_MAX_AGE_SECS");
        test()
    }

    fn hibernation_fixture_paths(root: &Path) -> (LaunchPaths, PathBuf) {
        let data_dir = root.join("data");
        let kernel_path = data_dir.join("bin/vmlinux");
        let rootfs_path = data_dir.join("browser-vm/rootfs.ext4");
        let initramfs_path = data_dir.join("bin/initrd");
        let profile_disk_path = data_dir.join("profiles/profile.ext4");
        fs::create_dir_all(kernel_path.parent().unwrap()).unwrap();
        fs::create_dir_all(rootfs_path.parent().unwrap()).unwrap();
        fs::create_dir_all(initramfs_path.parent().unwrap()).unwrap();
        fs::create_dir_all(profile_disk_path.parent().unwrap()).unwrap();
        fs::write(&kernel_path, b"kernel-a").unwrap();
        fs::write(&rootfs_path, b"rootfs-a").unwrap();
        fs::write(&initramfs_path, b"initramfs-a").unwrap();
        fs::write(&profile_disk_path, b"profile-a").unwrap();
        (
            LaunchPaths {
                data_dir,
                kernel_path,
                rootfs_path,
                initramfs_path: Some(initramfs_path),
                state_dir: root.join("state"),
                rootfs_cache_dir: root.join("rootfs-cache"),
                session_dir: root.join("session"),
                control_socket_path: root.join("session/c.sock"),
                runtime_stream_path: root.join("runtime.sock"),
                control_port: DEFAULT_CONTROL_PORT,
                relay_port: DEFAULT_RELAY_PORT,
                memory_mib: 2048,
                vcpu_count: 2,
            },
            profile_disk_path,
        )
    }

    fn hibernation_launch() -> Value {
        json!({
            "schema": "elastos.browser.engine.launch-request/v1",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm"
        })
    }

    fn hibernation_for(
        paths: &LaunchPaths,
        launch: &Value,
        profile_key: &str,
        profile_disk_path: &Path,
        boot_args: &str,
    ) -> BrowserVmHibernation {
        BrowserVmHibernation::from_env(paths, launch, profile_key, profile_disk_path, boot_args)
            .unwrap()
            .expect("hibernation should be enabled for this test")
    }

    fn create_hibernation_cache_entry(root: &Path, index: u64) -> PathBuf {
        let key = format!("{index:064x}");
        let path = root.join(&key[..2]).join(key);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("rootfs.ext4"), b"rootfs").unwrap();
        fs::write(path.join("machine.state"), b"state").unwrap();
        path
    }

    #[test]
    fn display_media_flags_follow_initial_offer_sdp() {
        let mut display = serde_json::Map::new();
        display.insert(
            "initial_offer".to_string(),
            json!({
                "schema": "elastos.browser.webrtc-offer/v1",
                "type": "offer",
                "sdp": "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 97\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
            }),
        );
        display.insert("audio".to_string(), json!(true));
        display.insert("video".to_string(), json!(false));

        normalize_display_media_from_offer(&mut display);

        assert_eq!(display.get("audio"), Some(&json!(false)));
        assert_eq!(display.get("video"), Some(&json!(true)));
    }

    #[test]
    fn display_media_flags_follow_split_audio_offer_sdp() {
        let mut display = serde_json::Map::new();
        display.insert(
            "initial_offer".to_string(),
            json!({
                "schema": "elastos.browser.webrtc-offer/v1",
                "type": "offer",
                "sdp": "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 97\r\n",
            }),
        );
        display.insert(
            "audio_offer".to_string(),
            json!({
                "schema": "elastos.browser.webrtc-offer/v1",
                "type": "offer",
                "sdp": "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            }),
        );
        display.insert("audio".to_string(), json!(false));
        display.insert("video".to_string(), json!(false));

        normalize_display_media_from_offer(&mut display);

        assert_eq!(display.get("audio"), Some(&json!(true)));
        assert_eq!(display.get("video"), Some(&json!(true)));
    }

    #[test]
    fn display_boot_args_include_launch_viewport() {
        let launch = json!({
            "schema": "elastos.browser.engine.launch-request/v1",
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "stream_id": "stream_test",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "adapter_ipc": {
                "kind": "unix_socket",
                "path": "/tmp/elastos-browser-adapter.sock",
                "runtime_stream_path": "/tmp/elastos-browser-runtime-stream.sock"
            },
            "viewport": {
                "width": 1470,
                "height": 758
            }
        });
        validate_launch_request(&launch, false).unwrap();
        let mut boot_args = "console=hvc0".to_string();

        append_browser_display_boot_args_with_ice_config(&mut boot_args, &launch, true, None)
            .unwrap();

        assert!(boot_args.contains("elastos.browser_display_mode=webrtc_remote_display"));
        assert!(boot_args.contains("elastos.browser_width=1470"));
        assert!(boot_args.contains("elastos.browser_height=758"));
    }

    #[test]
    fn launch_requires_runtime_owned_stream_path_for_egress() {
        let launch = json!({
            "schema": "elastos.browser.engine.launch-request/v1",
            "adapter": "browser-vm-product",
            "engine": "chromium_microvm",
            "stream_id": "stream_test",
            "display_mode": "webrtc_remote_display",
            "guarantee_level": "mechanism_microvm",
            "network_mode": "runtime_net_only",
            "direct_network": false,
            "wallet_injection": false,
            "relay_ipc": {
                "kind": "unix_socket",
                "path": "/tmp/elastos-browser-local-exit-relay.sock"
            }
        });

        let err = validate_launch_request(&launch, false).unwrap_err();

        assert!(err.contains("adapter_ipc.runtime_stream_path"));
    }

    #[test]
    fn runtime_stream_path_must_be_a_unix_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let plain_file = tmp.path().join("runtime-stream.sock");
        fs::write(&plain_file, b"not a socket").unwrap();

        let err = validate_runtime_stream_socket_path(&plain_file).unwrap_err();

        assert!(err.contains("adapter_ipc.runtime_stream_path is not a Unix socket"));
    }

    #[test]
    fn unix_socket_path_budget_counts_utf8_bytes() {
        let path_text = format!("/tmp/{}", "\u{00e9}".repeat(50));
        assert!(path_text.chars().count() < UNIX_SOCKET_PATH_BUDGET);
        assert!(path_text.len() >= UNIX_SOCKET_PATH_BUDGET);

        let err = validate_unix_socket_path_budget("guest control socket", Path::new(&path_text))
            .unwrap_err();

        assert!(err.contains("too long for macOS Unix sockets"));
        assert!(err.contains("bytes"));
    }

    #[test]
    fn guest_control_open_preserves_microvm_guarantee() {
        let request = json!({
            "schema": "elastos.browser.vm-engine.open/v1",
            "launch_request": {
                "schema": "elastos.browser.engine.launch-request/v1",
                "adapter": "browser-vm-product",
                "engine": "chromium_microvm",
                "stream_id": "stream_test",
                "display_mode": "webrtc_remote_display",
                "guarantee_level": "mechanism_microvm",
                "network_mode": "runtime_net_only",
                "direct_network": false,
                "wallet_injection": false,
            },
        });

        let guest_request = guest_control_open_request(&request);

        assert_eq!(
            guest_request.get("schema").and_then(Value::as_str),
            Some("elastos.browser.vm-guest.open/v1")
        );
        assert_eq!(
            guest_request
                .get("launch_request")
                .and_then(|launch| launch.get("engine"))
                .and_then(Value::as_str),
            Some("selkies_gstreamer")
        );
        assert_eq!(
            guest_request
                .get("launch_request")
                .and_then(|launch| launch.get("guarantee_level"))
                .and_then(Value::as_str),
            Some("mechanism_microvm")
        );
        assert_eq!(
            request
                .get("launch_request")
                .and_then(|launch| launch.get("engine"))
                .and_then(Value::as_str),
            Some("chromium_microvm")
        );
    }

    #[test]
    fn browser_profile_uses_principal_owned_data_disk_descriptor() {
        let tmp = tempfile::tempdir().unwrap();
        let disk_path = tmp
            .path()
            .join("Users/0123456789ab/BrowserProfiles/default/profile.ext4");
        let mut vm_config = VmConfig {
            vm_id: "browser-vm-test".to_string(),
            kernel_path: tmp.path().join("vmlinux"),
            boot_args: "console=hvc0".to_string(),
            rootfs_path: tmp.path().join("rootfs.ext4"),
            rootfs_readonly: false,
            mem_size_mib: 1024,
            vcpu_count: 1,
            http_port: None,
            data_disk_path: None,
            vsock_cid: 3,
            network: None,
            network_disabled: false,
            interactive_stdio: false,
            carrier_socket_path: None,
            initramfs_path: None,
        };
        let request = json!({
            "schema": "elastos.browser.engine.launch-request/v1",
            "profile": {
                "schema": "elastos.browser.profile/v1",
                "scope": "active_principal",
                "storage": "principal_owned_profile_disk",
                "storage_posture": "principal_owned_reset_scoped_unprotected",
                "protected_storage": false,
                "encrypted": false,
                "recoverable": false,
                "recovery": "not_recovery_kit_packaged",
                "uri": "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4",
                "public_uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4",
                "profile_key": "profile-99bb2b58175e1e062cd2fb6b1b00feec63d169f520dd0a8cfe7230517cfc43e4",
                "disk_path": disk_path,
                "reset": "whole_profile"
            }
        });

        attach_browser_profile_disk(&mut vm_config, &request).unwrap();

        let disk = vm_config.data_disk_path.as_ref().unwrap();
        assert_eq!(disk, &disk_path);
        assert!(disk.is_file());
        assert!(vm_config.boot_args.contains(
            "elastos.browser_profile=profile-99bb2b58175e1e062cd2fb6b1b00feec63d169f520dd0a8cfe7230517cfc43e4"
        ));
        assert!(vm_config
            .boot_args
            .contains("elastos.browser_profile_disk=required"));
    }

    #[test]
    fn principal_profile_disk_rejects_a_second_vm_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let disk_path = tmp
            .path()
            .join("Users/0123456789ab/BrowserProfiles/default/profile.ext4");
        let request = json!({
            "profile": {
                "schema": "elastos.browser.profile/v1",
                "scope": "active_principal",
                "storage": "principal_owned_profile_disk",
                "storage_posture": "principal_owned_reset_scoped_unprotected",
                "protected_storage": false,
                "encrypted": false,
                "recoverable": false,
                "recovery": "not_recovery_kit_packaged",
                "uri": "localhost://Users/0123456789ab/BrowserProfiles/default/profile.ext4",
                "public_uri": "localhost://Users/self/BrowserProfiles/default/profile.ext4",
                "profile_key": "profile-99bb2b58175e1e062cd2fb6b1b00feec63d169f520dd0a8cfe7230517cfc43e4",
                "disk_path": disk_path,
                "reset": "whole_profile"
            }
        });
        let owner = prepare_browser_profile_disk(&request).unwrap();
        let lock_path = disk_lifetime_lock_path(&disk_path);
        assert_eq!(
            lock_path,
            PathBuf::from(format!(
                "{}{}",
                disk_path.display(),
                DISK_LIFETIME_LOCK_SUFFIX
            ))
        );
        assert_eq!(fs::metadata(&lock_path).unwrap().mode() & 0o777, 0o600);
        assert_disk_inode_is_unlocked(&disk_path);

        let error = prepare_browser_profile_disk(&request).unwrap_err();
        let typed: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(typed["schema"], "elastos.browser.engine.launch-error/v1");
        assert_eq!(typed["code"], "resources_in_use");
        assert_eq!(typed["resource"], "principal Browser profile disk");
        assert_eq!(typed["path"], disk_path.to_string_lossy().as_ref());

        drop(owner);
        prepare_browser_profile_disk(&request).unwrap();
    }

    #[test]
    fn bridge_propagates_runtime_eof_to_guest_and_exits() {
        let (mut guest_client, guest_bridge) = UnixStream::pair().unwrap();
        let (runtime_bridge, mut runtime_client) = UnixStream::pair().unwrap();
        guest_client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (done_tx, done_rx) = mpsc::channel();

        let bridge = thread::spawn(move || {
            let result = forward_pair(
                DuplexStream::Unix(guest_bridge),
                DuplexStream::Unix(runtime_bridge),
            );
            done_tx.send(()).unwrap();
            result
        });

        runtime_client.write_all(b"pong").unwrap();
        runtime_client.shutdown(Shutdown::Write).unwrap();

        let mut response = [0_u8; 4];
        guest_client.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"pong");
        let mut eof = [0_u8; 1];
        assert_eq!(guest_client.read(&mut eof).unwrap(), 0);
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("VZ egress bridge should close after Runtime EOF");

        let (guest_to_runtime, runtime_to_guest) = bridge.join().unwrap().unwrap();
        assert_eq!(guest_to_runtime, 0);
        assert_eq!(runtime_to_guest, 4);
    }

    #[test]
    fn guest_control_response_timeout_has_clear_error() {
        let (client, _server) = UnixStream::pair().unwrap();
        let fd = unsafe { OwnedFd::from_raw_fd(client.into_raw_fd()) };
        let started = Instant::now();

        let error = http_json_over_fd(
            fd,
            "POST",
            "/pages",
            Some(&json!({ "schema": "elastos.test/v1" })),
            Some(Duration::from_millis(25)),
        )
        .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            error.contains("Browser VM control HTTP response timed out"),
            "{error}"
        );
    }

    #[test]
    fn hibernation_key_changes_when_profile_artifacts_resources_or_boot_args_change() {
        let tmp = tempfile::tempdir().unwrap();
        with_hibernation_env(&tmp.path().join("hibernation"), || {
            let (mut paths, profile_disk_path) = hibernation_fixture_paths(tmp.path());
            let launch = hibernation_launch();
            let base_key = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;

            let profile_key_changed = hibernation_for(
                &paths,
                &launch,
                "profile-b",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, profile_key_changed);

            let profile_disk_b = tmp.path().join("data/profiles/profile-b.ext4");
            fs::write(&profile_disk_b, b"profile-b").unwrap();
            let profile_disk_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_b,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, profile_disk_changed);

            paths.memory_mib = 4096;
            let memory_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, memory_changed);
            paths.memory_mib = 2048;

            paths.vcpu_count = 4;
            let vcpu_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, vcpu_changed);
            paths.vcpu_count = 2;

            let kernel_b = tmp.path().join("data/bin/vmlinux-b");
            fs::write(&kernel_b, b"kernel-b").unwrap();
            paths.kernel_path = kernel_b;
            let kernel_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, kernel_changed);
            paths.kernel_path = tmp.path().join("data/bin/vmlinux");

            let rootfs_b = tmp.path().join("data/browser-vm/rootfs-b.ext4");
            fs::write(&rootfs_b, b"rootfs-b").unwrap();
            paths.rootfs_path = rootfs_b;
            let rootfs_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1280",
            )
            .key;
            assert_ne!(base_key, rootfs_changed);
            paths.rootfs_path = tmp.path().join("data/browser-vm/rootfs.ext4");

            let boot_args_changed = hibernation_for(
                &paths,
                &launch,
                "profile-a",
                &profile_disk_path,
                "console=hvc0 elastos.browser_width=1470",
            )
            .key;
            assert_ne!(base_key, boot_args_changed);
        });
    }

    #[test]
    fn hibernation_prepare_launch_rootfs_removes_stale_state_when_cache_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        with_hibernation_env(&tmp.path().join("hibernation"), || {
            let (paths, profile_disk_path) = hibernation_fixture_paths(tmp.path());
            let hibernation = hibernation_for(
                &paths,
                &hibernation_launch(),
                "profile-a",
                &profile_disk_path,
                "console=hvc0",
            );
            fs::create_dir_all(&hibernation.state_dir).unwrap();
            fs::write(&hibernation.state_path, b"stale machine state").unwrap();
            assert!(!hibernation.launch_rootfs_path.exists());

            let launch_rootfs = prepare_launch_rootfs(&paths, Some(&hibernation)).unwrap();

            assert_eq!(launch_rootfs.path, hibernation.launch_rootfs_path);
            assert!(hibernation.launch_rootfs_path.is_file());
            assert!(!hibernation.state_path.exists());
        });
    }

    #[test]
    fn shared_writable_rootfs_rejects_a_second_vm_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvVarRestore::capture("ELASTOS_BROWSER_VM_ROOTFS_PER_LAUNCH");
        std::env::set_var("ELASTOS_BROWSER_VM_ROOTFS_PER_LAUNCH", "0");
        let (paths, _) = hibernation_fixture_paths(tmp.path());
        let owner = prepare_launch_rootfs(&paths, None).unwrap();
        let lock_path = disk_lifetime_lock_path(&paths.rootfs_path);
        assert_eq!(
            lock_path,
            PathBuf::from(format!(
                "{}{}",
                paths.rootfs_path.display(),
                DISK_LIFETIME_LOCK_SUFFIX
            ))
        );
        assert_eq!(fs::metadata(&lock_path).unwrap().mode() & 0o777, 0o600);
        assert_disk_inode_is_unlocked(&paths.rootfs_path);

        let error = prepare_launch_rootfs(&paths, None).unwrap_err();
        let typed: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(typed["code"], "resources_in_use");
        assert_eq!(typed["resource"], "shared writable Browser VZ rootfs");
        assert_eq!(typed["path"], paths.rootfs_path.to_string_lossy().as_ref());

        drop(owner);
        prepare_launch_rootfs(&paths, None).unwrap();
    }

    fn assert_disk_inode_is_unlocked(disk_path: &Path) {
        let disk = OpenOptions::new()
            .read(true)
            .write(true)
            .open(disk_path)
            .unwrap();
        assert!(
            try_lock_exclusive(&disk).unwrap(),
            "disk inode should remain available to Virtualization.framework"
        );
        assert_eq!(unsafe { libc::flock(disk.as_raw_fd(), libc::LOCK_UN) }, 0);
    }

    #[test]
    fn hibernation_restore_failure_cleanup_removes_bad_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        with_hibernation_env(&tmp.path().join("hibernation"), || {
            let (paths, profile_disk_path) = hibernation_fixture_paths(tmp.path());
            let hibernation = hibernation_for(
                &paths,
                &hibernation_launch(),
                "profile-a",
                &profile_disk_path,
                "console=hvc0",
            );
            fs::create_dir_all(&hibernation.state_dir).unwrap();
            fs::write(&hibernation.state_path, b"bad machine state").unwrap();

            discard_bad_hibernation_state(&hibernation);

            assert!(!hibernation.state_path.exists());
        });
    }

    #[test]
    fn hibernation_save_failure_cleanup_removes_tmp_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        with_hibernation_env(&tmp.path().join("hibernation"), || {
            let (paths, profile_disk_path) = hibernation_fixture_paths(tmp.path());
            let hibernation = hibernation_for(
                &paths,
                &hibernation_launch(),
                "profile-a",
                &profile_disk_path,
                "console=hvc0",
            );
            fs::create_dir_all(&hibernation.state_dir).unwrap();
            fs::write(&hibernation.state_tmp_path, b"partial machine state").unwrap();

            discard_hibernation_tmp_state(&hibernation);

            assert!(!hibernation.state_tmp_path.exists());
        });
    }

    #[test]
    fn hibernation_lifetime_lock_rejects_a_second_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let state_dir = tmp.path().join("state");
        let lease = HibernationLease::acquire(&state_dir).unwrap();

        let error = HibernationLease::acquire(&state_dir).unwrap_err();
        let typed: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(typed["code"], "resources_in_use");
        drop(lease);
        HibernationLease::acquire(&state_dir).unwrap();
    }

    #[test]
    fn hibernation_cache_prune_bounds_entries_without_removing_live_or_current_state() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("hibernation");
        let mut entries = Vec::new();
        for index in 0..6 {
            entries.push(create_hibernation_cache_entry(&root, index));
        }
        let current = entries[5].clone();
        let live = entries[4].clone();
        let _live_lease = HibernationLease::acquire(&live).unwrap();

        let removed = prune_hibernation_cache_at(
            &root,
            &current,
            2,
            Duration::from_secs(DEFAULT_HIBERNATION_MAX_AGE_SECS),
            SystemTime::now(),
        )
        .unwrap();

        assert_eq!(removed, 4);
        assert!(current.is_dir());
        assert!(live.is_dir());
        assert_eq!(hibernation_cache_entries(&root).unwrap().len(), 2);
    }

    #[test]
    fn lifetime_lock_child_process() {
        let Some(path) = std::env::var_os("ELASTOS_TEST_LIFETIME_LOCK_PATH") else {
            return;
        };
        let _lock =
            LifetimeFileLock::acquire_lock_file(Path::new(&path), "test Browser VM resource")
                .unwrap();
        println!("ELASTOS_TEST_LIFETIME_LOCK_READY");
        std::io::stdout().flush().unwrap();
        thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn kernel_lifetime_lock_releases_after_owner_process_death() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("owner-death.lock");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("lifetime_lock_child_process")
            .arg("--nocapture")
            .env("ELASTOS_TEST_LIFETIME_LOCK_PATH", &lock_path)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = std::io::BufReader::new(stdout).lines();
        let mut ready = false;
        for _ in 0..20 {
            let Some(line) = lines.next() else {
                break;
            };
            if line.unwrap().contains("ELASTOS_TEST_LIFETIME_LOCK_READY") {
                ready = true;
                break;
            }
        }
        assert!(ready, "lock-owner child did not acquire its lifetime lock");
        let error = LifetimeFileLock::acquire_lock_file(&lock_path, "test Browser VM resource")
            .unwrap_err();
        let typed: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(typed["code"], "resources_in_use");

        child.kill().unwrap();
        child.wait().unwrap();

        LifetimeFileLock::acquire_lock_file(&lock_path, "test Browser VM resource").unwrap();
    }

    #[test]
    fn hibernation_cache_prune_expires_inactive_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("hibernation");
        let current = create_hibernation_cache_entry(&root, 0);
        let expired = create_hibernation_cache_entry(&root, 1);

        let removed = prune_hibernation_cache_at(
            &root,
            &current,
            8,
            Duration::from_secs(DEFAULT_HIBERNATION_MAX_AGE_SECS),
            SystemTime::now() + Duration::from_secs(DEFAULT_HIBERNATION_MAX_AGE_SECS + 60),
        )
        .unwrap();

        assert_eq!(removed, 1);
        assert!(current.is_dir());
        assert!(!expired.exists());
    }
}
