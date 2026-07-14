//! concurrent-launch stress for the Vz backend.
//!
//! Three concerns are exercised here:
//!
//! 1. The per-VM dispatch queue behavior means
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
//!    test visibly skips when the host, install artefacts, or local
//!    code-signing entitlements are missing; it is the
//!    developer-facing escape hatch for validating multi-VM boot on
//!    a real Apple Silicon Mac with a real kernel artefact.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use elastos_common::{
    CapsuleManifest, CapsuleRole, CapsuleType, MicroVmConfig, ResourceLimits, SCHEMA_V1,
};
use elastos_compute::ComputeProvider;

use elastos_vz::{is_supported, VmConfig, VzConfig, VzProvider};

use tracing::field::{Field, Visit};
use tracing::Event;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

fn microvm_manifest(name: &str) -> CapsuleManifest {
    CapsuleManifest {
        schema: SCHEMA_V1.into(),
        version: "0.1.0".into(),
        name: name.into(),
        description: None,
        author: None,
        role: CapsuleRole::App,
        capsule_type: CapsuleType::MicroVM,
        runtime_abi: None,
        bus_contract: None,
        wit_world_sha256: None,
        execution: None,
        projections: Vec::new(),
        entrypoint: "rootfs.ext4".into(),
        requires: Vec::new(),
        provides: None,
        capabilities: Vec::new(),
        interfaces: Vec::new(),
        resources: ResourceLimits {
            memory_mb: 128,
            cpu_shares: 100,
            gpu: false,
        },
        permissions: Default::default(),
        // None here means "no authority constraint" for the synthetic
        // concurrent-launch fixture.
        authority: None,
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

/// Platform-aware base data directory the integration tests read
/// from. Mirrors `elastos-server::sources::default_data_dir`:
///
/// - Linux: `$XDG_DATA_HOME/elastos` (default `~/.local/share/elastos`)
/// - macOS: `~/Library/Application Support/elastos`
///
/// replaces three previously hard-coded
/// `~/.local/share/elastos/` paths in the discover helpers
/// (kernel/initrd/rootfs). Existing Linux test workflows are
/// byte-identical because `dirs::data_dir()` on Linux returns the
/// same `~/.local/share` path. macOS now finds artefacts that
/// `elastos setup` actually installs, instead of looking under a
/// directory that doesn't exist on this platform.
///
/// We keep this `pub(super)`-style (free function in the test
/// crate) intentionally: the test crate is an integration boundary
/// and shouldn't take a runtime dep on `elastos-server` just to
/// reach `default_data_dir`. The 3-line lookup is cheap to
/// replicate; correctness is enforced by the matching pinned
/// `dirs = "5.0"` in this crate's dev-deps and in elastos-server's
/// runtime deps.
fn test_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("elastos"))
}

/// Auto-discover the canonical kernel install path. Reads from
/// `<data_dir>/bin/vmlinux` where `<data_dir>` resolves through
/// [`test_data_dir`]. `ELASTOS_VZ_TEST_KERNEL` overrides the
/// discovery for developer-driven runs against a kernel in a
/// non-standard location.
fn discover_kernel() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_KERNEL") {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    let candidate = test_data_dir()?.join("bin/vmlinux");
    candidate.is_file().then_some(candidate)
}

/// Auto-discover any installed capsule rootfs. The supervisor
/// extracts capsules to `<data_dir>/capsules/<name>/` with the
/// rootfs at `<name>/rootfs.ext4`. We pick the first match — every
/// capsule's rootfs is bootable; the test only needs to prove
/// parallel VMs load. `ELASTOS_VZ_TEST_ROOTFS` is the override.
fn discover_rootfs() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_ROOTFS") {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    let capsules_dir = test_data_dir()?.join("capsules");
    let entries = std::fs::read_dir(&capsules_dir).ok()?;
    for entry in entries.flatten() {
        let rootfs = entry.path().join("rootfs.ext4");
        if rootfs.is_file() {
            return Some(rootfs);
        }
    }
    None
}

