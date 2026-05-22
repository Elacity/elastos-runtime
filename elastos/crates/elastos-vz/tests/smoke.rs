//! Public-API smoke tests for the elastos-vz crate.
//!
//! These tests live in `tests/` (integration tests) so they exercise
//! only the crate's public surface — exactly what downstream callers
//! (elastos-server's `main.rs` registration block and the supervisor)
//! can see. If a smoke fails here, callers break too.
//!
//! Updated for Phase 2 Day 3: `load`, `start`, `stop`, `status`,
//! `info`, `http_port` now return real typed errors (not the
//! Phase 1 stub) when the input is invalid or the handle was
//! never loaded. The Phase 1 stub message is reserved for the
//! still-unimplemented surfaces (`set_session_for_vm`,
//! `append_boot_args_for_vm`, `set_network_for_vm`) and for the
//! non-macOS fall-through path in `load` once all input
//! validations have passed (i.e. "you gave me a valid capsule on
//! Linux; Vz won't run here").

use elastos_common::{
    CapsuleManifest, CapsuleRole, CapsuleType, ElastosError, MicroVmConfig, ResourceLimits,
    SCHEMA_V1,
};
use elastos_compute::{CapsuleHandle, ComputeProvider};

use elastos_vz::{is_supported, NetworkConfig, VmConfig, VzConfig, VzProvider};

