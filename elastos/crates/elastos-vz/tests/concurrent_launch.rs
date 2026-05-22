//! Phase 4 Day 1 — concurrent-launch stress for the Vz backend.
//!
//! Three concerns are exercised here:
//!
//! 1. The per-VM dispatch queue refactor (Phase 4 Day 1) means
//!    `VzMachineHandle::new` constructs its own `VzDispatchQueue`
//!    instead of cloning a provider-owned one. This file proves
//!    that path is exercised by N≥3 concurrent load attempts
//!    against a single `VzProvider`.
//!
//! 2. The provider's `vms: Arc<RwLock<HashMap<CapsuleId, RunningVm>>>`
//!    must serialize correctly under N concurrent writers. The
//!    rejection-path test (`concurrent_load_rejections_isolate_per_vm`)
//!    runs without needing a real kernel — each request fails with
//!    `Compute("Kernel not found: …")` and the error carries the
//!    requesting VM's own path, proving no state crosses the
//!    lock boundary.
//!
//! 3. The opt-in `concurrent_load_with_real_kernel` test exercises
//!    the actual builder + per-VM-queue path when a real kernel +
//!    rootfs are available via env vars
//!    (`ELASTOS_VZ_TEST_KERNEL`, `ELASTOS_VZ_TEST_ROOTFS`). The
//!    test is `#[ignore]` so CI never runs it; it is the
//!    developer-facing escape hatch for validating multi-VM boot
//!    on a real Apple Silicon Mac with a real kernel artefact.

use std::path::PathBuf;

use elastos_common::{
    CapsuleManifest, CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1,
};

use elastos_vz::{is_supported, VmConfig, VzConfig, VzProvider};

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

/// Build a `VmConfig` pointing at a path that does NOT exist. This
/// triggers the "Kernel not found" early rejection in
/// `load_with_vm_config` after passing manifest-type validation,
/// which is the lock-contention surface we want to stress.
fn vm_config_with_missing_kernel(
    vm_id: &str,
    kernel_path: PathBuf,
    rootfs_path: PathBuf,
) -> VmConfig {
    VmConfig {
        vm_id: vm_id.to_string(),
        kernel_path,
        boot_args: "console=hvc0 init=/init".into(),
        rootfs_path,
        rootfs_readonly: false,
        mem_size_mib: 128,
        vcpu_count: 1,
        http_port: None,
        data_disk_path: None,
        vsock_cid: 3,
        network: None,
        interactive_stdio: false,
        carrier_socket_path: None,
        initramfs_path: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_load_rejections_isolate_per_vm() {
    // Three concurrent `load_with_vm_config` calls against ONE
    // provider, each with its own unique missing-kernel path.
    // Every call must fail with the per-VM kernel path verbatim
    // — proof that no state crosses the provider's RwLock.

    let tmp = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(
        VzProvider::new(
            VzConfig::new()
                .with_state_dir(tmp.path().join("vz"))
                .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
                .with_kernel_path(tmp.path().join("nonexistent-kernel")),
        )
        .unwrap(),
    );

    let cases = [
        (
            "vm-alpha",
            tmp.path().join("alpha.bin"),
            tmp.path().join("alpha.ext4"),
        ),
        (
            "vm-bravo",
            tmp.path().join("bravo.bin"),
            tmp.path().join("bravo.ext4"),
        ),
        (
            "vm-charlie",
            tmp.path().join("charlie.bin"),
            tmp.path().join("charlie.ext4"),
        ),
    ];

    let mut set = tokio::task::JoinSet::new();
    for (vm_id, kernel, rootfs) in cases.iter() {
        let provider = provider.clone();
        let vm_config = vm_config_with_missing_kernel(vm_id, kernel.clone(), rootfs.clone());
        let manifest = microvm_manifest(vm_id);
        let expected_kernel = kernel.display().to_string();

        set.spawn(async move {
            let err = provider
                .load_with_vm_config(vm_config, manifest)
                .await
                .expect_err("load_with_vm_config must reject missing kernel");
            (expected_kernel, err.to_string())
        });
    }

    let mut completed = 0;
    while let Some(joined) = set.join_next().await {
        let (expected, message) = joined.expect("join must not panic");
        assert!(
            message.contains(&expected),
            "error message for VM must contain ITS OWN kernel path; expected {expected}, got: {message}"
        );
        completed += 1;
    }
    assert_eq!(completed, 3, "all three tasks must complete");
}

/// Auto-discover the canonical kernel install path. The
/// supervisor's `VzConfig::default()` resolves to
/// `~/.local/share/elastos/bin/vmlinux` (see `VzConfig::new`).
/// `ELASTOS_VZ_TEST_KERNEL` overrides the discovery for
/// developer-driven runs against a kernel in a non-standard
/// location.
fn discover_kernel() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_KERNEL") {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join(".local/share/elastos/bin/vmlinux");
    candidate.is_file().then_some(candidate)
}

/// Auto-discover any installed capsule rootfs. The supervisor
/// extracts capsules to `~/.local/share/elastos/capsules/<name>/`
/// with the rootfs at `<name>/rootfs.ext4`. We pick the first
/// match — every capsule's rootfs is bootable; the test only
/// needs to prove parallel VMs load.
/// `ELASTOS_VZ_TEST_ROOTFS` is the override.
fn discover_rootfs() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_ROOTFS") {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    let home = std::env::var_os("HOME")?;
    let capsules_dir = PathBuf::from(home).join(".local/share/elastos/capsules");
    let entries = std::fs::read_dir(&capsules_dir).ok()?;
    for entry in entries.flatten() {
        let rootfs = entry.path().join("rootfs.ext4");
        if rootfs.is_file() {
            return Some(rootfs);
        }
    }
    None
}