fn is_missing_virtualization_entitlement(err: &impl std::fmt::Display) -> bool {
    err.to_string()
        .contains("missing com.apple.security.virtualization entitlement")
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
/// contamination — proves the per-VM dispatch queue behavior
/// holds up under a real concurrent boot
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
                 $ELASTOS_VZ_TEST_KERNEL or <data_dir>/bin/vmlinux. \
                 Run `elastos setup --profile minimal` first."
            );
            return;
        }
    };
    let rootfs = match discover_rootfs() {
        Some(p) => p,
        None => {
            eprintln!(
                "concurrent_load_with_real_kernel: skipping — no rootfs found at \
                 $ELASTOS_VZ_TEST_ROOTFS or <data_dir>/capsules/*/rootfs.ext4. \
                 Run `elastos setup --profile minimal` first."
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
            Err(err) if is_missing_virtualization_entitlement(&err) => {
                eprintln!(
                    "concurrent_load_with_real_kernel: skipping - test binary lacks \
                     com.apple.security.virtualization entitlement. Sign with \
                     scripts/dev/sign-elastos-vz/ for real VZ boot proof."
                );
                return;
            }
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

// ─── Boot-to-userspace validation ─────────────────────────────────────────────
//
// Captures the kernel-console `tracing` stream (target = "vm_console")
// emitted by `src/ffi/console_forwarder.rs` and asserts that booting a
// real Linux kernel + initramfs through the substrate produces
// recognizable boot markers (e.g. "Linux version", "Run /init").
//
// Discovery contract mirrors `concurrent_load_with_real_kernel`:
//
//   - $ELASTOS_VZ_TEST_KERNEL  → kernel Image path
//   - $ELASTOS_VZ_TEST_INITRD  → initramfs path
//   - else: ~/.local/share/elastos/bin/{vmlinux,initrd-generic}
//
// The test cleanly skips (eprintln + return) on unsupported hosts or
// when artefacts are missing — same skip pattern as the load test.

/// Lines tagged with target = "vm_console" since process start. Shared
/// across all tests in this binary; populated by `VmConsoleCaptureLayer`
/// (installed exactly once via `init_vm_console_capture`).
type LineBuffer = Arc<Mutex<Vec<String>>>;
static VM_CONSOLE_LINES: OnceLock<LineBuffer> = OnceLock::new();

/// Install the capturing tracing subscriber on first call. Subsequent
/// calls return the same shared buffer. Idempotent across tests in the
/// same binary so the no-tracing-setup-needed contract of the other
/// tests stays intact.
fn init_vm_console_capture() -> LineBuffer {
    VM_CONSOLE_LINES
        .get_or_init(|| {
            let buf: LineBuffer = Arc::new(Mutex::new(Vec::new()));
            let layer = VmConsoleCaptureLayer { lines: buf.clone() };
            // `try_init` is non-panicking: if some other test already
            // installed a global subscriber, we silently fall through
            // (our buffer just stays empty, which the boot test will
            // surface as a clear assertion failure).
            let _ = tracing_subscriber::registry().with(layer).try_init();
            buf
        })
        .clone()
}

/// Minimal tracing `Layer` that collects the message body of every
/// `vm_console`-targeted event into a shared `Vec<String>`. Drops all
/// other targets unchanged (so the `concurrent_load_*` tests in this
/// same file remain unaffected).
struct VmConsoleCaptureLayer {
    lines: LineBuffer,
}

impl<S> Layer<S> for VmConsoleCaptureLayer
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "vm_console" {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        if !visitor.0.is_empty() {
            if let Ok(mut guard) = self.lines.lock() {
                guard.push(visitor.0);
            }
        }
    }
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // `tracing::info!(..., "{line}")` ends up as a Debug
            // record on the message field at the layer boundary;
            // the actual line string lives inside that Debug repr.
            self.0.push_str(&format!("{value:?}"));
        }
    }
}