fn microvm_manifest(name: &str) -> CapsuleManifest {
    CapsuleManifest {
        schema: SCHEMA_V1.into(),
        version: "0.1.0".into(),
        name: name.into(),
        description: None,
        author: None,
        role: CapsuleRole::App,
        capsule_type: CapsuleType::MicroVM,
        entrypoint: "rootfs.ext4".into(),
        requires: Vec::new(),
        provides: None,
        capabilities: Vec::new(),
        resources: ResourceLimits {
            memory_mb: 128,
            cpu_shares: 100,
            gpu: false,
        },
        permissions: Default::default(),
        microvm: Some(MicroVmConfig {
            kernel: None,
            boot_args: "console=ttyS0".into(),
            http_port: None,
            vcpu_count: Some(1),
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

#[test]
fn is_supported_reports_bool_without_panicking() {
    let _: bool = is_supported();
}

#[test]
fn off_mac_is_supported_is_strictly_false() {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    assert!(
        !is_supported(),
        "is_supported() must be false everywhere outside Apple Silicon macOS"
    );
}

#[test]
fn vz_provider_constructable_with_defaults_in_phase_1() {
    // Phase 1 deliberately does not require a kernel on disk so
    // contributors on a Mac without the runtime install can still
    // build and test.
    let provider = VzProvider::with_defaults();
    assert!(provider.is_ok());
}

#[test]
fn vz_provider_supports_only_microvm() {
    let provider = VzProvider::with_defaults().unwrap();
    assert!(provider.supports(&CapsuleType::MicroVM));
    assert!(!provider.supports(&CapsuleType::Wasm));
    assert!(!provider.supports(&CapsuleType::Oci));
}

#[tokio::test]
async fn vz_provider_init_creates_state_dirs_idempotently() {
    let tmp = tempfile::tempdir().unwrap();
    let config = VzConfig::new()
        .with_state_dir(tmp.path().join("vz"))
        .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"));
    let provider = VzProvider::new(config).unwrap();

    // First init creates.
    provider.init().await.unwrap();
    assert!(tmp.path().join("vz").is_dir());
    assert!(tmp.path().join("rootfs-cache").is_dir());

    // Second init is a no-op (idempotency check).
    provider.init().await.unwrap();
}

#[tokio::test]
async fn vz_provider_load_returns_rootfs_not_found_for_missing_capsule() {
    // Day 3: input validation moved ahead of the Vz / stub
    // branches. A missing rootfs is now surfaced as a typed
    // CapsuleNotFound error regardless of platform — the
    // Phase 1 stub message is reserved for the non-macOS
    // fall-through (covered by the test below).
    let provider = VzProvider::with_defaults().unwrap();
    let manifest = microvm_manifest("smoke-load");

    let err = provider
        .load(std::path::Path::new("/tmp/no-such-capsule"), manifest)
        .await
        .unwrap_err();

    assert!(
        matches!(err, ElastosError::CapsuleNotFound(_)),
        "expected CapsuleNotFound for missing rootfs, got: {err}"
    );
    assert!(err.to_string().contains("Rootfs not found"));
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn vz_provider_load_falls_closed_with_phase_marker_on_non_macos() {
    // On Linux a perfectly-formed capsule still cannot boot
    // because Apple's Vz framework is unreachable. After
    // input-validation, load() must surface the Phase 2 stub
    // message + a pointer to the plan doc so the operator
    // knows where to look.
    let tmp = tempfile::tempdir().unwrap();
    let capsule_dir = tmp.path().join("capsule");
    std::fs::create_dir_all(&capsule_dir).unwrap();
    let rootfs = capsule_dir.join("rootfs.ext4");
    std::fs::write(&rootfs, b"fake-rootfs").unwrap();
    let kernel = tmp.path().join("vmlinux");
    std::fs::write(&kernel, b"fake-kernel").unwrap();

    let config = VzConfig::new()
        .with_state_dir(tmp.path().join("vz-state"))
        .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
        .with_kernel_path(kernel);
    let provider = VzProvider::new(config).unwrap();

    let err = provider
        .load(&capsule_dir, microvm_manifest("smoke-stub-non-macos"))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("Phase 2"), "expected Phase-2 marker: {msg}");
    assert!(
        msg.contains("docs/vz-backend/PLAN.md"),
        "expected plan-doc anchor: {msg}"
    );
}

#[tokio::test]
async fn vz_provider_load_rejects_wasm_type_with_clear_message() {
    let provider = VzProvider::with_defaults().unwrap();
    let mut wasm = microvm_manifest("not-a-vm");
    wasm.capsule_type = CapsuleType::Wasm;

    let err = provider
        .load(std::path::Path::new("/tmp"), wasm)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only supports MicroVM"));
}

#[tokio::test]
async fn vz_provider_lifecycle_methods_fail_closed_for_unloaded_handle() {
    // Day 3 contract: start / status / info / http_port on an
    // unknown handle return CapsuleNotFound; stop is
    // idempotent (Ok). Mirrors crosvm's contract so the
    // supervisor can treat both substrates uniformly.
    let provider = VzProvider::with_defaults().unwrap();
    let handle = CapsuleHandle {
        id: elastos_common::CapsuleId::new("unloaded-smoke".to_string()),
        manifest: microvm_manifest("unloaded-smoke"),
        args: vec![],
    };

    for (label, err) in [
        ("start", provider.start(&handle).await.unwrap_err()),
        ("status", provider.status(&handle).await.unwrap_err()),
        ("info", provider.info(&handle).await.unwrap_err()),
        ("http_port", provider.http_port(&handle).await.unwrap_err()),
    ] {
        assert!(
            matches!(err, ElastosError::CapsuleNotFound(_)),
            "{label}: expected CapsuleNotFound, got: {err}"
        );
    }

    // stop on an unknown handle is intentionally Ok(()) —
    // same as crosvm.
    provider.stop(&handle).await.unwrap();
}

#[test]
fn vm_config_from_manifest_translates_to_vz_console_naming() {
    let manifest = microvm_manifest("vm-args");
    let config = VmConfig::from_manifest(
        &manifest,
        std::path::Path::new("/c"),
        std::path::Path::new("/k/vmlinux"),
    );
    // The crosvm-style `console=ttyS0` in the manifest must be
    // rewritten to `console=hvc0` for Vz boot.
    assert!(config.boot_args.contains("console=hvc0"));
    assert!(!config.boot_args.contains("ttyS0"));
}

#[test]
fn network_config_new_is_deterministic_and_shape_compatible() {
    let a = NetworkConfig::new("smoke-net");
    let b = NetworkConfig::new("smoke-net");
    // Deterministic for the same vm_id.
    assert_eq!(a.host_ip, b.host_ip);
    assert_eq!(a.guest_ip, b.guest_ip);
    assert_eq!(a.guest_mac, b.guest_mac);
    // Shape matches crosvm.
    assert_eq!(a.prefix_len, 30);
    assert!(a.host_ip.ends_with(".1"));
    assert!(a.guest_ip.ends_with(".2"));
}