/// Real-kernel multi-VM boot. Auto-discovers the kernel and
/// rootfs from the supervisor's canonical install path
/// (`~/.local/share/elastos/bin/vmlinux` and
/// `~/.local/share/elastos/capsules/<name>/rootfs.ext4`);
/// `ELASTOS_VZ_TEST_KERNEL` / `ELASTOS_VZ_TEST_ROOTFS` env
/// vars override the discovery. Visibly skips (via `eprintln!`,
/// NOT `#[ignore]`) when the host is not an Apple Silicon Mac
/// or when no kernel/rootfs is installed.
///
/// Three VMs differ only in `vm_id`. All must reach a loaded
/// `CapsuleHandle` without panics, deadlocks, or cross-VM state
/// contamination — proves the per-VM dispatch queue refactor
/// (Phase 4 Day 1) holds up under a real concurrent boot
/// attempt, even when only `validateWithError` succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_load_with_real_kernel() {
    if !is_supported() {
        eprintln!(
            "concurrent_load_with_real_kernel: skipping — is_supported() == false \
             (off Apple Silicon macOS, Vz framework unreachable)"
        );
        return;
    }

    let kernel = match discover_kernel() {
        Some(p) => p,
        None => {
            eprintln!(
                "concurrent_load_with_real_kernel: skipping — no kernel found at \
                 $ELASTOS_VZ_TEST_KERNEL or ~/.local/share/elastos/bin/vmlinux. \
                 Run `elastos setup --with vmlinux` first."
            );
            return;
        }
    };
    let rootfs = match discover_rootfs() {
        Some(p) => p,
        None => {
            eprintln!(
                "concurrent_load_with_real_kernel: skipping — no rootfs found at \
                 $ELASTOS_VZ_TEST_ROOTFS or ~/.local/share/elastos/capsules/*/rootfs.ext4. \
                 Run `elastos setup` and pull at least one MicroVM capsule first."
            );
            return;
        }
    };

    eprintln!(
        "concurrent_load_with_real_kernel: using kernel={} rootfs={}",
        kernel.display(),
        rootfs.display()
    );

    let tmp = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(
        VzProvider::new(
            VzConfig::new()
                .with_state_dir(tmp.path().join("vz"))
                .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
                .with_kernel_path(kernel.clone()),
        )
        .unwrap(),
    );
    provider.init().await.unwrap();

    let ids = ["vm-alpha", "vm-bravo", "vm-charlie"];
    let mut set = tokio::task::JoinSet::new();
    for vm_id in ids.iter() {
        let provider = provider.clone();
        let vm_id = vm_id.to_string();
        let kernel = kernel.clone();
        let rootfs = rootfs.clone();
        set.spawn(async move {
            let vm_config = VmConfig {
                vm_id: vm_id.clone(),
                kernel_path: kernel,
                boot_args: "console=hvc0 init=/init random.trust_cpu=on".into(),
                rootfs_path: rootfs,
                rootfs_readonly: true,
                mem_size_mib: 128,
                vcpu_count: 1,
                http_port: None,
                data_disk_path: None,
                vsock_cid: 3,
                network: None,
                interactive_stdio: false,
                carrier_socket_path: None,
                initramfs_path: None,
            };
            provider
                .load_with_vm_config(vm_config, microvm_manifest(&vm_id))
                .await
                .map(|h| (vm_id, h))
        });
    }

    let mut loaded = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined.expect("join must not panic") {
            Ok((vm_id, handle)) => loaded.push((vm_id, handle)),
            Err(err) => panic!("concurrent load failed: {err}"),
        }
    }

    assert_eq!(loaded.len(), 3, "all three VMs must have loaded");

    // Every handle must carry a distinct CapsuleId. Proves the
    // per-launch UUID minting is not contended (no duplicate
    // collisions even when three loads complete on the same
    // millisecond).
    let mut ids: Vec<String> = loaded.iter().map(|(_, h)| h.id.0.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "CapsuleId must be unique per concurrent load");
}