fn discover_initrd() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ELASTOS_VZ_TEST_INITRD") {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    let data_dir = test_data_dir()?;
    // prefer the second-stage `bin/initrd-overlay`
    // (writable-rootfs tmpfs overlay variant `elastos setup` now
    // builds) over the pristine `bin/initrd` (the canonical
    // published variant). `bin/initrd-generic` is the
    // pre-standardisation upstream filename, kept as a final
    // fallback so an operator who fetched the artefact directly
    // (without `elastos setup`) still has a working discovery
    // path. The overlay variant intentionally wins: the
    // integration test should exercise the same boot lane
    // `elastos run ubuntu-base` does in production.
    for name in ["bin/initrd-overlay", "bin/initrd", "bin/initrd-generic"] {
        let candidate = data_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Returns true once the captured `vm_console` buffer contains any of
/// the recognised Linux-boot markers.
fn observed_boot_markers(buf: &LineBuffer) -> Option<&'static str> {
    // These three are present in *every* arm64 Linux 5.x boot. We
    // accept any one of them as proof of "kernel reached
    // initialisation"; the union covers cases where one or another
    // is filtered by a `quiet` boot arg or a custom printk level.
    const MARKERS: &[&str] = &["Linux version", "Booting Linux", "Run /init"];
    let lines = buf.lock().ok()?;
    // clippy `manual_find` (RUSTSEC equivalent: idiomatic Iterator::find).
    // The original explicit `for + return Some` was caught by `-D warnings` under
    // clippy after formatting invalidated cached analysis.
    MARKERS
        .iter()
        .find(|&marker| lines.iter().any(|l| l.contains(marker)))
        .copied()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_vm_boots_to_userspace() {
    if !is_supported() {
        eprintln!(
            "single_vm_boots_to_userspace: skipping — is_supported() == false \
             (off Apple Silicon macOS, Vz framework unreachable)"
        );
        return;
    }

    let kernel = match discover_kernel() {
        Some(p) => p,
        None => {
            eprintln!(
                "single_vm_boots_to_userspace: skipping — no kernel at \
                 $ELASTOS_VZ_TEST_KERNEL or <data_dir>/bin/vmlinux. \
                 Run `elastos setup --profile minimal` first."
            );
            return;
        }
    };
    let initrd = match discover_initrd() {
        Some(p) => p,
        None => {
            eprintln!(
                "single_vm_boots_to_userspace: skipping — no initramfs at \
                 $ELASTOS_VZ_TEST_INITRD or <data_dir>/bin/initrd (or the \
                 bin/initrd-generic compatibility fallback). Run `elastos setup --profile minimal` first."
            );
            return;
        }
    };

    // A 1 MB zero-filled scratch rootfs is valid here:
    // Apple's `validateWithError:` only checks file
    // existence, and the boot path never tries to mount it — the
    // initramfs is the rootfs at this stage. Reuse if present;
    // otherwise create one on the fly so the test is self-contained.
    let rootfs = std::env::var("ELASTOS_VZ_TEST_ROOTFS")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or_else(|| {
            let tmp_root = std::env::temp_dir().join("elastos-vz-boot-rootfs.raw");
            if !tmp_root.is_file() {
                let f = std::fs::File::create(&tmp_root).expect("create scratch rootfs");
                f.set_len(1024 * 1024).expect("size scratch rootfs");
            }
            tmp_root
        });

    eprintln!(
        "single_vm_boots_to_userspace: kernel={} initrd={} rootfs={}",
        kernel.display(),
        initrd.display(),
        rootfs.display()
    );

    let console_lines = init_vm_console_capture();
    let baseline_len = console_lines.lock().unwrap().len();

    let tmp = tempfile::tempdir().unwrap();
    let provider = std::sync::Arc::new(
        VzProvider::new(
            VzConfig::new()
                .with_state_dir(tmp.path().join("vz"))
                .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
                .with_kernel_path(kernel.clone())
                .with_initramfs_path(initrd.clone()),
        )
        .unwrap(),
    );
    provider.init().await.unwrap();

    // `console=hvc0` is the Vz-native virtio-console; `init=/init`
    // tells the kernel to execute the initramfs's `/init` directly
    // (Ubuntu's initramfs ships one). We deliberately keep verbosity
    // ON (no `quiet`) so the early printk reaches our capture before
    // any userspace handover that might silence the kernel ring.
    let vm_config = VmConfig {
        vm_id: "vz-boot-probe".to_string(),
        kernel_path: kernel.clone(),
        boot_args: "console=hvc0 init=/init".to_string(),
        rootfs_path: rootfs.clone(),
        rootfs_readonly: true,
        mem_size_mib: 256, // initramfs unpacking needs more headroom than 128
        vcpu_count: 1,
        http_port: None,
        data_disk_path: None,
        vsock_cid: 3,
        network: None,
        interactive_stdio: false,
        carrier_socket_path: None,
        initramfs_path: Some(initrd.clone()),
    };

    let handle = match provider
        .load_with_vm_config(vm_config, microvm_manifest("vz-boot-probe"))
        .await
    {
        Ok(handle) => handle,
        Err(err) if is_missing_virtualization_entitlement(&err) => {
            eprintln!(
                "single_vm_boots_to_userspace: skipping - test binary lacks \
                 com.apple.security.virtualization entitlement. Sign with \
                 scripts/dev/sign-elastos-vz/ for real VZ boot proof."
            );
            return;
        }
        Err(err) => panic!("load_with_vm_config must accept kernel+initramfs config: {err}"),
    };

    provider
        .start(&handle)
        .await
        .expect("start must succeed once validateWithError has passed");

    // Poll the capture buffer for boot markers — up to 30s wall
    // clock. The first `Linux version` line typically appears
    // within 1–2 seconds of `start()` returning on M1/M2.
    let deadline = Instant::now() + Duration::from_secs(30);
    let found = loop {
        if let Some(marker) = observed_boot_markers(&console_lines) {
            break Some(marker);
        }
        if Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // Capture a slice of the post-boot console for diagnostic
    // logging (first 30 lines past our baseline). This shows up in
    // `cargo test -- --nocapture` so a reader can confirm the VM
    // actually booted — even when the assertion passes — and so
    // failures include real evidence.
    let captured: Vec<String> = console_lines
        .lock()
        .unwrap()
        .iter()
        .skip(baseline_len)
        .take(30)
        .cloned()
        .collect();
    if !captured.is_empty() {
        eprintln!("=== first ≤30 kernel-console lines ===");
        for line in &captured {
            eprintln!("    {line}");
        }
        eprintln!("=== end console capture ===");
    } else {
        eprintln!("=== no kernel-console output captured (capture layer race or pipe stalled) ===");
    }

    // Stop the VM. Best-effort: even if the kernel panicked we still
    // want a clean teardown so the test binary exits cleanly.
    let _ = provider.stop(&handle).await;

    match found {
        Some(marker) => {
            eprintln!("single_vm_boots_to_userspace: PASS (marker '{marker}' observed)");
        }
        None => panic!(
            "single_vm_boots_to_userspace: 30s elapsed with no Linux-boot marker; \
             the substrate did not surface a booting kernel. Total captured lines: {}. \
             First captured line (if any): {:?}",
            captured.len(),
            captured.first()
        ),
    }
}
